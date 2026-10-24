# foster

foster is one entry point for long-running processes on one machine. `foster run` starts a command in a persistent PTY session and attaches you to it. The process keeps running after you detach or your ssh link drops. `foster ls` lists every process started this way, and `foster attach` reconnects from any terminal.

foster does not own PTYs. It drives `foster-zmx`, a build of [zmx](https://github.com/neurosnap/zmx) (MIT) with the explicit-leadership patches from [agterm](https://github.com/umputun/agterm) (MIT, see `zmx/patches/NOTICE`), so the terminal that attaches takes the PTY size at once.

## Build

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

Without a terminal (a script, an agent, a dropped ssh link) the command starts detached and foster prints `started web`.

List processes, running or finished:

```
foster ls
foster ls --json
```

Attach to a process. Detach with Ctrl+\:

```
foster attach web
```

Stop a process and remove its record:

```
foster rm web
```

A process that exits stays listed as `exited <code>` with its last screen until you run `foster rm` or start a new `foster run` under the same name.

Sessions live in `~/.local/state/foster/zmx`. Plain `zmx ls` does not see them.
