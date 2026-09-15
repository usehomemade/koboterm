//! Panel: the thing the renderer draws into. On the device this is the e-ink
//! framebuffer (via FBInk); on the host it is `FakePanel`, which records what
//! would have been refreshed so tests can count refreshes.
//!
//! The panel works in *cells*, not pixels. Converting a cell rect to a pixel
//! rect (font size, margins, rotation) is the panel's business, so the renderer
//! and its tests never need a font.

/// E-ink waveform class. Names describe intent, not vendor waveform IDs; the
/// device panel maps them (e.g. Fast -> DU/A2, Partial -> GL16, Full -> GC16).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Waveform {
    /// Fast, no flash, 1-bit. Leaves ghosting. Use for text updates.
    Fast,
    /// Slower greyscale partial update, mild flash on some panels.
    Partial,
    /// Full refresh with flash. Clears ghosting.
    Full,
}

/// Rectangle in cell coordinates. `cols`/`rows` are sizes, never zero when used.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub struct CellRect {
    pub col: u16,
    pub row: u16,
    pub cols: u16,
    pub rows: u16,
}

impl CellRect {
    pub fn area(&self) -> u32 {
        self.cols as u32 * self.rows as u32
    }
    pub fn contains(&self, col: u16, row: u16) -> bool {
        col >= self.col && col < self.col + self.cols && row >= self.row && row < self.row + self.rows
    }
}

/// One character cell. Colours are already reduced to what e-ink can show.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub struct Cell {
    /// `'\0'` marks the trailing half of a wide character.
    pub ch: char,
    pub bold: bool,
    pub inverse: bool,
    pub underline: bool,
    /// Rendered as grey text on capable panels; otherwise ignored.
    pub dim: bool,
    pub wide: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', bold: false, inverse: false, underline: false, dim: false, wide: false }
    }
}

impl Cell {
    pub const BLANK: Cell = Cell { ch: ' ', bold: false, inverse: false, underline: false, dim: false, wide: false };
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Geometry {
    pub cols: u16,
    pub rows: u16,
}

/// Drawing surface. `draw` only updates the panel's backing store; nothing is
/// visible until `refresh` pushes a rect to the glass. Every `refresh` costs
/// time and, on e-ink, image quality, so the renderer treats calls as scarce.
pub trait Panel {
    fn geometry(&self) -> Geometry;
    fn draw(&mut self, col: u16, row: u16, cell: &Cell);
    fn refresh(&mut self, rect: CellRect, wf: Waveform);
}

/// A panel view shifted down by `row0` rows and limited to `rows` rows, so a
/// renderer can own a band of the screen (e.g. below a top bar).
pub struct Offset<'a> {
    inner: &'a mut dyn Panel,
    row0: u16,
    rows: u16,
}

impl<'a> Offset<'a> {
    pub fn new(inner: &'a mut dyn Panel, row0: u16, rows: u16) -> Self {
        Offset { inner, row0, rows }
    }
}

impl Panel for Offset<'_> {
    fn geometry(&self) -> Geometry {
        Geometry { cols: self.inner.geometry().cols, rows: self.rows }
    }
    fn draw(&mut self, col: u16, row: u16, cell: &Cell) {
        if row < self.rows {
            self.inner.draw(col, row + self.row0, cell);
        }
    }
    fn refresh(&mut self, rect: CellRect, wf: Waveform) {
        let rows = rect.rows.min(self.rows.saturating_sub(rect.row));
        if rows > 0 {
            self.inner.refresh(CellRect { row: rect.row + self.row0, rows, ..rect }, wf);
        }
    }
}

/// Host-side panel. Keeps the cell grid and a log of every refresh.
#[derive(Clone, Debug)]
pub struct FakePanel {
    geo: Geometry,
    cells: Vec<Cell>,
    pub refreshes: Vec<(CellRect, Waveform)>,
}

impl FakePanel {
    pub fn new(cols: u16, rows: u16) -> Self {
        FakePanel {
            geo: Geometry { cols, rows },
            cells: vec![Cell::BLANK; cols as usize * rows as usize],
            refreshes: Vec::new(),
        }
    }

    pub fn cell(&self, col: u16, row: u16) -> &Cell {
        &self.cells[row as usize * self.geo.cols as usize + col as usize]
    }

    /// Text of one row, trailing blanks trimmed. Wide-char trailing halves are skipped.
    pub fn row_text(&self, row: u16) -> String {
        let s: String = (0..self.geo.cols)
            .map(|c| self.cell(c, row).ch)
            .filter(|&ch| ch != '\0')
            .collect();
        s.trim_end().to_string()
    }

    pub fn count(&self, wf: Waveform) -> usize {
        self.refreshes.iter().filter(|(_, w)| *w == wf).count()
    }

    pub fn total_refreshes(&self) -> usize {
        self.refreshes.len()
    }
}

impl Panel for FakePanel {
    fn geometry(&self) -> Geometry {
        self.geo
    }
    fn draw(&mut self, col: u16, row: u16, cell: &Cell) {
        let i = row as usize * self.geo.cols as usize + col as usize;
        self.cells[i] = *cell;
    }
    fn refresh(&mut self, rect: CellRect, wf: Waveform) {
        assert!(rect.cols > 0 && rect.rows > 0, "empty refresh rect");
        assert!(rect.col + rect.cols <= self.geo.cols && rect.row + rect.rows <= self.geo.rows, "refresh rect out of bounds: {rect:?}");
        self.refreshes.push((rect, wf));
    }
}
