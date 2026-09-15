//! Terminal model: bytes in, cell grid out. Wraps `vt100` and reduces its cell
//! attributes to what the panel can show. Pure; no I/O.

use panel::Cell;

/// A full snapshot of the visible screen. Cheap to build (a few thousand cells)
/// and cheap to compare, which is what the renderer does every tick.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Grid {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<Cell>,
    /// `(col, row)` of the cursor, or `None` if hidden.
    pub cursor: Option<(u16, u16)>,
}

impl Grid {
    pub fn blank(cols: u16, rows: u16) -> Self {
        Grid { cols, rows, cells: vec![Cell::BLANK; cols as usize * rows as usize], cursor: None }
    }
    pub fn cell(&self, col: u16, row: u16) -> &Cell {
        &self.cells[row as usize * self.cols as usize + col as usize]
    }
    pub fn cell_mut(&mut self, col: u16, row: u16) -> &mut Cell {
        &mut self.cells[row as usize * self.cols as usize + col as usize]
    }
    pub fn row_text(&self, row: u16) -> String {
        let s: String = (0..self.cols).map(|c| self.cell(c, row).ch).filter(|&ch| ch != '\0').collect();
        s.trim_end().to_string()
    }
}

/// Translates the DEC Special Graphics charset (`ESC ( 0`, used by tmux, dialog,
/// ncurses when the terminal is not known to be UTF-8) into Unicode box drawing
/// before the bytes reach the VT parser, which does not track charsets. It
/// tracks escape sequences just enough not to touch bytes inside them.
#[derive(Default)]
struct AcsFilter {
    g0_graphics: bool,
    g1_graphics: bool,
    shifted_out: bool,
    state: AcsState,
}

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum AcsState {
    #[default]
    Ground,
    Esc,
    Csi,
    Osc,
    OscEsc,
    Designate(u8),
}

impl AcsFilter {
    fn active(&self) -> bool {
        if self.shifted_out { self.g1_graphics } else { self.g0_graphics }
    }

    fn map(b: u8) -> Option<&'static str> {
        Some(match b {
            b'j' => "┘",
            b'k' => "┐",
            b'l' => "┌",
            b'm' => "└",
            b'n' => "┼",
            b'q' => "─",
            b't' => "├",
            b'u' => "┤",
            b'v' => "┴",
            b'w' => "┬",
            b'x' => "│",
            b'a' => "▒",
            b'`' => "◆",
            b'f' => "°",
            b'g' => "±",
            b'o' => "⎺",
            b'p' => "⎻",
            b'r' => "⎼",
            b's' => "⎽",
            b'y' => "≤",
            b'z' => "≥",
            b'{' => "π",
            b'|' => "≠",
            b'}' => "£",
            b'~' => "·",
            _ => return None,
        })
    }

    fn feed(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for &b in bytes {
            match self.state {
                AcsState::Ground => match b {
                    0x1b => {
                        self.state = AcsState::Esc;
                        out.push(b);
                    }
                    0x0e => self.shifted_out = true,
                    0x0f => self.shifted_out = false,
                    _ if self.active() && (0x60..=0x7e).contains(&b) => match Self::map(b) {
                        Some(u) => out.extend_from_slice(u.as_bytes()),
                        None => out.push(b),
                    },
                    _ => out.push(b),
                },
                AcsState::Esc => {
                    out.push(b);
                    self.state = match b {
                        b'[' => AcsState::Csi,
                        b']' | b'P' | b'^' | b'_' => AcsState::Osc,
                        b'(' | b')' => AcsState::Designate(b),
                        _ => AcsState::Ground,
                    };
                }
                AcsState::Designate(which) => {
                    // Strip the designation: the parser would not understand it anyway.
                    out.pop(); // the '(' or ')'
                    out.pop(); // the ESC
                    let graphics = b == b'0';
                    if which == b'(' { self.g0_graphics = graphics } else { self.g1_graphics = graphics }
                    self.state = AcsState::Ground;
                }
                AcsState::Csi => {
                    out.push(b);
                    if (0x40..=0x7e).contains(&b) {
                        self.state = AcsState::Ground;
                    }
                }
                AcsState::Osc => {
                    out.push(b);
                    if b == 0x07 {
                        self.state = AcsState::Ground;
                    } else if b == 0x1b {
                        self.state = AcsState::OscEsc;
                    }
                }
                AcsState::OscEsc => {
                    out.push(b);
                    self.state = if b == b'\\' { AcsState::Ground } else { AcsState::Osc };
                }
            }
        }
    }
}

