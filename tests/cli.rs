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
  foster attach <name>                attach to a process (detach: Ctrl+C)
  foster history <name> [-f]          print a process's output, -f keeps following
  foster tail <name>                  stream a process's new output
  foster rm <name>                    stop a process and remove it
  foster restore                      start again what was running before a reboot
  foster task [<name> | --json]       list or start a task from .zed/tasks.json
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
            "#!/bin/sh\necho \"$* ZMX_DIR=$ZMX_DIR ZMX_SESSION=${{ZMX_SESSION-unset}} CLAUDECODE=${{CLAUDECODE-unset}}\" >> '{}'\ncase \"$1\" in ls) cat '{}' ;; attach) echo \"cwd=$(pwd) managed=${{ZMX_MANAGED-unset}}\" >> '{}' ;; history) printf 'line one\\nline two\\n' ;; tail) printf 'streamed\\n' ;; esac\n",
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
fn history_prints_the_session_output() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    let output = harness.output(&["history", "web"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "line one\nline two\n"
    );
    assert_eq!(
        zmx_calls_without_env(&harness),
        vec!["ls".to_string(), "history web".to_string()]
    );
}

#[test]
fn history_follow_prints_the_past_then_the_stream() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    for args in [["history", "web", "-f"], ["history", "--follow", "web"]] {
        let output = harness.output(&args);
        assert_eq!(output.status.code(), Some(0));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "line one\nline two\nstreamed\n"
        );
    }
}

#[test]
fn history_follow_stops_its_tail_when_terminated() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    let fake = harness.fake_bin.join("foster-zmx");
    let pid_file = harness.home.path().join("tail.pid");
    let script = fs::read_to_string(&fake).expect("fake script");
    let script = script.replace(
        "tail) printf 'streamed\\n' ;;",
        &format!(
            "tail) echo $$ > '{}'; exec sleep 300 ;;",
            pid_file.display()
        ),
    );
    fs::write(&fake, script).expect("rewrite fake");
    let mut follow = harness
        .foster(&["history", "web", "-f"])
        .stdout(Stdio::null())
        .spawn()
        .expect("spawn history -f");
    poll_until(Duration::from_secs(5), Duration::from_millis(50), || {
        fs::read_to_string(&pid_file).is_ok_and(|text| !text.trim().is_empty())
    });
    let tail_pid: u32 = fs::read_to_string(&pid_file)
        .expect("tail pid")
        .trim()
        .parse()
        .expect("numeric pid");
    let status = Command::new("kill")
        .args(["-TERM", &follow.id().to_string()])
        .status()
        .expect("send SIGTERM");
    assert!(status.success());
    poll_until(Duration::from_secs(5), Duration::from_millis(50), || {
        follow.try_wait().expect("try_wait").is_some()
    });
    let code = follow.wait().expect("wait").code();
    assert_eq!(code, Some(143));
    poll_until(Duration::from_secs(5), Duration::from_millis(50), || {
        process_gone(tail_pid)
    });
}

#[test]
fn history_survives_a_reader_that_closes_early() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    let fake = harness.fake_bin.join("foster-zmx");
    let script = fs::read_to_string(&fake).expect("fake script");
    let script = script.replace(
        "history) printf 'line one\\nline two\\n' ;;",
        "history) yes 'a long line of server output' | head -c 400000 ;;",
    );
    fs::write(&fake, script).expect("rewrite fake");
    let mut history = harness
        .foster(&["history", "web"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn history");
    let mut stdout = history.stdout.take().expect("stdout");
    let mut first = [0u8; 16];
    std::io::Read::read_exact(&mut stdout, &mut first).expect("first bytes");
    drop(stdout);
    let output = history.wait_with_output().expect("wait history");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(output.status.code(), Some(0), "{stderr}");
}

#[test]
fn history_of_a_missing_session_fails() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let output = harness.output(&["history", "web"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: no session web\n"
    );
    assert_eq!(zmx_calls_without_env(&harness), vec!["ls".to_string()]);
}

#[test]
fn tail_streams_the_session_output() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\n");
    let output = harness.output(&["tail", "web"]);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "streamed\n");
    assert_eq!(
        zmx_calls_without_env(&harness),
        vec!["ls".to_string(), "tail web".to_string()]
    );
}

