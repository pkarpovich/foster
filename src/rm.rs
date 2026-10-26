use crate::name::SessionName;
use crate::state;
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
    if let Some(mode) = found {
        zmx.kill(&name, mode)?;
    }
    let had_record = state::remove(&name)?;
    if found.is_none() && !had_record {
        return Err(format!("no session {}", name.as_str()));
    }
    println!("removed {}", name.as_str());
    Ok(())
}
