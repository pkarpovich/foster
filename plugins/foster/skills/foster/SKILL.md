---
name: foster
description: >
  Start, check, read and stop long-running processes with foster, a process registry for one machine
  that keeps a command alive in its own PTY after the caller exits. Use it whenever you need a dev server,
  watcher, worker or any command that keeps running (pnpm dev, vite, go run ./cmd/server, cargo watch,
  docker compose up in the foreground) to outlive your turn or your session, instead of `&`, `nohup`,
  `run_in_background` or a tmux window. Also use it when the user asks whether something is running,
  wants the output or error of a server that was started earlier, or asks to stop or restart one,
  and whenever `foster ls`, `foster run`, `foster history` or a foster session name like
  `project:service` comes up. Skip it for commands that finish on their own (tests, builds, one-off scripts).
---

# foster

foster runs a command inside a persistent PTY session (a patched zmx daemon underneath) and records it.
The process survives the terminal, the ssh link and the agent session that started it. Anyone can list
it, read its output or attach to it later. A process that exits stays listed with its exit code and its
last screen until it is removed or started again.

Check that it is installed before relying on it: `command -v foster` must print a path. If it does not,
tell the user and fall back to whatever they normally use.

## Commands

| Command | What it does | Agent use |
|---|---|---|
| `foster run <name> -- <command...>` | Start the command in the current directory. With a terminal it attaches; without one it prints `started <name>` and returns. | Yes |
| `foster ls --json` | One JSON array: `name`, `status` (`running`, `exited <code>`, `error`), `exit_code`, `attached`, `started` (epoch seconds), `cwd`, `command`. | Yes |
| `foster history <name>` | Scrollback and screen as plain text, then exit. | Yes |
| `foster history <name> -f` | The same, then keeps streaming new output until killed. | Only under a timeout |
| `foster tail <name>` | Streams only new output, escape codes included, until killed. | Only under a timeout |
| `foster rm <name>` | Stops the process and removes its record. | See "Stopping" |
| `foster attach <name>` | Interactive attach for a human. Needs a terminal and fails without one. | No |

Your Bash tool has no terminal, so `foster run` always takes the detached path for you. Never call
`foster attach`; tell the user the command to run instead.

## Starting a process

- Pick a stable name. Letters, digits, `-`, `_`, `.` and `:`, at most 50 characters. Use
  `<project>:<service>` (for example `turtle-hub:yotei-app`) so names from different repos do not collide
  and the user can guess them.
- `cd` to the directory the command must run in first: foster records the caller's directory and starts
  the command there.
- The command is an argv list, not a shell line. Pipes, `&&`, redirects and globs need a shell:
  `foster run api -- sh -c 'make migrate && ./bin/api'`. Set variables with `env`:
  `foster run web -- env PORT=5173 pnpm dev`.
- Running the same name again is safe. The same command from the same directory while it runs prints
  `already running <name>` and exits 0, so you can call it unconditionally. A different command or
  directory under a running name fails with `<name> is already running a different command`. An exited
  or broken session under that name is replaced.
- The process inherits your environment minus Claude Code's session markers, so a `claude` started under
  foster behaves like one started from a plain terminal.

## Waiting until it is ready

`foster run` returns as soon as the session exists, not when the server is up. Poll `foster history`
for the line that means ready, with a deadline, and check `foster ls --json` for an early exit:

```sh
for i in $(seq 1 60); do
  foster history web | grep -q 'Local:' && break
  foster ls --json | grep -q '"name":"web","status":"exited' && break
  sleep 1
done
foster history web | tail -n 20
```

Pick the ready marker from what the program prints (`Local:` for vite, `Listening on`, `ready in`).
If you stream with `history -f` or `tail` instead, always bound it, because neither exits on its own:
`timeout 30 foster history web -f` (or `gtimeout` on macOS without coreutils linked).

## Reading output and diagnosing

- `foster history <name> | tail -n 100` gives the recent output as clean text. Use it after a crash, when
  the user asks what a server printed, or to find the port it chose.
- `foster ls --json` tells you whether it is still running. `exited <code>` means the command ended; the
  session then holds a login shell, and its history keeps the last screen and a line
  `foster: <command> exited with <code>`.
- `error` means the session daemon does not answer; it can only be removed with `foster rm`.

## Stopping

`foster rm <name>` kills the whole process group and drops the record. The processes in foster usually
belong to the user, often started from their editor or another machine. Stop only a session you started
in this conversation, or one the user explicitly asked you to stop. When a port is busy or a name is
taken, report it with `foster ls` instead of removing someone else's process.

To restart with the same command after a crash, run `foster run` again: an exited session is replaced.
To change the command of a running session, ask the user before `foster rm`.

## Telling the user

After starting something, give the user the name and the commands they need:
`foster attach <name>` to watch it (Ctrl+C detaches and leaves it running), `foster rm <name>` to stop it.
When a second terminal attaches, the first shows a cover; any key there takes the session back.
