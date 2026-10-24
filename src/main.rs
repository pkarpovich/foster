use std::process::ExitCode;

const USAGE: &str = "usage:
  foster run <name> -- <command...>   start a command and attach to it
  foster ls [--json]                  list processes started with foster
  foster attach <name>                attach to a process (detach: Ctrl+\\)
  foster rm <name>                    stop a process and remove it";

fn dispatch(args: &[&str]) -> Result<(), String> {
    match args {
        [] | ["help" | "-h" | "--help"] => {
            println!("{USAGE}");
            Ok(())
        }
        ["version" | "-V" | "--version"] => {
            println!("foster {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        ["run" | "ls" | "attach" | "rm", ..] => Err("not implemented".to_string()),
        [command, ..] => Err(format!("unknown command \"{command}\"\n\n{USAGE}")),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut words = Vec::new();
    for arg in &args {
        words.push(arg.as_str());
    }
    match dispatch(&words) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("foster: {message}");
            ExitCode::FAILURE
        }
    }
}
