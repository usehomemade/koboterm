//! Reader-style popups: a framed panel with a title, sliders and segmented
//! choices, like Nickel's brightness and text menus. Sliders are draggable.

use crate::draw;
use panel::{CellRect, Panel, Waveform};

pub struct Slider {
    pub label: String,
    pub min: i32,
    pub max: i32,
    pub value: i32,
    pub step: i32,
    pub left_icon: &'static str,
    pub right_icon: &'static str,
    /// How the value is shown next to the label, e.g. "52%".
    pub fmt: fn(i32) -> String,
    row: u16,
    track: CellRect,
    minus: CellRect,
    plus: CellRect,
}

impl Slider {
    fn value_at(&self, col: u16) -> i32 {
        let t = self.track;
        let span = (t.cols as i32 - 1).max(1);
        let pos = (col as i32 - t.col as i32).clamp(0, span);
        let v = self.min + ((pos * (self.max - self.min) + span / 2) / span);
        ((v - self.min) / self.step * self.step + self.min).clamp(self.min, self.max)
    }

    fn knob_col(&self) -> u16 {
        let t = self.track;
        let span = (t.cols as i32 - 1).max(1);
        let range = (self.max - self.min).max(1);
        t.col + ((self.value - self.min) * span / range) as u16
    }
}

pub struct Choice {
    pub label: String,
    pub options: Vec<String>,
    pub selected: usize,
    row: u16,
    rects: Vec<CellRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupHit {
    /// Tap on the close button or outside the panel.
    Close,
    /// Slider `i` is now `value` (tap on the track or on -/+).
    Slider(usize, i32),
    /// Choice `c` selected option `o`.
    Choice(usize, usize),
    Nothing,
}

pub struct Popup {
    pub area: CellRect,
    title: String,
    pub sliders: Vec<Slider>,
    pub choices: Vec<Choice>,
    close: CellRect,
}

impl Popup {
    /// Panel spanning `cols` columns from `top_row`. Contents are laid out
    /// with `add_slider`/`add_choice`; call `finish` before drawing.
    pub fn new(cols: u16, top_row: u16, title: &str) -> Self {
        let area = CellRect { col: 1, row: top_row, cols: cols.saturating_sub(2), rows: 3 };
        Popup { area, title: title.to_string(), sliders: Vec::new(), choices: Vec::new(), close: CellRect { col: area.col + area.cols - 4, row: top_row, cols: 3, rows: 1 } }
    }

    fn next_row(&self) -> u16 {
        self.area.row + self.area.rows - 1
    }

    pub fn add_slider(&mut self, label: &str, min: i32, max: i32, value: i32, step: i32, left_icon: &'static str, right_icon: &'static str, fmt: fn(i32) -> String) -> &mut Self {
        let row = self.next_row();
        let a = self.area;
        let minus = CellRect { col: a.col + 2, row: row + 1, cols: 3, rows: 1 };
        let plus = CellRect { col: a.col + a.cols - 5, row: row + 1, cols: 3, rows: 1 };
        let track = CellRect { col: minus.col + 4, row: row + 1, cols: plus.col.saturating_sub(minus.col + 5), rows: 1 };
        self.sliders.push(Slider { label: label.into(), min, max, value: value.clamp(min, max), step: step.max(1), left_icon, right_icon, fmt, row, track, minus, plus });
        self.area.rows += 3;
        self
    }

    pub fn add_choice(&mut self, label: &str, options: &[&str], selected: usize) -> &mut Self {
        let row = self.next_row();
        let a = self.area;
        let n = options.len().max(1) as u16;
        let avail = a.cols.saturating_sub(4);
        let w = (avail / n).max(4);
        let rects = (0..n).map(|i| CellRect { col: a.col + 2 + i * w, row: row + 1, cols: w - 1, rows: 1 }).collect();
        self.choices.push(Choice { label: label.into(), options: options.iter().map(|s| s.to_string()).collect(), selected, row, rects });
        self.area.rows += 3;
        self
    }

    pub fn finish(&mut self) {
        self.area.rows += 1;
    }

    pub fn draw(&self, panel: &mut dyn Panel) {
        let a = self.area;
        draw::fill(panel, a, ' ', false);
        draw::frame(panel, a, false);
        draw::text(panel, a.col + 2, a.row + 1, &self.title, true, false);
        draw::text(panel, self.close.col, self.close.row + 1, " ✕ ", false, true);
        for i in 0..self.sliders.len() {
            self.draw_slider(panel, i);
        }
        for c in &self.choices {
            draw::text(panel, a.col + 2, c.row, &c.label, false, false);
            for (i, (opt, r)) in c.options.iter().zip(&c.rects).enumerate() {
                draw::fill(panel, *r, ' ', i == c.selected);
                let pad = r.cols.saturating_sub(opt.chars().count() as u16) / 2;
                draw::text(panel, r.col + pad, r.row, opt, false, i == c.selected);
            }
        }
        panel.refresh(a, Waveform::Partial);
    }

