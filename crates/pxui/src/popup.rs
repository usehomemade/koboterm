//! Nickel-style popups: a bordered panel under the toolbar with a small notch,
//! serif section titles, centred values, double-ring sliders with end icons,
//! and segmented choices ("Justification: Off | ≡ | ≡" style).

use crate::{icons, Fonts, BLACK, GRAY, LIGHT, WHITE};
use px::{PxCanvas, PxRect, PxWave};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndIcon {
    None,
    SunSmall,
    SunLarge,
    Gear,
    Moon,
    Minus,
    Plus,
    TextSmall,
    TextLarge,
}

pub struct Slider {
    pub min: i32,
    pub max: i32,
    pub value: i32,
    pub step: i32,
    pub left: EndIcon,
    pub right: EndIcon,
    /// Shown centred above the track ("52%"), if Some(formatter).
    pub value_fmt: Option<fn(i32) -> String>,
    track: (i32, i32, i32), // x0, x1, y
    #[allow(dead_code)]
    row: PxRect,
}

impl Slider {
    pub fn value_at(&self, x: i32) -> i32 {
        let (x0, x1, _) = self.track;
        let span = (x1 - x0).max(1);
        let pos = (x - x0).clamp(0, span);
        let v = self.min + ((pos * (self.max - self.min)) as f32 / span as f32).round() as i32;
        ((v - self.min + self.step / 2) / self.step * self.step + self.min).clamp(self.min, self.max)
    }
    fn knob_x(&self) -> i32 {
        let (x0, x1, _) = self.track;
        x0 + (x1 - x0) * (self.value - self.min) / (self.max - self.min).max(1)
    }
}

pub struct Choice {
    pub options: Vec<String>,
    pub selected: usize,
    rects: Vec<PxRect>,
}

pub enum Row {
    Title(String),
    Divider,
    /// Section slider, full width, optional centred value above it.
    Slider(Slider),
    /// "Label:" on the left, slider on the right, with - and + ends.
    LabeledSlider(String, Slider),
    LabeledChoice(String, Choice),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupHit {
    Close,
    Slider(usize, i32),
    Choice(usize, usize),
    Nothing,
}

pub struct Popup {
    pub rect: PxRect,
    notch_x: Option<i32>,
    rows: Vec<(Row, PxRect)>,
    sliders: Vec<usize>, // row indices that hold a slider, in order
    choices: Vec<usize>,
}

impl Popup {
    /// Panel of width `w - 2*margin` starting at `y`; `notch_x` draws the
    /// little pointer towards the toolbar icon that opened it.
    pub fn new(w: i32, margin: i32, y: i32, notch_x: Option<i32>) -> Self {
        Popup { rect: PxRect::new(margin, y, w - 2 * margin, 30), notch_x, rows: Vec::new(), sliders: Vec::new(), choices: Vec::new() }
    }

    fn scale(&self) -> f32 {
        (self.rect.w + 80) as f32 / 1072.0
    }

    fn push(&mut self, row: Row, h: i32) {
        let r = PxRect::new(self.rect.x, self.rect.y + self.rect.h - 10, self.rect.w, h);
        self.rect.h += h;
        self.rows.push((row, r));
    }

    pub fn title(&mut self, t: &str) -> &mut Self {
        self.push(Row::Title(t.into()), (70.0 * self.scale()) as i32);
        self
    }

    pub fn divider(&mut self) -> &mut Self {
        self.push(Row::Divider, (30.0 * self.scale()) as i32);
        self
    }

    fn make_slider(&self, min: i32, max: i32, value: i32, step: i32, left: EndIcon, right: EndIcon, value_fmt: Option<fn(i32) -> String>, row: PxRect, x0: i32, x1: i32, y: i32) -> Slider {
        Slider { min, max, value: value.clamp(min, max), step: step.max(1), left, right, value_fmt, track: (x0, x1, y), row }
    }

