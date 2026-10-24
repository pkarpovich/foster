use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process, test_kill_process};

use tempfile::TempDir;

const USAGE: &str = "usage:
  foster run <name> -- <command...>   start a command and attach to it
  foster ls [--json]                  list processes started with foster
  foster attach <name>                attach to a process (detach: Ctrl+\\)
  foster rm <name>                    stop a process and remove it
";

fn poll_until(timeout: Duration, interval: Duration, mut check: impl FnMut() -> bool) {
    let start = Instant::now();
    while !check() {
        assert!(start.elapsed() < timeout, "timed out after {timeout:?}");
        std::thread::sleep(interval);
    }
}

fn pid_of(raw: u32) -> Pid {
    Pid::from_raw(i32::try_from(raw).expect("pid fits")).expect("valid pid")
}

fn process_gone(raw: u32) -> bool {
    if test_kill_process(pid_of(raw)).is_err() {
        return true;
    }
    let output = Command::new("ps")
        .args(["-o", "stat=", "-p", &raw.to_string()])
        .output()
        .expect("run ps");
    let state = String::from_utf8_lossy(&output.stdout);
    let state = state.trim();
    state.is_empty() || state.starts_with('Z')
}

fn read_pid(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    text.trim().parse().ok()
}

struct Harness {
    home: TempDir,
    fake_bin: PathBuf,
}

impl Harness {
    fn new() -> Harness {
        let home = tempfile::tempdir().expect("temp home");
        let fake_bin = home.path().join("fake-bin");
        fs::create_dir(&fake_bin).expect("fake-bin");
        Harness { home, fake_bin }
    }

    fn foster(&self, args: &[&str]) -> Command {
        let path = format!("{}:/usr/bin:/bin", self.fake_bin.display());
        let mut command = Command::new(env!("CARGO_BIN_EXE_foster"));
        command
            .args(args)
            .env_clear()
            .env("HOME", self.home.path())
            .env("PATH", path)
            .stdin(Stdio::null());
        command
    }

    fn install_fake_zmx(&self, ls_output: &str) {
        let ls_file = self.home.path().join("ls-output");
        fs::write(&ls_file, ls_output).expect("ls output");
        let script = format!(
            "#!/bin/sh\necho \"$* ZMX_DIR=$ZMX_DIR ZMX_SESSION=${{ZMX_SESSION-unset}} CLAUDECODE=${{CLAUDECODE-unset}}\" >> '{}'\ncase \"$1\" in ls) cat '{}' ;; attach) echo \"cwd=$(pwd) managed=${{ZMX_MANAGED-unset}}\" >> '{}' ;; esac\n",
            self.zmx_log().display(),
            ls_file.display(),
            self.attach_log().display()
        );
        let path = self.fake_bin.join("foster-zmx");
        fs::write(&path, script).expect("fake zmx");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    fn install_fake_shell(&self) -> PathBuf {
        let script = format!(
            "#!/bin/sh\necho \"$* cwd=$(pwd)\" >> '{}'\n",
            self.shell_log().display()
        );
        let path = self.fake_bin.join("fake-shell");
        fs::write(&path, script).expect("fake shell");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
        path
    }

    fn shell_log(&self) -> PathBuf {
        self.home.path().join("shell.log")
    }

    fn shell_calls(&self) -> String {
        fs::read_to_string(self.shell_log()).unwrap_or_default()
    }

    fn hold(&self, shell: &Path, args: &[&str]) -> Command {
        let mut command = self.foster(&["hold"]);
        command
            .args(args)
            .env("SHELL", shell)
            .current_dir(self.home.path());
        command
    }

    fn spawn_hold(&self, shell: &Path, args: &[&str]) -> Child {
        self.hold(shell, args)
            .process_group(0)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn hold")
    }

    fn zmx_log(&self) -> PathBuf {
        self.home.path().join("zmx.log")
    }

    fn zmx_calls(&self) -> String {
        fs::read_to_string(self.zmx_log()).unwrap_or_default()
    }

    fn attach_log(&self) -> PathBuf {
        self.home.path().join("attach.log")
    }

    fn attach_details(&self) -> String {
        fs::read_to_string(self.attach_log()).unwrap_or_default()
    }

    fn fail_zmx_command(&self, command: &str, message: &str) {
        let path = self.fake_bin.join("foster-zmx");
        let mut script = fs::read_to_string(&path).expect("read fake");
        script.push_str(&format!(
            "if [ \"$1\" = {command} ]; then echo {message} >&2; exit 1; fi\n"
        ));
        fs::write(&path, script).expect("write fake");
    }

    fn output(&self, args: &[&str]) -> Output {
        self.foster(args).output().expect("run foster")
    }
}

#[test]
fn help_aliases_print_usage() {
    let harness = Harness::new();
    for args in [&[][..], &["help"], &["-h"], &["--help"]] {
        let output = harness.output(args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout), USAGE, "{args:?}");
        assert_eq!(String::from_utf8_lossy(&output.stderr), "", "{args:?}");
    }
}