    /// Redraw one slider's rows (label with value, and the track).
    pub fn draw_slider(&self, panel: &mut dyn Panel, i: usize) {
        let s = &self.sliders[i];
        let a = self.area;
        let inner = CellRect { col: a.col + 1, row: s.row, cols: a.cols - 2, rows: 2 };
        draw::fill(panel, inner, ' ', false);
        draw::text(panel, a.col + 2, s.row, &s.label, false, false);
        let v = (s.fmt)(s.value);
        draw::text(panel, a.col + a.cols - 2 - v.chars().count() as u16, s.row, &v, true, false);
        draw::text(panel, s.minus.col, s.minus.row, &format!("{} ", s.left_icon), false, false);
        draw::text(panel, s.plus.col, s.plus.row, &format!(" {}", s.right_icon), false, false);
        let knob = s.knob_col();
        for c in s.track.col..s.track.col + s.track.cols {
            let ch = if c == knob { '●' } else if c < knob { '━' } else { '─' };
            draw::put(panel, c, s.track.row, ch, false, false);
        }
    }

    pub fn refresh_slider(&self, panel: &mut dyn Panel, i: usize) {
        let s = &self.sliders[i];
        panel.refresh(CellRect { col: self.area.col, row: s.row, cols: self.area.cols, rows: 2 }, Waveform::Fast);
    }

    pub fn refresh_choice(&self, panel: &mut dyn Panel, c: usize) {
        let ch = &self.choices[c];
        panel.refresh(CellRect { col: self.area.col, row: ch.row + 1, cols: self.area.cols, rows: 1 }, Waveform::Fast);
    }

    /// Slider index under a finger, for dragging (track row, any column).
    pub fn slider_at(&self, col: u16, row: u16) -> Option<usize> {
        self.sliders.iter().position(|s| row == s.track.row && col + 2 >= s.track.col && col <= s.track.col + s.track.cols + 1)
    }

    /// Value slider `i` takes when the finger is at `col`.
    pub fn slider_value_at(&self, i: usize, col: u16) -> i32 {
        self.sliders[i].value_at(col)
    }

    pub fn hit(&self, col: u16, row: u16) -> PopupHit {
        if self.close.contains(col, row + 0) || (row == self.close.row + 1 && col >= self.close.col && col < self.close.col + 3) {
            return PopupHit::Close;
        }
        if !self.area.contains(col, row) {
            return PopupHit::Close;
        }
        for (i, s) in self.sliders.iter().enumerate() {
            if s.minus.contains(col, row) {
                return PopupHit::Slider(i, (s.value - s.step).max(s.min));
            }
            if s.plus.contains(col, row) {
                return PopupHit::Slider(i, (s.value + s.step).min(s.max));
            }
            if row == s.track.row && col >= s.track.col && col < s.track.col + s.track.cols {
                return PopupHit::Slider(i, s.value_at(col));
            }
        }
        for (ci, c) in self.choices.iter().enumerate() {
            for (oi, r) in c.rects.iter().enumerate() {
                if r.contains(col, row) {
                    return PopupHit::Choice(ci, oi);
                }
            }
        }
        PopupHit::Nothing
    }
}

pub fn pct(v: i32) -> String {
    format!("{v}%")
}
pub fn plain(v: i32) -> String {
    v.to_string()
}
pub fn px(v: i32) -> String {
    format!("{v} px")
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel::FakePanel;

    fn brightness() -> Popup {
        let mut p = Popup::new(65, 3, "Brightness");
        p.add_slider("Brightness", 0, 100, 52, 1, "☼", "☼", pct);
        p.add_slider("Natural light", 0, 10, 0, 1, "☼", "☾", plain);
        p.finish();
        p
    }

    #[test]
    fn draws_title_sliders_and_values() {
        let mut fp = FakePanel::new(65, 44);
        let p = brightness();
        p.draw(&mut fp);
        assert!(fp.row_text(4).contains("Brightness"));
        let rows: Vec<String> = (3..3 + p.area.rows).map(|r| fp.row_text(r)).collect();
        assert!(rows.iter().any(|r| r.contains("52%")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("━") && r.contains("●") && r.contains("─")), "{rows:#?}");
        assert!(rows.iter().any(|r| r.contains("✕")));
        assert_eq!(fp.total_refreshes(), 1);
    }

    #[test]
    fn taps_and_drags_change_values() {
        let p = brightness();
        let s = &p.sliders[0];
        assert_eq!(p.hit(s.minus.col, s.minus.row), PopupHit::Slider(0, 51));
        assert_eq!(p.hit(s.plus.col, s.plus.row), PopupHit::Slider(0, 53));
        assert_eq!(p.hit(s.track.col, s.track.row), PopupHit::Slider(0, 0));
        assert_eq!(p.hit(s.track.col + s.track.cols - 1, s.track.row), PopupHit::Slider(0, 100));
        assert_eq!(p.slider_at(s.track.col + 5, s.track.row), Some(0));
        let mid = p.slider_value_at(0, s.track.col + s.track.cols / 2);
        assert!((45..=55).contains(&mid), "{mid}");
        assert_eq!(p.hit(0, 0), PopupHit::Close);
        assert_eq!(p.hit(p.close.col + 1, p.close.row + 1), PopupHit::Close);
    }

    #[test]
    fn choices_select() {
        let mut p = Popup::new(65, 3, "Text");
        p.add_choice("Font", &["Spleen", "Terminus"], 0);
        p.add_slider("Size", 0, 4, 2, 1, "A", "A", plain);
        p.finish();
        let r = p.choices[0].rects[1];
        assert_eq!(p.hit(r.col + 1, r.row), PopupHit::Choice(0, 1));
        let mut fp = FakePanel::new(65, 44);
        p.draw(&mut fp);
        assert!((3..20).any(|r| fp.row_text(r).contains("Terminus")));
    }
}