    pub fn slider(&mut self, min: i32, max: i32, value: i32, step: i32, left: EndIcon, right: EndIcon, value_fmt: Option<fn(i32) -> String>) -> &mut Self {
        let s = self.scale();
        let h = (if value_fmt.is_some() { 130.0 } else { 90.0 } * s) as i32;
        let row = PxRect::new(self.rect.x, self.rect.y + self.rect.h - 10, self.rect.w, h);
        let pad = (110.0 * s) as i32;
        let y = row.y + h - (50.0 * s) as i32;
        let sl = self.make_slider(min, max, value, step, left, right, value_fmt, row, row.x + pad, row.right() - pad, y);
        self.sliders.push(self.rows.len());
        self.push(Row::Slider(sl), h);
        self
    }

    pub fn labeled_slider(&mut self, label: &str, min: i32, max: i32, value: i32, step: i32) -> &mut Self {
        let s = self.scale();
        let h = (100.0 * s) as i32;
        let row = PxRect::new(self.rect.x, self.rect.y + self.rect.h - 10, self.rect.w, h);
        let x0 = row.x + (330.0 * s) as i32;
        let x1 = row.right() - (130.0 * s) as i32;
        let y = row.y + h / 2;
        let sl = self.make_slider(min, max, value, step, EndIcon::Minus, EndIcon::Plus, None, row, x0, x1, y);
        self.sliders.push(self.rows.len());
        self.push(Row::LabeledSlider(label.into(), sl), h);
        self
    }

    pub fn labeled_choice(&mut self, label: &str, options: &[&str], selected: usize) -> &mut Self {
        let s = self.scale();
        let h = (100.0 * s) as i32;
        let row = PxRect::new(self.rect.x, self.rect.y + self.rect.h - 10, self.rect.w, h);
        let x0 = row.x + (300.0 * s) as i32;
        let x1 = row.right() - (40.0 * s) as i32;
        let n = options.len().max(1) as i32;
        let cw = (x1 - x0) / n;
        let rects = (0..n).map(|i| PxRect::new(x0 + i * cw, row.y + h / 2 - (30.0 * s) as i32, cw, (60.0 * s) as i32)).collect();
        self.choices.push(self.rows.len());
        self.push(Row::LabeledChoice(label.into(), Choice { options: options.iter().map(|o| o.to_string()).collect(), selected, rects }), h);
        self
    }

    pub fn finish(&mut self) {
        self.rect.h += (20.0 * self.scale()) as i32;
    }

    pub fn slider_count(&self) -> usize {
        self.sliders.len()
    }

    fn slider_ref(&self, i: usize) -> &Slider {
        match &self.rows[self.sliders[i]].0 {
            Row::Slider(s) | Row::LabeledSlider(_, s) => s,
            _ => unreachable!(),
        }
    }

    fn slider_mut(&mut self, i: usize) -> &mut Slider {
        let idx = self.sliders[i];
        match &mut self.rows[idx].0 {
            Row::Slider(s) | Row::LabeledSlider(_, s) => s,
            _ => unreachable!(),
        }
    }

    pub fn slider_value(&self, i: usize) -> i32 {
        self.slider_ref(i).value
    }

    pub fn set_slider(&mut self, i: usize, value: i32) {
        let s = self.slider_mut(i);
        s.value = value.clamp(s.min, s.max);
    }

    pub fn set_slider_max(&mut self, i: usize, max: i32) {
        let s = self.slider_mut(i);
        s.max = max.max(s.min + 1);
        s.value = s.value.min(s.max);
    }

    pub fn set_choice(&mut self, c: usize, selected: usize) {
        if let Row::LabeledChoice(_, ch) = &mut self.rows[self.choices[c]].0 {
            ch.selected = selected.min(ch.options.len() - 1);
        }
    }

    pub fn draw(&self, c: &mut dyn PxCanvas, f: &Fonts) {
        let r = self.rect;
        c.fill(PxRect::new(r.x, r.y - 14, r.w, r.h + 14), WHITE);
        c.rect_outline(r, 2, BLACK);
        if let Some(nx) = self.notch_x {
            // Small upward pointer, like Nickel's popovers.
            for i in 0..12 {
                c.hline(nx - i, nx + i, r.y - 12 + i, 1, if i >= 10 { BLACK } else { WHITE });
                c.put(nx - i, r.y - 12 + i, BLACK);
                c.put(nx + i, r.y - 12 + i, BLACK);
            }
            c.hline(nx - 11, nx + 11, r.y, 2, WHITE);
        }
        for i in 0..self.rows.len() {
            self.draw_row(c, f, i);
        }
        c.refresh_px(PxRect::new(r.x, r.y - 14, r.w, r.h + 14), PxWave::Quality);
    }

