use std::io::Write;
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::sync::atomic::{AtomicI32, Ordering};

use rustix::process::{Pid, Signal, kill_process};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::low_level::{exit, register};

use crate::name::SessionName;
use crate::zmx::Zmx;

pub fn emit(bytes: &[u8]) {
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(bytes);
    let _ = stdout.flush();
}

static TAIL_PID: AtomicI32 = AtomicI32::new(0);

enum Follow {
    No,
    Yes,
}

pub fn history(args: &[&str]) -> Result<(), String> {
    let (name, follow) = match args {
        [name] => (name, Follow::No),
        [name, "-f" | "--follow"] | ["-f" | "--follow", name] => (name, Follow::Yes),
        _ => return Err("usage: foster history <name> [-f]".to_string()),
    };
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    zmx.require_live(&name)?;
    match follow {
        Follow::No => {
            emit(zmx.history(&name)?.as_bytes());
            Ok(())
        }
        Follow::Yes => follow_history(&zmx, &name),
    }
}

fn follow_history(zmx: &Zmx, name: &SessionName) -> Result<(), String> {
    let mut tail = zmx
        .tail_command(name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|error| format!("start foster-zmx tail: {error}"))?;
    TAIL_PID.store(i32::try_from(tail.id()).unwrap_or(0), Ordering::SeqCst);
    install_handlers()?;
    let Some(mut stream) = tail.stdout.take() else {
        let _ = tail.kill();
        return Err("foster-zmx tail has no stdout".to_string());
    };
    let mut stdout = std::io::stdout();
    let history = match zmx.history(name) {
        Ok(history) => history,
        Err(error) => {
            let _ = tail.kill();
            let _ = tail.wait();
            return Err(error);
        }
    };
    let _ = stdout.write_all(history.as_bytes());
    let _ = stdout.flush();
    let _ = std::io::copy(&mut stream, &mut stdout);
    let _ = tail.kill();
    let _ = tail.wait();
    Ok(())
}

fn stop_tail() {
    if let Some(pid) = Pid::from_raw(TAIL_PID.load(Ordering::SeqCst)) {
        let _ = kill_process(pid, Signal::TERM);
    }
}

fn install_handlers() -> Result<(), String> {
    let failed = |error| format!("install signal handlers: {error}");
    unsafe {
        register(SIGTERM, || {
            stop_tail();
            exit(143);
        })
        .map_err(failed)?;
        register(SIGHUP, || {
            stop_tail();
            exit(129);
        })
        .map_err(failed)?;
        register(SIGINT, || {
            stop_tail();
            exit(130);
        })
        .map_err(failed)?;
    }
    Ok(())
}

pub fn tail(args: &[&str]) -> Result<(), String> {
    let [name] = args else {
        return Err("usage: foster tail <name>".to_string());
    };
    let name = SessionName::parse(name)?;
    let zmx = Zmx::locate()?;
    zmx.require_live(&name)?;
    let error = zmx.tail_command(&name).exec();
    Err(format!("exec foster-zmx: {error}"))
}
