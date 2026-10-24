use std::io::{Read, Write};
use std::sync::mpsc::{self, Sender};
use std::thread;

use portable_pty::{Child, MasterPty, PtySize, native_pty_system};
use rustix::termios::{OptionalActions, Termios, tcgetattr, tcgetwinsize, tcsetattr};
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;

use crate::name::SessionName;
use crate::zmx::{Claim, Zmx};

const ROLE_PREFIX: &[u8] = b"2;zmx-role;";
const PENDING_LIMIT: usize = 4096;
const COVER_TITLE: &str = "In use in another terminal";
const COVER_HINT: &str = "Press any key to use it here, Ctrl+C to detach";
const DETACH_KEY: u8 = 0x1c;
const INTERRUPT_KEY: u8 = 0x03;

#[derive(Debug, PartialEq)]
pub enum Role {
    Leader,
    Follower,
    Unowned,
}

#[derive(Debug, PartialEq)]
pub struct RoleReport {
    pub nonce: String,
    pub role: Role,
}

#[derive(Default)]
pub struct RoleFilter {
    pending: Vec<u8>,
}

impl RoleFilter {
    pub fn feed(&mut self, bytes: &[u8]) -> (Vec<u8>, Vec<RoleReport>) {
        let mut data = std::mem::take(&mut self.pending);
        data.extend_from_slice(bytes);
        let mut shown = Vec::new();
        let mut reports = Vec::new();
        let mut index = 0;
        while index < data.len() {
            if data[index] != 0x1b {
                shown.push(data[index]);
                index += 1;
                continue;
            }
            let Some(&next) = data.get(index + 1) else {
                self.pending = data[index..].to_vec();
                break;
            };
            if next != b']' {
                shown.push(data[index]);
                index += 1;
                continue;
            }
            let Some((payload_end, sequence_end)) = osc_end(&data, index + 2) else {
                if data.len() - index > PENDING_LIMIT {
                    shown.extend_from_slice(&data[index..]);
                } else {
                    self.pending = data[index..].to_vec();
                }
                break;
            };
            let payload = &data[index + 2..payload_end];
            match payload.strip_prefix(ROLE_PREFIX) {
                Some(report) => {
                    if let Some(report) = parse_report(report) {
                        reports.push(report);
                    }
                }
                None => shown.extend_from_slice(&data[index..sequence_end]),
            }
            index = sequence_end;
        }
        (shown, reports)
    }
}

fn wants_detach(bytes: &[u8]) -> bool {
    bytes.contains(&DETACH_KEY) || bytes.contains(&INTERRUPT_KEY)
}

fn before_detach(bytes: &[u8]) -> Option<&[u8]> {
    for (index, byte) in bytes.iter().enumerate() {
        if *byte == DETACH_KEY || *byte == INTERRUPT_KEY {
            return Some(&bytes[..index]);
        }
    }
    None
}

fn osc_end(data: &[u8], start: usize) -> Option<(usize, usize)> {
    let mut index = start;
    while index < data.len() {
        if data[index] == 0x07 {
            return Some((index, index + 1));
        }
        if data[index] == 0x1b && data.get(index + 1) == Some(&b'\\') {
            return Some((index, index + 2));
        }
        index += 1;
    }
    None
}

fn parse_report(report: &[u8]) -> Option<RoleReport> {
    let report = std::str::from_utf8(report).ok()?;
    let mut fields = report.split(':');
    let nonce = fields.next()?;
    let role = match fields.next()? {
        "leader" => Role::Leader,
        "follower" => Role::Follower,
        "unowned" => Role::Unowned,
        _ => return None,
    };
    fields.next()?.parse::<u32>().ok()?;
    if fields.next().is_some() {
        return None;
    }
    Some(RoleReport {
        nonce: nonce.to_string(),
        role,
    })
}

enum Event {
    Input(Vec<u8>),
    Output(u64, Vec<u8>),
    Exited(u64),
    Resize,
}

#[derive(Clone, Copy, PartialEq)]
enum Screen {
    Live,
    Covered,
}

struct Client {
    generation: u64,
    nonce: String,
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    master: Box<dyn MasterPty + Send>,
}

struct RawTerminal {
    saved: Termios,
}

impl RawTerminal {
    fn enter() -> Result<RawTerminal, String> {
        let stdin = std::io::stdin();
        let saved = tcgetattr(&stdin).map_err(|error| format!("read terminal mode: {error}"))?;
        let mut raw = saved.clone();
        raw.make_raw();
        tcsetattr(&stdin, OptionalActions::Now, &raw)
            .map_err(|error| format!("set raw mode: {error}"))?;
        Ok(RawTerminal { saved })
    }
}

