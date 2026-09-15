//! Bitmap font for the terminal grid. Parses a BDF file at startup (a few ms)
//! into fixed-size glyph bitmaps. Ships Spleen 16x32 (BSD-2-Clause, see
//! `assets/LICENSE-spleen`), which is designed at exactly this pixel size, so
//! nothing is scaled and every stroke is crisp on e-ink.

use std::collections::HashMap;

pub const SPLEEN_16X32: &str = include_str!("../assets/spleen-16x32.bdf");
pub const SPLEEN_12X24: &str = include_str!("../assets/spleen-12x24.bdf");

/// One glyph, one `u32` per row, MSB = leftmost pixel. Width <= 32.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Glyph {
    pub rows: Vec<u32>,
}

#[derive(Debug)]
pub struct Font {
    pub width: u16,
    pub height: u16,
    glyphs: HashMap<char, Glyph>,
    fallback: Glyph,
}

impl Font {
    /// Parse a BDF font. Only fixed-cell glyphs of exactly the font's bounding
    /// box are accepted; others are skipped. Panics on a malformed file, which
    /// is a build-time asset error, not a runtime condition.
    pub fn from_bdf(src: &str) -> Font {
        let mut width = 0u16;
        let mut height = 0u16;
        let mut yoff_font = 0i32;
        let mut glyphs = HashMap::new();
        let mut lines = src.lines();
        while let Some(line) = lines.next() {
            if let Some(rest) = line.strip_prefix("FONTBOUNDINGBOX ") {
                let v: Vec<i32> = rest.split_whitespace().map(|s| s.parse().unwrap()).collect();
                width = v[0] as u16;
                height = v[1] as u16;
                yoff_font = v[3];
            } else if line.starts_with("STARTCHAR") {
                let mut enc: Option<u32> = None;
                let mut bbx = (0i32, 0i32, 0i32, 0i32);
                let mut rows: Vec<u32> = Vec::new();
                let mut in_bitmap = false;
                for l in lines.by_ref() {
                    if l == "ENDCHAR" {
                        break;
                    }
                    if in_bitmap {
                        let bits = u32::from_str_radix(l.trim(), 16).unwrap();
                        // Hex rows are padded to a byte boundary; left-align to 32 bits.
                        let hex_bits = (l.trim().len() * 4) as u32;
                        rows.push(bits << (32 - hex_bits));
                        continue;
                    }
                    if let Some(r) = l.strip_prefix("ENCODING ") {
                        enc = r.trim().parse::<i64>().ok().filter(|v| *v >= 0).map(|v| v as u32);
                    } else if let Some(r) = l.strip_prefix("BBX ") {
                        let v: Vec<i32> = r.split_whitespace().map(|s| s.parse().unwrap()).collect();
                        bbx = (v[0], v[1], v[2], v[3]);
                    } else if l == "BITMAP" {
                        in_bitmap = true;
                    }
                }
                let Some(enc) = enc else { continue };
                let Some(ch) = char::from_u32(enc) else { continue };
                // Normalise to the full cell: shift by x offset, pad rows per y offset.
                let (bw, bh, bx, by) = bbx;
                if bw > width as i32 || bh > height as i32 {
                    continue;
                }
                let top_pad = (height as i32 - bh) - (by - yoff_font);
                let mut cell = vec![0u32; height as usize];
                for (i, r) in rows.iter().enumerate() {
                    let y = top_pad + i as i32;
                    if y >= 0 && y < height as i32 {
                        cell[y as usize] = if bx >= 0 { r >> bx } else { r << (-bx) };
                    }
                }
                glyphs.insert(ch, Glyph { rows: cell });
            }
        }
        assert!(width > 0 && height > 0, "BDF has no FONTBOUNDINGBOX");
        // Fallback: a hollow box, for glyphs the font lacks.
        let mut fb = vec![0u32; height as usize];
        let full = if width >= 32 { u32::MAX } else { !0u32 << (32 - width as u32) };
        let edge = (1u32 << 31) | (1u32 << (32 - width as u32));
        for (y, r) in fb.iter_mut().enumerate() {
            let y = y as u16;
            *r = if y == 2 || y == height - 3 { full } else if y > 2 && y < height - 3 { edge } else { 0 };
        }
        Font { width, height, glyphs, fallback: Glyph { rows: fb } }
    }

