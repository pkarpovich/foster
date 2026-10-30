use crate::name::SessionName;
use crate::route::{self, Event};
use crate::state;
use crate::zmx::Zmx;

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name] = args else {
        return Err("usage: foster rm <name>".to_string());
    };
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    let lock = zmx.lock(&name)?;
    let mut found = None;
    for session in zmx.sessions()? {
        if session.name() == name.as_str() {
            found = Some(session.kill_mode());
        }
    }
    if let Some(mode) = found {
        zmx.kill(&name, mode)?;
    }
    let (domain, port) = match state::load(&name) {
        Ok(Some(record)) => (record.domain, record.port),
        Ok(None) | Err(_) => (None, None),
    };
    let had_record = state::remove(&name)?;
    drop(lock);
    if found.is_none() && !had_record {
        return Err(format!("no session {}", name.as_str()));
    }
    route::run_hook(Event::Remove, name.as_str(), domain.as_deref(), port);
    println!("removed {}", name.as_str());
    Ok(())
}
