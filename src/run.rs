use std::io::IsTerminal;

use crate::name::SessionName;
use crate::proxy;
use crate::route::{self, Claims};
use crate::state::{self, Record};
use crate::zmx::{Session, Zmx};

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name, "--", command @ ..] = args else {
        return Err(usage());
    };
    if command.is_empty() {
        return Err(usage());
    }
    SessionName::parse(name)?;
    let domain = route::domain_of_name(name)?;
    let mut words = Vec::new();
    for word in command {
        words.push(word.to_string());
    }
    start(name, words, domain)
}

pub fn start(name: &str, command: Vec<String>, domain: String) -> Result<(), String> {
    for word in &command {
        if word.chars().any(char::is_control) {
            return Err(format!(
                "command word {word:?} has a control character, which the session list cannot hold"
            ));
        }
    }
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    let cwd = std::env::current_dir().map_err(|error| format!("current_dir: {error}"))?;
    let lock = zmx.lock(&name)?;
    for session in zmx.sessions()? {
        if session.name() != name.as_str() {
            continue;
        }
        if session.is_running() {
            drop(lock);
            return join(&zmx, &name, &session);
        }
        zmx.kill(&name, session.kill_mode())?;
    }
    let records = state::load_all()?;
    let Claims { ports, owner } = route::claims(&records, name.as_str(), &domain);
    if let Some(owner) = owner {
        return Err(format!(
            "domain {domain} belongs to {owner}: set FOSTER_DOMAIN in the task or foster rm {owner}"
        ));
    }
    let port = route::pick_port(&ports, None)?;
    let argv = hold_argv(&name, &command)?;
    state::save(&Record {
        name: name.as_str().to_string(),
        command,
        cwd: cwd.clone(),
        started: state::now(),
        exit_code: None,
        domain: Some(domain.clone()),
        port: Some(port),
    })?;
    if std::io::stdin().is_terminal() {
        drop(lock);
        return proxy::attach(&zmx, &name, &argv);
    }
    zmx.create_detached(&name, &argv, &cwd)?;
    println!(
        "started {} on https://{domain} (port {port})",
        name.as_str()
    );
    Ok(())
}

pub fn hold_argv(name: &SessionName, command: &[String]) -> Result<Vec<String>, String> {
    let current = std::env::current_exe().map_err(|error| format!("current_exe: {error}"))?;
    let mut argv = vec![
        current.to_string_lossy().into_owned(),
        "hold".to_string(),
        name.as_str().to_string(),
        "--".to_string(),
    ];
    argv.extend_from_slice(command);
    Ok(argv)
}

fn join(zmx: &Zmx, name: &SessionName, session: &Session) -> Result<(), String> {
    if std::io::stdin().is_terminal() {
        return proxy::attach(zmx, name, &[]);
    }
    match session.launched() {
        Some(command) => println!("already running {}: {command}", name.as_str()),
        None => println!("already running {}", name.as_str()),
    }
    Ok(())
}

fn usage() -> String {
    "usage: foster run <name> -- <command...>".to_string()
}
