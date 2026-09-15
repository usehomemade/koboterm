//! Line icons in the style of Nickel's toolbar, drawn around a centre point.

use px::{Face, PxCanvas, PxRect};

pub fn sun(c: &mut dyn PxCanvas, cx: i32, cy: i32, r: i32, thick: i32, gray: u8) {
    c.circle(cx, cy, r, thick, gray);
    let r0 = r + 5;
    let r1 = r + 5 + (r * 3 / 4).max(4);
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let (s, co) = a.sin_cos();
        c.line(cx + (co * r0 as f32) as i32, cy + (s * r0 as f32) as i32, cx + (co * r1 as f32) as i32, cy + (s * r1 as f32) as i32, thick, gray);
    }
}

pub fn moon(c: &mut dyn PxCanvas, cx: i32, cy: i32, r: i32, thick: i32, gray: u8) {
    c.circle(cx, cy, r, thick, gray);
    // Bite out of the upper right with the background colour.
    c.circle(cx + r * 2 / 3, cy - r * 2 / 3, r, r, 0xFF);
    c.circle(cx + r * 2 / 3, cy - r * 2 / 3, r, thick, 0xFF);
}

pub fn gear(c: &mut dyn PxCanvas, cx: i32, cy: i32, r: i32, thick: i32, gray: u8) {
    c.circle(cx, cy, r, thick, gray);
    c.circle(cx, cy, (r / 2).max(3), thick, gray);
    for k in 0..8 {
        let a = k as f32 * std::f32::consts::FRAC_PI_4;
        let (s, co) = a.sin_cos();
        c.line(cx + (co * (r - 1) as f32) as i32, cy + (s * (r - 1) as f32) as i32, cx + (co * (r + 6) as f32) as i32, cy + (s * (r + 6) as f32) as i32, thick + 1, gray);
    }
}

pub fn dots(c: &mut dyn PxCanvas, cx: i32, cy: i32, r: i32, gap: i32, gray: u8) {
    for k in -1..=1 {
        c.circle(cx + k * gap, cy, r, r, gray);
    }
}

pub fn back_arrow(c: &mut dyn PxCanvas, x: i32, cy: i32, len: i32, thick: i32, gray: u8) {
    c.line(x, cy, x + len, cy, thick, gray);
    c.line(x, cy, x + len / 3, cy - len / 3, thick, gray);
    c.line(x, cy, x + len / 3, cy + len / 3, thick, gray);
}

pub fn keyboard(c: &mut dyn PxCanvas, cx: i32, cy: i32, w: i32, h: i32, thick: i32, gray: u8) {
    let r = PxRect::new(cx - w / 2, cy - h / 2, w, h);
    c.rect_outline(r, thick, gray);
    let d = 3;
    for row in 0..2 {
        for col in 0..5 {
            let x = r.x + 7 + col * (w - 14) / 4;
            let y = r.y + 7 + row * (h - 14) / 2;
            c.fill(PxRect::new(x - 1, y - 1, d, d), gray);
        }
    }
    c.fill(PxRect::new(r.x + 10, r.bottom() - 8, w - 20, 3), gray);
}

pub fn battery(c: &mut dyn PxCanvas, x: i32, y: i32, w: i32, h: i32, pct: Option<u8>, charging: bool, gray: u8) {
    c.rect_outline(PxRect::new(x, y, w, h), 2, gray);
    c.fill(PxRect::new(x + w, y + h / 4, 4, h / 2), gray);
    if let Some(p) = pct {
        let inner = PxRect::new(x, y, w, h).inset(4);
        let fw = (inner.w * p as i32 / 100).max(if p > 0 { 2 } else { 0 });
        c.fill(PxRect::new(inner.x, inner.y, fw, inner.h), gray);
    }
    if charging {
        let (cx, cy) = (x + w / 2, y + h / 2);
        c.line(cx + 3, cy - h / 2 + 2, cx - 3, cy + 1, 2, 0xFF);
        c.line(cx - 3, cy + 1, cx + 3, cy + 1, 2, 0xFF);
        c.line(cx + 3, cy + 1, cx - 3, cy + h / 2 - 2, 2, 0xFF);
    }
}

/// "Aa" in a filled disc, as on Nickel's toolbar.
pub fn aa_badge(c: &mut dyn PxCanvas, face: &Face, cx: i32, cy: i32, r: i32, filled: bool) {
    if filled {
        c.circle(cx, cy, r, r, 0x00);
    }
    let size = r as f32 * 1.05;
    let w = face.width(size, "Aa");
    let asc = face.ascent(size);
    c.text(face, size, cx - w / 2, cy + asc / 2 - 2, "Aa", if filled { 0xFF } else { 0x00 });
}

/// Small "A" and large "A", the ends of Nickel's font size slider.
pub fn text_size_marks(c: &mut dyn PxCanvas, face: &Face, x_small: i32, x_large: i32, cy: i32) {
    let (s, l) = (22.0, 34.0);
    c.text(face, s, x_small - face.width(s, "A") / 2, cy + face.ascent(s) / 2, "A", 0x00);
    c.text(face, l, x_large - face.width(l, "A") / 2, cy + face.ascent(l) / 2, "A", 0x00);
}
