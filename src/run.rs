use std::io::IsTerminal;
use std::os::unix::process::CommandExt;

use crate::name::SessionName;
use crate::zmx::{Session, Zmx};

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name, "--", command @ ..] = args else {
        return Err(usage());
    };
    if command.is_empty() {
        return Err(usage());
    }
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    for session in zmx.sessions()? {
        if session.name() != name.as_str() {
            continue;
        }
        if session.is_running() {
            return Err(format!("{} is already running", name.as_str()));
        }
        zmx.kill(&name, matches!(session, Session::Broken { .. }))?;
    }
    let current = std::env::current_exe().map_err(|error| format!("current_exe: {error}"))?;
    let mut argv = vec![
        current.to_string_lossy().into_owned(),
        "hold".to_string(),
        name.as_str().to_string(),
        "--".to_string(),
    ];
    for word in command {
        argv.push((*word).to_string());
    }
    if std::io::stdin().is_terminal() {
        let error = zmx.attach_command(&name).args(&argv).exec();
        return Err(format!("exec foster-zmx: {error}"));
    }
    zmx.create_detached(&name, &argv)?;
    println!("started {}", name.as_str());
    Ok(())
}

fn usage() -> String {
    "usage: foster run <name> -- <command...>".to_string()
}
