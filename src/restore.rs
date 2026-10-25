use crate::name::SessionName;
use crate::run::hold_argv;
use crate::state::{self, Record};
use crate::zmx::Zmx;

pub fn run(args: &[&str]) -> Result<(), String> {
    let [] = args else {
        return Err("usage: foster restore".to_string());
    };
    let zmx = Zmx::locate()?;
    let mut failed = 0;
    for record in state::load_all()? {
        if record.exit_code.is_some() {
            continue;
        }
        match restore(&zmx, record) {
            Ok(Outcome::Started(name)) => println!("restored {name}"),
            Ok(Outcome::Running(name)) => println!("skipped {name}: already running"),
            Err(error) => {
                eprintln!("foster: {error}");
                failed += 1;
            }
        }
    }
    if failed > 0 {
        return Err(format!("{failed} not restored"));
    }
    Ok(())
}

enum Outcome {
    Started(String),
    Running(String),
}

fn restore(zmx: &Zmx, record: Record) -> Result<Outcome, String> {
    let Record {
        name,
        command,
        cwd,
        started: _,
        exit_code: _,
    } = record;
    let session = SessionName::parse(&name)?;
    let _lock = zmx.lock(&session)?;
    for existing in zmx.sessions()? {
        if existing.name() == name && existing.is_running() {
            return Ok(Outcome::Running(name));
        }
    }
    if !cwd.is_dir() {
        return Err(format!("{name}: directory {} is gone", cwd.display()));
    }
    let argv = hold_argv(&session, &command)?;
    state::save(&Record {
        name: name.clone(),
        command,
        cwd: cwd.clone(),
        started: state::now(),
        exit_code: None,
    })?;
    zmx.create_detached(&session, &argv, &cwd)
        .map_err(|error| format!("{name}: {error}"))?;
    Ok(Outcome::Started(name))
}