    fn draw_row(&self, c: &mut dyn PxCanvas, f: &Fonts, i: usize) {
        let s = self.scale();
        let (row, r) = &self.rows[i];
        let inner = PxRect::new(r.x + 2, r.y, r.w - 4, r.h);
        c.fill(inner, WHITE);
        let pad = (40.0 * s) as i32;
        match row {
            Row::Title(t) => {
                let size = 34.0 * s;
                c.text(&f.serif, size, r.x + pad, r.y + r.h / 2 + f.serif.ascent(size) / 2, t, BLACK);
            }
            Row::Divider => c.hline(r.x + 2, r.right() - 3, r.y + r.h / 2, 2, LIGHT),
            Row::Slider(sl) => {
                if let Some(fmt) = sl.value_fmt {
                    let size = 28.0 * s;
                    let v = fmt(sl.value);
                    let w = f.sans.width(size, &v);
                    c.text(&f.sans, size, r.x + r.w / 2 - w / 2, sl.track.2 - (40.0 * s) as i32, &v, BLACK);
                }
                self.draw_track(c, f, sl, s);
            }
            Row::LabeledSlider(label, sl) => {
                let size = 30.0 * s;
                c.text(&f.sans, size, r.x + pad, sl.track.2 + f.sans.ascent(size) / 2, label, BLACK);
                self.draw_track(c, f, sl, s);
            }
            Row::LabeledChoice(label, ch) => {
                let size = 30.0 * s;
                c.text(&f.sans, size, r.x + pad, r.y + r.h / 2 + f.sans.ascent(size) / 2, label, BLACK);
                for (k, (opt, cr)) in ch.options.iter().zip(&ch.rects).enumerate() {
                    let sel = k == ch.selected;
                    c.fill(*cr, if sel { BLACK } else { LIGHT });
                    let size = 26.0 * s;
                    let w = f.sans_bold.width(size, opt);
                    c.text(&f.sans_bold, size, cr.x + cr.w / 2 - w / 2, cr.y + cr.h / 2 + f.sans_bold.ascent(size) / 2, opt, if sel { WHITE } else { BLACK });
                }
            }
        }
    }

    fn draw_track(&self, c: &mut dyn PxCanvas, f: &Fonts, sl: &Slider, s: f32) {
        let (x0, x1, y) = sl.track;
        let kx = sl.knob_x();
        c.hline(x0, kx, y, 3, BLACK);
        c.hline(kx, x1, y, 3, GRAY);
        let (r_out, r_in) = ((18.0 * s) as i32, (9.0 * s) as i32);
        c.circle(kx, y, r_out, r_out, WHITE);
        c.circle(kx, y, r_out, 3, BLACK);
        c.circle(kx, y, r_in, 3, BLACK);
        let d = (50.0 * s) as i32;
        for (icon, x) in [(sl.left, x0 - d), (sl.right, x1 + d)] {
            match icon {
                EndIcon::None => {}
                EndIcon::SunSmall => icons::sun(c, x, y, 5, 2, BLACK),
                EndIcon::SunLarge => icons::sun(c, x, y, 9, 3, BLACK),
                EndIcon::Gear => icons::gear(c, x, y, 10, 2, BLACK),
                EndIcon::Moon => icons::moon(c, x, y, 11, 3, BLACK),
                EndIcon::Minus => c.hline(x - 10, x + 10, y, 3, BLACK),
                EndIcon::Plus => {
                    c.hline(x - 10, x + 10, y, 3, BLACK);
                    c.vline(x, y - 10, y + 10, 3, BLACK);
                }
                EndIcon::TextSmall => icons::text_size_marks(c, &f.sans, x, x, y),
                EndIcon::TextLarge => icons::text_size_marks(c, &f.sans, x, x, y),
            }
        }
    }

    /// Repaint one slider's row after a value change.
    pub fn refresh_slider(&self, c: &mut dyn PxCanvas, f: &Fonts, i: usize, wave: PxWave) {
        let idx = self.sliders[i];
        self.draw_row(c, f, idx);
        c.refresh_px(self.rows[idx].1, wave);
    }

