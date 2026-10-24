use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde::Serialize;

use crate::output::emit;
use crate::zmx::{Session, Zmx, decode_word};

const HEADER: [&str; 6] = ["NAME", "STATUS", "ATTACHED", "STARTED", "CWD", "COMMAND"];
const CWD_MIN: usize = 20;
const COMMAND_MIN: usize = 10;

#[derive(Debug, PartialEq, Serialize)]
struct Row {
    name: String,
    status: String,
    exit_code: Option<i64>,
    attached: bool,
    started: Option<i64>,
    cwd: Option<PathBuf>,
    command: Option<String>,
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
    let mut rows = Vec::new();
    for session in zmx.sessions()? {
        rows.push(row(session));
    }
    rows.sort_by(|left, right| left.name.cmp(&right.name));
    match format {
        Format::Table => {
            let home = std::env::var_os("HOME").map(PathBuf::from);
            emit(render_table(&rows, home.as_deref(), terminal_width()).as_bytes());
        }
        Format::Json => {
            let json = serde_json::to_string(&rows).map_err(|error| format!("json: {error}"))?;
            emit(format!("{json}\n").as_bytes());
        }
    }
    Ok(())
}

fn decode_command(command: &str) -> String {
    let mut decoded = String::new();
    let mut token = String::new();
    let mut quoted = false;
    let mut characters = command.chars().chain(Some(' '));
    while let Some(character) = characters.next() {
        if character == '\\' && !quoted {
            token.push(character);
            token.extend(characters.next());
            continue;
        }
        if character == ' ' && !quoted {
            let word = decode_word(&token);
            if !token.contains('\'') && word.chars().any(char::is_control) {
                decoded.push_str(&format!("'{word}'"));
            } else {
                decoded.push_str(&word);
            }
            decoded.push(' ');
            token.clear();
            continue;
        }
        if character == '\'' {
            quoted = !quoted;
        }
        token.push(character);
    }
    decoded.pop();
    decoded
}

fn user_command(cmd: &str) -> String {
    match cmd.split_once(" -- ") {
        Some((_, command)) => decode_command(command),
        None => cmd.to_string(),
    }
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
            let command = cmd.map(|cmd| user_command(&cmd));
            Row {
                name,
                status,
                exit_code,
                attached: clients > 0,
                started: Some(created),
                cwd,
                command,
            }
        }
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

fn terminal_width() -> Option<usize> {
    let stdout = std::io::stdout();
    if !stdout.is_terminal() {
        return None;
    }
    let size = rustix::termios::tcgetwinsize(&stdout).ok()?;
    if size.ws_col == 0 {
        return None;
    }
    Some(usize::from(size.ws_col))
}

fn cut_left(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    let mut kept = String::from("…");
    for character in text.chars().skip(count + 1 - width) {
        kept.push(character);
    }
    kept
}

fn cut_right(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let mut kept = String::new();
    for character in text.chars().take(width - 1) {
        kept.push(character);
    }
    kept.push('…');
    kept
}

fn fit(widths: &mut [usize; 5], cells: &[[String; 6]], terminal: usize) -> Option<usize> {
    let mut longest_command = 0;
    for line in cells {
        longest_command = longest_command.max(line[5].chars().count());
    }
    let mut fixed = 0;
    for width in &widths[..4] {
        fixed += width + 2;
    }
    let total = fixed + widths[4] + 2 + longest_command;
    if total <= terminal {
        return None;
    }
    let spare = widths[4].saturating_sub(CWD_MIN);
    widths[4] -= (total - terminal).min(spare);
    let room = terminal.saturating_sub(fixed + widths[4] + 2);
    Some(room.max(COMMAND_MIN))
}

fn render_table(rows: &[Row], home: Option<&Path>, terminal: Option<usize>) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let mut cells = vec![HEADER.map(str::to_string)];
    for Row {
        name,
        status,
        exit_code: _,
        attached,
        started,
        cwd,
        command,
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
        cells.push([
            visible(name),
            status.clone(),
            attached.to_string(),
            started,
            visible(&cwd),
            visible(&command),
        ]);
    }
    let mut widths = [0usize; 5];
    for line in &cells {
        for (index, width) in widths.iter_mut().enumerate() {
            *width = (*width).max(line[index].chars().count());
        }
    }
    let command_width = match terminal {
        Some(terminal) => fit(&mut widths, &cells, terminal),
        None => None,
    };
    let mut table = String::new();
    for line in &cells {
        for (index, width) in widths.iter().enumerate() {
            let cell = if index == 4 {
                cut_left(&line[index], *width)
            } else {
                line[index].clone()
            };
            table.push_str(&cell);
            for _ in cell.chars().count()..*width + 2 {
                table.push(' ');
            }
        }
        match command_width {
            Some(width) => table.push_str(&cut_right(&line[5], width)),
            None => table.push_str(&line[5]),
        }
        table.push('\n');
    }
    table
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
    fn user_command_decodes_transport_encoding() {
        assert_eq!(
            user_command("/x/foster hold web -- echo %2541 a%09b"),
            "echo %41 'a\tb'"
        );
    }

    #[test]
    fn user_command_keeps_quoted_words_whole() {
        assert_eq!(
            user_command("/x/foster hold web -- sh -c 'a b%09c' x  y"),
            "sh -c 'a b\tc' x  y"
        );
    }

    #[test]
    fn user_command_keeps_escaped_apostrophes() {
        assert_eq!(
            user_command("/x/foster hold web -- echo 'it'\\''s' later"),
            "echo 'it'\\''s' later"
        );
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
            },
            Row {
                name: "bad".to_string(),
                status: "error".to_string(),
                exit_code: None,
                attached: false,
                started: None,
                cwd: None,
                command: None,
            },
        ];
        let expected = "NAME  STATUS   ATTACHED  STARTED  CWD  COMMAND\n\
                        web   running  no        -        ~/p  pnpm dev\n\
                        bad   error    no        -        -    -\n";
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
        }];
        let table = render_table(&rows, None, None);
        assert!(!table.contains('\x1b'), "{table:?}");
        assert!(!table.contains('\x07'), "{table:?}");
        assert_eq!(table.lines().count(), 2);
        assert!(table.contains("/h/\\u{1b}]52;c;x\\u{7}"), "{table:?}");
        assert!(table.ends_with("a\\tb\\nc\n"), "{table:?}");
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
        }
    }

    #[test]
    fn cut_keeps_short_text() {
        assert_eq!(cut_left("abc", 3), "abc");
        assert_eq!(cut_right("abc", 3), "abc");
    }

    #[test]
    fn cut_left_keeps_the_tail() {
        assert_eq!(cut_left("/a/b/services/app", 8), "…ces/app");
    }

    #[test]
    fn cut_right_keeps_the_head() {
        assert_eq!(cut_right("mise exec -- pnpm dev", 10), "mise exec…");
    }

    #[test]
    fn table_fits_the_terminal_by_cutting_cwd_first() {
        let table = render_table(&[wide_row()], Some(Path::new("/h")), Some(110));
        let expected = "NAME                  STATUS   ATTACHED  STARTED  CWD                                    COMMAND\n\
                        turtle-hub:yotei-app  running  yes       -        …app-sheet-mermaid/services/yotei-app  mise exec -- pnpm dev\n";
        assert_eq!(table, expected);
        assert_eq!(
            table.lines().nth(1).unwrap_or_default().chars().count(),
            110
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
