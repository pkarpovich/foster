use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

const SOCKET_PATH_LIMIT: usize = 103;

fn poll_until(timeout: Duration, interval: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < timeout, "timed out after {timeout:?}");
        std::thread::sleep(interval);
    }
}

fn processes_with(marker: &str) -> Vec<String> {
    let output = Command::new("ps")
        .args(["-axo", "command="])
        .output()
        .expect("run ps");
    let text = String::from_utf8_lossy(&output.stdout);
    let mut lines = Vec::new();
    for line in text.lines() {
        if line.contains(marker) && !line.starts_with("ps -axo") {
            lines.push(line.to_string());
        }
    }
    lines
}

fn random_marker() -> String {
    let mut bytes = [0u8; 8];
    let mut urandom = File::open("/dev/urandom").expect("open /dev/urandom");
    urandom.read_exact(&mut bytes).expect("read /dev/urandom");
    let mut marker = String::from("fstr");
    for byte in bytes {
        marker.push_str(&format!("{byte:02x}"));
    }
    marker
}

fn short_tempdir() -> TempDir {
    tempfile::Builder::new()
        .prefix("f")
        .tempdir_in("/tmp")
        .expect("temp dir in /tmp")
}

struct Real {
    home: TempDir,
    bin: PathBuf,
    zmx_dir: PathBuf,
    marker: String,
    sessions: Vec<&'static str>,
}

impl Real {
    fn new(sessions: Vec<&'static str>) -> Real {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let zmx_source = manifest.join("zmx/out/foster-zmx");
        assert!(zmx_source.exists(), "run zmx/build.sh first");

        let home = short_tempdir();
        let bin = home.path().join("bin");
        fs::create_dir(&bin).expect("bin dir");
        fs::copy(env!("CARGO_BIN_EXE_foster"), bin.join("foster")).expect("copy foster");
        fs::copy(&zmx_source, bin.join("foster-zmx")).expect("copy foster-zmx");

        let zmx_dir = home.path().join(".local/state/foster/zmx");
        let longest = zmx_dir.join("0123456789");
        assert!(
            longest.as_os_str().len() <= SOCKET_PATH_LIMIT,
            "socket path too long: {}",
            longest.display()
        );

        let marker = random_marker();
        let target = manifest.join("target");
        fs::create_dir_all(&target).expect("target dir");
        fs::write(target.join("foster-real-marker"), &marker).expect("write marker");

        Real {
            home,
            bin,
            zmx_dir,
            marker,
            sessions,
        }
    }

    fn command(&self, program: &str, zmx_dir: &Path) -> Command {
        let mut command = Command::new(self.bin.join(program));
        command
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/sh")
            .env("ZMX_DIR", zmx_dir)
            .stdin(Stdio::null());
        command
    }

    fn foster(&self, args: &[&str]) -> Output {
        self.command("foster", &self.zmx_dir)
            .args(args)
            .output()
            .expect("run foster")
    }

    fn foster_stdout(&self, args: &[&str]) -> String {
        let output = self.foster(args);
        assert!(
            output.status.success(),
            "foster {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn script(&self, body: &str) -> String {
        format!("{body}; : {}", self.marker)
    }

    fn ls_entry(&self, name: &str) -> Option<Value> {
        let stdout = self.foster_stdout(&["ls", "--json"]);
        let Ok(Value::Array(entries)) = serde_json::from_str::<Value>(&stdout) else {
            return None;
        };
        let mut found = None;
        for entry in entries {
            if entry["name"] == name {
                found = Some(entry);
                break;
            }
        }
        found
    }

    fn history(&self, name: &str) -> String {
        let output = self
            .command("foster-zmx", &self.zmx_dir)
            .args(["history", name])
            .output()
            .expect("run foster-zmx history");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for Real {
    fn drop(&mut self) {
        for name in &self.sessions {
            let _ = self
                .command("foster-zmx", &self.zmx_dir)
                .args(["kill", name])
                .output();
        }
    }
}

#[test]
fn acceptance() {
    let real = Real::new(vec!["web", "boom"]);
    let cwd = std::env::current_dir().expect("cwd");
    let timeout = Duration::from_secs(10);
    let interval = Duration::from_millis(100);

    let web_script = real.script("echo ready; sleep 300");
    let started = real.foster_stdout(&["run", "web", "--", "sh", "-c", &web_script]);
    assert_eq!(started, "started web\n");
    let web = real.ls_entry("web").expect("web is listed");
    assert_eq!(web["status"], "running");
    assert_eq!(web["attached"], false);
    assert_eq!(web["cwd"], cwd.to_str().expect("utf-8 cwd"));
    assert_eq!(
        web["command"],
        format!("sh -c 'echo ready; sleep 300; : {}'", real.marker)
    );

    let boom_script = real.script("echo kaboom; exit 3");
    let started = real.foster_stdout(&["run", "boom", "--", "sh", "-c", &boom_script]);
    assert_eq!(started, "started boom\n");
    poll_until(timeout, interval, || {
        let Some(boom) = real.ls_entry("boom") else {
            return false;
        };
        boom["status"] == "exited 3" && boom["exit_code"] == 3
    });
    let status_line = format!(
        "foster: sh -c 'echo kaboom; exit 3; : {}' exited with 3",
        real.marker
    );
    poll_until(timeout, interval, || {
        let history = real.history("boom");
        history.contains("kaboom") && history.contains(&status_line)
    });

    let rerun_script = real.script("sleep 300");
    let started = real.foster_stdout(&["run", "boom", "--", "sh", "-c", &rerun_script]);
    assert_eq!(started, "started boom\n");
    let boom = real.ls_entry("boom").expect("boom is listed");
    assert_eq!(boom["status"], "running");

    let duplicate = real.foster(&["run", "web", "--", "true"]);
    assert!(!duplicate.status.success());
    assert_eq!(
        String::from_utf8_lossy(&duplicate.stderr),
        "foster: web is already running\n"
    );

    assert_eq!(real.foster_stdout(&["rm", "web"]), "removed web\n");
    assert_eq!(real.foster_stdout(&["rm", "boom"]), "removed boom\n");
    assert_eq!(real.foster_stdout(&["ls"]), "");
    poll_until(timeout, interval, || {
        processes_with(&real.marker).is_empty()
    });
}

#[test]
fn isolation() {
    let real = Real::new(vec![]);
    let other_dir = short_tempdir();
    let script = real.script("sleep 300");
    let created = real
        .command("foster-zmx", other_dir.path())
        .args(["attach", "other", "sh", "-c", &script])
        .output()
        .expect("create other session");
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );

    let listed = real.foster_stdout(&["ls"]);

    let killed = real
        .command("foster-zmx", other_dir.path())
        .args(["kill", "other"])
        .output()
        .expect("kill other session");
    assert!(killed.status.success());
    assert_eq!(listed, "");
}
