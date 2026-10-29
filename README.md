# foster

[![CI](https://github.com/pkarpovich/foster/actions/workflows/ci.yml/badge.svg)](https://github.com/pkarpovich/foster/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

foster is one entry point for long-running processes on one machine. `foster run` starts a command in a persistent PTY session and attaches you to it. The process keeps running after you detach or your ssh link drops. `foster ls` lists every process started this way, and `foster attach` reconnects from any terminal.

foster does not own PTYs. It drives `foster-zmx`, a build of [zmx](https://github.com/neurosnap/zmx) (MIT) with the explicit-leadership patches from [agterm](https://github.com/umputun/agterm) (MIT, see `zmx/patches/NOTICE`), so the terminal that attaches takes the PTY size at once.

## Install

Homebrew on Apple Silicon Macs and on x86_64 Linux:

```
brew install pkarpovich/apps/foster
```

The formula installs `foster` and `foster-zmx` side by side.

Linux on x86_64 without Homebrew: static binaries from the latest release, `~/.local/bin` must be on `PATH`:

```
mkdir -p ~/.local/bin
curl -fsSL https://github.com/pkarpovich/foster/releases/latest/download/foster-x86_64-unknown-linux-musl.tar.gz | tar -xz -C ~/.local/bin foster foster-zmx
```

## Build

Requires macOS or Linux, mise, git and network access. `zmx/build.sh` fetches zmx and ghostty at pinned revisions and builds them with the zig version from `mise.toml`, as a static musl binary on Linux. On macOS it also patches `include/float.h` in that zig install once to work around a macOS SDK build failure, and keeps the original as `float.h.orig`.

```
mise install
mise exec -- zmx/build.sh
mise exec -- cargo install --path .
install -m 0755 zmx/out/foster-zmx "$(dirname "$(command -v foster)")/"
```

## Commands

Start a command and attach to it:

```
foster run web -- pnpm dev
```

A name has 1 to 50 characters: letters, digits, `-`, `_`, `.` and `:`. A running process is identified by its name alone: `foster run` with a name that is already running attaches to it (or, without a terminal, prints `already running <name>: <command>`), whatever command you pass, so a Zed task or a script can call it repeatedly. To change the command of a running process, `foster rm` it first. Command words cannot contain control characters such as tabs or newlines.

Without a terminal (a script, an agent, a dropped ssh link) the command starts detached and foster prints `started web`.

List processes, running or finished:

```
foster ls
foster ls --json
```

`--json` prints one array. Each object has `name`, `status` (`running`, `exited <code>` or `error`), `exit_code` (number or null), `attached` (bool), `started` (epoch seconds or null), `cwd` (path or null) and `command` (string or null).

Attach to a process. Ctrl+C (or Ctrl+\) detaches and leaves the process running. Ctrl+C never reaches the process, so stop it with `foster rm`. When a second terminal attaches, the first one shows a cover; press any key there to take the process back:

```
foster attach web
```

Read what a process printed, for a script or an agent that has no terminal. `history` prints the scrollback and the screen as plain text and exits. `history -f` prints the same and then keeps streaming new output until interrupted. `tail` streams only new output. Streamed output keeps the program's escape codes:

```
foster history web
foster history web -f
foster tail web
```

Start a task the project already defines for Zed. foster reads `.zed/tasks.json` at the root of the git checkout, names each task `<repository>:<label as a slug>`, and starts it like `foster run` with the task's `command` and `args` through `/bin/sh -c` (the same for every caller), its `cwd` and its `env`. `$ZED_WORKTREE_ROOT` and `$ZED_MAIN_GIT_WORKTREE` are filled in; a task that needs any other editor variable (`$ZED_FILE`, `$ZED_STEM`, ...) without a default is listed but refused. The repository name comes from git, so every worktree of a repository uses the same names:

```
foster task                     # list: name, Zed label, command
foster task --json              # the same as JSON
foster task web-dev             # start <repository>:web-dev, the task labelled "web: dev"
```

A reboot ends every session: the zmx daemons live in memory. foster keeps one record per process in `~/.local/state/foster/sessions/` (name, command, directory, start time, and the exit code once the command ends on its own), so after a reboot `foster ls` shows what was running as `stopped`, and one command brings all of it back:

```
foster restore                  # start every stopped process again, detached
```

`restore` skips processes that exited before the reboot and names already running. It starts each one in its recorded directory with the environment of the terminal you call it from; inherited variables are not stored, so put anything a process needs into its command (`env FOO=bar ...`) or the Zed task's `env`.

Stop a process and remove its record, so `restore` no longer starts it:

```
foster rm web
```

When the command exits, foster prints `foster: <command> exited with <code>` in the session and starts your login shell there. A process that exits stays listed as `exited <code>` with its last screen until you run `foster rm` or start a new `foster run` under the same name.

Sessions live in `~/.local/state/foster/zmx`. Plain `zmx ls` does not see them.

## Agent skill

The repository is also a Claude Code plugin marketplace with one skill that teaches an agent to start servers under foster, wait until they are ready, read their output and leave the user's processes alone:

```
/plugin install foster --marketplace pkarpovich/foster
```
