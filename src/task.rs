use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::hold::render_command;
use crate::output::emit;
use crate::route;
use crate::run;
use crate::table;

const TASKS_FILE: &str = ".zed/tasks.json";
const WORKTREE_ROOT: &str = "ZED_WORKTREE_ROOT";
const MAIN_WORKTREE: &str = "ZED_MAIN_GIT_WORKTREE";
const TASK_SHELL: &str = "/bin/sh";
const DOMAIN_VARIABLE: &str = "FOSTER_DOMAIN";
const HEADER: [&str; 4] = ["NAME", "LABEL", "DOMAIN", "COMMAND"];

#[derive(Deserialize)]
struct ZedTask {
    label: String,
    command: String,
    #[serde(default)]
    args: Vec<String>,
    cwd: Option<String>,
    #[serde(default)]
    env: BTreeMap<String, String>,
}

struct Project {
    root: PathBuf,
    main: PathBuf,
    repo: String,
    worktree: Option<String>,
}

#[derive(Debug, PartialEq, Serialize)]
struct Entry {
    name: String,
    label: String,
    domain: String,
    command: String,
    cwd: String,
    needs: Option<String>,
}

struct Resolved {
    entry: Entry,
    argv: Vec<String>,
}

enum Format {
    Table,
    Json,
}

pub fn run(args: &[&str]) -> Result<(), String> {
    let project = locate()?;
    let tasks = read_tasks(&project)?;
    let resolved = resolve_all(&project, tasks)?;
    match args {
        [] => list(&resolved, Format::Table),
        ["--json"] => list(&resolved, Format::Json),
        [name] => start(&project, resolved, name),
        _ => Err("usage: foster task [<name> | --json]".to_string()),
    }
}

fn list(resolved: &[Resolved], format: Format) -> Result<(), String> {
    match format {
        Format::Json => {
            let mut entries = Vec::new();
            for Resolved { entry, argv: _ } in resolved {
                entries.push(entry);
            }
            let json = serde_json::to_string(&entries).map_err(|error| format!("json: {error}"))?;
            emit(format!("{json}\n").as_bytes());
        }
        Format::Table => {
            let mut lines = Vec::new();
            for Resolved { entry, argv: _ } in resolved {
                let label = match &entry.needs {
                    Some(variable) => format!("{} (needs Zed: ${variable})", entry.label),
                    None => entry.label.clone(),
                };
                lines.push(vec![
                    entry.name.clone(),
                    label,
                    entry.domain.clone(),
                    entry.command.clone(),
                ]);
            }
            let table = table::render(&HEADER, &lines, None, table::terminal_width());
            emit(table.as_bytes());
        }
    }
    Ok(())
}

fn text_of(characters: &[char]) -> String {
    let mut text = String::new();
    for character in characters {
        text.push(*character);
    }
    text
}

fn start(project: &Project, resolved: Vec<Resolved>, name: &str) -> Result<(), String> {
    let wanted = qualified(project, name);
    let mut found = None;
    for Resolved { entry, argv } in resolved {
        if entry.name == wanted || entry.name == name {
            found = Some((entry, argv));
        }
    }
    let Some((entry, argv)) = found else {
        return Err(format!(
            "no task {name} in {}: run foster task to list them",
            project.root.join(TASKS_FILE).display()
        ));
    };
    if let Some(variable) = entry.needs {
        return Err(format!("task \"{}\" needs Zed (${variable})", entry.label));
    }
    std::env::set_current_dir(&entry.cwd).map_err(|error| format!("cd {}: {error}", entry.cwd))?;
    run::start(&entry.name, argv, entry.domain)
}

fn locate() -> Result<Project, String> {
    let cwd = std::env::current_dir().map_err(|error| format!("current_dir: {error}"))?;
    if let Some(root) = git(&cwd, &["rev-parse", "--show-toplevel"]) {
        let root = PathBuf::from(root);
        let common = git(
            &cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        );
        let main = match common {
            Some(common) => match Path::new(&common).parent() {
                Some(parent) => parent.to_path_buf(),
                None => root.clone(),
            },
            None => root.clone(),
        };
        if root.join(TASKS_FILE).is_file() {
            let repo = repo_name(&main);
            let worktree = if root == main {
                None
            } else {
                Some(route::label(&repo_name(&root)))
            };
            return Ok(Project {
                root,
                main,
                repo,
                worktree,
            });
        }
    }
    Err(format!("no {TASKS_FILE} in this project"))
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    Some(text.to_string())
}

fn repo_name(path: &Path) -> String {
    let name = match path.file_name() {
        Some(name) => name.to_string_lossy().into_owned(),
        None => "project".to_string(),
    };
    let mut cleaned = String::new();
    for character in name.chars() {
        if character.is_ascii_alphanumeric() || "-_.".contains(character) {
            cleaned.push(character);
        } else {
            cleaned.push('-');
        }
    }
    cleaned
}

