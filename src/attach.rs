use std::io::IsTerminal;
use std::os::unix::process::CommandExt;

use crate::name::SessionName;
use crate::zmx::Zmx;

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name] = args else {
        return Err("usage: foster attach <name>".to_string());
    };
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    let mut exists = false;
    for session in zmx.sessions()? {
        if session.name() == name.as_str() {
            exists = true;
        }
    }
    if !exists {
        return Err(format!("no session {}", name.as_str()));
    }
    if !std::io::stdin().is_terminal() {
        return Err("attach needs a terminal".to_string());
    }
    let error = zmx.attach_command(&name).exec();
    Err(format!("exec foster-zmx: {error}"))
}