#[test]
fn history_and_tail_print_usage_without_a_name() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let history = harness.output(&["history"]);
    assert_eq!(
        String::from_utf8_lossy(&history.stderr),
        "foster: usage: foster history <name> [-f]\n"
    );
    let tail = harness.output(&["tail"]);
    assert_eq!(
        String::from_utf8_lossy(&tail.stderr),
        "foster: usage: foster tail <name>\n"
    );
}

fn task_project(harness: &Harness, tasks: &str) -> PathBuf {
    let repo = harness.home.path().join("myrepo");
    fs::create_dir_all(repo.join(".zed")).expect("project dir");
    fs::write(repo.join(".zed/tasks.json"), tasks).expect("tasks file");
    let status = Command::new("/usr/bin/git")
        .args(["init", "-q"])
        .current_dir(&repo)
        .env_clear()
        .env("HOME", harness.home.path())
        .status()
        .expect("git init");
    assert!(status.success());
    repo.canonicalize().expect("canonical repo")
}

const TASKS: &str = r#"[
    /* servers */
    {
        "label": "web: dev",
        "command": "pnpm dev",
        "cwd": "$ZED_WORKTREE_ROOT/web",
        "env": { "PORT": "5173", },
    },
    { "label": "test current file", "command": "go test -run $ZED_STEM", },
]"#;

#[test]
fn task_lists_the_zed_tasks() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let repo = task_project(&harness, TASKS);
    let output = harness
        .foster(&["task"])
        .current_dir(&repo)
        .env("SHELL", "/bin/sh")
        .output()
        .expect("run foster");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "NAME                      LABEL                                     DOMAIN                       COMMAND\n\
         myrepo:web-dev            web: dev                                  web-dev.localhost            pnpm dev\n\
         myrepo:test-current-file  test current file (needs Zed: $ZED_STEM)  test-current-file.localhost  go test -run $ZED_STEM\n"
    );
}

#[test]
fn task_starts_through_run_in_the_task_directory() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let repo = task_project(&harness, TASKS);
    fs::create_dir_all(repo.join("web")).expect("web dir");
    let output = harness
        .foster(&["task", "web-dev"])
        .current_dir(&repo)
        .env("SHELL", "/bin/sh")
        .output()
        .expect("run foster");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let (domain, _) = started(&output, "myrepo:web-dev");
    assert_eq!(domain, "web-dev.localhost");
    let expected = vec![
        "ls".to_string(),
        format!(
            "attach myrepo:web-dev {} hold myrepo:web-dev -- env PORT=5173 /bin/sh -c pnpm dev",
            foster_path()
        ),
    ];
    assert_eq!(zmx_calls_without_env(&harness), expected);
    assert_eq!(
        harness.attach_details(),
        format!("cwd={} managed=unset\n", repo.join("web").display())
    );
}

fn git(repo: &Path, args: &[&str]) {
    let status = Command::new("/usr/bin/git")
        .args(["-c", "user.name=t", "-c", "user.email=t@t"])
        .args(args)
        .current_dir(repo)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git");
    assert!(status.success(), "git {args:?}");
}

#[test]
fn task_in_a_linked_worktree_gets_its_own_name_and_domain() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let repo = task_project(&harness, TASKS);
    git(&repo, &["add", "-A"]);
    git(&repo, &["commit", "-q", "-m", "init"]);
    let worktree = repo.parent().expect("parent").join("feat-x");
    git(
        &repo,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "feat-x",
            worktree.to_str().expect("utf-8"),
        ],
    );
    let output = harness
        .foster(&["task", "--json"])
        .current_dir(&worktree)
        .output()
        .expect("run foster");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let tasks: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(tasks[0]["name"], "myrepo:feat-x:web-dev");
    assert_eq!(tasks[0]["domain"], "feat-x.web-dev.localhost");
}

#[test]
fn task_refuses_a_task_that_needs_zed() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let repo = task_project(&harness, TASKS);
    let output = harness
        .foster(&["task", "test-current-file"])
        .current_dir(&repo)
        .output()
        .expect("run foster");
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: task \"test current file\" needs Zed ($ZED_STEM)\n"
    );
    assert_eq!(harness.zmx_calls(), "");
}

