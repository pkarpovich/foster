use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::name::SessionName;

const EXECUTABLE: &str = "foster-zmx";
const NOT_FOUND: &str =
    "foster-zmx not found next to foster or on PATH; build it with zmx/build.sh";
const EMPTY_LIST_PREFIX: &str = "no sessions found in";
const REMOVED_VARS: [&str; 14] = [
    "ZMX_SESSION",
    "ZMX_SESSION_PREFIX",
    "ZMX_MANAGED",
    "ZMX_MANAGED_CLAIM",
    "CLAUDECODE",
    "CLAUDE_PID",
    "AI_AGENT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_EXECPATH",
    "CLAUDE_CODE_MESSAGING_SOCKET",
    "CLAUDE_CODE_MESSAGING_TOKEN",
    "CLAUDE_CODE_SESSION_ATTENDED",
    "CLAUDE_CODE_SESSION_ID",
];

#[derive(Debug, PartialEq)]
pub enum Session {
    Live { name: String },
    Broken { name: String },
}

impl Session {
    pub fn name(&self) -> &str {
        match self {
            Session::Live { name } | Session::Broken { name } => name,
        }
    }
}

pub fn clean_env(command: &mut Command) {
    for var in REMOVED_VARS {
        command.env_remove(var);
    }
}

pub struct Zmx {
    executable: PathBuf,
    socket_dir: PathBuf,
}

fn is_executable(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

fn find_executable() -> Result<PathBuf, String> {
    if let Ok(current) = std::env::current_exe()
        && let Some(directory) = current.parent()
    {
        let candidate = directory.join(EXECUTABLE);
        if is_executable(&candidate) {
            return Ok(candidate);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for directory in std::env::split_paths(&path) {
            let candidate = directory.join(EXECUTABLE);
            if is_executable(&candidate) {
                return Ok(candidate);
            }
        }
    }
    Err(NOT_FOUND.to_string())
}

fn parse_line(line: &str) -> Option<Session> {
    if line.starts_with(EMPTY_LIST_PREFIX) {
        return None;
    }
    let mut name = None;
    let mut broken = false;
    for field in line.split('\t') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        match key {
            "name" => name = Some(value.to_string()),
            "err" => broken = true,
            _ => {}
        }
    }
    let name = name?;
    if broken {
        return Some(Session::Broken { name });
    }
    Some(Session::Live { name })
}

pub fn parse_sessions(output: &str) -> Vec<Session> {
    let mut sessions = Vec::new();
    for line in output.lines() {
        if let Some(session) = parse_line(line) {
            sessions.push(session);
        }
    }
    sessions
}

impl Zmx {
    pub fn locate() -> Result<Zmx, String> {
        let executable = find_executable()?;
        let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
        let socket_dir = PathBuf::from(home).join(".local/state/foster/zmx");
        std::fs::create_dir_all(&socket_dir)
            .map_err(|error| format!("create {}: {error}", socket_dir.display()))?;
        Ok(Zmx {
            executable,
            socket_dir,
        })
    }

    fn command(&self) -> Command {
        let mut command = Command::new(&self.executable);
        command.env("ZMX_DIR", &self.socket_dir);
        clean_env(&mut command);
        command
    }

    fn run(&self, action: &str, mut command: Command) -> Result<String, String> {
        let output = command
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("zmx {action}: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!("zmx {action} failed: {}", stderr.trim_end()));
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    pub fn sessions(&self) -> Result<Vec<Session>, String> {
        let mut command = self.command();
        command.arg("ls");
        let stdout = self.run("ls", command)?;
        Ok(parse_sessions(&stdout))
    }

    pub fn set_labels(&self, name: &SessionName, labels: &[(&str, String)]) -> Result<(), String> {
        let mut command = self.command();
        command.arg("set").arg(name.as_str());
        for (key, value) in labels {
            command.arg(format!("{key}={value}"));
        }
        self.run("set", command)?;
        Ok(())
    }

    pub fn kill(&self, name: &SessionName) -> Result<(), String> {
        let mut command = self.command();
        command.arg("kill").arg(name.as_str());
        self.run("kill", command)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_live_line() {
        let line = "name=web\tpid=4242\tclients=0\tcreated=1792866550\tcwd=file://h/x\tcmd=a -- b\tended=1\n";
        let sessions = parse_sessions(line);
        assert_eq!(
            sessions,
            vec![Session::Live {
                name: "web".to_string()
            }]
        );
    }

    #[test]
    fn parses_err_line() {
        let sessions = parse_sessions("name=bad\terr=ConnectionRefused\tstatus=cleaning up\n");
        assert_eq!(
            sessions,
            vec![Session::Broken {
                name: "bad".to_string()
            }]
        );
    }

    #[test]
    fn empty_list_text_gives_no_sessions() {
        assert_eq!(parse_sessions("no sessions found in /tmp/x\n"), vec![]);
        assert_eq!(parse_sessions(""), vec![]);
    }
}
