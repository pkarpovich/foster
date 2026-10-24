use crate::name::SessionName;
use crate::zmx::Zmx;

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name] = args else {
        return Err("usage: foster rm <name>".to_string());
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
    zmx.kill(&name)?;
    println!("removed {}", name.as_str());
    Ok(())
}
