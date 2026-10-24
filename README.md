# foster

foster is one entry point for long-running processes on one machine. `foster run` starts a command in a persistent PTY session and attaches you to it. The process keeps running after you detach or your ssh link drops. `foster ls` lists every process started this way, and `foster attach` reconnects from any terminal.

foster does not own PTYs. It drives `foster-zmx`, a build of [zmx](https://github.com/neurosnap/zmx) (MIT) with the explicit-leadership patches from [agterm](https://github.com/umputun/agterm) (MIT, see `zmx/patches/NOTICE`), so the terminal that attaches takes the PTY size at once.

## Build

Requires macOS, mise, git and network access. `zmx/build.sh` fetches zmx and ghostty at pinned revisions and builds them with the zig version from `mise.toml`. On macOS it also patches `include/float.h` in that zig install once to work around a macOS SDK build failure, and keeps the original as `float.h.orig`.

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

A name has 1 to 50 characters: letters, digits, `-`, `_`, `.` and `:`. Running `foster run` again with the same name, command and directory attaches to the process that is already running, so a Zed task or a script can call it repeatedly. The same name with a different command or directory is refused; run `foster rm` first.

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

Stop a process and remove its record:

```
foster rm web
```

When the command exits, foster prints `foster: <command> exited with <code>` in the session and starts your login shell there. A process that exits stays listed as `exited <code>` with its last screen until you run `foster rm` or start a new `foster run` under the same name.

Sessions live in `~/.local/state/foster/zmx`. Plain `zmx ls` does not see them.
