# foster

A process registry for one machine over a patched zmx. README.md says what it does and how it is installed. This file is what a change to the code has to respect.

## Layout

- `src/main.rs` - argument dispatch and the usage text.
- `src/run.rs`, `task.rs`, `attach.rs`, `list.rs`, `output.rs` (history, tail), `restore.rs`, `rm.rs` - one file per command.
- `src/hold.rs` - `foster hold`, the process inside the PTY: runs the command, forwards SIGHUP and SIGTERM to its group, records the exit code, then execs a login shell.
- `src/proxy.rs` - the attach proxy: raw mode, the cover on the follower, takeover, detach on Ctrl+C.
- `src/zmx.rs` - everything that talks to `foster-zmx`. `src/state.rs` - the records on disk. `src/route.rs` - domains, ports and the hook. `src/table.rs` - the table for `ls` and `task`. `src/name.rs` - `SessionName`.
- `zmx/` - `build.sh` and the vendored patches. `plugins/foster/` - the Claude Code plugin with the agent skill.

## Commands

```
mise exec -- zmx/build.sh
mise exec -- cargo fmt --check
mise exec -- cargo clippy --all-targets -- -D warnings
mise exec -- cargo test
claude plugin validate .
```

## Style

- `for` loops instead of iterator chains, `let ... else` for early returns, shadowing instead of `raw_`/`parsed_` names, newtypes (`SessionName`, `Nonce`) instead of bare strings.
- Full `match` without `_` arms and without `matches!`, explicit destructuring.
- No comments in code and no `#[allow]`.

## Invariants

- A session is identified by its name alone. `run` and `task` on a running name attach, or without a terminal print `already running <name>: <command>` and exit 0. Never compare commands: zmx caps the `cmd` and `cwd` fields of `ls` at 256 bytes and cuts them with `...`.
- A task from a linked worktree is named `<repo>:<worktree>:<slug>` and its domain gets the worktree in front, so worktrees never share a session.
- Every session gets a port from 51000-51999 and a domain, both stored in its record. `hold` passes them to the command as `PORT` and `FOSTER_DOMAIN`, so `restore` keeps them. Two records without an exit code cannot share a domain.
- foster never talks to a proxy. It runs `~/.config/foster/hook` on start and exit (from `hold`) and after `rm`. A failing hook only warns.
- zmx label values accept only `[a-zA-Z0-9-_.]`, so a label cannot carry a path or a command. Those live in the record.
- Records in `~/.local/state/foster/sessions/<name>.json` are written through a temp file and a rename. `hold` marks a record exited, `rm` removes it, `restore` starts the records without an exit code.
- Task commands run through `/bin/sh -c`, never `$SHELL`, so a task behaves the same whoever calls foster.
- Leadership is explicit. Every attach sets `ZMX_MANAGED=<nonce>`, and the attach that takes the PTY also sets `ZMX_MANAGED_CLAIM=1`. The role comes back in the OSC title `zmx-role;<nonce>:<role>:<gen>`, which the proxy strips before the terminal sees it.
- `clean_env` removes the `ZMX_*` variables and the Claude Code session markers from everything foster starts.
- Output goes through `output::emit`, which ignores EPIPE. `print!` panics when the reader closes early.
- On Linux `zmx/build.sh` builds static musl. Release binaries must not depend on the glibc of the machine.

## Tests

- `tests/cli.rs` runs the foster binary against a fake `foster-zmx` shell script. It needs `kill` and `ps` from procps.
- `tests/real_zmx.rs` runs against `zmx/out/foster-zmx`, so `zmx/build.sh` comes first. Its socket directories live under `/tmp`: a unix socket path is limited to about 100 bytes.
- A bug fix comes with a test that reproduced it.

## Agent skill

`plugins/foster/skills/foster/SKILL.md` tells agents how every command behaves. A change to a command, its output or its exit codes updates the skill in the same commit, followed by `claude plugin validate .`.

## Releasing

Bump `version` in `Cargo.toml` and in both plugin manifests, merge, then publish a GitHub release `v<version>`. `release.yml` refuses a tag that does not match `Cargo.toml`, builds the macOS and Linux tarballs and writes `Formula/foster.rb` in `pkarpovich/homebrew-apps`.
