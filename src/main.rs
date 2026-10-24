use std::process::ExitCode;

mod attach;
mod hold;
mod list;
mod name;
mod rm;
mod run;
mod zmx;

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
        ["hold", rest @ ..] => hold::run(rest),
        ["rm", rest @ ..] => rm::run(rest),
        ["run", rest @ ..] => run::run(rest),
        ["ls", rest @ ..] => list::run(rest),
        ["attach", rest @ ..] => attach::run(rest),
        [command, ..] => Err(format!("unknown command \"{command}\"\n\n{USAGE}")),
    }
}

fn main() -> ExitCode {
    let mut args = Vec::new();
    for arg in std::env::args_os().skip(1) {
        let Ok(arg) = arg.into_string() else {
            eprintln!("foster: arguments must be valid UTF-8");
            return ExitCode::FAILURE;
        };
        args.push(arg);
    }
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