#[test]
fn version_aliases_print_version() {
    let harness = Harness::new();
    let expected = format!("foster {}\n", env!("CARGO_PKG_VERSION"));
    for args in [&["version"][..], &["-V"], &["--version"]] {
        let output = harness.output(args);
        assert_eq!(output.status.code(), Some(0), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            expected,
            "{args:?}"
        );
    }
}

#[test]
fn unknown_command_prints_usage_to_stderr() {
    let harness = Harness::new();
    let output = harness.output(&["frobnicate"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    let expected = format!("foster: unknown command \"frobnicate\"\n\n{USAGE}");
    assert_eq!(String::from_utf8_lossy(&output.stderr), expected);
}

#[test]
fn attach_without_name_prints_usage() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let output = harness.output(&["attach"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: usage: foster attach <name>\n"
    );
    assert_eq!(harness.zmx_calls(), "");
}

#[test]
fn attach_missing_session_fails() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=other\tpid=1\tclients=0\tcreated=1\n");
    let output = harness.output(&["attach", "web"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: no session web\n"
    );
    assert_eq!(zmx_calls_without_env(&harness), vec!["ls".to_string()]);
}

#[test]
fn attach_broken_session_fails() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\terr=ConnectionRefused\tstatus=cleaning up\n");
    let output = harness.output(&["attach", "web"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: session web is broken: run foster rm web\n"
    );
    assert_eq!(zmx_calls_without_env(&harness), vec!["ls".to_string()]);
}

#[test]
fn attach_without_terminal_fails() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    let output = harness.output(&["attach", "web"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: attach needs a terminal\n"
    );
    assert_eq!(zmx_calls_without_env(&harness), vec!["ls".to_string()]);
}

#[test]
fn rm_kills_existing_session() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    let output = harness
        .foster(&["rm", "web"])
        .env("ZMX_SESSION", "outer")
        .env("CLAUDECODE", "1")
        .output()
        .expect("run foster");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "removed web\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let zmx_dir = harness.home.path().join(".local/state/foster/zmx");
    let zmx_dir = zmx_dir.display();
    let expected = format!(
        "ls ZMX_DIR={zmx_dir} ZMX_SESSION=unset CLAUDECODE=unset\nkill web ZMX_DIR={zmx_dir} ZMX_SESSION=unset CLAUDECODE=unset\n"
    );
    assert_eq!(harness.zmx_calls(), expected);
}

#[test]
fn rm_removes_broken_session() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\terr=ConnectionRefused\tstatus=cleaning up\n");
    let output = harness.output(&["rm", "web"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "removed web\n");
    assert!(harness.zmx_calls().contains("kill web --force"));
}

#[test]
fn rm_missing_session_fails() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    let output = harness.output(&["rm", "nope"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: no session nope\n"
    );
    assert!(!harness.zmx_calls().contains("kill"));
}

#[test]
fn rm_with_no_sessions_fails() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let output = harness.output(&["rm", "web"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: no session web\n"
    );
}

#[test]
fn rm_invalid_name_fails_before_zmx() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let output = harness.output(&["rm", "a b"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: invalid name a b: use letters, digits, - _ . :\n"
    );
    assert_eq!(harness.zmx_calls(), "");
}

#[test]
fn rm_wrong_arity_prints_usage() {
    let harness = Harness::new();
    for args in [&["rm"][..], &["rm", "a", "b"]] {
        let output = harness.output(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "foster: usage: foster rm <name>\n",
            "{args:?}"
        );
    }
}

#[test]
fn rm_without_zmx_reports_not_found() {
    let harness = Harness::new();
    let output = harness.output(&["rm", "web"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: foster-zmx not found next to foster or on PATH; build it with zmx/build.sh\n"
    );
}

#[test]
fn rm_reports_zmx_failure() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    let path = harness.fake_bin.join("foster-zmx");
    let mut script = fs::read_to_string(&path).expect("read fake");
    script.push_str("if [ \"$1\" = kill ]; then echo boom >&2; exit 1; fi\n");
    fs::write(&path, script).expect("write fake");
    let output = harness.output(&["rm", "web"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: zmx kill failed: boom\n"
    );
}

#[test]
fn rm_reports_kill_failure_with_zero_exit() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    let path = harness.fake_bin.join("foster-zmx");
    let mut script = fs::read_to_string(&path).expect("read fake");
    script.push_str(
        "if [ \"$1\" = kill ]; then echo 'failed to kill session=web: SessionUnresponsive' >&2; exit 0; fi\n",
    );
    fs::write(&path, script).expect("write fake");
    let output = harness.output(&["rm", "web"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: zmx kill failed: failed to kill session=web: SessionUnresponsive\n"
    );
}

fn foster_path() -> String {
    fs::canonicalize(env!("CARGO_BIN_EXE_foster"))
        .expect("canonical foster path")
        .display()
        .to_string()
}

fn zmx_calls_without_env(harness: &Harness) -> Vec<String> {
    let mut calls = Vec::new();
    for line in harness.zmx_calls().lines() {
        let Some((call, _)) = line.split_once(" ZMX_DIR=") else {
            continue;
        };
        calls.push(call.to_string());
    }
    calls
}

#[test]
fn run_starts_detached_session_without_terminal() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let cwd = harness.home.path().canonicalize().expect("canonical home");
    let output = harness
        .foster(&["run", "web", "--", "sh", "-c", "sleep 1"])
        .current_dir(&cwd)
        .output()
        .expect("run foster");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "started web\n");
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    let expected = vec![
        "ls".to_string(),
        format!("attach web {} hold web -- sh -c sleep 1", foster_path()),
    ];
    assert_eq!(zmx_calls_without_env(&harness), expected);
    assert_eq!(
        harness.attach_details(),
        format!("cwd={} managed=unset\n", cwd.display())
    );
}

