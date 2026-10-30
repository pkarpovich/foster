use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;

use crate::hold::render_command;
use crate::output::emit;
use crate::state::{self, Record};
use crate::table;
use crate::zmx::{Session, Zmx, launched_command};

const HEADER: [&str; 8] = [
    "NAME", "STATUS", "ATTACHED", "STARTED", "DOMAIN", "PORT", "CWD", "COMMAND",
];
const CWD_COLUMN: usize = 6;

#[derive(Debug, PartialEq, Serialize)]
struct Row {
    name: String,
    status: String,
    exit_code: Option<i64>,
    attached: bool,
    started: Option<i64>,
    cwd: Option<PathBuf>,
    command: Option<String>,
    domain: Option<String>,
    port: Option<u16>,
}

enum Format {
    Table,
    Json,
}

pub fn run(args: &[&str]) -> Result<(), String> {
    let format = match args {
        [] => Format::Table,
        ["--json"] => Format::Json,
        _ => return Err("usage: foster ls [--json]".to_string()),
    };
    let zmx = Zmx::locate()?;
    let records = state::load_all()?;
    let mut rows = Vec::new();
    for session in zmx.sessions()? {
        let mut row = row(session);
        for record in &records {
            if record.name == row.name {
                row.domain.clone_from(&record.domain);
                row.port = record.port;
            }
        }
        rows.push(row);
    }
    for record in records {
        let mut live = false;
        for existing in &rows {
            if existing.name == record.name {
                live = true;
            }
        }
        if !live {
            rows.push(record_row(record));
        }
    }
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    match format {
        Format::Table => {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            emit(render_table(&rows, home.as_deref(), table::terminal_width()).as_bytes());
        }
        Format::Json => {
            let json = serde_json::to_string(&rows).map_err(|error| format!("json: {error}"))?;
            emit(format!("{json}\n").as_bytes());
        }
    }
    Ok(())
}

fn row(session: Session) -> Row {
    match session {
        Session::Broken { name } => Row {
            name,
            status: "error".to_string(),
            exit_code: None,
            attached: false,
            started: None,
            cwd: None,
            command: None,
            domain: None,
            port: None,
        },
        Session::Live {
            name,
            clients,
            created,
            cwd,
            cmd,
            labels,
        } => {
            let code = labels.get("exit_code");
            let status = match code {
                Some(code) => format!("exited {code}"),
                None => "running".to_string(),
            };
            let exit_code = code.and_then(|code| code.parse().ok());
            let command = cmd.map(|cmd| launched_command(&cmd));
            Row {
                name,
                status,
                exit_code,
                attached: clients > 0,
                started: Some(created),
                cwd,
                command,
                domain: None,
                port: None,
            }
        }
    }
}

fn record_row(record: Record) -> Row {
    let Record {
        name,
        command,
        cwd,
        started,
        exit_code,
        domain,
        port,
    } = record;
    let status = match exit_code {
        Some(code) => format!("exited {code}"),
        None => "stopped".to_string(),
    };
    Row {
        name,
        status,
        exit_code: exit_code.map(i64::from),
        attached: false,
        started: Some(started),
        cwd: Some(cwd),
        command: Some(render_command(&command)),
        domain,
        port,
    }
}

fn format_started(epoch: i64) -> String {
    let Ok(timestamp) = Timestamp::from_second(epoch) else {
        return "-".to_string();
    };
    let zoned = timestamp.to_zoned(TimeZone::system());
    zoned.strftime("%Y-%m-%d %H:%M").to_string()
}

fn format_cwd(cwd: &Path, home: Option<&Path>) -> String {
    let Some(home) = home else {
        return cwd.display().to_string();
    };
    let Ok(rest) = cwd.strip_prefix(home) else {
        return cwd.display().to_string();
    };
    if rest.as_os_str().is_empty() {
        return "~".to_string();
    }
    format!("~/{}", rest.display())
}

fn visible(text: &str) -> String {
    let mut shown = String::new();
    for character in text.chars() {
        if character.is_control() {
            shown.push_str(&character.escape_default().to_string());
        } else {
            shown.push(character);
        }
    }
    shown
}