    /// Pixel-double every glyph (16x32 from 8x16, 24x48 from 12x24).
    pub fn scaled_2x(&self) -> Font {
        fn dbl(bits: u32, w: u16) -> u32 {
            let mut out = 0u32;
            for x in 0..w as u32 {
                if bits & (1 << (31 - x)) != 0 {
                    out |= 0b11 << (30 - 2 * x);
                }
            }
            out
        }
        let w = self.width;
        let scale = |g: &Glyph| Glyph { rows: g.rows.iter().flat_map(|r| [dbl(*r, w), dbl(*r, w)]).collect() };
        Font {
            width: self.width * 2,
            height: self.height * 2,
            glyphs: self.glyphs.iter().map(|(c, g)| (*c, scale(g))).collect(),
            fallback: scale(&self.fallback),
        }
    }

    pub fn glyph(&self, ch: char) -> &Glyph {
        self.glyphs.get(&ch).unwrap_or(&self.fallback)
    }

    pub fn has(&self, ch: char) -> bool {
        self.glyphs.contains_key(&ch)
    }

    pub fn len(&self) -> usize {
        self.glyphs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.glyphs.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render(f: &Font, ch: char) -> Vec<String> {
        f.glyph(ch)
            .rows
            .iter()
            .map(|r| (0..f.width).map(|x| if r & (1 << (31 - x)) != 0 { '#' } else { '.' }).collect())
            .collect()
    }

    #[test]
    fn spleen_16x32_parses_with_box_drawing() {
        let f = Font::from_bdf(SPLEEN_16X32);
        assert_eq!((f.width, f.height), (16, 32));
        assert!(f.len() > 900, "{}", f.len());
        for ch in ['A', 'a', '0', '─', '│', '┌', '█', '●', '·'] {
            assert!(f.has(ch), "missing {ch:?}");
        }
    }

    #[test]
    fn capital_a_looks_like_an_a() {
        let f = Font::from_bdf(SPLEEN_16X32);
        let rows = render(&f, 'A');
        assert_eq!(rows.len(), 32);
        // Top of the glyph is blank, the crossbar row is solid across the letter.
        assert_eq!(rows[0], "................");
        assert!(rows.iter().any(|r| r == "..############.."), "{rows:#?}");
    }

    #[test]
    fn vertical_bar_spans_the_full_cell_height() {
        let f = Font::from_bdf(SPLEEN_16X32);
        let rows = render(&f, '│');
        assert!(rows.iter().all(|r| r.contains('#')), "box-drawing must be continuous: {rows:#?}");
    }

    #[test]
    fn missing_glyph_gets_the_fallback_box() {
        let f = Font::from_bdf(SPLEEN_16X32);
        assert!(!f.has('\u{10FFFF}'));
        assert_eq!(f.glyph('\u{10FFFF}'), &f.fallback);
    }

    #[test]
    fn scaling_doubles_every_pixel() {
        let f = Font::from_bdf(SPLEEN_12X24).scaled_2x();
        assert_eq!((f.width, f.height), (24, 48));
        let a = render(&f, 'A');
        assert_eq!(a.len(), 48);
        assert_eq!(a[0], a[1], "rows come in identical pairs");
        assert!(a.iter().any(|r| r.contains("##") && !r.contains("#.#")), "columns are doubled");
    }

    #[test]
    fn spleen_12x24_parses_too() {
        let f = Font::from_bdf(SPLEEN_12X24);
        assert_eq!((f.width, f.height), (12, 24));
        assert!(f.has('│'));
    }
}