#[test]
fn run_encodes_control_characters_for_hold() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let output = harness.output(&["run", "web", "--", "echo", "a\tb\nc", "%41"]);
    assert_eq!(output.status.code(), Some(0));
    let expected = vec![
        "ls".to_string(),
        format!(
            "attach web {} hold web -- echo a%09b%0Ac %2541",
            foster_path()
        ),
    ];
    assert_eq!(zmx_calls_without_env(&harness), expected);
}

#[test]
fn run_waits_for_the_name_lock() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let state = harness.home.path().join(".local/state/foster");
    fs::create_dir_all(&state).expect("state dir");
    let lock = fs::File::create(state.join("web.lock")).expect("lock file");
    lock.lock().expect("take lock");
    let mut run = harness
        .foster(&["run", "web", "--", "true"])
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn run");
    std::thread::sleep(Duration::from_millis(500));
    assert!(run.try_wait().expect("try_wait").is_none());
    assert_eq!(harness.zmx_calls(), "");
    drop(lock);
    poll_until(Duration::from_secs(5), Duration::from_millis(50), || {
        run.try_wait().expect("try_wait").is_some()
    });
    assert!(harness.zmx_calls().contains("attach web"));
}

#[test]
fn run_refuses_running_duplicate() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    let output = harness.output(&["run", "web", "--", "true"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: web is already running\n"
    );
    assert_eq!(zmx_calls_without_env(&harness), vec!["ls".to_string()]);
}

#[test]
fn run_replaces_exited_session() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\tended=5\texit_code=3\n");
    let output = harness.output(&["run", "web", "--", "true"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "started web\n");
    let expected = vec![
        "ls".to_string(),
        "kill web".to_string(),
        format!("attach web {} hold web -- true", foster_path()),
    ];
    assert_eq!(zmx_calls_without_env(&harness), expected);
}

#[test]
fn run_replaces_broken_session() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\terr=ConnectionRefused\tstatus=cleaning up\n");
    let output = harness.output(&["run", "web", "--", "true"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "started web\n");
    let expected = vec![
        "ls".to_string(),
        "kill web --force".to_string(),
        format!("attach web {} hold web -- true", foster_path()),
    ];
    assert_eq!(zmx_calls_without_env(&harness), expected);
}

