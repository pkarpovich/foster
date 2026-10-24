use crate::name::SessionName;
use crate::zmx::Zmx;

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name] = args else {
        return Err("usage: foster rm <name>".to_string());
    };
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    let _lock = zmx.lock(&name)?;
    let mut found = None;
    for session in zmx.sessions()? {
        if session.name() == name.as_str() {
            found = Some(session.kill_mode());
        }
    }
    let Some(mode) = found else {
        return Err(format!("no session {}", name.as_str()));
    };
    zmx.kill(&name, mode)?;
    println!("removed {}", name.as_str());
    Ok(())
}
