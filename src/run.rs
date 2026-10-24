use std::io::IsTerminal;

use crate::hold::render_command;
use crate::name::SessionName;
use crate::proxy;
use crate::zmx::{Zmx, encode_word};

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name, "--", command @ ..] = args else {
        return Err(usage());
    };
    if command.is_empty() {
        return Err(usage());
    }
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    let current = std::env::current_exe().map_err(|error| format!("current_exe: {error}"))?;
    let current = current.to_string_lossy().into_owned();
    check_exe_path(&current)?;
    let cwd = std::env::current_dir().map_err(|error| format!("current_dir: {error}"))?;
    let mut encoded = Vec::new();
    for word in command {
        encoded.push(encode_word(word));
    }
    let launch = render_command(&encoded);
    let lock = zmx.lock(&name)?;
    for session in zmx.sessions()? {
        if session.name() != name.as_str() {
            continue;
        }
        if session.is_running() {
            if !session.matches_launch(&launch, &cwd) {
                return Err(format!(
                    "{} is already running a different command",
                    name.as_str()
                ));
            }
            drop(lock);
            return join(&zmx, &name);
        }
        zmx.kill(&name, session.kill_mode())?;
    }
    let mut argv = vec![
        current,
        "hold".to_string(),
        name.as_str().to_string(),
        "--".to_string(),
    ];
    argv.extend(encoded);
    if std::io::stdin().is_terminal() {
        drop(lock);
        return proxy::attach(&zmx, &name, &argv);
    }
    zmx.create_detached(&name, &argv)?;
    println!("started {}", name.as_str());
    Ok(())
}

fn join(zmx: &Zmx, name: &SessionName) -> Result<(), String> {
    if std::io::stdin().is_terminal() {
        return proxy::attach(zmx, name, &[]);
    }
    println!("already running {}", name.as_str());
    Ok(())
}

fn check_exe_path(path: &str) -> Result<(), String> {
    if path.chars().any(char::is_control) || path.contains(" -- ") {
        return Err(format!(
            "foster path {path:?} has a control character or \" -- \": move the binary"
        ));
    }
    Ok(())
}

fn usage() -> String {
    "usage: foster run <name> -- <command...>".to_string()
}

#[cfg(test)]
mod tests {
    use super::check_exe_path;

    #[test]
    fn accepts_plain_path() {
        assert!(check_exe_path("/usr/local/bin/foster").is_ok());
    }

    #[test]
    fn rejects_tab_newline_and_separator() {
        assert!(check_exe_path("/a\texit_code=0/foster").is_err());
        assert!(check_exe_path("/a\nname=x/foster").is_err());
        assert!(check_exe_path("/a -- b/foster").is_err());
    }
}
