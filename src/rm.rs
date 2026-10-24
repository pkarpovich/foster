use crate::name::SessionName;
use crate::zmx::{Session, Zmx};

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name] = args else {
        return Err("usage: foster rm <name>".to_string());
    };
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    let mut found = None;
    for session in zmx.sessions()? {
        if session.name() == name.as_str() {
            found = Some(matches!(session, Session::Broken { .. }));
        }
    }
    let Some(broken) = found else {
        return Err(format!("no session {}", name.as_str()));
    };
    zmx.kill(&name, broken)?;
    println!("removed {}", name.as_str());
    Ok(())
}
