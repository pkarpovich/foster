use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use tempfile::TempDir;

const USAGE: &str = "usage:
  foster run <name> -- <command...>   start a command and attach to it
  foster ls [--json]                  list processes started with foster
  foster attach <name>                attach to a process (detach: Ctrl+\\)
  foster rm <name>                    stop a process and remove it
";

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
            "#!/bin/sh\necho \"$* ZMX_DIR=$ZMX_DIR ZMX_SESSION=${{ZMX_SESSION-unset}} CLAUDECODE=${{CLAUDECODE-unset}}\" >> '{}'\ncase \"$1\" in ls) cat '{}' ;; esac\n",
            self.zmx_log().display(),
            ls_file.display()
        );
        let path = self.fake_bin.join("foster-zmx");
        fs::write(&path, script).expect("fake zmx");
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod");
    }

    fn zmx_log(&self) -> PathBuf {
        self.home.path().join("zmx.log")
    }

    fn zmx_calls(&self) -> String {
        fs::read_to_string(self.zmx_log()).unwrap_or_default()
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
fn unimplemented_commands_fail() {
    let harness = Harness::new();
    for command in ["run", "ls", "attach"] {
        let output = harness.output(&[command]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "foster: not implemented\n",
            "{command}"
        );
    }
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
    assert!(harness.zmx_calls().contains("kill web "));
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