impl Drop for RawTerminal {
    fn drop(&mut self) {
        let _ = tcsetattr(std::io::stdin(), OptionalActions::Now, &self.saved);
        let mut stdout = std::io::stdout();
        let _ = stdout.write_all(b"\x1b[?25h");
        let _ = stdout.flush();
    }
}

fn outer_size() -> PtySize {
    let Ok(size) = tcgetwinsize(std::io::stdin()) else {
        return PtySize::default();
    };
    if size.ws_row == 0 || size.ws_col == 0 {
        return PtySize::default();
    }
    PtySize {
        rows: size.ws_row,
        cols: size.ws_col,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn spawn_client(
    zmx: &Zmx,
    name: &SessionName,
    claim: Claim,
    extra: &[String],
    generation: u64,
    events: &Sender<Event>,
) -> Result<Client, String> {
    let pair = native_pty_system()
        .openpty(outer_size())
        .map_err(|error| format!("open pty: {error}"))?;
    let (builder, nonce) = zmx.attach_builder(name, claim, extra)?;
    let child = pair
        .slave
        .spawn_command(builder)
        .map_err(|error| format!("start foster-zmx: {error}"))?;
    drop(pair.slave);
    let mut reader = pair
        .master
        .try_clone_reader()
        .map_err(|error| format!("read pty: {error}"))?;
    let writer = pair
        .master
        .take_writer()
        .map_err(|error| format!("write pty: {error}"))?;
    let events = events.clone();
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            let count = match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            if events
                .send(Event::Output(generation, buffer[..count].to_vec()))
                .is_err()
            {
                return;
            }
        }
        let _ = events.send(Event::Exited(generation));
    });
    Ok(Client {
        generation,
        nonce: nonce.as_str().to_string(),
        child,
        writer,
        master: pair.master,
    })
}

fn draw_cover(stdout: &mut impl Write) -> std::io::Result<()> {
    let size = outer_size();
    let rows = usize::from(size.rows);
    let cols = usize::from(size.cols);
    let mut screen = String::from("\x1b[?25l\x1b[2J");
    let top = (rows / 2).max(1);
    let lines = [COVER_TITLE, COVER_HINT];
    for (offset, line) in lines.iter().enumerate() {
        let column = cols.saturating_sub(line.chars().count()) / 2 + 1;
        screen.push_str(&format!("\x1b[{};{}H{}", top + offset, column, line));
    }
    stdout.write_all(screen.as_bytes())?;
    stdout.flush()
}