#[test]
fn task_reports_an_unknown_name_and_a_missing_file() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let repo = task_project(&harness, TASKS);
    let unknown = harness
        .foster(&["task", "nope"])
        .current_dir(&repo)
        .output()
        .expect("run foster");
    assert_eq!(unknown.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&unknown.stderr).starts_with("foster: no task nope in "),
        "{unknown:?}"
    );
    let empty = harness.home.path().join("empty");
    fs::create_dir_all(&empty).expect("empty dir");
    let missing = harness
        .foster(&["task"])
        .current_dir(&empty)
        .output()
        .expect("run foster");
    assert_eq!(
        String::from_utf8_lossy(&missing.stderr),
        "foster: no .zed/tasks.json in this project\n"
    );
}

fn record_path(harness: &Harness, name: &str) -> PathBuf {
    harness
        .home
        .path()
        .join(".local/state/foster/sessions")
        .join(format!("{name}.json"))
}

fn write_record(
    harness: &Harness,
    name: &str,
    command: &[&str],
    cwd: &Path,
    exit_code: Option<i32>,
) {
    let path = record_path(harness, name);
    fs::create_dir_all(path.parent().expect("sessions dir")).expect("create sessions dir");
    let record = serde_json::json!({
        "name": name,
        "command": command,
        "cwd": cwd,
        "started": 1,
        "exit_code": exit_code,
    });
    fs::write(path, record.to_string()).expect("write record");
}

#[test]
fn run_records_the_session_on_disk() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let cwd = harness.home.path().canonicalize().expect("canonical home");
    let output = harness
        .foster(&["run", "web", "--", "pnpm", "dev"])
        .current_dir(&cwd)
        .output()
        .expect("run foster");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let (domain, port) = started(&output, "web");
    let text = fs::read_to_string(record_path(&harness, "web")).expect("record");
    let record: serde_json::Value = serde_json::from_str(&text).expect("json record");
    assert_eq!(record["command"], serde_json::json!(["pnpm", "dev"]));
    assert_eq!(record["cwd"], serde_json::json!(cwd));
    assert_eq!(record["exit_code"], serde_json::Value::Null);
    assert_eq!(record["domain"], serde_json::json!(domain));
    assert_eq!(record["port"], serde_json::json!(port));
}

fn write_routed_record(harness: &Harness, name: &str, cwd: &Path, domain: &str, port: u16) {
    let path = record_path(harness, name);
    fs::create_dir_all(path.parent().expect("sessions dir")).expect("create sessions dir");
    let record = serde_json::json!({
        "name": name,
        "command": ["true"],
        "cwd": cwd,
        "started": 1,
        "exit_code": null,
        "domain": domain,
        "port": port,
    });
    fs::write(path, record.to_string()).expect("write record");
}

fn install_hook(harness: &Harness, exit: i32) -> PathBuf {
    let dir = harness.home.path().join(".config/foster");
    fs::create_dir_all(&dir).expect("hook dir");
    let log = harness.home.path().join("hook.log");
    let script = format!(
        "#!/bin/sh\necho \"$FOSTER_EVENT $FOSTER_NAME $FOSTER_DOMAIN $FOSTER_PORT\" >> '{}'\necho hook-said-no >&2\nexit {exit}\n",
        log.display()
    );
    let path = dir.join("hook");
    fs::write(&path, script).expect("hook");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    log
}

fn free_port() -> u16 {
    for port in (51500..=51999).rev() {
        if std::net::TcpListener::bind(("0.0.0.0", port)).is_ok() {
            return port;
        }
    }
    panic!("no free port in 51500-51999");
}

#[test]
fn run_refuses_a_domain_another_session_owns() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    write_routed_record(
        &harness,
        "other:web",
        harness.home.path(),
        "web.localhost",
        51999,
    );
    let output = harness.output(&["run", "web", "--", "true"]);
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "foster: domain web.localhost belongs to other:web: set FOSTER_DOMAIN in the task or foster rm other:web\n"
    );
    assert_eq!(zmx_calls_without_env(&harness), vec!["ls".to_string()]);
}

