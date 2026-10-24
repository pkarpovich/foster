use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;

use crate::zmx::{Session, Zmx};

const HEADER: [&str; 6] = ["NAME", "STATUS", "ATTACHED", "STARTED", "CWD", "COMMAND"];

#[derive(Debug, PartialEq, Serialize)]
struct Row {
    name: String,
    status: String,
    exit_code: Option<i64>,
    attached: bool,
    started: Option<i64>,
    cwd: Option<PathBuf>,
    command: Option<String>,
}

enum Format {
    Table,
    Json,
}

pub fn run(args: &[&str]) -> Result<(), String> {
    let format = match args {
        [] => Format::Table,
        ["--json"] => Format::Json,
        _ => return Err("usage: foster ls [--json]".to_string()),
    };
    let zmx = Zmx::locate()?;
    let mut rows = Vec::new();
    for session in zmx.sessions()? {
        rows.push(row(session));
    }
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    match format {
        Format::Table => {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            print!("{}", render_table(&rows, home.as_deref()));
        }
        Format::Json => {
            let json = serde_json::to_string(&rows).map_err(|error| format!("json: {error}"))?;
            println!("{json}");
        }
    }
    Ok(())
}

fn user_command(cmd: &str) -> String {
    match cmd.split_once(" -- ") {
        Some((_, command)) => command.to_string(),
        None => cmd.to_string(),
    }
}

fn row(session: Session) -> Row {
    match session {
        Session::Broken { name } => Row {
            name,
            status: "error".to_string(),
            exit_code: None,
            attached: false,
            started: None,
            cwd: None,
            command: None,
        },
        Session::Live {
            name,
            clients,
            created,
            cwd,
            cmd,
            labels,
        } => {
            let code = labels.get("exit_code");
            let status = match code {
                Some(code) => format!("exited {code}"),
                None => "running".to_string(),
            };
            let exit_code = code.and_then(|code| code.parse().ok());
            let command = cmd.map(|cmd| user_command(&cmd));
            Row {
                name,
                status,
                exit_code,
                attached: clients > 0,
                started: Some(created),
                cwd,
                command,
            }
        }
    }
}

fn format_started(epoch: i64) -> String {
    let Ok(timestamp) = Timestamp::from_second(epoch) else {
        return "-".to_string();
    };
    let zoned = timestamp.to_zoned(TimeZone::system());
    zoned.strftime("%Y-%m-%d %H:%M").to_string()
}

fn format_cwd(cwd: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return cwd.display().to_string();
    };
    let Ok(rest) = cwd.strip_prefix(home) else {
        return cwd.display().to_string();
    };
    if rest.as_os_str().is_empty() {
        return "~".to_string();
    }
    format!("~/{}", rest.display())
}

fn render_table(rows: &[Row], home: Option<&Path>) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut cells = vec![HEADER.map(str::to_string)];
    for Row {
        name,
        status,
        exit_code: _,
        attached,
        started,
        cwd,
        command,
    } in rows
    {
        let attached = if *attached { "yes" } else { "no" };
        let started = match started {
            Some(epoch) => format_started(*epoch),
            None => "-".to_string(),
        };
        let cwd = match cwd {
            Some(cwd) => format_cwd(cwd, home),
            None => "-".to_string(),
        };
        let command = match command {
            Some(command) => command.clone(),
            None => "-".to_string(),
        };
        cells.push([
            name.clone(),
            status.clone(),
            attached.to_string(),
            started,
            cwd,
            command,
        ]);
    }
    let mut widths = [0usize; 5];
    for line in &cells {
        for (index, width) in widths.iter_mut().enumerate() {
            *width = (*width).max(line[index].chars().count());
        }
    }
    let mut table = String::new();
    for line in &cells {
        for (index, width) in widths.iter().enumerate() {
            table.push_str(&line[index]);
            for _ in line[index].chars().count()..*width + 2 {
                table.push(' ');
            }
        }
        table.push_str(&line[5]);
        table.push('\n');
    }
    table
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn live(cmd: Option<&str>, cwd: Option<&str>, exit_code: Option<&str>) -> Session {
        let mut labels = BTreeMap::new();
        if let Some(code) = exit_code {
            labels.insert("exit_code".to_string(), code.to_string());
        }
        Session::Live {
            name: "web".to_string(),
            clients: 1,
            created: 1792866550,
            cwd: cwd.map(PathBuf::from),
            cmd: cmd.map(str::to_string),
            labels,
        }
    }

    #[test]
    fn running_row() {
        let session = live(
            Some("/x/foster hold web -- sh -c 'a -- b'"),
            Some("/p"),
            None,
        );
        assert_eq!(
            row(session),
            Row {
                name: "web".to_string(),
                status: "running".to_string(),
                exit_code: None,
                attached: true,
                started: Some(1792866550),
                cwd: Some(PathBuf::from("/p")),
                command: Some("sh -c 'a -- b'".to_string()),
            }
        );
    }

    #[test]
    fn exited_row() {
        let row = row(live(Some("plain cmd"), None, Some("3")));
        assert_eq!(row.status, "exited 3");
        assert_eq!(row.exit_code, Some(3));
        assert_eq!(row.command, Some("plain cmd".to_string()));
    }

    #[test]
    fn broken_row_has_no_values() {
        let session = Session::Broken {
            name: "bad".to_string(),
        };
        assert_eq!(
            row(session),
            Row {
                name: "bad".to_string(),
                status: "error".to_string(),
                exit_code: None,
                attached: false,
                started: None,
                cwd: None,
                command: None,
            }
        );
    }

    #[test]
    fn cwd_is_shortened_under_home_only() {
        let home = Path::new("/Users/me");
        assert_eq!(
            format_cwd(Path::new("/Users/me/proj"), Some(home)),
            "~/proj"
        );
        assert_eq!(format_cwd(Path::new("/Users/me"), Some(home)), "~");
        assert_eq!(
            format_cwd(Path::new("/Users/meow/x"), Some(home)),
            "/Users/meow/x"
        );
        assert_eq!(format_cwd(Path::new("/tmp/x"), Some(home)), "/tmp/x");
        assert_eq!(format_cwd(Path::new("/tmp/x"), None), "/tmp/x");
    }

    #[test]
    fn table_aligns_without_trailing_spaces() {
        let rows = vec![
            Row {
                name: "web".to_string(),
                status: "running".to_string(),
                exit_code: None,
                attached: false,
                started: None,
                cwd: Some(PathBuf::from("/h/p")),
                command: Some("pnpm dev".to_string()),
            },
            Row {
                name: "bad".to_string(),
                status: "error".to_string(),
                exit_code: None,
                attached: false,
                started: None,
                cwd: None,
                command: None,
            },
        ];
        let expected = "NAME  STATUS   ATTACHED  STARTED  CWD  COMMAND\n\
                        web   running  no        -        ~/p  pnpm dev\n\
                        bad   error    no        -        -    -\n";
        let table = render_table(&rows, Some(Path::new("/h")));
        assert_eq!(table, expected);
        for line in table.lines() {
            assert_eq!(line, line.trim_end());
        }
    }

    #[test]
    fn started_has_minute_precision() {
        assert_eq!(format_started(1792866550).len(), "2026-10-24 18:29".len());
    }

    #[test]
    fn empty_list_renders_nothing() {
        assert_eq!(render_table(&[], None), "");
    }
}
