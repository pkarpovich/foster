use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use portable_pty::CommandBuilder;

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

#[derive(Clone, Copy)]
pub enum Claim {
    Take,
    Leave,
}

#[derive(Clone, Copy)]
pub enum KillMode {
    Graceful,
    Force,
}

#[derive(Debug, PartialEq)]
pub enum Session {
    Live {
        name: String,
        clients: u32,
        created: i64,
        cwd: Option<PathBuf>,
        cmd: Option<String>,
        labels: BTreeMap<String, String>,
    },
    Broken {
        name: String,
    },
}

impl Session {
    pub fn name(&self) -> &str {
        match self {
            Session::Live {
                name,
                clients: _,
                created: _,
                cwd: _,
                cmd: _,
                labels: _,
            }
            | Session::Broken { name } => name,
        }
    }

    pub fn kill_mode(&self) -> KillMode {
        match self {
            Session::Live {
                name: _,
                clients: _,
                created: _,
                cwd: _,
                cmd: _,
                labels: _,
            } => KillMode::Graceful,
            Session::Broken { name: _ } => KillMode::Force,
        }
    }

    pub fn launched(&self) -> Option<String> {
        match self {
            Session::Live {
                name: _,
                clients: _,
                created: _,
                cwd: _,
                cmd,
                labels: _,
            } => cmd.as_deref().map(launched_command),
            Session::Broken { name: _ } => None,
        }
    }

    pub fn is_running(&self) -> bool {
        match self {
            Session::Live {
                name: _,
                clients: _,
                created: _,
                cwd: _,
                cmd: _,
                labels,
            } => !labels.contains_key("exit_code"),
            Session::Broken { name: _ } => false,
        }
    }
}

pub struct Nonce(String);

impl Nonce {
    pub fn generate() -> Nonce {
        let mut bytes = [0u8; 16];
        let mut urandom = File::open("/dev/urandom").expect("open /dev/urandom");
        urandom.read_exact(&mut bytes).expect("read /dev/urandom");
        let mut hex = String::new();
        for byte in bytes {
            hex.push_str(&format!("{byte:02x}"));
        }
        Nonce(hex)
    }

