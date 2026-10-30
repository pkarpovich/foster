use std::net::{Ipv4Addr, TcpListener};
use std::path::PathBuf;
use std::process::Command;

use crate::state::Record;

const SUFFIX: &str = ".localhost";
const FIRST_PORT: u16 = 51000;
const LAST_PORT: u16 = 51999;

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

pub fn claims(records: &[Record], name: &str, domain: &str) -> Claims {
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
        } = record;
        if other == name || exit_code.is_some() {
            continue;
        }
        if let Some(port) = port {
            ports.push(*port);
        }
        if other_domain.as_deref() == Some(domain) {
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
        Record {
            name: name.to_string(),
            command: vec!["true".to_string()],
            cwd: PathBuf::from("/p"),
            started: 1,
            exit_code,
            domain: Some(domain.to_string()),
            port: Some(port),
        }
    }

    #[test]
    fn claims_ignore_the_same_name_and_exited_records() {
        let records = vec![
            record("web", "web.localhost", 51000, None),
            record("api", "api.localhost", 51001, None),
            record("old", "web.localhost", 51002, Some(0)),
        ];
        let Claims { ports, owner } = claims(&records, "web", "web.localhost");
        assert_eq!(ports, vec![51001]);
        assert_eq!(owner, None);
        let Claims { ports, owner } = claims(&records, "web2", "api.localhost");
        assert_eq!(ports, vec![51000, 51001]);
        assert_eq!(owner, Some("api".to_string()));
    }
}