#[test]
fn hold_gives_the_command_its_port_and_domain() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    write_routed_record(&harness, "web", harness.home.path(), "web.localhost", 51234);
    let seen = harness.home.path().join("seen");
    let script = format!("echo \"$PORT $FOSTER_DOMAIN\" > '{}'", seen.display());
    let output = harness
        .hold(&shell, &["web", "--", "sh", "-c", &script])
        .output()
        .expect("run hold");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        fs::read_to_string(&seen).expect("seen"),
        "51234 web.localhost\n"
    );
}

#[test]
fn hold_runs_the_hook_on_start_and_exit() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    let log = install_hook(&harness, 0);
    write_routed_record(&harness, "web", harness.home.path(), "web.localhost", 51234);
    let output = harness
        .hold(&shell, &["web", "--", "true"])
        .output()
        .expect("run hold");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        fs::read_to_string(&log).expect("hook log"),
        "start web web.localhost 51234\nexit web web.localhost 51234\n"
    );
}

#[test]
fn rm_runs_the_hook_and_a_failing_hook_only_warns() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let log = install_hook(&harness, 3);
    write_routed_record(&harness, "web", harness.home.path(), "web.localhost", 51234);
    let output = harness.output(&["rm", "web"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "removed web\n");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.starts_with("foster: hook ") && stderr.contains("hook-said-no"),
        "{stderr}"
    );
    assert_eq!(
        fs::read_to_string(&log).expect("hook log"),
        "rm web web.localhost 51234\n"
    );
}

#[test]
fn restore_keeps_a_free_port() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let port = free_port();
    write_routed_record(&harness, "web", harness.home.path(), "web.localhost", port);
    let output = harness.output(&["restore"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = fs::read_to_string(record_path(&harness, "web")).expect("record");
    let record: serde_json::Value = serde_json::from_str(&text).expect("json record");
    assert_eq!(record["port"], serde_json::json!(port));
    assert_eq!(record["domain"], "web.localhost");
}

#[test]
fn rm_removes_the_record_even_without_a_session() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    write_record(&harness, "web", &["pnpm", "dev"], harness.home.path(), None);
    let output = harness.output(&["rm", "web"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "removed web\n");
    assert!(!record_path(&harness, "web").exists());
    assert_eq!(zmx_calls_without_env(&harness), vec!["ls".to_string()]);
}

#[test]
fn hold_marks_the_record_exited() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let shell = harness.install_fake_shell();
    write_record(
        &harness,
        "web",
        &["sh", "-c", "exit 3"],
        harness.home.path(),
        None,
    );
    let output = harness
        .hold(&shell, &["web", "--", "sh", "-c", "exit 3"])
        .output()
        .expect("run hold");
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    let text = fs::read_to_string(record_path(&harness, "web")).expect("record");
    let record: serde_json::Value = serde_json::from_str(&text).expect("json record");
    assert_eq!(record["exit_code"], serde_json::json!(3));
}

#[test]
fn restore_starts_stopped_records_and_skips_the_rest() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=api\tpid=1\tclients=0\tcreated=1\n");
    fs::create_dir_all(harness.home.path().join("web")).expect("web dir");
    let web_dir = harness
        .home
        .path()
        .join("web")
        .canonicalize()
        .expect("canonical web");
    write_record(&harness, "web", &["pnpm", "dev"], &web_dir, None);
    write_record(
        &harness,
        "api",
        &["go", "run", "."],
        harness.home.path(),
        None,
    );
    write_record(&harness, "old", &["true"], harness.home.path(), Some(1));
    let output = harness.output(&["restore"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "skipped api: already running\nrestored web\n"
    );
    let calls = zmx_calls_without_env(&harness);
    let mut attaches = Vec::new();
    for call in &calls {
        if call.starts_with("attach") {
            attaches.push(call.clone());
        }
    }
    assert_eq!(
        attaches,
        vec![format!("attach web {} hold web -- pnpm dev", foster_path())]
    );
    assert_eq!(
        harness.attach_details(),
        format!("cwd={} managed=unset\n", web_dir.display())
    );
}