    pub fn as_str(&self) -> &str {
        &self.0
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

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn percent_decode(text: &str) -> Vec<u8> {
    let bytes = text.as_bytes();
    let mut decoded = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if byte == b'%'
            && index + 2 < bytes.len()
            && let Some(high) = hex_value(bytes[index + 1])
            && let Some(low) = hex_value(bytes[index + 2])
        {
            decoded.push(high * 16 + low);
            index += 3;
            continue;
        }
        decoded.push(byte);
        index += 1;
    }
    decoded
}

pub fn launched_command(cmd: &str) -> String {
    match cmd.split_once(" -- ") {
        Some((_, command)) => command.to_string(),
        None => cmd.to_string(),
    }
}

fn decode_cwd(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let slash = rest.find('/')?;
    let path = &rest[slash..];
    Some(PathBuf::from(OsString::from_vec(percent_decode(path))))
}

fn parse_line(line: &str) -> Option<Session> {
    if line.starts_with(EMPTY_LIST_PREFIX) {
        return None;
    }
    let mut name = None;
    let mut broken = false;
    let mut clients = 0;
    let mut created = 0;
    let mut cwd = None;
    let mut cmd = None;
    let mut labels = BTreeMap::new();
    for field in line.split('\t') {
        let Some((key, value)) = field.split_once('=') else {
            continue;
        };
        let key = key.trim_start();
        match key {
            "name" => name = Some(value.to_string()),
            "err" => broken = true,
            "clients" => clients = value.parse().unwrap_or(0),
            "created" => created = value.parse().unwrap_or(0),
            "cwd" => cwd = decode_cwd(value),
            "cmd" => cmd = Some(value.to_string()),
            "pid" | "status" => {}
            _ => {
                labels.insert(key.to_string(), value.to_string());
            }
        }
    }
    let name = name?;
    if broken {
        return Some(Session::Broken { name });
    }
    Some(Session::Live {
        name,
        clients,
        created,
        cwd,
        cmd,
        labels,
    })
}

fn starts_record(line: &str) -> bool {
    let line = line.strip_prefix("→").unwrap_or(line);
    line.trim_start().starts_with("name=")
}

pub fn parse_sessions(output: &str) -> Vec<Session> {
    let mut records: Vec<String> = Vec::new();
    for line in output.lines() {
        match records.last_mut() {
            Some(record) if !starts_record(line) && !line.starts_with(EMPTY_LIST_PREFIX) => {
                record.push('\n');
                record.push_str(line);
            }
            _ => records.push(line.to_string()),
        }
    }
    let mut sessions = Vec::new();
    for record in &records {
        if let Some(session) = parse_line(record) {
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

    pub fn attach_builder(
        &self,
        name: &SessionName,
        claim: Claim,
        extra: &[String],
    ) -> Result<(CommandBuilder, Nonce), String> {
        let cwd = std::env::current_dir().map_err(|error| format!("current_dir: {error}"))?;
        let nonce = Nonce::generate();
        let mut builder = CommandBuilder::new(&self.executable);
        builder.arg("attach");
        builder.arg(name.as_str());
        builder.args(extra);
        builder.cwd(cwd);
        for var in REMOVED_VARS {
            builder.env_remove(var);
        }
        builder.env("ZMX_DIR", &self.socket_dir);
        builder.env("ZMX_MANAGED", nonce.as_str());
        match claim {
            Claim::Take => builder.env("ZMX_MANAGED_CLAIM", "1"),
            Claim::Leave => {}
        }
        Ok((builder, nonce))
    }

    pub fn lock(&self, name: &SessionName) -> Result<File, String> {
        let path = self
            .socket_dir
            .with_file_name(format!("{}.lock", name.as_str()));
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| format!("open {}: {error}", path.display()))?;
        file.lock()
            .map_err(|error| format!("lock {}: {error}", path.display()))?;
        Ok(file)
    }

    pub fn create_detached(
        &self,
        name: &SessionName,
        argv: &[String],
        cwd: &Path,
    ) -> Result<(), String> {
        let mut command = self.command();
        command
            .arg("attach")
            .arg(name.as_str())
            .args(argv)
            .current_dir(cwd);
        self.run("attach", command)?;
        Ok(())
    }

    pub fn require_live(&self, name: &SessionName) -> Result<(), String> {
        let mut found = None;
        for session in self.sessions()? {
            if session.name() == name.as_str() {
                found = Some(session);
            }
        }
        match found {
            None => Err(format!("no session {}", name.as_str())),
            Some(Session::Broken { name: _ }) => Err(format!(
                "session {} is broken: run foster rm {}",
                name.as_str(),
                name.as_str()
            )),
            Some(Session::Live {
                name: _,
                clients: _,
                created: _,
                cwd: _,
                cmd: _,
                labels: _,
            }) => Ok(()),
        }
    }

    pub fn history(&self, name: &SessionName) -> Result<String, String> {
        let mut command = self.command();
        command.arg("history").arg(name.as_str());
        self.run("history", command)
    }

    pub fn tail_command(&self, name: &SessionName) -> Command {
        let mut command = self.command();
        command.arg("tail").arg(name.as_str());
        command
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

    pub fn kill(&self, name: &SessionName, mode: KillMode) -> Result<(), String> {
        let mut command = self.command();
        command.arg("kill").arg(name.as_str());
        match mode {
            KillMode::Graceful => {}
            KillMode::Force => {
                command.arg("--force");
            }
        }
        let output = command
            .stdin(Stdio::null())
            .output()
            .map_err(|error| format!("zmx kill: {error}"))?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() || stderr.contains("failed to kill") {
            return Err(format!("zmx kill failed: {}", stderr.trim_end()));
        }
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
        let mut labels = BTreeMap::new();
        labels.insert("ended".to_string(), "1".to_string());
        assert_eq!(
            sessions,
            vec![Session::Live {
                name: "web".to_string(),
                clients: 0,
                created: 1792866550,
                cwd: Some(PathBuf::from("/x")),
                cmd: Some("a -- b".to_string()),
                labels
            }]
        );
    }

    #[test]
    fn keeps_newline_in_cmd_inside_one_record() {
        let output = "name=job\tpid=1\tclients=0\tcreated=5\tcmd=sh -c 'make\nexit 3'\tended=9\texit_code=3\nname=other\tpid=2\tclients=0\tcreated=6\n";
        let sessions = parse_sessions(output);
        assert_eq!(sessions.len(), 2);
        let Session::Live {
            name,
            clients: _,
            created: _,
            cwd: _,
            cmd,
            labels,
        } = &sessions[0]
        else {
            panic!("expected live session");
        };
        assert_eq!(name, "job");
        assert_eq!(cmd.as_deref(), Some("sh -c 'make\nexit 3'"));
        assert_eq!(labels.get("exit_code").map(String::as_str), Some("3"));
        assert_eq!(sessions[1].name(), "other");
    }

    #[test]
    fn parses_indented_line() {
        let sessions = parse_sessions("  name=web\tclients=1\tcreated=5\n");
        assert_eq!(
            sessions,
            vec![Session::Live {
                name: "web".to_string(),
                clients: 1,
                created: 5,
                cwd: None,
                cmd: None,
                labels: BTreeMap::new()
            }]
        );
    }

    #[test]
    fn decodes_cwd_uri() {
        assert_eq!(
            decode_cwd("file://host.local/Users/me/my%20proj%C3%A9"),
            Some(PathBuf::from("/Users/me/my projé"))
        );
        assert_eq!(decode_cwd("file:///a%zz"), Some(PathBuf::from("/a%zz")));
        assert_eq!(decode_cwd("file:///a%4"), Some(PathBuf::from("/a%4")));
        assert_eq!(decode_cwd("/plain"), None);
    }

    #[test]
    fn parses_exit_code_and_ended_labels() {
        let line = "name=web\tpid=1\tclients=0\tcreated=2\tended=1792870000\texit_code=3\n";
        let sessions = parse_sessions(line);
        let [session] = sessions.as_slice() else {
            panic!("expected one session");
        };
        let Session::Live {
            name: _,
            clients: _,
            created: _,
            cwd: _,
            cmd: _,
            labels,
        } = session
        else {
            panic!("expected a live session");
        };
        assert_eq!(labels.get("exit_code").map(String::as_str), Some("3"));
        assert_eq!(labels.get("ended").map(String::as_str), Some("1792870000"));
        assert!(!session.is_running());
    }

    #[test]
    fn nonce_is_32_lowercase_hex_and_unique() {
        let first = Nonce::generate();
        let second = Nonce::generate();
        assert_eq!(first.as_str().len(), 32);
        for character in first.as_str().chars() {
            assert!(character.is_ascii_digit() || ('a'..='f').contains(&character));
        }
        assert_ne!(first.as_str(), second.as_str());
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