#[test]
fn run_wrong_shape_prints_usage() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    for args in [
        &["run"][..],
        &["run", "web"],
        &["run", "web", "--"],
        &["run", "web", "true"],
        &["run", "--", "true"],
    ] {
        let output = harness.output(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "foster: usage: foster run <name> -- <command...>\n",
            "{args:?}"
        );
    }
    assert_eq!(harness.zmx_calls(), "");
}

#[test]
fn run_invalid_name_fails_before_zmx() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let output = harness.output(&["run", "a b", "--", "true"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: invalid name a b: use letters, digits, - _ . :\n"
    );
    assert_eq!(harness.zmx_calls(), "");
}

#[test]
fn run_reports_attach_failure() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    harness.fail_zmx_command("attach", "boom");
    let output = harness.output(&["run", "web", "--", "true"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: zmx attach failed: boom\n"
    );
}

#[test]
fn hold_records_exit_code_and_runs_shell() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    let output = harness
        .hold(&shell, &["web", "--", "sh", "-c", "exit 3"])
        .output()
        .expect("run hold");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\r\nfoster: sh -c 'exit 3' exited with 3\r\n"),
        "{stdout:?}"
    );
    let calls = harness.zmx_calls();
    let ended = calls
        .strip_prefix("set web exit_code=3 ended=")
        .expect("set call");
    let digits = ended.split(' ').next().expect("ended value");
    assert!(digits.parse::<u64>().is_ok(), "{digits:?}");
    let cwd = harness.home.path().canonicalize().expect("canonical home");
    assert_eq!(harness.shell_calls(), format!("-l cwd={}\n", cwd.display()));
}

#[test]
fn hold_decodes_encoded_words() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    let output = harness
        .hold(
            &shell,
            &[
                "web",
                "--",
                "sh",
                "-c",
                "test \"$0\" = \"a\tb%2541\"",
                "a%09b%2541",
            ],
        )
        .output()
        .expect("run hold");
    assert_eq!(output.status.code(), Some(0));
    assert!(harness.zmx_calls().starts_with("set web exit_code=0 "));
}

#[test]
fn hold_reports_unrecorded_exit_status() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    harness.fail_zmx_command("set", "boom");
    let shell = harness.install_fake_shell();
    let output = harness
        .hold(&shell, &["web", "--", "true"])
        .output()
        .expect("run hold");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\r\nfoster: exit status not recorded: zmx set failed: boom\r\n"),
        "{stdout:?}"
    );
    assert!(harness.shell_calls().starts_with("-l "));
}

#[test]
fn hold_reports_permission_denied_as_126() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    let plain = harness.home.path().join("plain-file");
    fs::write(&plain, "x").expect("plain file");
    let output = harness
        .hold(&shell, &["web", "--", plain.to_str().expect("utf-8 path")])
        .output()
        .expect("run hold");
    assert_eq!(output.status.code(), Some(0));
    assert!(harness.zmx_calls().contains("exit_code=126 "));
}

#[test]
fn hold_hides_zmx_dir_from_command_and_shell() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    let seen = harness.home.path().join("seen");
    let script = format!("echo \"${{ZMX_DIR-unset}}\" > '{}'", seen.display());
    let output = harness
        .hold(&shell, &["web", "--", "sh", "-c", &script])
        .env("ZMX_DIR", "/leak")
        .output()
        .expect("run hold");
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(fs::read_to_string(&seen).expect("seen"), "unset\n");
}

#[test]
fn hold_reports_signal_death_as_128_plus_signal() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    let output = harness
        .hold(&shell, &["web", "--", "sh", "-c", "kill -9 $$"])
        .output()
        .expect("run hold");
    assert_eq!(output.status.code(), Some(0));
    assert!(harness.zmx_calls().contains("exit_code=137 "));
}

#[test]
fn hold_reports_spawn_failure_as_127() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    let output = harness
        .hold(&shell, &["web", "--", "nosuchcmd"])
        .output()
        .expect("run hold");
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("foster: nosuchcmd: No such file or directory (os error 2)"),
        "{stdout:?}"
    );
    assert!(harness.zmx_calls().contains("exit_code=127 "));
    assert!(harness.shell_calls().starts_with("-l "));
}

#[test]
fn hold_wrong_arity_prints_usage() {
    let harness = Harness::new();
    for args in [&["hold"][..], &["hold", "web"], &["hold", "web", "--"]] {
        let output = harness.output(args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "foster: usage: foster hold <name> -- <command...>\n",
            "{args:?}"
        );
    }
}

