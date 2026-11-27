//! A minimal VT screen model, just enough to read what a CLI shows (#466).
//!
//! The raw PTY stream cannot be text-matched. ConPTY diff-renders its viewport: when a
//! cell already holds the right character it moves the cursor past it (`ESC[1C`) instead
//! of writing it, so a captured codex trust prompt arrives as `repo-c\x1b[1Cdex-wt2`.
//! Recognising a prompt therefore needs the rendered grid, not the byte stream.
//!
//! This handles cursor addressing, erasure, scrolling regions, line insert/delete and the
//! alternate screen. Colours and attributes are parsed and dropped. Anything else is
//! consumed and ignored, so an unknown sequence never leaks into the text.

const MAX_COLS: usize = 1000;
const MAX_ROWS: usize = 500;
const MAX_PARAM_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Parse {
    Ground,
    Escape,
    /// `ESC` followed by an intermediate byte (`ESC ( B`): the next byte ends it.
    EscapeIntermediate,
    Csi,
    /// OSC, DCS, APC, PM and SOS strings: skipped up to BEL or `ESC \`.
    String,
    StringEscape,
}

#[derive(Debug, Clone)]
pub struct Screen {
    cols: usize,
    rows: usize,
    grid: Vec<Vec<char>>,
    row: usize,
    col: usize,
    /// Set after a character lands in the last column; the next one wraps.
    wrap_pending: bool,
    top: usize,
    bottom: usize,
    saved: (usize, usize),
    state: Parse,
    params: Vec<u8>,
    utf8: Vec<u8>,
}

impl Screen {
    /// The grid size a screen for a `cols` x `rows` terminal actually has.
    pub fn clamped(cols: u16, rows: u16) -> (u16, u16) {
        ((cols as usize).clamp(1, MAX_COLS) as u16, (rows as usize).clamp(1, MAX_ROWS) as u16)
    }

    pub fn new(cols: u16, rows: u16) -> Self {
        let cols = (cols as usize).clamp(1, MAX_COLS);
        let rows = (rows as usize).clamp(1, MAX_ROWS);
        Self {
            cols,
            rows,
            grid: vec![vec![' '; cols]; rows],
            row: 0,
            col: 0,
            wrap_pending: false,
            top: 0,
            bottom: rows - 1,
            saved: (0, 0),
            state: Parse::Ground,
            params: Vec::new(),
            utf8: Vec::new(),
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols as u16, self.rows as u16)
    }

