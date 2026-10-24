use std::io::IsTerminal;

use crate::name::SessionName;
use crate::proxy;
use crate::zmx::{Session, Zmx};

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name] = args else {
        return Err("usage: foster attach <name>".to_string());
    };
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    let mut found = None;
    for session in zmx.sessions()? {
        if session.name() == name.as_str() {
            found = Some(session);
        }
    }
    let Some(session) = found else {
        return Err(format!("no session {}", name.as_str()));
    };
    match session {
        Session::Live {
            name: _,
            clients: _,
            created: _,
            cwd: _,
            cmd: _,
            labels: _,
        } => {}
        Session::Broken { name: _ } => {
            return Err(format!(
                "session {} is broken: run foster rm {}",
                name.as_str(),
                name.as_str()
            ));
        }
    }
    if !std::io::stdin().is_terminal() {
        return Err("attach needs a terminal".to_string());
    }
    proxy::attach(&zmx, &name, &[])
}
