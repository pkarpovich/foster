use std::io::IsTerminal;
use std::os::unix::process::CommandExt;

use crate::name::SessionName;
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
    let _lock = zmx.lock(&name)?;
    for session in zmx.sessions()? {
        if session.name() != name.as_str() {
            continue;
        }
        if session.is_running() {
            return Err(format!("{} is already running", name.as_str()));
        }
        zmx.kill(&name, session.kill_mode())?;
    }
    let mut argv = vec![
        current,
        "hold".to_string(),
        name.as_str().to_string(),
        "--".to_string(),
    ];
    for word in command {
        argv.push(encode_word(word));
    }
    if std::io::stdin().is_terminal() {
        let error = zmx.attach_command(&name).args(&argv).exec();
        return Err(format!("exec foster-zmx: {error}"));
    }
    zmx.create_detached(&name, &argv)?;
    println!("started {}", name.as_str());
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
