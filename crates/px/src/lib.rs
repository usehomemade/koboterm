//! Pixel-level drawing for the reader-style chrome (top bar, popups): grey
//! fills, lines, circles and antialiased TrueType text. Anything that can
//! `put` a grey pixel and refresh a pixel rectangle is a `PxCanvas`.

use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct PxRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PxRect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        PxRect { x, y, w, h }
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn inset(&self, d: i32) -> PxRect {
        PxRect { x: self.x + d, y: self.y + d, w: (self.w - 2 * d).max(0), h: (self.h - 2 * d).max(0) }
    }
    pub fn union(&self, o: &PxRect) -> PxRect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        PxRect { x, y, w: self.right().max(o.right()) - x, h: self.bottom().max(o.bottom()) - y }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PxWave {
    Fast,
    Quality,
    Full,
}

pub trait PxCanvas {
    fn size(&self) -> (i32, i32);
    fn put(&mut self, x: i32, y: i32, gray: u8);
    fn get(&self, x: i32, y: i32) -> u8;
    fn refresh_px(&mut self, r: PxRect, wave: PxWave);

    fn fill(&mut self, r: PxRect, gray: u8) {
        let (w, h) = self.size();
        for y in r.y.max(0)..r.bottom().min(h) {
            for x in r.x.max(0)..r.right().min(w) {
                self.put(x, y, gray);
            }
        }
    }

    /// Blend a coverage mask (0..255) of colour `gray` over what is there.
    fn blend(&mut self, x: i32, y: i32, cov: u8, gray: u8) {
        if cov == 0 {
            return;
        }
        let (w, h) = self.size();
        if x < 0 || y < 0 || x >= w || y >= h {
            return;
        }
        if cov == 255 {
            self.put(x, y, gray);
            return;
        }
        let bg = self.get(x, y) as i32;
        let v = bg + (gray as i32 - bg) * cov as i32 / 255;
        self.put(x, y, v.clamp(0, 255) as u8);
    }

    fn hline(&mut self, x0: i32, x1: i32, y: i32, thick: i32, gray: u8) {
        self.fill(PxRect::new(x0.min(x1), y, (x1 - x0).abs() + 1, thick.max(1)), gray);
    }

    fn vline(&mut self, x: i32, y0: i32, y1: i32, thick: i32, gray: u8) {
        self.fill(PxRect::new(x, y0.min(y1), thick.max(1), (y1 - y0).abs() + 1), gray);
    }

    /// Line of the given thickness (square pen).
    fn line(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, thick: i32, gray: u8) {
        let (dx, dy) = ((x1 - x0).abs(), -(y1 - y0).abs());
        let (sx, sy) = (if x0 < x1 { 1 } else { -1 }, if y0 < y1 { 1 } else { -1 });
        let (mut x, mut y, mut err) = (x0, y0, dx + dy);
        let half = thick / 2;
        loop {
            self.fill(PxRect::new(x - half, y - half, thick.max(1), thick.max(1)), gray);
            if x == x1 && y == y1 {
                break;
            }
            let e2 = 2 * err;
            if e2 >= dy {
                err += dy;
                x += sx;
            }
            if e2 <= dx {
                err += dx;
                y += sy;
            }
        }
    }

    /// Ring of outer radius `r` and thickness `thick`; `thick >= r` gives a disc.
    fn circle(&mut self, cx: i32, cy: i32, r: i32, thick: i32, gray: u8) {
        let r_in = (r - thick).max(0);
        let (r2, ri2) = ((r * r) as f32 + r as f32, (r_in * r_in) as f32 - r_in as f32);
        for y in -r..=r {
            for x in -r..=r {
                let d = (x * x + y * y) as f32;
                if d <= r2 && d >= ri2.max(0.0) {
                    // Soft edge: partial coverage on the outer boundary.
                    let dist = d.sqrt();
                    let edge = (r as f32 + 0.5 - dist).clamp(0.0, 1.0);
                    let inner = if r_in > 0 { (dist - (r_in as f32 - 0.5)).clamp(0.0, 1.0) } else { 1.0 };
                    let cov = (255.0 * edge * inner) as u8;
                    self.blend(cx + x, cy + y, cov, gray);
                }
            }
        }
    }

    fn rect_outline(&mut self, r: PxRect, thick: i32, gray: u8) {
        self.fill(PxRect::new(r.x, r.y, r.w, thick), gray);
        self.fill(PxRect::new(r.x, r.bottom() - thick, r.w, thick), gray);
        self.fill(PxRect::new(r.x, r.y, thick, r.h), gray);
        self.fill(PxRect::new(r.right() - thick, r.y, thick, r.h), gray);
    }