    /// The visible rows, right-trimmed.
    pub fn lines(&self) -> Vec<String> {
        self.grid
            .iter()
            .map(|row| row.iter().collect::<String>().trim_end().to_string())
            .collect()
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.byte(byte);
        }
    }

    fn byte(&mut self, byte: u8) {
        match self.state {
            Parse::Ground => self.ground(byte),
            Parse::Escape => self.escape(byte),
            Parse::EscapeIntermediate => self.state = Parse::Ground,
            Parse::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    self.state = Parse::Ground;
                    let params = std::mem::take(&mut self.params);
                    self.csi(&params, byte);
                } else if byte == 0x1b {
                    self.params.clear();
                    self.state = Parse::Escape;
                } else if self.params.len() < MAX_PARAM_BYTES {
                    self.params.push(byte);
                }
            }
            Parse::String => match byte {
                0x07 => self.state = Parse::Ground,
                0x1b => self.state = Parse::StringEscape,
                _ => {}
            },
            Parse::StringEscape => {
                self.state = if byte == b'\\' { Parse::Ground } else { Parse::String };
            }
        }
    }

    fn ground(&mut self, byte: u8) {
        if !self.utf8.is_empty() || byte >= 0x80 {
            self.utf8_byte(byte);
            return;
        }
        match byte {
            0x1b => self.state = Parse::Escape,
            b'\r' => {
                self.col = 0;
                self.wrap_pending = false;
            }
            b'\n' | 0x0b | 0x0c => self.line_feed(),
            0x08 => {
                self.col = self.col.saturating_sub(1);
                self.wrap_pending = false;
            }
            b'\t' => {
                self.col = ((self.col / 8) + 1) * 8;
                self.col = self.col.min(self.cols - 1);
                self.wrap_pending = false;
            }
            0x20..=0x7e => self.print(byte as char),
            _ => {}
        }
    }

    fn utf8_byte(&mut self, byte: u8) {
        // A stray continuation byte or a lead byte cutting a sequence short is dropped.
        if byte < 0x80 {
            self.utf8.clear();
            self.ground(byte);
            return;
        }
        if byte & 0xc0 != 0x80 {
            self.utf8.clear();
        } else if self.utf8.is_empty() {
            return;
        }
        self.utf8.push(byte);
        let expected = match self.utf8[0] {
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            _ => {
                self.utf8.clear();
                return;
            }
        };
        if self.utf8.len() < expected {
            return;
        }
        let decoded = std::str::from_utf8(&self.utf8).ok().and_then(|s| s.chars().next());
        self.utf8.clear();
        if let Some(ch) = decoded {
            self.print(ch);
        }
    }

    fn print(&mut self, ch: char) {
        if self.wrap_pending {
            self.col = 0;
            self.line_feed();
        }
        self.grid[self.row][self.col] = ch;
        let width = if is_wide(ch) { 2 } else { 1 };
        if self.col + width >= self.cols {
            self.col = self.cols - 1;
            self.wrap_pending = true;
        } else {
            if width == 2 {
                self.grid[self.row][self.col + 1] = ' ';
            }
            self.col += width;
        }
    }

    fn line_feed(&mut self) {
        self.wrap_pending = false;
        if self.row == self.bottom {
            self.scroll_up(1);
        } else if self.row + 1 < self.rows {
            self.row += 1;
        }
    }

    fn reverse_index(&mut self) {
        self.wrap_pending = false;
        if self.row == self.top {
            self.scroll_down(1);
        } else {
            self.row = self.row.saturating_sub(1);
        }
    }

    fn scroll_up(&mut self, n: usize) {
        for _ in 0..n.min(self.bottom - self.top + 1) {
            self.grid.remove(self.top);
            self.grid.insert(self.bottom, vec![' '; self.cols]);
        }
    }

    fn scroll_down(&mut self, n: usize) {
        for _ in 0..n.min(self.bottom - self.top + 1) {
            self.grid.remove(self.bottom);
            self.grid.insert(self.top, vec![' '; self.cols]);
        }
    }

    fn escape(&mut self, byte: u8) {
        self.state = Parse::Ground;
        match byte {
            b'[' => {
                self.params.clear();
                self.state = Parse::Csi;
            }
            b']' | b'P' | b'_' | b'^' | b'X' => self.state = Parse::String,
            b'(' | b')' | b'*' | b'+' | b'#' | b'%' | b' ' => self.state = Parse::EscapeIntermediate,
            b'7' => self.saved = (self.row, self.col),
            b'8' => {
                (self.row, self.col) = self.saved;
                self.wrap_pending = false;
            }
            b'D' => self.line_feed(),
            b'E' => {
                self.col = 0;
                self.line_feed();
            }
            b'M' => self.reverse_index(),
            b'c' => *self = Self::new(self.cols as u16, self.rows as u16),
            _ => {}
        }
    }

    fn csi(&mut self, raw: &[u8], final_byte: u8) {
        let private = raw.first().is_some_and(|b| matches!(b, b'?' | b'>' | b'<' | b'='));
        let body = if private { &raw[1..] } else { raw };
        // Intermediate bytes (` `, `$`, ...) mark sequences this model does not draw.
        if body.iter().any(|b| (0x20..=0x2f).contains(b)) {
            return;
        }
        let params: Vec<usize> = std::str::from_utf8(body)
            .unwrap_or("")
            .split([';', ':'])
            .map(|p| p.parse().unwrap_or(0))
            .collect();
        let arg = |i: usize, default: usize| params.get(i).copied().filter(|v| *v != 0).unwrap_or(default);

        if private {
            self.private_mode(&params, final_byte);
            return;
        }

        self.wrap_pending = false;
        let last_row = self.rows - 1;
        let last_col = self.cols - 1;
        match final_byte {
            b'H' | b'f' => {
                self.row = (arg(0, 1) - 1).min(last_row);
                self.col = (arg(1, 1) - 1).min(last_col);
            }
            b'A' => self.row = self.row.saturating_sub(arg(0, 1)),
            b'B' | b'e' => self.row = self.row.saturating_add(arg(0, 1)).min(last_row),
            b'C' | b'a' => self.col = self.col.saturating_add(arg(0, 1)).min(last_col),
            b'D' => self.col = self.col.saturating_sub(arg(0, 1)),
            b'E' => {
                self.row = self.row.saturating_add(arg(0, 1)).min(last_row);
                self.col = 0;
            }
            b'F' => {
                self.row = self.row.saturating_sub(arg(0, 1));
                self.col = 0;
            }
            b'G' | b'`' => self.col = (arg(0, 1) - 1).min(last_col),
            b'd' => self.row = (arg(0, 1) - 1).min(last_row),
            b'J' => self.erase_display(params.first().copied().unwrap_or(0)),
            b'K' => self.erase_line(params.first().copied().unwrap_or(0)),
            b'X' => self.erase_in_row(self.row, self.col, self.col.saturating_add(arg(0, 1))),
            b'P' => {
                let n = arg(0, 1).min(self.cols - self.col);
                let line = &mut self.grid[self.row];
                line.drain(self.col..self.col + n);
                line.extend(std::iter::repeat_n(' ', n));
            }
            b'@' => {
                let n = arg(0, 1).min(self.cols - self.col);
                let line = &mut self.grid[self.row];
                for _ in 0..n {
                    line.insert(self.col, ' ');
                }
                line.truncate(self.cols);
            }
            b'L' => self.insert_lines(arg(0, 1)),
            b'M' => self.delete_lines(arg(0, 1)),
            b'S' => self.scroll_up(arg(0, 1)),
            b'T' => self.scroll_down(arg(0, 1)),
            b'r' => {
                let top = arg(0, 1) - 1;
                let bottom = arg(1, self.rows) - 1;
                if top < bottom && bottom < self.rows {
                    self.top = top;
                    self.bottom = bottom;
                } else {
                    self.top = 0;
                    self.bottom = last_row;
                }
                self.row = 0;
                self.col = 0;
            }
            b's' => self.saved = (self.row, self.col),
            b'u' => (self.row, self.col) = self.saved,
            _ => {}
        }
    }

    /// `DECSET`/`DECRST`: only the alternate screen changes what is shown.
    fn private_mode(&mut self, params: &[usize], final_byte: u8) {
        if matches!(final_byte, b'h' | b'l') && params.iter().any(|p| matches!(p, 47 | 1047 | 1049)) {
            self.clear_all();
            if final_byte == b'h' {
                self.row = 0;
                self.col = 0;
            }
        }
    }

    fn erase_display(&mut self, mode: usize) {
        match mode {
            0 => {
                self.erase_in_row(self.row, self.col, self.cols);
                for row in self.row + 1..self.rows {
                    self.erase_in_row(row, 0, self.cols);
                }
            }
            1 => {
                for row in 0..self.row {
                    self.erase_in_row(row, 0, self.cols);
                }
                self.erase_in_row(self.row, 0, self.col + 1);
            }
            2 => self.clear_all(),
            // 3 erases saved scrollback only; the visible grid stays as it is.
            _ => {}
        }
    }

    fn erase_line(&mut self, mode: usize) {
        match mode {
            0 => self.erase_in_row(self.row, self.col, self.cols),
            1 => self.erase_in_row(self.row, 0, self.col + 1),
            _ => self.erase_in_row(self.row, 0, self.cols),
        }
    }

    fn insert_lines(&mut self, n: usize) {
        if (self.top..=self.bottom).contains(&self.row) {
            for _ in 0..n.min(self.bottom - self.row + 1) {
                self.grid.remove(self.bottom);
                self.grid.insert(self.row, vec![' '; self.cols]);
            }
        }
    }

    fn delete_lines(&mut self, n: usize) {
        if (self.top..=self.bottom).contains(&self.row) {
            for _ in 0..n.min(self.bottom - self.row + 1) {
                self.grid.remove(self.row);
                self.grid.insert(self.bottom, vec![' '; self.cols]);
            }
        }
    }

    fn erase_in_row(&mut self, row: usize, from: usize, to: usize) {
        let to = to.min(self.cols);
        if from < to {
            self.grid[row][from..to].fill(' ');
        }
    }

    fn clear_all(&mut self) {
        for row in &mut self.grid {
            row.fill(' ');
        }
    }
}

