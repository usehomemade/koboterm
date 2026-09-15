//! Bitmap font for the terminal grid. Parses a BDF file at startup (a few ms)
//! into fixed-size glyph bitmaps. Ships Spleen 16x32 (BSD-2-Clause, see
//! `assets/LICENSE-spleen`), which is designed at exactly this pixel size, so
//! nothing is scaled and every stroke is crisp on e-ink.

use std::collections::HashMap;

pub const SPLEEN_8X16: &str = include_str!("../assets/spleen-8x16.bdf");
pub const SPLEEN_12X24: &str = include_str!("../assets/spleen-12x24.bdf");
pub const SPLEEN_16X32: &str = include_str!("../assets/spleen-16x32.bdf");
pub const SPLEEN_32X64: &str = include_str!("../assets/spleen-32x64.bdf");
pub const TERMINUS_8X16: &str = include_str!("../assets/ter-u16n.bdf");
pub const TERMINUS_10X20: &str = include_str!("../assets/ter-u20n.bdf");
pub const TERMINUS_12X24: &str = include_str!("../assets/ter-u24n.bdf");
pub const TERMINUS_14X28: &str = include_str!("../assets/ter-u28n.bdf");
pub const TERMINUS_16X32: &str = include_str!("../assets/ter-u32n.bdf");

/// A font family with a list of pixel sizes (some produced by 2x scaling).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Family {
    Spleen,
    Terminus,
}

impl Family {
    pub const ALL: [Family; 2] = [Family::Spleen, Family::Terminus];

    pub fn name(self) -> &'static str {
        match self {
            Family::Spleen => "Spleen",
            Family::Terminus => "Terminus",
        }
    }

    pub fn parse(s: &str) -> Family {
        match s.to_ascii_lowercase().as_str() {
            "terminus" => Family::Terminus,
            _ => Family::Spleen,
        }
    }

    /// (source, scale) per size step, smallest first.
    fn steps(self) -> &'static [(&'static str, u8)] {
        match self {
            Family::Spleen => &[(SPLEEN_8X16, 1), (SPLEEN_12X24, 1), (SPLEEN_16X32, 1), (SPLEEN_12X24, 2), (SPLEEN_32X64, 1)],
            Family::Terminus => &[(TERMINUS_8X16, 1), (TERMINUS_10X20, 1), (TERMINUS_12X24, 1), (TERMINUS_14X28, 1), (TERMINUS_16X32, 1), (TERMINUS_12X24, 2), (TERMINUS_16X32, 2)],
        }
    }

    pub fn size_count(self) -> usize {
        self.steps().len()
    }

    /// Cell size in pixels of size step `idx`, without parsing the font.
    pub fn cell_size(self, idx: usize) -> (u16, u16) {
        let (src, scale) = self.steps()[idx.min(self.size_count() - 1)];
        let line = src.lines().find(|l| l.starts_with("FONTBOUNDINGBOX ")).unwrap_or("FONTBOUNDINGBOX 8 16 0 0");
        let v: Vec<u16> = line.split_whitespace().skip(1).take(2).map(|x| x.parse().unwrap_or(8)).collect();
        (v[0] * scale as u16, v[1] * scale as u16)
    }

    pub fn load(self, idx: usize) -> Font {
        let (src, scale) = self.steps()[idx.min(self.size_count() - 1)];
        let f = Font::from_bdf(src);
        if scale == 2 { f.scaled_2x() } else { f }
    }
}
/// Symbol blocks of GNU Unifont (OFL 1.1), used for glyphs the main font lacks.
pub const UNIFONT_SYMBOLS: &str = include_str!("../assets/unifont-symbols.hex");

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
        let mut font = Font { width, height, glyphs, fallback: Glyph { rows: fb } };
        font.add_hex_fallbacks(UNIFONT_SYMBOLS);
        font
    }

    /// Add glyphs from a Unifont `.hex` file for code points the font lacks,
    /// fitted into this font's cell: integer-scaled up when there is room,
    /// sampled down otherwise, and centred.
    pub fn add_hex_fallbacks(&mut self, hex: &str) {
        let (cw, ch) = (self.width as i32, self.height as i32);
        for line in hex.lines() {
            let Some((cp, bits)) = line.split_once(':') else { continue };
            let Some(chr) = u32::from_str_radix(cp, 16).ok().and_then(char::from_u32) else { continue };
            if self.glyphs.contains_key(&chr) {
                continue;
            }
            let gw: i32 = match bits.len() {
                32 => 8,
                64 => 16,
                _ => continue,
            };
            let per_row = (gw / 4) as usize;
            let mut src = [0u16; 16];
            for (i, r) in src.iter_mut().enumerate() {
                *r = u16::from_str_radix(&bits[i * per_row..(i + 1) * per_row], 16).unwrap_or(0) << (16 - gw);
            }
            let scale = (cw / gw).min(ch / 16).max(1);
            let (dw, dh) = if cw >= gw { (gw * scale, 16 * scale) } else { (cw, 16.min(ch)) };
            let (x0, y0) = ((cw - dw) / 2, (ch - dh) / 2);
            let mut rows = vec![0u32; ch as usize];
            for dy in 0..dh {
                let sy = if cw >= gw { dy / scale } else { dy * 16 / dh } as usize;
                let mut out = 0u32;
                for dx in 0..dw {
                    let sx = if cw >= gw { dx / scale } else { dx * gw / dw };
                    if src[sy] & (1 << (15 - sx)) != 0 {
                        out |= 1 << (31 - (x0 + dx));
                    }
                }
                rows[(y0 + dy) as usize] = out;
            }
            self.glyphs.insert(chr, Glyph { rows });
        }
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
    fn claude_code_markers_come_from_unifont() {
        for src in [SPLEEN_16X32, SPLEEN_12X24] {
            let f = Font::from_bdf(src);
            for ch in ['\u{23FA}', '\u{273B}', '\u{23BF}', '\u{2713}', '\u{2801}', '\u{2026}', '\u{00E9}'] {
                assert!(f.has(ch), "{ch:?} missing at {}x{}", f.width, f.height);
                let g = f.glyph(ch);
                assert!(g.rows.iter().any(|r| *r != 0), "{ch:?} is blank");
                assert_ne!(g, &f.fallback);
            }
        }
        // 16x32: a halfwidth Unifont glyph is doubled to fill the cell exactly.
        let f = Font::from_bdf(SPLEEN_16X32);
        let rows = render(&f, '\u{2026}');
        assert!(rows[0].len() == 16 && rows.iter().any(|r| r.contains("##")));
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
    fn families_load_every_size_with_box_drawing() {
        for fam in Family::ALL {
            for i in 0..fam.size_count() {
                let f = fam.load(i);
                assert_eq!((f.width, f.height), fam.cell_size(i), "{fam:?} step {i}");
                assert!(f.has('│') && f.has('A') && f.has('\u{23FA}'), "{fam:?} step {i}");
            }
        }
        assert_eq!(Family::Terminus.cell_size(4), (16, 32));
        assert_eq!(Family::Spleen.cell_size(3), (24, 48));
    }

    #[test]
    fn spleen_12x24_parses_too() {
        let f = Font::from_bdf(SPLEEN_12X24);
        assert_eq!((f.width, f.height), (12, 24));
        assert!(f.has('│'));
    }
}