    pub fn refresh_choice(&self, c: &mut dyn PxCanvas, f: &Fonts, ci: usize) {
        let idx = self.choices[ci];
        self.draw_row(c, f, idx);
        c.refresh_px(self.rows[idx].1, PxWave::Fast);
    }

    /// Slider under a finger (its track row, any x), for dragging.
    pub fn slider_at(&self, x: i32, y: i32) -> Option<usize> {
        (0..self.sliders.len()).find(|&i| {
            let s = self.slider_ref(i);
            let (x0, x1, ty) = s.track;
            (y - ty).abs() <= 40 && x >= x0 - 30 && x <= x1 + 30
        })
    }

    pub fn slider_value_at(&self, i: usize, x: i32) -> i32 {
        self.slider_ref(i).value_at(x)
    }

    pub fn hit(&self, x: i32, y: i32) -> PopupHit {
        if !self.rect.contains(x, y) {
            return PopupHit::Close;
        }
        for (i, &idx) in self.sliders.iter().enumerate() {
            let s = self.slider_ref(i);
            let (x0, x1, ty) = s.track;
            let d = 50;
            if (y - ty).abs() <= 40 {
                if x < x0 - 10 && x >= x0 - d - 40 {
                    return PopupHit::Slider(i, (s.value - s.step).max(s.min));
                }
                if x > x1 + 10 && x <= x1 + d + 40 {
                    return PopupHit::Slider(i, (s.value + s.step).min(s.max));
                }
                if x >= x0 - 10 && x <= x1 + 10 {
                    return PopupHit::Slider(i, s.value_at(x));
                }
            }
            let _ = idx;
        }
        for (ci, &idx) in self.choices.iter().enumerate() {
            if let Row::LabeledChoice(_, ch) = &self.rows[idx].0 {
                for (k, cr) in ch.rects.iter().enumerate() {
                    if cr.contains(x, y) {
                        return PopupHit::Choice(ci, k);
                    }
                }
            }
        }
        PopupHit::Nothing
    }
}

pub fn pct(v: i32) -> String {
    format!("{v}%")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_popup_geometry_and_hits() {
        let mut p = Popup::new(1072, 40, 160, Some(700));
        p.title("Brightness").slider(0, 100, 52, 1, EndIcon::SunSmall, EndIcon::SunLarge, Some(pct)).divider().title("Natural light").slider(0, 10, 3, 1, EndIcon::Gear, EndIcon::Moon, None);
        p.finish();
        assert_eq!(p.slider_count(), 2);
        assert!(p.rect.h > 300 && p.rect.h < 600, "{}", p.rect.h);
        let s0 = p.slider_ref(0);
        let (x0, x1, y) = s0.track;
        assert_eq!(p.hit(x0, y), PopupHit::Slider(0, 0));
        assert_eq!(p.hit(x1, y), PopupHit::Slider(0, 100));
        let mid = p.slider_value_at(0, (x0 + x1) / 2);
        assert!((48..=52).contains(&mid), "{mid}");
        assert_eq!(p.slider_at((x0 + x1) / 2, y + 20), Some(0));
        assert_eq!(p.hit(10, 10), PopupHit::Close);
        let s1 = p.slider_ref(1);
        assert_eq!(p.hit(s1.track.0 - 40, s1.track.2), PopupHit::Slider(1, 2));
        assert_eq!(p.hit(s1.track.1 + 40, s1.track.2), PopupHit::Slider(1, 4));
    }

    #[test]
    fn text_popup_choices_and_steps() {
        let mut p = Popup::new(1072, 40, 160, None);
        p.labeled_choice("Font Face:", &["Spleen", "Terminus"], 0).labeled_slider("Font Size:", 0, 4, 2, 1).labeled_slider("Margins:", 0, 48, 12, 4);
        p.finish();
        if let Row::LabeledChoice(_, ch) = &p.rows[0].0 {
            let r = ch.rects[1];
            assert_eq!(p.hit(r.x + 5, r.y + 5), PopupHit::Choice(0, 1));
        }
        let s = p.slider_ref(1);
        assert_eq!(p.hit(s.track.1 + 40, s.track.2), PopupHit::Slider(1, 16));
        p.set_slider_max(0, 6);
        assert_eq!(p.slider_ref(0).max, 6);
    }
}