#[test]
fn hold_terminates_without_labels_or_shell() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    let pidfile = harness.home.path().join("sleep.pid");
    let script = format!("sleep 300 & echo $! > '{}'; wait", pidfile.display());
    let mut hold = harness.spawn_hold(&shell, &["web", "--", "sh", "-c", &script]);
    let second = Duration::from_millis(50);
    poll_until(Duration::from_secs(5), second, || {
        read_pid(&pidfile).is_some()
    });
    let sleep_pid = read_pid(&pidfile).expect("sleep pid");
    kill_process(pid_of(hold.id()), Signal::HUP).expect("send HUP");
    poll_until(Duration::from_secs(5), second, || {
        hold.try_wait().expect("try_wait").is_some()
    });
    assert_eq!(harness.shell_calls(), "");
    assert_eq!(harness.zmx_calls(), "");
    poll_until(Duration::from_secs(5), second, || process_gone(sleep_pid));
}

#[test]
fn hold_survives_sigint() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    let pidfile = harness.home.path().join("sh.pid");
    let script = format!("echo $$ > '{}'; sleep 300", pidfile.display());
    let mut hold = harness.spawn_hold(&shell, &["web", "--", "sh", "-c", &script]);
    let second = Duration::from_millis(50);
    poll_until(Duration::from_secs(5), second, || {
        read_pid(&pidfile).is_some()
    });
    kill_process(pid_of(hold.id()), Signal::INT).expect("send INT");
    std::thread::sleep(Duration::from_millis(500));
    assert!(hold.try_wait().expect("try_wait").is_none());
    assert_eq!(harness.shell_calls(), "");
    kill_process(pid_of(hold.id()), Signal::TERM).expect("send TERM");
    poll_until(Duration::from_secs(5), second, || {
        hold.try_wait().expect("try_wait").is_some()
    });
}

const LS_OUTPUT: &str = "name=web\tpid=1\tclients=1\tcreated=1792866550\tcwd=file://host.local/tmp/home/my%20proj\tcmd=/x/foster hold web -- sh -c 'echo ready; sleep 300'\n\
name=boom\tpid=2\tclients=0\tcreated=1792866600\tcwd=file://host.local/opt/x\tcmd=/x/foster hold boom -- sh -c 'exit 3'\tended=1792866700\texit_code=3\n\
name=bad\terr=ConnectionRefused\tstatus=cleaning up\n";

fn ls_output(harness: &Harness, args: &[&str]) -> Output {
    harness.install_fake_zmx(LS_OUTPUT);
    let mut command = harness.foster(args);
    command.env("TZ", "UTC").env("HOME", "/tmp/home");
    command.output().expect("run foster ls")
}

#[test]
fn ls_prints_table() {
    let harness = Harness::new();
    let output = ls_output(&harness, &["ls"]);
    assert_eq!(output.status.code(), Some(0));
    let expected = "NAME  STATUS    ATTACHED  STARTED           CWD        COMMAND\n\
bad   error     no        -                 -          -\n\
boom  exited 3  no        2026-10-24 18:30  /opt/x     sh -c 'exit 3'\n\
web   running   yes       2026-10-24 18:29  ~/my proj  sh -c 'echo ready; sleep 300'\n";
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn ls_prints_json() {
    let harness = Harness::new();
    let output = ls_output(&harness, &["ls", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let expected = "[{\"name\":\"bad\",\"status\":\"error\",\"exit_code\":null,\"attached\":false,\"started\":null,\"cwd\":null,\"command\":null},\
{\"name\":\"boom\",\"status\":\"exited 3\",\"exit_code\":3,\"attached\":false,\"started\":1792866600,\"cwd\":\"/opt/x\",\"command\":\"sh -c 'exit 3'\"},\
{\"name\":\"web\",\"status\":\"running\",\"exit_code\":null,\"attached\":true,\"started\":1792866550,\"cwd\":\"/tmp/home/my proj\",\"command\":\"sh -c 'echo ready; sleep 300'\"}]\n";
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn ls_with_no_sessions_prints_nothing() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let output = harness.output(&["ls"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "");
}

#[test]
fn ls_rejects_other_arguments() {
    let harness = Harness::new();
    let output = harness.output(&["ls", "--bogus"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: usage: foster ls [--json]\n"
    );
}
