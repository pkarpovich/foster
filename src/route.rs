use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::process::Command;

use crate::state::Record;

const SUFFIX: &str = ".localhost";
const FIRST_PORT: u16 = 51000;
const LAST_PORT: u16 = 51999;
const PATH_PUNCTUATION: &str = "-._~/%:@+;=!$&*";

pub enum Event {
    Start,
    Exit,
    Remove,
}

impl Event {
    fn as_str(&self) -> &'static str {
        match self {
            Event::Start => "start",
            Event::Exit => "exit",
            Event::Remove => "rm",
        }
    }
}

pub fn label(text: &str) -> String {
    let mut label = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            label.push(character.to_ascii_lowercase());
        } else if !label.ends_with('-') {
            label.push('-');
        }
    }
    label.trim_matches('-').to_string()
}

pub fn domain(base: &str, worktree: Option<&str>) -> Result<String, String> {
    let base = base.strip_suffix(SUFFIX).unwrap_or(base);
    let base = label(base);
    if base.is_empty() {
        return Err("empty domain name".to_string());
    }
    match worktree {
        Some(worktree) => Ok(format!("{}.{base}{SUFFIX}", label(worktree))),
        None => Ok(format!("{base}{SUFFIX}")),
    }
}

pub fn parse_paths(text: &str) -> Result<Vec<String>, String> {
    let mut paths: Vec<String> = Vec::new();
    for word in text.split_whitespace() {
        if !word.starts_with('/') {
            return Err(format!("path prefix {word} does not start with /"));
        }
        for character in word.chars() {
            if !(character.is_ascii_alphanumeric() || PATH_PUNCTUATION.contains(character)) {
                return Err(format!(
                    "path prefix {word} has {character:?}: use letters, digits and {PATH_PUNCTUATION}"
                ));
            }
        }
        let path = word.trim_end_matches('/');
        if path.is_empty() {
            return Err(
                "path prefix / covers the whole domain: leave FOSTER_PATHS out instead".to_string(),
            );
        }
        for known in &paths {
            if overlaps(known, path) {
                return Err(format!("path prefixes {known} and {path} overlap"));
            }
        }
        paths.push(path.to_string());
    }
    if paths.is_empty() {
        return Err("FOSTER_PATHS has no path prefix".to_string());
    }
    Ok(paths)
}

fn overlaps(left: &str, right: &str) -> bool {
    let covers = |outer: &str, inner: &str| {
        inner == outer
            || inner
                .strip_prefix(outer)
                .is_some_and(|rest| rest.starts_with('/'))
    };
    covers(left, right) || covers(right, left)
}

fn shares_any(left: &[String], right: &[String]) -> bool {
    if left.is_empty() || right.is_empty() {
        return left.is_empty() && right.is_empty();
    }
    for outer in left {
        for inner in right {
            if overlaps(outer, inner) {
                return true;
            }
        }
    }
    false
}

pub fn shown(domain: &str, paths: &[String]) -> String {
    if paths.is_empty() {
        return domain.to_string();
    }
    format!("{domain} {}", paths.join(" "))
}

pub fn domain_of_name(name: &str) -> Result<String, String> {
    let last = name.rsplit(':').next().unwrap_or(name);
    domain(last, None)
}

fn is_free(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
        && TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok()
}

pub fn pick_port(taken: &[u16], preferred: Option<u16>) -> Result<u16, String> {
    if let Some(port) = preferred
        && !taken.contains(&port)
        && is_free(port)
    {
        return Ok(port);
    }
    for port in FIRST_PORT..=LAST_PORT {
        if !taken.contains(&port) && is_free(port) {
            return Ok(port);
        }
    }
    Err(format!("no free port in {FIRST_PORT}-{LAST_PORT}"))
}

pub struct Claims {
    pub ports: Vec<u16>,
    pub owner: Option<String>,
}

pub fn claims(records: &[Record], name: &str, domain: &str, paths: &[String]) -> Claims {
    let mut ports = Vec::new();
    let mut owner = None;
    for record in records {
        let Record {
            name: other,
            command: _,
            cwd: _,
            started: _,
            exit_code,
            domain: other_domain,
            port,
            paths: other_paths,
        } = record;
        if other == name {
            continue;
        }
        if let Some(port) = port {
            ports.push(*port);
        }
        if exit_code.is_some() {
            continue;
        }
        if other_domain.as_deref() == Some(domain) && shares_any(other_paths, paths) {
            owner = Some(other.clone());
        }
    }
    Claims { ports, owner }
}

fn hook_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let path = PathBuf::from(home).join(".config/foster/hook");
    path.is_file().then_some(path)
}