    /// Draw `text` with its baseline at `y`, left edge `x`. Returns the advance width.
    fn text(&mut self, face: &Face, size: f32, x: i32, y: i32, text: &str, gray: u8) -> i32 {
        let mut pen = x as f32;
        let mut prev: Option<char> = None;
        for ch in text.chars() {
            if let Some(p) = prev {
                pen += face.kern(p, ch, size);
            }
            let (m, bitmap) = face.raster(ch, size);
            let gx = pen.round() as i32 + m.xmin;
            let gy = y - m.ymin - m.height as i32;
            for row in 0..m.height {
                for col in 0..m.width {
                    self.blend(gx + col as i32, gy + row as i32, bitmap[row * m.width + col], gray);
                }
            }
            pen += m.advance_width;
            prev = Some(ch);
        }
        (pen - x as f32).round() as i32
    }
}

/// A TrueType face with a glyph cache.
pub struct Face {
    font: fontdue::Font,
    cache: std::cell::RefCell<HashMap<(char, u32), (fontdue::Metrics, Vec<u8>)>>,
}

impl Face {
    pub fn from_bytes(bytes: &[u8]) -> Option<Face> {
        let font = fontdue::Font::from_bytes(bytes, fontdue::FontSettings::default()).ok()?;
        Some(Face { font, cache: Default::default() })
    }

    pub fn load(path: &str) -> Option<Face> {
        Self::from_bytes(&std::fs::read(path).ok()?)
    }

    fn raster(&self, ch: char, size: f32) -> (fontdue::Metrics, Vec<u8>) {
        let key = (ch, size.to_bits());
        if let Some(v) = self.cache.borrow().get(&key) {
            return v.clone();
        }
        let v = self.font.rasterize(ch, size);
        self.cache.borrow_mut().insert(key, v.clone());
        v
    }

    fn kern(&self, a: char, b: char, size: f32) -> f32 {
        self.font.horizontal_kern(a, b, size).unwrap_or(0.0)
    }

    pub fn width(&self, size: f32, text: &str) -> i32 {
        let mut w = 0.0;
        let mut prev = None;
        for ch in text.chars() {
            if let Some(p) = prev {
                w += self.kern(p, ch, size);
            }
            w += self.font.metrics(ch, size).advance_width;
            prev = Some(ch);
        }
        w.round() as i32
    }

    /// Distance from baseline to the top of capitals, roughly, for vertical centring.
    pub fn ascent(&self, size: f32) -> i32 {
        self.font.horizontal_line_metrics(size).map(|m| m.ascent.round() as i32).unwrap_or((size * 0.75) as i32)
    }

    pub fn descent(&self, size: f32) -> i32 {
        self.font.horizontal_line_metrics(size).map(|m| (-m.descent).round() as i32).unwrap_or((size * 0.25) as i32)
    }
}

/// In-memory canvas for tests and the host simulator.
pub struct FakeCanvas {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u8>,
    pub refreshes: Vec<(PxRect, PxWave)>,
}

impl FakeCanvas {
    pub fn new(w: i32, h: i32) -> Self {
        FakeCanvas { w, h, px: vec![255; (w * h) as usize], refreshes: Vec::new() }
    }
    pub fn dark_pixels(&self, r: PxRect) -> usize {
        let mut n = 0;
        for y in r.y.max(0)..r.bottom().min(self.h) {
            for x in r.x.max(0)..r.right().min(self.w) {
                if self.px[(y * self.w + x) as usize] < 128 {
                    n += 1;
                }
            }
        }
        n
    }
}

impl PxCanvas for FakeCanvas {
    fn size(&self) -> (i32, i32) {
        (self.w, self.h)
    }
    fn put(&mut self, x: i32, y: i32, gray: u8) {
        if x >= 0 && y >= 0 && x < self.w && y < self.h {
            self.px[(y * self.w + x) as usize] = gray;
        }
    }
    fn get(&self, x: i32, y: i32) -> u8 {
        if x >= 0 && y >= 0 && x < self.w && y < self.h { self.px[(y * self.w + x) as usize] } else { 255 }
    }
    fn refresh_px(&mut self, r: PxRect, wave: PxWave) {
        self.refreshes.push((r, wave));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_paint_where_expected() {
        let mut c = FakeCanvas::new(200, 100);
        c.fill(PxRect::new(10, 10, 20, 5), 0);
        assert_eq!(c.dark_pixels(PxRect::new(0, 0, 200, 100)), 100);
        c.circle(100, 50, 20, 20, 0);
        let disc = c.dark_pixels(PxRect::new(80, 30, 41, 41));
        assert!((1150..=1350).contains(&disc), "disc pixels {disc}");
        c.line(0, 90, 199, 90, 2, 0);
        assert!(c.dark_pixels(PxRect::new(0, 89, 200, 3)) >= 400);
        c.rect_outline(PxRect::new(150, 10, 40, 30), 2, 0);
        assert_eq!(c.get(170, 25), 255, "outline leaves the interior alone");
        assert_eq!(c.get(150, 10), 0);
    }

    #[test]
    fn blend_mixes_towards_the_colour() {
        let mut c = FakeCanvas::new(4, 4);
        c.blend(1, 1, 128, 0);
        assert!((120..=135).contains(&c.get(1, 1)));
        c.blend(2, 2, 0, 0);
        assert_eq!(c.get(2, 2), 255);
    }
}
