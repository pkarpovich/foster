---
name: foster
description: >
  Start, check, read and stop long-running processes with foster, a process registry for one machine
  that keeps a command alive in its own PTY after the caller exits. Use it whenever you need a dev server,
  watcher, worker or any command that keeps running (pnpm dev, vite, go run ./cmd/server, cargo watch,
  docker compose up in the foreground) to outlive your turn or your session, instead of `&`, `nohup`,
  `run_in_background` or a tmux window. Also use it when the user asks whether something is running,
  wants the output or error of a server that was started earlier, asks to stop or restart one,
  or wants their processes back after a reboot,
  and whenever `foster ls`, `foster run`, `foster task`, `foster history`, `foster restore`, a project's `.zed/tasks.json` or a foster session name like
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
| `foster run <name> -- <command...>` | Start the command in the current directory. With a terminal it attaches; without one it prints `started <name> on https://<domain> (port <port>)` and returns. | Yes |
| `foster ls --json` | One JSON array: `name`, `status` (`running`, `exited <code>`, `stopped`, `error`), `exit_code`, `attached`, `started` (epoch seconds), `cwd`, `command`, `domain`, `port`. | Yes |
| `foster history <name>` | Scrollback and screen as plain text, then exit. | Yes |
| `foster history <name> -f` | The same, then keeps streaming new output until killed. | Only under a timeout |
| `foster tail <name>` | Streams only new output, escape codes included, until killed. | Only under a timeout |
| `foster restore` | Start again every process that was running before a reboot (`stopped` in `ls`). | Ask first |
| `foster rm <name>` | Stops the process and removes its record. | See "Stopping" |
| `foster task [<name> \| --json]` | List the project's `.zed/tasks.json` tasks, or start one like `run`. | Yes |
| `foster attach <name>` | Interactive attach for a human. Needs a terminal and fails without one. | No |

Your Bash tool has no terminal, so `foster run` always takes the detached path for you. Never call
`foster attach`; tell the user the command to run instead.

## Starting a project's own tasks

If the project has `.zed/tasks.json`, prefer its tasks over writing the command yourself: the user already
named them and they carry the right directory and environment.

- `foster task --json` lists them: `name` (`<repository>:<label slug>`), `label`, `domain`, `command`, `cwd`,
  and `needs` (an editor variable such as `ZED_STEM` that makes the task Zed-only, else null).
- `foster task <name>` starts one exactly like `foster run`; the short slug (`web-dev`) or the full name (`myrepo:web-dev`)
  both work. A task with `needs` set cannot run outside Zed; say so instead of rewriting it.
- The repository part comes from git. A linked worktree adds its folder to the name and the domain
  (`myrepo:feat-x:web-dev` on `feat-x.web-dev.localhost`), so starting a task there runs that worktree's
  code instead of attaching to the main checkout's process.
- `FOSTER_DOMAIN` in a task's `env` names its domain (`yotei` gives `yotei.localhost`); foster removes it
  from the command's environment and passes the final domain.
- Task commands run through `/bin/sh -c`, whoever calls foster, so they behave the same from an agent,
  a terminal or Zed.

## Starting a process

- Pick a stable name. Letters, digits, `-`, `_`, `.` and `:`, at most 50 characters. Use
  `<project>:<service>` (for example `turtle-hub:yotei-app`) so names from different repos do not collide
  and the user can guess them.
- `cd` to the directory the command must run in first: foster records the caller's directory and starts
  the command there.
- The command is an argv list, not a shell line. Pipes, `&&`, redirects and globs need a shell:
  `foster run api -- sh -c 'make migrate && ./bin/api'`. Set variables with `env`:
  `foster run web -- env PORT=5173 pnpm dev`.
- Running the same name again is safe: a running session is identified by its name alone. Without a terminal
  `foster run` then prints `already running <name>: <command>` and exits 0, so you can call it
  unconditionally. It does not compare commands: if the printed command is not the one you meant, the name
  is taken by something else. Report it, or `foster rm` it only if it is yours. An exited or broken session
  under that name is replaced.
- Command words cannot contain control characters (tabs, newlines); put such text in a `sh -c` script.
- Every process gets a free port from 51000-51999 as `PORT` and a domain as `FOSTER_DOMAIN`
  (the last part of the name plus `.localhost`). The service should listen on `$PORT`: a port set by hand
  (`env PORT=5173`, a hard-coded `--port`) overrides it, and the domain then points at nothing.
  `foster ls --json` shows both. Two running processes cannot share a domain: foster refuses the second.
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

## After a reboot

Sessions do not survive a reboot, but their records do. `foster ls --json` then reports what was running as `status: "stopped"`. `foster restore` starts all of them again, detached, each in its recorded directory, and skips the ones that exited before or are running already. It is the user's call: run it when they ask to bring things back, or ask before running it yourself, since it starts every stopped process at once. A `stopped` entry that should stay down is removed with `foster rm <name>`.

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