pub fn run_hook(event: Event, name: &str, domain: Option<&str>, port: Option<u16>) {
    let Some(path) = hook_path() else {
        return;
    };
    let mut command = Command::new(&path);
    command
        .env("FOSTER_EVENT", event.as_str())
        .env("FOSTER_NAME", name)
        .env("FOSTER_DOMAIN", domain.unwrap_or_default())
        .env(
            "FOSTER_PORT",
            port.map(|port| port.to_string()).unwrap_or_default(),
        )
        .env_remove("ZMX_DIR");
    match command.output() {
        Ok(output) if output.status.success() => {}
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            eprintln!(
                "foster: hook {} failed ({}): {}",
                path.display(),
                output.status,
                stderr.trim()
            );
        }
        Err(error) => eprintln!("foster: hook {}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_lowercases_and_collapses_separators() {
        assert_eq!(label("Feat/Yotei App"), "feat-yotei-app");
        assert_eq!(label("a..b__c"), "a-b-c");
        assert_eq!(label("--x--"), "x");
    }

    #[test]
    fn domain_adds_the_worktree_in_front() {
        assert_eq!(domain("yotei", None), Ok("yotei.localhost".to_string()));
        assert_eq!(
            domain("yotei.localhost", Some("feat-x")),
            Ok("feat-x.yotei.localhost".to_string())
        );
        assert_eq!(
            domain("Web: Dev", None),
            Ok("web-dev.localhost".to_string())
        );
        assert_eq!(domain("..", None), Err("empty domain name".to_string()));
    }

    #[test]
    fn domain_of_a_name_uses_its_last_part() {
        assert_eq!(
            domain_of_name("turtle-hub:yotei-app"),
            Ok("yotei-app.localhost".to_string())
        );
        assert_eq!(domain_of_name("web"), Ok("web.localhost".to_string()));
        assert_eq!(domain_of_name("a:b_c.d"), Ok("b-c-d.localhost".to_string()));
    }

    #[test]
    fn pick_port_skips_taken_and_busy_ports() {
        let busy = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("bind");
        let busy = busy.local_addr().expect("addr").port();
        let port = pick_port(&[FIRST_PORT], None).expect("free port");
        assert!((FIRST_PORT + 1..=LAST_PORT).contains(&port), "{port}");
        assert_ne!(pick_port(&[], Some(busy)).expect("free port"), busy);
    }

    #[test]
    fn pick_port_keeps_a_free_preferred_port() {
        let port = pick_port(&[], None).expect("free port");
        assert_eq!(pick_port(&[], Some(port)), Ok(port));
        assert_ne!(pick_port(&[port], Some(port)), Ok(port));
    }

    fn record(name: &str, domain: &str, port: u16, exit_code: Option<i32>) -> Record {
        routed(name, domain, port, exit_code, &[])
    }

    fn routed(
        name: &str,
        domain: &str,
        port: u16,
        exit_code: Option<i32>,
        paths: &[&str],
    ) -> Record {
        let mut owned = Vec::new();
        for path in paths {
            owned.push(path.to_string());
        }
        Record {
            name: name.to_string(),
            command: vec!["true".to_string()],
            cwd: PathBuf::from("/p"),
            started: 1,
            exit_code,
            domain: Some(domain.to_string()),
            port: Some(port),
            paths: owned,
        }
    }

    fn strings(words: &[&str]) -> Vec<String> {
        let mut owned = Vec::new();
        for word in words {
            owned.push(word.to_string());
        }
        owned
    }

    #[test]
    fn parse_paths_splits_on_whitespace_and_drops_trailing_slashes() {
        assert_eq!(
            parse_paths("  /api   /mcp/\t/.well-known/oauth-protected-resource "),
            Ok(strings(&[
                "/api",
                "/mcp",
                "/.well-known/oauth-protected-resource"
            ]))
        );
    }

    #[test]
    fn parse_paths_refuses_bad_prefixes() {
        assert_eq!(
            parse_paths("api /mcp"),
            Err("path prefix api does not start with /".to_string())
        );
        assert_eq!(
            parse_paths("/api,/mcp"),
            Err(format!(
                "path prefix /api,/mcp has ',': use letters, digits and {PATH_PUNCTUATION}"
            ))
        );
        assert_eq!(
            parse_paths("/a{b}"),
            Err(format!(
                "path prefix /a{{b}} has '{{': use letters, digits and {PATH_PUNCTUATION}"
            ))
        );
        assert_eq!(
            parse_paths("/"),
            Err(
                "path prefix / covers the whole domain: leave FOSTER_PATHS out instead".to_string()
            )
        );
        assert_eq!(
            parse_paths("   "),
            Err("FOSTER_PATHS has no path prefix".to_string())
        );
        assert_eq!(
            parse_paths("/api /api/v2"),
            Err("path prefixes /api and /api/v2 overlap".to_string())
        );
    }

    #[test]
    fn overlap_respects_segment_boundaries() {
        assert!(overlaps("/api", "/api"));
        assert!(overlaps("/api", "/api/v2"));
        assert!(overlaps("/api/v2", "/api"));
        assert!(!overlaps("/api", "/apix"));
        assert!(!overlaps("/api", "/mcp"));
    }

    #[test]
    fn claims_share_a_domain_by_disjoint_paths() {
        let records = vec![
            record("app", "yotei.localhost", 51000, None),
            routed("api", "yotei.localhost", 51001, None, &["/api", "/mcp"]),
        ];
        let Claims { ports: _, owner } = claims(
            &records,
            "health",
            "yotei.localhost",
            &strings(&["/healthz"]),
        );
        assert_eq!(owner, None);
        let Claims { ports: _, owner } =
            claims(&records, "v2", "yotei.localhost", &strings(&["/api/v2"]));
        assert_eq!(owner, Some("api".to_string()));
        let Claims { ports: _, owner } = claims(&records, "app2", "yotei.localhost", &[]);
        assert_eq!(owner, Some("app".to_string()));
    }

    #[test]
    fn claims_keep_the_ports_of_exited_records_but_not_their_domains() {
        let records = vec![
            record("web", "web.localhost", 51000, None),
            record("api", "api.localhost", 51001, None),
            record("old", "web.localhost", 51002, Some(0)),
        ];
        let Claims { ports, owner } = claims(&records, "web", "web.localhost", &[]);
        assert_eq!(ports, vec![51001, 51002]);
        assert_eq!(owner, None);
        let Claims { ports, owner } = claims(&records, "web2", "api.localhost", &[]);
        assert_eq!(ports, vec![51000, 51001, 51002]);
        assert_eq!(owner, Some("api".to_string()));
    }
}
