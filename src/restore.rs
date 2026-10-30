use crate::name::SessionName;
use crate::route::{self, Claims};
use crate::run::hold_argv;
use crate::state::{self, Record};
use crate::zmx::Zmx;

pub fn run(args: &[&str]) -> Result<(), String> {
    let [] = args else {
        return Err("usage: foster restore".to_string());
    };
    let zmx = Zmx::locate()?;
    let mut failed = 0;
    let records = state::load_all()?;
    let mut assigned = Vec::new();
    for record in records.clone() {
        if record.exit_code.is_some() {
            continue;
        }
        match restore(&zmx, &records, &mut assigned, record) {
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

fn restore(
    zmx: &Zmx,
    records: &[Record],
    assigned: &mut Vec<u16>,
    record: Record,
) -> Result<Outcome, String> {
    let Record {
        name,
        command,
        cwd,
        started: _,
        exit_code: _,
        domain,
        port,
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
    let domain = match domain {
        Some(domain) => domain,
        None => route::domain_of_name(&name)?,
    };
    let Claims {
        mut ports,
        owner: _,
    } = route::claims(records, &name, &domain);
    ports.extend_from_slice(assigned);
    let port = route::pick_port(&ports, port).map_err(|error| format!("{name}: {error}"))?;
    assigned.push(port);
    let argv = hold_argv(&session, &command)?;
    state::save(&Record {
        name: name.clone(),
        command,
        cwd: cwd.clone(),
        started: state::now(),
        exit_code: None,
        domain: Some(domain),
        port: Some(port),
    })?;
    zmx.create_detached(&session, &argv, &cwd)
        .map_err(|error| format!("{name}: {error}"))?;
    Ok(Outcome::Started(name))
}
