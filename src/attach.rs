use std::io::IsTerminal;

use crate::name::SessionName;
use crate::proxy;
use crate::zmx::Zmx;

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name] = args else {
        return Err("usage: foster attach <name>".to_string());
    };
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    zmx.require_live(&name)?;
    if !std::io::stdin().is_terminal() {
        return Err("attach needs a terminal".to_string());
    }
    proxy::attach(&zmx, &name, &[])
}