/// Characters a terminal draws two cells wide. Only the ranges these CLIs actually
/// print need to be right; a miss shifts the rest of one line, not the next one.
fn is_wide(ch: char) -> bool {
    matches!(ch as u32,
        0x1100..=0x115f
        | 0x2728
        | 0x2e80..=0xa4cf
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe30..=0xfe4f
        | 0xff00..=0xff60
        | 0xffe0..=0xffe6
        | 0x1f300..=0x1f64f
        | 0x1f900..=0x1faff
        | 0x20000..=0x3fffd)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(cols: u16, rows: u16, bytes: &[u8]) -> Vec<String> {
        let mut screen = Screen::new(cols, rows);
        screen.feed(bytes);
        screen.lines()
    }

    #[test]
    fn cursor_forward_keeps_the_cell_already_on_screen() {
        // The ConPTY diff-render shape from the codex capture.
        let lines = render(40, 3, b"repo-codex\x1b[1;1Hrepo-c\x1b[1Cdex-wt2");
        assert_eq!(lines[0], "repo-codex-wt2");
    }

    #[test]
    fn addressing_erasure_and_colours() {
        let lines = render(20, 4, b"\x1b[2;3H\x1b[38;5;32mhi\x1b[m there\x1b[1;1Hxx\x1b[K\x1b[2;6H\x1b[K");
        assert_eq!(lines[0], "xx");
        assert_eq!(lines[1], "  hi");
        let lines = render(10, 2, b"abcdef\x1b[1;3H\x1b[2X");
        assert_eq!(lines[0], "ab  ef");
    }

    #[test]
    fn strings_and_titles_never_reach_the_grid() {
        let lines = render(30, 2, b"\x1b]0;C:\\title\x07\x1b]7501;?\x07\x1bP1$r0m\x1b\\ok\x1b[>4;2m\x1b[?u");
        assert_eq!(lines[0], "ok");
    }

    #[test]
    fn line_feed_at_the_bottom_scrolls_and_wraps_at_the_edge() {
        let lines = render(4, 2, b"one\r\ntwo\r\nsix");
        assert_eq!(lines, ["two", "six"]);
        let lines = render(4, 3, b"abcdef");
        assert_eq!(lines[0], "abcd");
        assert_eq!(lines[1], "ef");
    }

    #[test]
    fn alternate_screen_and_full_clear_reset_the_view() {
        let lines = render(10, 2, b"old\x1b[?1049hnew");
        assert_eq!(lines[0], "new");
        let lines = render(10, 2, b"old\x1b[2J\x1b[Hnew");
        assert_eq!(lines[0], "new");
    }

    #[test]
    fn huge_parameters_saturate_instead_of_overflowing() {
        let lines = render(10, 3, b"\x1b[18446744073709551615Bx\x1b[18446744073709551615C\x1b[18446744073709551615X\x1b[18446744073709551615E");
        assert_eq!(lines[2].trim(), "x");
    }

    #[test]
    fn scrollback_erase_keeps_the_visible_rows() {
        let lines = render(10, 2, b"kept\x1b[3J");
        assert_eq!(lines[0], "kept");
    }

    #[test]
    fn utf8_split_across_feeds_is_reassembled() {
        let mut screen = Screen::new(10, 1);
        let arrow = "› 1.".as_bytes();
        screen.feed(&arrow[..1]);
        screen.feed(&arrow[1..]);
        assert_eq!(screen.lines()[0], "› 1.");
    }
}