pub fn attach(zmx: &Zmx, name: &SessionName, first: &[String]) -> Result<(), String> {
    let terminal = RawTerminal::enter()?;
    let (events, receiver) = mpsc::channel();
    let input = events.clone();
    thread::spawn(move || {
        let mut stdin = std::io::stdin();
        let mut buffer = [0u8; 4096];
        loop {
            let count = match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => return,
                Ok(count) => count,
            };
            if input.send(Event::Input(buffer[..count].to_vec())).is_err() {
                return;
            }
        }
    });
    let resize = events.clone();
    let mut signals = Signals::new([SIGWINCH]).map_err(|error| format!("watch resize: {error}"))?;
    thread::spawn(move || {
        for _ in signals.forever() {
            if resize.send(Event::Resize).is_err() {
                return;
            }
        }
    });
    let mut client = spawn_client(zmx, name, Claim::Take, first, 0, &events)?;
    let mut filter = RoleFilter::default();
    let mut screen = Screen::Live;
    let mut stdout = std::io::stdout();
    let result = loop {
        let Ok(event) = receiver.recv() else {
            break Err("event channel closed".to_string());
        };
        match event {
            Event::Input(bytes) => match screen {
                Screen::Live => {
                    match before_detach(&bytes) {
                        Some(prefix) => {
                            let _ = client.writer.write_all(prefix);
                            let _ = client.writer.write_all(&[DETACH_KEY]);
                        }
                        None => {
                            let _ = client.writer.write_all(&bytes);
                        }
                    }
                    let _ = client.writer.flush();
                }
                Screen::Covered => {
                    let _ = client.child.kill();
                    if wants_detach(&bytes) {
                        let _ = stdout.write_all(b"\x1b[2J\x1b[H");
                        break Ok(());
                    }
                    let _ = stdout.write_all(b"\x1b[?25h");
                    client =
                        spawn_client(zmx, name, Claim::Take, &[], client.generation + 1, &events)?;
                    filter = RoleFilter::default();
                    screen = Screen::Live;
                }
            },
            Event::Output(generation, bytes) => {
                if generation != client.generation {
                    continue;
                }
                let (shown, reports) = filter.feed(&bytes);
                for RoleReport { nonce, role } in reports {
                    if nonce != client.nonce {
                        continue;
                    }
                    match role {
                        Role::Leader => screen = Screen::Live,
                        Role::Follower => {
                            screen = Screen::Covered;
                            let _ = draw_cover(&mut stdout);
                        }
                        Role::Unowned => {
                            let _ = client.child.kill();
                            client = spawn_client(
                                zmx,
                                name,
                                Claim::Leave,
                                &[],
                                client.generation + 1,
                                &events,
                            )?;
                            filter = RoleFilter::default();
                            screen = Screen::Live;
                        }
                    }
                }
                if screen == Screen::Live {
                    let _ = stdout.write_all(&shown);
                    let _ = stdout.flush();
                }
            }
            Event::Exited(generation) => {
                if generation != client.generation {
                    continue;
                }
                let status = client
                    .child
                    .wait()
                    .map_err(|error| format!("wait foster-zmx: {error}"))?;
                if status.success() {
                    break Ok(());
                }
                break Err(format!("foster-zmx exited with {}", status.exit_code()));
            }
            Event::Resize => {
                let _ = client.master.resize(outer_size());
                if screen == Screen::Covered {
                    let _ = draw_cover(&mut stdout);
                }
            }
        }
    };
    drop(terminal);
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(nonce: &str, role: Role) -> RoleReport {
        RoleReport {
            nonce: nonce.to_string(),
            role,
        }
    }

    #[test]
    fn ctrl_c_and_ctrl_backslash_detach_from_the_cover() {
        assert!(wants_detach(b"\x1c"));
        assert!(wants_detach(b"\x03"));
        assert!(!wants_detach(b"x"));
    }

    #[test]
    fn live_input_stops_at_the_first_detach_key() {
        assert_eq!(before_detach(b"ab\x03cd"), Some(&b"ab"[..]));
        assert_eq!(before_detach(b"\x1c"), Some(&b""[..]));
        assert_eq!(before_detach(b"plain"), None);
    }

    #[test]
    fn passes_plain_output_through() {
        let mut filter = RoleFilter::default();
        let (shown, reports) = filter.feed(b"hello\x1b[31mred\x1b[0m");
        assert_eq!(shown, b"hello\x1b[31mred\x1b[0m");
        assert!(reports.is_empty());
    }

    #[test]
    fn strips_role_titles_with_bel_and_st() {
        let mut filter = RoleFilter::default();
        let (shown, reports) =
            filter.feed(b"a\x1b]2;zmx-role;n1:leader:1\x07b\x1b]2;zmx-role;n1:follower:2\x1b\\c");
        assert_eq!(shown, b"abc");
        assert_eq!(
            reports,
            vec![report("n1", Role::Leader), report("n1", Role::Follower)]
        );
    }

    #[test]
    fn keeps_other_titles() {
        let mut filter = RoleFilter::default();
        let (shown, reports) = filter.feed(b"\x1b]2;vim\x07x");
        assert_eq!(shown, b"\x1b]2;vim\x07x");
        assert!(reports.is_empty());
    }

    #[test]
    fn joins_a_title_split_across_chunks() {
        let mut filter = RoleFilter::default();
        let (first, reports) = filter.feed(b"ab\x1b]2;zmx-ro");
        assert_eq!(first, b"ab");
        assert!(reports.is_empty());
        let (second, reports) = filter.feed(b"le;n9:unowned:0\x07cd");
        assert_eq!(second, b"cd");
        assert_eq!(reports, vec![report("n9", Role::Unowned)]);
    }

    #[test]
    fn holds_a_lone_escape_until_the_next_chunk() {
        let mut filter = RoleFilter::default();
        let (first, _) = filter.feed(b"x\x1b");
        assert_eq!(first, b"x");
        let (second, _) = filter.feed(b"[1m");
        assert_eq!(second, b"\x1b[1m");
    }

    #[test]
    fn drops_malformed_role_titles() {
        let mut filter = RoleFilter::default();
        let (shown, reports) = filter.feed(b"\x1b]2;zmx-role;n1:boss:1\x07ok");
        assert_eq!(shown, b"ok");
        assert!(reports.is_empty());
    }

    #[test]
    fn flushes_an_unterminated_sequence_past_the_limit() {
        let mut filter = RoleFilter::default();
        let mut bytes = b"\x1b]2;".to_vec();
        bytes.extend(vec![b'x'; PENDING_LIMIT + 10]);
        let (shown, _) = filter.feed(&bytes);
        assert_eq!(shown, bytes);
    }
}
