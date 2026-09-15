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

pub struct Terminal {
    parser: vt100::Parser,
}

impl Terminal {
    pub fn new(cols: u16, rows: u16, scrollback: usize) -> Self {
        Terminal { parser: vt100::Parser::new(rows, cols, scrollback) }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
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
    fn hidden_cursor_is_none() {
        let mut t = Terminal::new(10, 2, 0);
        t.feed(b"\x1b[?25l");
        assert_eq!(t.snapshot().cursor, None);
    }
}