fn read_tasks(project: &Project) -> Result<Vec<ZedTask>, String> {
    let path = project.root.join(TASKS_FILE);
    let text = std::fs::read_to_string(&path)
        .map_err(|error| format!("read {}: {error}", path.display()))?;
    json5::from_str(&text).map_err(|error| format!("parse {}: {error}", path.display()))
}

fn slug(label: &str) -> String {
    let mut slug = String::new();
    for character in label.chars() {
        if character.is_ascii_alphanumeric() {
            slug.push(character.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

fn qualified(project: &Project, name: &str) -> String {
    match &project.worktree {
        Some(worktree) => format!("{}:{worktree}:{name}", project.repo),
        None => format!("{}:{name}", project.repo),
    }
}

fn expand(text: &str, project: &Project) -> Result<String, String> {
    let mut characters = Vec::new();
    for character in text.chars() {
        characters.push(character);
    }
    let mut expanded = String::new();
    let mut index = 0;
    while index < characters.len() {
        if characters[index] != '$' {
            expanded.push(characters[index]);
            index += 1;
            continue;
        }
        let braced = characters.get(index + 1) == Some(&'{');
        let start = if braced { index + 2 } else { index + 1 };
        let mut end = start;
        while end < characters.len()
            && (characters[end].is_ascii_alphanumeric() || characters[end] == '_')
        {
            end += 1;
        }
        let variable = text_of(&characters[start..end]);
        let mut default = None;
        let mut after = end;
        if braced {
            let mut close = end;
            while close < characters.len() && characters[close] != '}' {
                close += 1;
            }
            if close == characters.len() {
                expanded.push('$');
                index += 1;
                continue;
            }
            if characters.get(end) == Some(&':') {
                default = Some(text_of(&characters[end + 1..close]));
            }
            after = close + 1;
        }
        if !variable.starts_with("ZED_") {
            for character in &characters[index..after] {
                expanded.push(*character);
            }
            index = after;
            continue;
        }
        let value = match variable.as_str() {
            WORKTREE_ROOT => project.root.display().to_string(),
            MAIN_WORKTREE => project.main.display().to_string(),
            _ => match default {
                Some(default) => default,
                None => return Err(variable),
            },
        };
        expanded.push_str(&value);
        index = after;
    }
    Ok(expanded)
}

fn resolve_all(project: &Project, tasks: Vec<ZedTask>) -> Result<Vec<Resolved>, String> {
    let mut resolved: Vec<Resolved> = Vec::new();
    for task in tasks {
        let next = resolve(project, task)?;
        for existing in &resolved {
            if existing.entry.name == next.entry.name {
                return Err(format!(
                    "tasks \"{}\" and \"{}\" both map to {}: rename one label",
                    existing.entry.label, next.entry.label, next.entry.name
                ));
            }
        }
        resolved.push(next);
    }
    Ok(resolved)
}

fn resolve(project: &Project, task: ZedTask) -> Result<Resolved, String> {
    let ZedTask {
        label,
        command,
        args,
        cwd,
        env,
    } = task;
    let slug = slug(&label);
    let name = qualified(project, &slug);
    let mut needs = None;
    let mut expand_or_note = |text: &str| match expand(text, project) {
        Ok(text) => text,
        Err(variable) => {
            needs.get_or_insert(variable);
            text.to_string()
        }
    };
    let mut line = expand_or_note(&command);
    let mut expanded_args = Vec::new();
    for arg in &args {
        expanded_args.push(expand_or_note(arg));
    }
    if !expanded_args.is_empty() {
        line.push(' ');
        line.push_str(&render_command(&expanded_args));
    }
    let cwd = match cwd {
        Some(cwd) => expand_or_note(&cwd),
        None => project.root.display().to_string(),
    };
    let mut base = slug;
    let mut assignments = Vec::new();
    for (key, value) in &env {
        let value = expand_or_note(value);
        if key == DOMAIN_VARIABLE {
            base = value;
        } else {
            assignments.push(format!("{key}={value}"));
        }
    }
    let domain = route::domain(&base, project.worktree.as_deref())
        .map_err(|error| format!("task \"{label}\": {error}"))?;
    let mut argv = Vec::new();
    if !assignments.is_empty() {
        argv.push("env".to_string());
        argv.extend(assignments);
    }
    argv.push(TASK_SHELL.to_string());
    argv.push("-c".to_string());
    argv.push(line.clone());
    Ok(Resolved {
        entry: Entry {
            name,
            label,
            domain,
            command: line,
            cwd,
            needs,
        },
        argv,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project() -> Project {
        Project {
            root: PathBuf::from("/r/repo"),
            main: PathBuf::from("/r/repo"),
            repo: "repo".to_string(),
            worktree: None,
        }
    }

    fn worktree_project() -> Project {
        Project {
            root: PathBuf::from("/w/feat-x"),
            main: PathBuf::from("/r/repo"),
            repo: "repo".to_string(),
            worktree: Some("feat-x".to_string()),
        }
    }

    #[test]
    fn slug_lowercases_and_collapses_separators() {
        assert_eq!(
            slug("ui: dev (proxy → prod farm)"),
            "ui-dev-proxy-prod-farm"
        );
        assert_eq!(slug("yotei-app"), "yotei-app");
        assert_eq!(slug("  Dev: Server!! "), "dev-server");
    }

    #[test]
    fn expands_known_zed_variables_in_both_forms() {
        let project = worktree_project();
        assert_eq!(
            expand("$ZED_WORKTREE_ROOT/app", &project),
            Ok("/w/feat-x/app".to_string())
        );
        assert_eq!(
            expand("${ZED_MAIN_GIT_WORKTREE}/x", &project),
            Ok("/r/repo/x".to_string())
        );
    }

    #[test]
    fn uses_the_default_of_an_unknown_zed_variable() {
        assert_eq!(
            expand("go test ${ZED_STEM:all}", &project()),
            Ok("go test all".to_string())
        );
    }

    #[test]
    fn refuses_an_unknown_zed_variable_without_default() {
        assert_eq!(
            expand("go test ./$ZED_DIRNAME/...", &project()),
            Err("ZED_DIRNAME".to_string())
        );
    }

    #[test]
    fn leaves_other_variables_to_the_shell() {
        assert_eq!(
            expand("echo $HOME ${PORT:-3000} $", &project()),
            Ok("echo $HOME ${PORT:-3000} $".to_string())
        );
    }

    #[test]
    fn parses_zed_tasks_with_comments_and_trailing_commas() {
        let text = r#"[
            /* a dev server */
            {
                "label": "web",
                "command": "pnpm dev",
                "cwd": "$ZED_WORKTREE_ROOT/web",
                "env": { "PORT": "5173", },
                "use_new_terminal": true,
            },
        ]"#;
        let tasks: Vec<ZedTask> = json5::from_str(text).expect("parse");
        assert_eq!(tasks.len(), 1);
        let resolved =
            resolve(&project(), tasks.into_iter().next().expect("task")).expect("resolve");
        assert_eq!(resolved.entry.name, "repo:web");
        assert_eq!(resolved.entry.domain, "web.localhost");
        assert_eq!(resolved.entry.cwd, "/r/repo/web");
        assert_eq!(resolved.entry.needs, None);
        assert_eq!(
            resolved.argv,
            vec!["env", "PORT=5173", "/bin/sh", "-c", "pnpm dev"]
        );
    }

    #[test]
    fn appends_args_quoted() {
        let task = ZedTask {
            label: "genesis".to_string(),
            command: "go run ./cmd".to_string(),
            args: vec!["--chat".to_string(), "two words".to_string()],
            cwd: None,
            env: BTreeMap::new(),
        };
        let resolved = resolve(&project(), task).expect("resolve");
        assert_eq!(resolved.entry.command, "go run ./cmd --chat 'two words'");
        assert_eq!(resolved.entry.cwd, "/r/repo");
    }

    fn served(label: &str, env: &[(&str, &str)]) -> ZedTask {
        let mut map = BTreeMap::new();
        for (key, value) in env {
            map.insert(key.to_string(), value.to_string());
        }
        ZedTask {
            label: label.to_string(),
            command: "pnpm dev".to_string(),
            args: Vec::new(),
            cwd: None,
            env: map,
        }
    }

    #[test]
    fn a_linked_worktree_gets_its_own_name_and_domain() {
        let resolved = resolve(&worktree_project(), served("yotei-app", &[])).expect("resolve");
        assert_eq!(resolved.entry.name, "repo:feat-x:yotei-app");
        assert_eq!(resolved.entry.domain, "feat-x.yotei-app.localhost");
        assert_eq!(resolved.entry.cwd, "/w/feat-x");
    }

    #[test]
    fn foster_domain_names_the_domain_and_stays_out_of_the_command_env() {
        let task = served("yotei-app", &[("FOSTER_DOMAIN", "yotei"), ("MODE", "dev")]);
        let main = resolve(&project(), task).expect("resolve");
        assert_eq!(main.entry.domain, "yotei.localhost");
        assert_eq!(
            main.argv,
            vec!["env", "MODE=dev", "/bin/sh", "-c", "pnpm dev"]
        );
        let task = served("yotei-app", &[("FOSTER_DOMAIN", "yotei.localhost")]);
        let linked = resolve(&worktree_project(), task).expect("resolve");
        assert_eq!(linked.entry.domain, "feat-x.yotei.localhost");
        assert_eq!(linked.argv, vec!["/bin/sh", "-c", "pnpm dev"]);
    }

    #[test]
    fn rejects_two_labels_with_the_same_slug() {
        let task = |label: &str| ZedTask {
            label: label.to_string(),
            command: "true".to_string(),
            args: Vec::new(),
            cwd: None,
            env: BTreeMap::new(),
        };
        let error = resolve_all(&project(), vec![task("ui: dev"), task("UI dev")])
            .err()
            .expect("collision");
        assert!(error.contains("both map to repo:ui-dev"), "{error}");
    }
}
