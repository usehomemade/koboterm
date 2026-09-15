//! Nickel's reading-view header: status line (clock, battery) and a toolbar
//! with a back arrow, a bold title and line icons on the right, over a thin
//! separator. 150 px tall on a 1072 px wide panel; scales with width.

use crate::{icons, Fonts, BLACK, GRAY};
use px::{PxCanvas, PxRect, PxWave};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarAction {
    Back,
    Brightness,
    Text,
    Keyboard,
    Settings,
    More,
    Nothing,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BarStatus {
    pub clock: String,
    pub title: String,
    pub battery_pct: Option<u8>,
    pub charging: bool,
}

pub struct TopBar {
    w: i32,
    h: i32,
    margin: i32,
    buttons: Vec<(BarAction, PxRect)>,
}

impl TopBar {
    pub fn new(w: i32) -> Self {
        let h = 150 * w / 1072;
        let margin = 40 * w / 1072;
        let icon_step = 72 * w / 1072;
        let cy = h * 70 / 100;
        let mut buttons = Vec::new();
        let mut cx = w - margin - icon_step / 2;
        for a in [BarAction::More, BarAction::Settings, BarAction::Keyboard, BarAction::Text, BarAction::Brightness] {
            buttons.push((a, PxRect::new(cx - icon_step / 2, cy - 45, icon_step, 90)));
            cx -= icon_step;
        }
        buttons.push((BarAction::Back, PxRect::new(0, cy - 45, cx + icon_step / 2, 90)));
        TopBar { w, h, margin, buttons }
    }

    pub fn height(&self) -> i32 {
        self.h
    }

    pub fn area(&self) -> PxRect {
        PxRect::new(0, 0, self.w, self.h)
    }

    fn status_rect(&self) -> PxRect {
        PxRect::new(0, 0, self.w, self.h * 40 / 100)
    }

    pub fn draw(&self, c: &mut dyn PxCanvas, f: &Fonts, st: &BarStatus) {
        c.fill(self.area(), 0xFF);
        self.paint_status(c, f, st);
        let cy = self.h * 70 / 100;
        // Back arrow + title.
        icons::back_arrow(c, self.margin, cy, 34 * self.w / 1072, 3, BLACK);
        let size = 30.0 * self.w as f32 / 1072.0;
        c.text(&f.sans_bold, size, self.margin + 64 * self.w / 1072, cy + f.sans_bold.ascent(size) / 2, &st.title, BLACK);
        // Icons, right to left.
        for (a, r) in &self.buttons {
            let (cx, cy) = (r.x + r.w / 2, cy);
            match a {
                BarAction::Brightness => icons::sun(c, cx, cy, 9, 3, BLACK),
                BarAction::Text => icons::aa_badge(c, &f.sans_bold, cx, cy, 26, true),
                BarAction::Keyboard => icons::keyboard(c, cx, cy, 44, 28, 3, BLACK),
                BarAction::Settings => icons::gear(c, cx, cy, 13, 3, BLACK),
                BarAction::More => icons::dots(c, cx, cy, 3, 12, BLACK),
                _ => {}
            }
        }
        c.hline(self.margin - 10, self.w - self.margin + 10, self.h - 4, 2, GRAY);
        c.refresh_px(self.area(), PxWave::Quality);
    }

    fn paint_status(&self, c: &mut dyn PxCanvas, f: &Fonts, st: &BarStatus) {
        let r = self.status_rect();
        c.fill(r, 0xFF);
        let size = 26.0 * self.w as f32 / 1072.0;
        let cy = r.h * 55 / 100;
        c.text(&f.sans, size, self.margin, cy + f.sans.ascent(size) / 2, &st.clock, BLACK);
        let (bw, bh) = (40 * self.w / 1072, 22 * self.w / 1072);
        icons::battery(c, self.w - self.margin - bw - 4, cy - bh / 2, bw, bh, st.battery_pct, st.charging, BLACK);
    }

    /// Redraw only the status line (clock / battery ticks).
    pub fn update_status(&self, c: &mut dyn PxCanvas, f: &Fonts, st: &BarStatus) {
        self.paint_status(c, f, st);
        c.refresh_px(self.status_rect(), PxWave::Fast);
    }

    pub fn hit(&self, x: i32, y: i32) -> BarAction {
        self.buttons.iter().find(|(_, r)| r.contains(x, y)).map(|(a, _)| *a).unwrap_or(BarAction::Nothing)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use px::FakeCanvas;

    #[test]
    fn buttons_are_laid_out_right_to_left_and_hit_test() {
        let tb = TopBar::new(1072);
        assert_eq!(tb.height(), 150);
        assert_eq!(tb.hit(1072 - 60, 105), BarAction::More);
        assert_eq!(tb.hit(1072 - 60 - 72 * 4, 105), BarAction::Brightness);
        assert_eq!(tb.hit(80, 105), BarAction::Back);
        assert_eq!(tb.hit(500, 20), BarAction::Nothing);
    }

    #[test]
    fn icons_paint_without_fonts() {
        // Icons only (fonts need the device); the separator and icons must leave ink.
        let mut c = FakeCanvas::new(1072, 200);
        let tb = TopBar::new(1072);
        for (a, r) in &tb.buttons {
            let (cx, cy) = (r.x + r.w / 2, 105);
            match a {
                BarAction::Brightness => icons::sun(&mut c, cx, cy, 9, 3, 0),
                BarAction::Keyboard => icons::keyboard(&mut c, cx, cy, 44, 28, 3, 0),
                BarAction::Settings => icons::gear(&mut c, cx, cy, 13, 3, 0),
                BarAction::More => icons::dots(&mut c, cx, cy, 3, 12, 0),
                _ => {}
            }
            if !matches!(a, BarAction::Back | BarAction::Text) {
                assert!(c.dark_pixels(*r) > 40, "{a:?} drew nothing");
            }
        }
        icons::battery(&mut c, 1000, 20, 40, 22, Some(50), false, 0);
        assert!(c.dark_pixels(PxRect::new(1000, 20, 46, 22)) > 100);
    }
}
