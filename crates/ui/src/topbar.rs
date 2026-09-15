//! Reader-style top bar: status line (clock, machine, battery) and a row of
//! touch buttons. Shown while the keyboard is hidden.

use crate::draw;
use panel::{CellRect, Panel, Waveform};

pub const TOPBAR_ROWS: u16 = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopAction {
    Home,
    BrightnessDown,
    BrightnessUp,
    TextSize,
    Keyboard,
    Nothing,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub clock: String,
    pub name: String,
    pub battery_pct: Option<u8>,
    pub charging: bool,
    pub brightness_pct: Option<u8>,
}

pub struct TopBar {
    cols: u16,
    buttons: Vec<(TopAction, CellRect, &'static str)>,
}

impl TopBar {
    pub fn new(cols: u16) -> Self {
        // Right-aligned buttons, 6 cells wide, 2 rows tall, one gap column.
        let specs: [(TopAction, &'static str); 4] = [
            (TopAction::BrightnessDown, "☼-"),
            (TopAction::BrightnessUp, "☼+"),
            (TopAction::TextSize, "Aa"),
            (TopAction::Keyboard, "⌨"),
        ];
        let w = 6u16;
        let mut buttons = Vec::new();
        let mut col = cols.saturating_sub(specs.len() as u16 * (w + 1));
        for (a, l) in specs {
            buttons.push((a, CellRect { col, row: 1, cols: w, rows: 2 }, l));
            col += w + 1;
        }
        let home_w = 8u16.min(cols / 3);
        buttons.push((TopAction::Home, CellRect { col: 0, row: 1, cols: home_w, rows: 2 }, "← Home"));
        TopBar { cols, buttons }
    }

    pub fn area(&self) -> CellRect {
        CellRect { col: 0, row: 0, cols: self.cols, rows: TOPBAR_ROWS }
    }

    fn status_line(&self, st: &Status) -> (String, String) {
        let left = if st.name.is_empty() { st.clock.clone() } else { format!("{}  {}", st.clock, st.name) };
        let mut right = String::new();
        if let Some(b) = st.brightness_pct {
            right.push_str(&format!("☼{b}%  "));
        }
        match st.battery_pct {
            Some(p) => {
                let bars = ((p as u16 + 12) / 25).min(4) as usize;
                right.push_str(&"▰".repeat(bars));
                right.push_str(&"▱".repeat(4 - bars));
                right.push_str(&format!(" {p}%"));
                if st.charging {
                    right.push('⚡');
                }
            }
            None => right.push_str("battery ?"),
        }
        (left, right)
    }

    pub fn draw(&self, panel: &mut dyn Panel, st: &Status) {
        draw::fill(panel, self.area(), ' ', false);
        self.draw_status(panel, st);
        for (_, r, label) in &self.buttons {
            let inner = CellRect { cols: r.cols.saturating_sub(1).max(1), ..*r };
            draw::fill(panel, inner, ' ', true);
            let pad = inner.cols.saturating_sub(label.chars().count() as u16) / 2;
            draw::text(panel, inner.col + pad, inner.row + inner.rows / 2, label, false, true);
        }
        panel.refresh(self.area(), Waveform::Partial);
    }

    fn draw_status(&self, panel: &mut dyn Panel, st: &Status) {
        let (left, right) = self.status_line(st);
        draw::fill(panel, CellRect { col: 0, row: 0, cols: self.cols, rows: 1 }, ' ', false);
        draw::text(panel, 1, 0, &left, false, false);
        let rc = self.cols.saturating_sub(right.chars().count() as u16 + 1);
        draw::text(panel, rc, 0, &right, false, false);
    }

    /// Redraw only the status line (clock/battery ticks).
    pub fn update_status(&self, panel: &mut dyn Panel, st: &Status) {
        self.draw_status(panel, st);
        panel.refresh(CellRect { col: 0, row: 0, cols: self.cols, rows: 1 }, Waveform::Fast);
    }

    pub fn hit(&self, col: u16, row: u16) -> TopAction {
        self.buttons.iter().find(|(_, r, _)| r.contains(col, row)).map(|(a, _, _)| *a).unwrap_or(TopAction::Nothing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel::FakePanel;

    #[test]
    fn draws_status_and_buttons_and_hit_tests() {
        for cols in [87u16, 65, 43] {
            let mut fp = FakePanel::new(cols, 20);
            let tb = TopBar::new(cols);
            let st = Status { clock: "12:36".into(), name: "MBP".into(), battery_pct: Some(21), charging: false, brightness_pct: Some(52) };
            tb.draw(&mut fp, &st);
            assert!(fp.row_text(0).starts_with(" 12:36  MBP"), "{cols}: {:?}", fp.row_text(0));
            assert!(fp.row_text(0).contains("▰▱▱▱ 21%"), "{cols}: {:?}", fp.row_text(0));
            assert!(fp.row_text(2).contains("Home"));
            assert_eq!(tb.hit(2, 2), TopAction::Home);
            assert_eq!(tb.hit(cols - 3, 2), TopAction::Keyboard);
            assert_eq!(tb.hit(cols - 10, 1), TopAction::TextSize);
            assert_eq!(tb.hit(20, 0), TopAction::Nothing);
        }
    }
}