pub struct Terminal {
    parser: vt100::Parser,
    acs: AcsFilter,
    buf: Vec<u8>,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        Terminal { parser: vt100::Parser::new(rows, cols, scrollback), acs: AcsFilter::default(), buf: Vec::new() }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.buf.clear();
        self.acs.feed(bytes, &mut self.buf);
        self.parser.process(&self.buf);
    }

    /// Bytes an application in mouse-reporting mode expects for a wheel step
    /// at cell (`col`, `row`), or None when mouse reporting is off.
    pub fn mouse_wheel_bytes(&self, up: bool, col: u16, row: u16) -> Option<Vec<u8>> {
        use vt100::{MouseProtocolEncoding, MouseProtocolMode};
        let screen = self.parser.screen();
        if screen.mouse_protocol_mode() == MouseProtocolMode::None {
            return None;
        }
        let button = if up { 64 } else { 65 };
        Some(match screen.mouse_protocol_encoding() {
            MouseProtocolEncoding::Sgr => format!("\x1b[<{button};{};{}M", col + 1, row + 1).into_bytes(),
            _ => vec![0x1b, b'[', b'M', 32 + button as u8, (32 + col + 1).min(255) as u8, (32 + row + 1).min(255) as u8],
        })
    }

    /// Move the local scrollback view by `delta` lines (positive = older). 0 = live.
    pub fn scroll_view(&mut self, delta: i32) {
        let cur = self.parser.screen().scrollback() as i32;
        let next = (cur + delta).max(0) as usize;
        self.parser.set_scrollback(next);
    }

    pub fn scroll_offset(&self) -> usize {
        self.parser.screen().scrollback()
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.parser.set_size(rows, cols);
    }

    pub fn cols(&self) -> u16 {
        self.parser.screen().size().1
    }

    pub fn rows(&self) -> u16 {
        self.parser.screen().size().0
    }

    /// Title set by the application via OSC 0/2, if any.
    pub fn title(&self) -> &str {
        self.parser.screen().title()
    }

    pub fn snapshot(&self) -> Grid {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let mut grid = Grid::blank(cols, rows);
        for row in 0..rows {
            for col in 0..cols {
                let Some(vc) = screen.cell(row, col) else { continue };
                let out = grid.cell_mut(col, row);
                if vc.is_wide_continuation() {
                    out.ch = '\0';
                    continue;
                }
                // First scalar only; combining marks are dropped for now.
                out.ch = vc.contents().chars().next().unwrap_or(' ');
                out.bold = vc.bold();
                out.inverse = vc.inverse();
                out.underline = vc.underline();
                out.dim = false; // vt100 does not expose SGR 2 (faint); map colours later
                out.wide = vc.is_wide();
            }
        }
        if !screen.hide_cursor() {
            let (r, c) = screen.cursor_position();
            grid.cursor = Some((c, r));
        }
        grid
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_text_lands_in_cells() {
        let mut t = Terminal::new(20, 4, 0);
        t.feed(b"hello\r\nworld");
        let g = t.snapshot();
        assert_eq!(g.row_text(0), "hello");
        assert_eq!(g.row_text(1), "world");
        assert_eq!(g.cursor, Some((5, 1)));
    }

    #[test]
    fn attributes_survive() {
        let mut t = Terminal::new(20, 2, 0);
        t.feed(b"\x1b[1mB\x1b[0m\x1b[7mI\x1b[0m");
        let g = t.snapshot();
        assert!(g.cell(0, 0).bold);
        assert!(!g.cell(0, 0).inverse);
        assert!(g.cell(1, 0).inverse);
    }

    #[test]
    fn scrolling_shifts_rows() {
        let mut t = Terminal::new(10, 2, 0);
        t.feed(b"a\r\nb\r\nc");
        let g = t.snapshot();
        assert_eq!(g.row_text(0), "b");
        assert_eq!(g.row_text(1), "c");
    }

    #[test]
    fn dec_line_drawing_becomes_box_characters() {
        let mut t = Terminal::new(20, 3, 0);
        t.feed(b"\x1b(0lqqk\x1b(B ok\r\n\x1b(0x\x1b(Bq");
        let g = t.snapshot();
        assert_eq!(g.row_text(0), "┌──┐ ok");
        assert_eq!(g.row_text(1), "│q");
    }

    #[test]
    fn line_drawing_does_not_touch_escape_sequences() {
        let mut t = Terminal::new(20, 3, 0);
        // 'q' as a CSI final byte (DECSCUSR) and inside an OSC title must survive.
        t.feed(b"\x1b(0\x1b[2 q\x1b]0;quux\x07q");
        let g = t.snapshot();
        assert_eq!(g.row_text(0), "─");
        assert_eq!(t.title(), "quux");
    }

    #[test]
    fn wheel_bytes_only_when_mouse_reporting_is_on() {
        let mut t = Terminal::new(20, 3, 0);
        assert_eq!(t.mouse_wheel_bytes(true, 0, 0), None);
        t.feed(b"\x1b[?1000h\x1b[?1006h");
        assert_eq!(t.mouse_wheel_bytes(true, 4, 2), Some(b"\x1b[<64;5;3M".to_vec()));
        assert_eq!(t.mouse_wheel_bytes(false, 4, 2), Some(b"\x1b[<65;5;3M".to_vec()));
    }

    #[test]
    fn local_scrollback_moves_the_view() {
        let mut t = Terminal::new(10, 2, 100);
        t.feed(b"one\r\ntwo\r\nthree");
        assert_eq!(t.snapshot().row_text(0), "two");
        t.scroll_view(1);
        assert_eq!(t.snapshot().row_text(0), "one");
        t.scroll_view(-5);
        assert_eq!(t.scroll_offset(), 0);
        assert_eq!(t.snapshot().row_text(1), "three");
    }

    #[test]
    fn hidden_cursor_is_none() {
        let mut t = Terminal::new(10, 2, 0);
        t.feed(b"\x1b[?25l");
        assert_eq!(t.snapshot().cursor, None);
    }
}