fn render_table(rows: &[Row], home: Option<&Path>, terminal: Option<usize>) -> String {
    let mut cells = Vec::new();
    for Row {
        name,
        status,
        exit_code: _,
        attached,
        started,
        cwd,
        command,
        domain,
        port,
    } in rows
    {
        let attached = if *attached { "yes" } else { "no" };
        let started = match started {
            Some(epoch) => format_started(*epoch),
            None => "-".to_string(),
        };
        let cwd = match cwd {
            Some(cwd) => format_cwd(cwd, home),
            None => "-".to_string(),
        };
        let command = match command {
            Some(command) => command.clone(),
            None => "-".to_string(),
        };
        let domain = match domain {
            Some(domain) => domain.clone(),
            None => "-".to_string(),
        };
        let port = match port {
            Some(port) => port.to_string(),
            None => "-".to_string(),
        };
        cells.push(vec![
            visible(name),
            status.clone(),
            attached.to_string(),
            started,
            visible(&domain),
            port,
            visible(&cwd),
            visible(&command),
        ]);
    }
    table::render(&HEADER, &cells, Some(CWD_COLUMN), terminal)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;

    fn live(cmd: Option<&str>, cwd: Option<&str>, exit_code: Option<&str>) -> Session {
        let mut labels = BTreeMap::new();
        if let Some(code) = exit_code {
            labels.insert("exit_code".to_string(), code.to_string());
        }
        Session::Live {
            name: "web".to_string(),
            clients: 1,
            created: 1792866550,
            cwd: cwd.map(PathBuf::from),
            cmd: cmd.map(str::to_string),
            labels,
        }
    }

    #[test]
    fn running_row() {
        let session = live(
            Some("/x/foster hold web -- sh -c 'a -- b'"),
            Some("/p"),
            None,
        );
        assert_eq!(
            row(session),
            Row {
                name: "web".to_string(),
                status: "running".to_string(),
                exit_code: None,
                attached: true,
                started: Some(1792866550),
                cwd: Some(PathBuf::from("/p")),
                command: Some("sh -c 'a -- b'".to_string()),
                domain: None,
                port: None,
            }
        );
    }

    #[test]
    fn exited_row() {
        let row = row(live(Some("plain cmd"), None, Some("3")));
        assert_eq!(row.status, "exited 3");
        assert_eq!(row.exit_code, Some(3));
        assert_eq!(row.command, Some("plain cmd".to_string()));
    }

    #[test]
    fn broken_row_has_no_values() {
        let session = Session::Broken {
            name: "bad".to_string(),
        };
        assert_eq!(
            row(session),
            Row {
                name: "bad".to_string(),
                status: "error".to_string(),
                exit_code: None,
                attached: false,
                started: None,
                cwd: None,
                command: None,
                domain: None,
                port: None,
            }
        );
    }

    #[test]
    fn cwd_is_shortened_under_home_only() {
        let home = Path::new("/Users/me");
        assert_eq!(
            format_cwd(Path::new("/Users/me/proj"), Some(home)),
            "~/proj"
        );
        assert_eq!(format_cwd(Path::new("/Users/me"), Some(home)), "~");
        assert_eq!(
            format_cwd(Path::new("/Users/meow/x"), Some(home)),
            "/Users/meow/x"
        );
        assert_eq!(format_cwd(Path::new("/tmp/x"), Some(home)), "/tmp/x");
        assert_eq!(format_cwd(Path::new("/tmp/x"), None), "/tmp/x");
    }

    #[test]
    fn table_aligns_without_trailing_spaces() {
        let rows = vec![
            Row {
                name: "web".to_string(),
                status: "running".to_string(),
                exit_code: None,
                attached: false,
                started: None,
                cwd: Some(PathBuf::from("/h/p")),
                command: Some("pnpm dev".to_string()),
                domain: Some("web.localhost".to_string()),
                port: Some(51000),
            },
            Row {
                name: "bad".to_string(),
                status: "error".to_string(),
                exit_code: None,
                attached: false,
                started: None,
                cwd: None,
                command: None,
                domain: None,
                port: None,
            },
        ];
        let expected = "NAME  STATUS   ATTACHED  STARTED  DOMAIN         PORT   CWD  COMMAND\n\
                        web   running  no        -        web.localhost  51000  ~/p  pnpm dev\n\
                        bad   error    no        -        -              -      -    -\n";
        let table = render_table(&rows, Some(Path::new("/h")), None);
        assert_eq!(table, expected);
        for line in table.lines() {
            assert_eq!(line, line.trim_end());
        }
    }

    #[test]
    fn table_shows_control_characters_visibly() {
        let rows = vec![Row {
            name: "web".to_string(),
            status: "running".to_string(),
            exit_code: None,
            attached: false,
            started: None,
            cwd: Some(PathBuf::from("/h/\x1b]52;c;x\x07")),
            command: Some("a\tb\nc".to_string()),
            domain: None,
            port: None,
        }];
        let table = render_table(&rows, None, None);
        assert!(!table.contains('\x1b'), "{table:?}");
        assert!(!table.contains('\x07'), "{table:?}");
        assert_eq!(table.lines().count(), 2);
        assert!(table.contains("/h/\\u{1b}]52;c;x\\u{7}"), "{table:?}");
        assert!(table.ends_with("a\\tb\\nc\n"), "{table:?}");
    }

    #[test]
    fn record_without_session_is_stopped_or_exited() {
        let record = Record {
            name: "web".to_string(),
            command: vec!["sh".to_string(), "-c".to_string(), "pnpm dev".to_string()],
            cwd: PathBuf::from("/p"),
            started: 5,
            exit_code: None,
            domain: None,
            port: None,
        };
        let stopped = record_row(record.clone());
        assert_eq!(stopped.status, "stopped");
        assert_eq!(stopped.command, Some("sh -c 'pnpm dev'".to_string()));
        assert!(!stopped.attached);
        let exited = record_row(Record {
            exit_code: Some(3),
            ..record
        });
        assert_eq!(exited.status, "exited 3");
        assert_eq!(exited.exit_code, Some(3));
    }

    #[test]
    fn started_has_minute_precision() {
        assert_eq!(format_started(1792866550).len(), "2026-10-24 18:29".len());
    }

    fn wide_row() -> Row {
        Row {
            name: "turtle-hub:yotei-app".to_string(),
            status: "running".to_string(),
            exit_code: None,
            attached: true,
            started: None,
            cwd: Some(PathBuf::from(
                "/h/Projects/worktrees/turtle-hub/fix-yotei-app-sheet-mermaid/services/yotei-app",
            )),
            command: Some("mise exec -- pnpm dev".to_string()),
            domain: Some("yotei.localhost".to_string()),
            port: Some(51004),
        }
    }

    #[test]
    fn table_fits_the_terminal_by_cutting_cwd_first() {
        let table = render_table(&[wide_row()], Some(Path::new("/h")), Some(130));
        let expected = "NAME                  STATUS   ATTACHED  STARTED  DOMAIN           PORT   CWD                                COMMAND\n\
                        turtle-hub:yotei-app  running  yes       -        yotei.localhost  51004  …sheet-mermaid/services/yotei-app  mise exec -- pnpm dev\n";
        assert_eq!(table, expected);
        assert_eq!(
            table.lines().nth(1).unwrap_or_default().chars().count(),
            130
        );
    }

    #[test]
    fn table_cuts_the_command_once_cwd_is_at_its_minimum() {
        let table = render_table(&[wide_row()], Some(Path::new("/h")), Some(80));
        let row = table.lines().nth(1).unwrap_or_default();
        assert!(row.contains("…/services/yotei-app"), "{row:?}");
        assert!(row.ends_with("mise exec…"), "{row:?}");
    }

    #[test]
    fn table_is_untouched_when_it_fits_or_has_no_terminal() {
        let wide = render_table(&[wide_row()], Some(Path::new("/h")), Some(400));
        let piped = render_table(&[wide_row()], Some(Path::new("/h")), None);
        assert_eq!(wide, piped);
        assert!(!piped.contains('…'), "{piped:?}");
    }

    #[test]
    fn empty_list_renders_nothing() {
        assert_eq!(render_table(&[], None, None), "");
    }
}
