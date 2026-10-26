use std::io::Write;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::process::{Command, ExitStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rustix::process::{Signal, kill_current_process_group};
use signal_hook::consts::{SIGHUP, SIGINT, SIGQUIT, SIGTERM};
use signal_hook::low_level::register;

use crate::name::SessionName;
use crate::state;
use crate::zmx::{Zmx, clean_env};

static TERMINATING: AtomicBool = AtomicBool::new(false);

const QUOTED: &str = " \t\"'\\$`!(){}[]|&;<>?*~#\n";
const COMMAND_NOT_FOUND: i32 = 127;
const COMMAND_NOT_EXECUTABLE: i32 = 126;

pub fn render_command(words: &[String]) -> String {
    let mut rendered = Vec::new();
    for word in words {
        let mut needs_quotes = word.is_empty();
        for character in word.chars() {
            if QUOTED.contains(character) || character.is_control() {
                needs_quotes = true;
            }
        }
        if needs_quotes {
            rendered.push(format!("'{}'", word.replace('\'', "'\\''")));
        } else {
            rendered.push(word.clone());
        }
    }
    rendered.join(" ")
}

fn install_handlers() -> Result<(), String> {
    let failed = |error| format!("install signal handlers: {error}");
    unsafe {
        register(SIGHUP, || {
            if !TERMINATING.swap(true, Ordering::SeqCst) {
                let _ = kill_current_process_group(Signal::HUP);
            }
        })
        .map_err(failed)?;
        register(SIGTERM, || {
            if !TERMINATING.swap(true, Ordering::SeqCst) {
                let _ = kill_current_process_group(Signal::TERM);
            }
        })
        .map_err(failed)?;
        register(SIGINT, || {}).map_err(failed)?;
        register(SIGQUIT, || {}).map_err(failed)?;
    }
    Ok(())
}

fn exit_code(status: ExitStatus) -> i32 {
    match (status.code(), status.signal()) {
        (Some(code), _) => code,
        (None, Some(signal)) => 128 + signal,
        (None, None) => 1,
    }
}

pub fn run(args: &[&str]) -> Result<(), String> {
    let [name, "--", program, rest @ ..] = args else {
        return Err("usage: foster hold <name> -- <command...>".to_string());
    };
    let name = SessionName::parse(name)?;
    install_handlers()?;
    let mut words = vec![program.to_string()];
    for word in rest {
        words.push(word.to_string());
    }
    let rendered = render_command(&words);
    let (code, line) = match Command::new(&words[0])
        .args(&words[1..])
        .env_remove("ZMX_DIR")
        .status()
    {
        Ok(status) => {
            let code = exit_code(status);
            (code, format!("{rendered} exited with {code}"))
        }
        Err(error) => {
            let code = if error.kind() == std::io::ErrorKind::NotFound {
                COMMAND_NOT_FOUND
            } else {
                COMMAND_NOT_EXECUTABLE
            };
            (code, format!("{rendered}: {error}"))
        }
    };
    if TERMINATING.load(Ordering::SeqCst) {
        std::process::exit(code);
    }
    let ended = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let labels = [
        ("exit_code", code.to_string()),
        ("ended", ended.to_string()),
    ];
    let recorded = Zmx::locate().and_then(|zmx| zmx.set_labels(&name, &labels));
    let _ = state::mark_exited(&name, code);
    let mut stdout = std::io::stdout();
    let _ = write!(stdout, "\r\nfoster: {line}\r\n");
    if let Err(error) = recorded {
        let _ = write!(stdout, "foster: exit status not recorded: {error}\r\n");
    }
    let _ = stdout.flush();
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
    let mut command = Command::new(&shell);
    command.arg("-l").env_remove("ZMX_DIR");
    clean_env(&mut command);
    let error = command.exec();
    Err(format!("exec {shell}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(words: &[&str]) -> String {
        let mut owned = Vec::new();
        for word in words {
            owned.push(word.to_string());
        }
        render_command(&owned)
    }

    #[test]
    fn plain_words_stay_bare() {
        assert_eq!(render(&["pnpm", "dev"]), "pnpm dev");
    }

    #[test]
    fn word_with_space_is_quoted() {
        assert_eq!(render(&["echo", "a b"]), "echo 'a b'");
    }

    #[test]
    fn empty_word_is_quoted() {
        assert_eq!(render(&["echo", ""]), "echo ''");
    }

    #[test]
    fn single_quote_is_escaped() {
        assert_eq!(render(&["echo", "it's"]), "echo 'it'\\''s'");
    }

    #[test]
    fn control_character_is_quoted() {
        assert_eq!(render(&["echo", "a\rb"]), "echo 'a\rb'");
    }

    #[test]
    fn shell_script_example() {
        assert_eq!(
            render(&["sh", "-c", "echo kaboom; exit 3"]),
            "sh -c 'echo kaboom; exit 3'"
        );
    }
}
