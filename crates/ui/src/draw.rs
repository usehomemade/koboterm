//! Small drawing helpers on top of `Panel::draw`.

use panel::{Cell, CellRect, Panel};

pub fn put(panel: &mut dyn Panel, col: u16, row: u16, ch: char, bold: bool, inverse: bool) {
    let g = panel.geometry();
    if col < g.cols && row < g.rows {
        panel.draw(col, row, &Cell { ch, bold, inverse, ..Cell::BLANK });
    }
}

pub fn text(panel: &mut dyn Panel, col: u16, row: u16, s: &str, bold: bool, inverse: bool) {
    for (i, ch) in s.chars().enumerate() {
        put(panel, col + i as u16, row, ch, bold, inverse);
    }
}

pub fn fill(panel: &mut dyn Panel, r: CellRect, ch: char, inverse: bool) {
    for row in r.row..r.row + r.rows {
        for col in r.col..r.col + r.cols {
            put(panel, col, row, ch, false, inverse);
        }
    }
}

/// Box-drawing frame around `r` (inclusive), interior left untouched.
pub fn frame(panel: &mut dyn Panel, r: CellRect, inverse: bool) {
    if r.cols < 2 || r.rows < 2 {
        return;
    }
    let (c0, r0, c1, r1) = (r.col, r.row, r.col + r.cols - 1, r.row + r.rows - 1);
    put(panel, c0, r0, '┌', false, inverse);
    put(panel, c1, r0, '┐', false, inverse);
    put(panel, c0, r1, '└', false, inverse);
    put(panel, c1, r1, '┘', false, inverse);
    for c in c0 + 1..c1 {
        put(panel, c, r0, '─', false, inverse);
        put(panel, c, r1, '─', false, inverse);
    }
    for r in r0 + 1..r1 {
        put(panel, c0, r, '│', false, inverse);
        put(panel, c1, r, '│', false, inverse);
    }
}

/// Framed box with a centred label, the whole thing optionally inverted.
pub fn button(panel: &mut dyn Panel, r: CellRect, label: &str, inverse: bool) {
    fill(panel, r, ' ', inverse);
    frame(panel, r, inverse);
    let inner = r.cols.saturating_sub(2) as usize;
    let label: String = label.chars().take(inner).collect();
    let pad = (inner - label.chars().count()) / 2;
    text(panel, r.col + 1 + pad as u16, r.row + r.rows / 2, &label, false, inverse);
}
