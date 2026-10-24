use std::fs;
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
    for command in ["run", "ls", "attach", "rm"] {
        let output = harness.output(&[command]);
        assert_eq!(output.status.code(), Some(1), "{command}");
        assert_eq!(
            String::from_utf8_lossy(&output.stderr),
            "foster: not implemented\n",
            "{command}"
        );
    }
}