#[test]
fn restore_reports_a_missing_directory() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    write_record(
        &harness,
        "web",
        &["pnpm", "dev"],
        Path::new("/nonexistent/foster/dir"),
        None,
    );
    let output = harness.output(&["restore"]);
    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("web: directory /nonexistent/foster/dir is gone"),
        "{stderr}"
    );
    assert!(stderr.contains("1 not restored"), "{stderr}");
}

#[test]
fn ls_shows_records_without_a_session_as_stopped() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    write_record(&harness, "web", &["pnpm", "dev"], Path::new("/p"), None);
    let output = harness.output(&["ls", "--json"]);
    let rows: serde_json::Value = serde_json::from_slice(&output.stdout).expect("json");
    assert_eq!(rows[0]["name"], "web");
    assert_eq!(rows[0]["status"], "stopped");
    assert_eq!(rows[0]["command"], "pnpm dev");
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

fn started(output: &Output, name: &str) -> (String, u16) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let prefix = format!("started {name} on https://");
    let rest = stdout
        .strip_prefix(&prefix)
        .unwrap_or_else(|| panic!("{stdout:?} starts with {prefix:?}"));
    let (domain, port) = rest.split_once(" (port ").expect("port part");
    let port = port.strip_suffix(")\n").expect("closing paren");
    (domain.to_string(), port.parse().expect("numeric port"))
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
    let (domain, port) = started(&output, "web");
    assert_eq!(domain, "web.localhost");
    assert!((51000..=51999).contains(&port), "{port}");
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
fn run_rejects_control_characters_in_the_command() {
    let harness = Harness::new();
    harness.install_fake_zmx("");
    let output = harness.output(&["run", "web", "--", "echo", "a\tb"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("has a control character"),
        "{output:?}"
    );
    assert_eq!(harness.zmx_calls(), "");
}

#[test]
fn run_joins_a_running_session_by_name_alone() {
    let harness = Harness::new();
    harness.install_fake_zmx(
        "name=web\tpid=1\tclients=0\tcreated=1\tcwd=file://host/elsewhere\tcmd=/x/foster hold web -- pnpm dev\n",
    );
    let output = harness.output(&["run", "web", "--", "pnpm", "build"]);
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "already running web: pnpm dev\n"
    );
    assert_eq!(String::from_utf8_lossy(&output.stderr), "");
    assert_eq!(zmx_calls_without_env(&harness), vec!["ls".to_string()]);
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
fn run_replaces_exited_session() {
    let harness = Harness::new();
    harness.install_fake_zmx("name=web\tpid=1\tclients=0\tcreated=1\tended=5\texit_code=3\n");
    let output = harness.output(&["run", "web", "--", "true"]);
    assert_eq!(output.status.code(), Some(0));
    started(&output, "web");
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
    started(&output, "web");
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
    let expected = "NAME  STATUS    ATTACHED  STARTED           DOMAIN  PORT  CWD        COMMAND\n\
bad   error     no        -                 -       -     -          -\n\
boom  exited 3  no        2026-10-24 18:30  -       -     /opt/x     sh -c 'exit 3'\n\
web   running   yes       2026-10-24 18:29  -       -     ~/my proj  sh -c 'echo ready; sleep 300'\n";
    assert_eq!(String::from_utf8_lossy(&output.stdout), expected);
}

#[test]
fn ls_prints_json() {
    let harness = Harness::new();
    let output = ls_output(&harness, &["ls", "--json"]);
    assert_eq!(output.status.code(), Some(0));
    let expected = "[{\"name\":\"bad\",\"status\":\"error\",\"exit_code\":null,\"attached\":false,\"started\":null,\"cwd\":null,\"command\":null,\"domain\":null,\"port\":null},\
{\"name\":\"boom\",\"status\":\"exited 3\",\"exit_code\":3,\"attached\":false,\"started\":1792866600,\"cwd\":\"/opt/x\",\"command\":\"sh -c 'exit 3'\",\"domain\":null,\"port\":null},\
{\"name\":\"web\",\"status\":\"running\",\"exit_code\":null,\"attached\":true,\"started\":1792866550,\"cwd\":\"/tmp/home/my proj\",\"command\":\"sh -c 'echo ready; sleep 300'\",\"domain\":null,\"port\":null}]\n";
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
