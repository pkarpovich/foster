use std::io::IsTerminal;

const SHRINK_MIN: usize = 20;
const LAST_MIN: usize = 10;

pub fn terminal_width() -> Option<usize> {
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

fn fit(
    widths: &mut [usize],
    cells: &[Vec<String>],
    shrink: Option<usize>,
    terminal: usize,
) -> Option<usize> {
    let last = widths.len();
    let mut longest_last = 0;
    for line in cells {
        longest_last = longest_last.max(line[last].chars().count());
    }
    let mut used = 0;
    for width in widths.iter() {
        used += width + 2;
    }
    let total = used + longest_last;
    if total <= terminal {
        return None;
    }
    if let Some(column) = shrink {
        let spare = widths[column].saturating_sub(SHRINK_MIN);
        let cut = (total - terminal).min(spare);
        widths[column] -= cut;
        used -= cut;
    }
    Some(terminal.saturating_sub(used).max(LAST_MIN))
}

pub fn render(
    header: &[&str],
    rows: &[Vec<String>],
    shrink: Option<usize>,
    terminal: Option<usize>,
) -> String {
    if rows.is_empty() {
        return String::new();
    }
    let last = header.len() - 1;
    let mut heading = Vec::new();
    for title in header {
        heading.push(title.to_string());
    }
    let mut cells = vec![heading];
    for row in rows {
        cells.push(row.clone());
    }
    let mut widths = vec![0usize; last];
    for line in &cells {
        for (index, width) in widths.iter_mut().enumerate() {
            *width = (*width).max(line[index].chars().count());
        }
    }
    let last_width = match terminal {
        Some(terminal) => fit(&mut widths, &cells, shrink, terminal),
        None => None,
    };
    let mut table = String::new();
    for line in &cells {
        let mut text = String::new();
        for (index, width) in widths.iter().enumerate() {
            let cell = if shrink == Some(index) {
                cut_left(&line[index], *width)
            } else {
                line[index].clone()
            };
            text.push_str(&cell);
            for _ in cell.chars().count()..*width + 2 {
                text.push(' ');
            }
        }
        match last_width {
            Some(width) => text.push_str(&cut_right(&line[last], width)),
            None => text.push_str(&line[last]),
        }
        table.push_str(text.trim_end_matches(' '));
        table.push('\n');
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[&str]) -> Vec<String> {
        let mut row = Vec::new();
        for cell in cells {
            row.push(cell.to_string());
        }
        row
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
    fn aligns_columns_without_trailing_spaces() {
        let table = render(
            &["A", "BB", "LAST"],
            &[row(&["long", "x", "end"]), row(&["s", "yyy", ""])],
            None,
            None,
        );
        assert_eq!(table, "A     BB   LAST\nlong  x    end\ns     yyy\n");
    }

    #[test]
    fn cuts_the_last_column_when_nothing_can_shrink() {
        let table = render(
            &["NAME", "COMMAND"],
            &[row(&["web", "mise exec -- pnpm dev --host"])],
            None,
            Some(16),
        );
        assert_eq!(table.lines().nth(1), Some("web   mise exec…"));
    }

    #[test]
    fn empty_rows_render_nothing() {
        assert_eq!(render(&["A", "B"], &[], None, Some(80)), "");
    }
}
