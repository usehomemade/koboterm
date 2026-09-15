//! The real e-ink panel, driven through FBInk. FBInk owns device detection,
//! the framebuffer mapping and the refresh ioctls; this crate owns the cell
//! grid, glyph blitting and the mapping from `Waveform` to a waveform mode.
//! Only compiled for Linux targets; the workspace still builds and tests on a
//! laptop.

#[cfg(target_os = "linux")]
mod imp {
    use anyhow::{bail, Result};
    use fbink_sys as fb;
    use font::Font;
    use panel::{Cell, CellRect, Geometry, Panel, Waveform};
    use px::{PxCanvas, PxRect, PxWave};

    /// A cell grid placed somewhere on the screen with one of the panel's fonts.
    /// The panel draws into whichever region is current; the app switches
    /// regions to mix a large-font terminal with a normal-font keyboard.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Region {
        pub font: usize,
        pub x0: u32,
        pub y0: u32,
        pub cols: u16,
        pub rows: u16,
        pub fw: u32,
        pub fh: u32,
    }

    impl Region {
        pub fn px_height(&self) -> u32 {
            self.rows as u32 * self.fh
        }
        pub fn geometry(&self) -> Geometry {
            Geometry { cols: self.cols, rows: self.rows }
        }
    }

    pub struct FbinkPanel {
        fd: core::ffi::c_int,
        cfg: fb::FBInkConfig,
        buf: *mut u8,
        buf_len: usize,
        stride: usize,
        bpp: u32,
        inverted_gray: bool,
        fonts: Vec<Font>,
        /// Extra blank pixels below each row, per font (line spacing).
        line_gaps: Vec<u32>,
        cur: Region,
        view_origin: (u32, u32),
        pub device_name: String,
        pub view: (u32, u32),
        pub refreshes: [u32; 3],
        pub touch_swap_axes: bool,
        pub touch_mirror_x: bool,
        pub touch_mirror_y: bool,
    }

    fn cstr(a: &[core::ffi::c_char]) -> String {
        unsafe { core::ffi::CStr::from_ptr(a.as_ptr()) }.to_string_lossy().into_owned()
    }

    impl FbinkPanel {
        pub fn open(font: Font) -> Result<Self> {
            Self::open_with_margin(font, 0)
        }

        pub fn open_with_margin(font: Font, margin: u32) -> Result<Self> {
            unsafe {
                let fd = fb::fbink_open();
                if fd < 0 {
                    bail!("fbink_open failed: {}", std::io::Error::last_os_error());
                }
                let mut cfg: fb::FBInkConfig = core::mem::zeroed();
                cfg.is_quiet = true;
                if fb::fbink_init(fd, &cfg) < 0 {
                    bail!("fbink_init failed");
                }
                let mut st: fb::FBInkState = core::mem::zeroed();
                fb::fbink_get_state(&cfg, &mut st);
                let mut buf_len = 0usize;
                let buf = fb::fbink_get_fb_pointer(fd, &mut buf_len);
                if buf.is_null() {
                    bail!("fbink_get_fb_pointer returned null");
                }
                let view = (st.view_width, st.view_height);
                let origin = (st.view_hori_origin as u32, st.view_vert_origin as u32);
                let cur = Self::band(&font, 0, 0, margin, 0, view.1, view, origin);
                Ok(FbinkPanel {
                    fd,
                    cfg,
                    buf,
                    buf_len,
                    stride: st.scanline_stride as usize,
                    bpp: st.bpp,
                    inverted_gray: st.inverted_grayscale,
                    fonts: vec![font],
                    line_gaps: vec![0],
                    cur,
                    view_origin: origin,
                    device_name: cstr(&st.device_name),
                    view,
                    refreshes: [0; 3],
                    touch_swap_axes: st.touch_swap_axes,
                    touch_mirror_x: st.touch_mirror_x,
                    touch_mirror_y: st.touch_mirror_y,
                })
            }
        }

        /// Grid that fits inside the horizontal band [top_px, bottom_px) of the
        /// view, with `margin` px kept free on the left and right; rows are
        /// top-aligned in the band, columns centred.
        fn band(font: &Font, gap: u32, font_idx: usize, margin: u32, top_px: u32, bottom_px: u32, view: (u32, u32), origin: (u32, u32)) -> Region {
            let (fw, fh) = (font.width as u32, font.height as u32 + gap);
            let cols = (view.0.saturating_sub(2 * margin) / fw) as u16;
            let rows = (bottom_px.saturating_sub(top_px) / fh) as u16;
            let x0 = origin.0 + (view.0 - cols as u32 * fw) / 2;
            let y0 = origin.1 + top_px;
            Region { font: font_idx, x0, y0, cols, rows, fw, fh }
        }

        /// Register a font; returns its index for `region`. Index 0 is the font passed to `open`.
        pub fn add_font(&mut self, font: Font) -> usize {
            self.fonts.push(font);
            self.line_gaps.push(0);
            self.fonts.len() - 1
        }

        pub fn replace_font(&mut self, idx: usize, font: Font) {
            self.fonts[idx] = font;
        }

        /// Extra pixels of spacing under every row drawn with font `idx`.
        pub fn set_line_gap(&mut self, idx: usize, gap: u32) {
            self.line_gaps[idx] = gap;
        }

        /// Region covering the band [top_px, bottom_px) of the view with the given font.
        pub fn region(&self, font_idx: usize, margin: u32, top_px: u32, bottom_px: u32) -> Region {
            Self::band(&self.fonts[font_idx], self.line_gaps[font_idx], font_idx, margin, top_px, bottom_px.min(self.view.1), self.view, self.view_origin)
        }

        /// Whole view with `margin` px on every side.
        pub fn region_full(&self, font_idx: usize, margin: u32) -> Region {
            self.region(font_idx, margin, margin, self.view.1.saturating_sub(margin))
        }

        pub fn use_region(&mut self, r: Region) {
            self.cur = r;
        }

        pub fn current_region(&self) -> Region {
            self.cur
        }

        pub fn font_of(&self, r: &Region) -> &Font {
            &self.fonts[r.font]
        }

        /// Cell of `r` under a screen pixel, if inside that region.
        pub fn cell_in(&self, r: &Region, x: i32, y: i32) -> Option<(u16, u16)> {
            let cx = (x - r.x0 as i32) / r.fw as i32;
            let cy = (y - r.y0 as i32) / r.fh as i32;
            if x < r.x0 as i32 || y < r.y0 as i32 || cx >= r.cols as i32 || cy >= r.rows as i32 {
                None
            } else {
                Some((cx as u16, cy as u16))
            }
        }

        /// Blank the whole view (not just the current region) and flash-refresh it.
        pub fn clear_all(&mut self) -> Result<()> {
            self.clear()
        }

        /// White the whole screen with a flashing full refresh.
        pub fn clear(&mut self) -> Result<()> {
            let mut cfg = self.cfg;
            cfg.wfm_mode = fb::WFM_MODE_INDEX_E_WFM_GC16;
            cfg.is_flashing = true;
            let rv = unsafe { fb::fbink_cls(self.fd, &cfg, core::ptr::null(), false) };
            if rv < 0 {
                bail!("fbink_cls failed");
            }
            self.refreshes[2] += 1;
            Ok(())
        }

        pub fn view_origin(&self) -> (u32, u32) {
            self.view_origin
        }

        pub fn font(&self) -> &Font {
            &self.fonts[self.cur.font]
        }

        /// Copy of the whole framebuffer, to hand the screen back on exit.
        pub fn save_screen(&self) -> Vec<u8> {
            unsafe { std::slice::from_raw_parts(self.buf, self.buf_len).to_vec() }
        }

        pub fn restore_screen(&mut self, saved: &[u8]) {
            let n = saved.len().min(self.buf_len);
            unsafe { std::ptr::copy_nonoverlapping(saved.as_ptr(), self.buf, n) };
            // Refresh the full view, not just the cell grid, so margins are covered too.
            let mut cfg = self.cfg;
            cfg.wfm_mode = fb::WFM_MODE_INDEX_E_WFM_GC16;
            cfg.is_flashing = true;
            unsafe {
                fb::fbink_refresh(self.fd, 0, 0, self.view.0, self.view.1, &cfg);
                fb::fbink_wait_for_complete(self.fd, fb::fbink_get_last_marker());
            }
        }

        /// Cell of the current region under a screen pixel.
        pub fn cell_at(&self, x: i32, y: i32) -> Option<(u16, u16)> {
            self.cell_in(&self.cur, x, y)
        }

        #[inline]
        fn put(&mut self, x: u32, y: u32, gray: u8) {
            let off = y as usize * self.stride;
            unsafe {
                match self.bpp {
                    32 => {
                        let p = self.buf.add(off + x as usize * 4);
                        if off + x as usize * 4 + 4 > self.buf_len {
                            return;
                        }
                        *p = gray;
                        *p.add(1) = gray;
                        *p.add(2) = gray;
                        *p.add(3) = 0xFF;
                    }
                    8 => {
                        let p = self.buf.add(off + x as usize);
                        *p = if self.inverted_gray { 0xFF - gray } else { gray };
                    }
                    16 => {
                        let g = gray as u16;
                        let v: u16 = ((g >> 3) << 11) | ((g >> 2) << 5) | (g >> 3);
                        let p = self.buf.add(off + x as usize * 2) as *mut u16;
                        p.write_unaligned(v);
                    }
                    _ => {}
                }
            }
        }

        fn px_rect(&self, r: CellRect) -> (u32, u32, u32, u32) {
            let c = self.cur;
            (c.y0 + r.row as u32 * c.fh, c.x0 + r.col as u32 * c.fw, r.cols as u32 * c.fw, r.rows as u32 * c.fh)
        }
    }

    impl Panel for FbinkPanel {
        fn geometry(&self) -> Geometry {
            self.cur.geometry()
        }

        fn draw(&mut self, col: u16, row: u16, cell: &Cell) {
            let c = self.cur;
            let (fw, fh) = (c.fw, c.fh);
            if col >= c.cols || row >= c.rows {
                return;
            }
            let (mut fg, mut bg) = (0x00u8, 0xFFu8);
            if cell.dim {
                fg = 0x60;
            }
            if cell.inverse {
                core::mem::swap(&mut fg, &mut bg);
            }
            let glyph_h = self.fonts[c.font].height as u32;
            let mut rows: Vec<u32> = if cell.ch == '\0' {
                vec![0; glyph_h as usize]
            } else {
                let g = &self.fonts[c.font].glyph(cell.ch).rows;
                if cell.bold { g.iter().map(|r| r | (r >> 1)).collect() } else { g.clone() }
            };
            rows.resize(fh as usize, 0); // line-spacing gap rows are background
            let px = c.x0 + col as u32 * fw;
            let py = c.y0 + row as u32 * fh;
            for (dy, bits) in rows.iter().enumerate() {
                let underline = cell.underline && dy as u32 >= glyph_h - 2 && (dy as u32) < glyph_h;
                for dx in 0..fw {
                    let on = underline || bits & (1u32 << (31 - dx)) != 0;
                    self.put(px + dx, py + dy as u32, if on { fg } else { bg });
                }
            }
        }

        fn refresh(&mut self, rect: CellRect, wf: Waveform) {
            let (top, left, w, h) = self.px_rect(rect);
            let mut cfg = self.cfg;
            cfg.wfm_mode = match wf {
                Waveform::Fast => fb::WFM_MODE_INDEX_E_WFM_DU,
                Waveform::Partial => fb::WFM_MODE_INDEX_E_WFM_GL16,
                Waveform::Full => fb::WFM_MODE_INDEX_E_WFM_GC16,
            };
            cfg.is_flashing = wf == Waveform::Full;
            unsafe {
                if wf == Waveform::Full {
                    fb::fbink_wait_for_complete(self.fd, fb::fbink_get_last_marker());
                }
                let rv = fb::fbink_refresh(self.fd, top, left, w, h, &cfg);
                if rv < 0 {
                    eprintln!("fbink_refresh({top},{left},{w},{h}) failed: {rv}");
                }
            }
            self.refreshes[match wf {
                Waveform::Fast => 0,
                Waveform::Partial => 1,
                Waveform::Full => 2,
            }] += 1;
        }
    }

    impl PxCanvas for FbinkPanel {
        fn size(&self) -> (i32, i32) {
            (self.view.0 as i32, self.view.1 as i32)
        }

        fn put(&mut self, x: i32, y: i32, gray: u8) {
            if x < 0 || y < 0 || x >= self.view.0 as i32 || y >= self.view.1 as i32 {
                return;
            }
            let (ox, oy) = self.view_origin;
            FbinkPanel::put(self, x as u32 + ox, y as u32 + oy, gray);
        }

        fn get(&self, x: i32, y: i32) -> u8 {
            if x < 0 || y < 0 || x >= self.view.0 as i32 || y >= self.view.1 as i32 {
                return 0xFF;
            }
            let (ox, oy) = self.view_origin;
            let (x, y) = (x as usize + ox as usize, y as usize + oy as usize);
            let off = y * self.stride;
            unsafe {
                match self.bpp {
                    32 => *self.buf.add(off + x * 4),
                    8 => {
                        let v = *self.buf.add(off + x);
                        if self.inverted_gray { 0xFF - v } else { v }
                    }
                    16 => {
                        let v = (self.buf.add(off + x * 2) as *const u16).read_unaligned();
                        ((v >> 11) as u8) << 3
                    }
                    _ => 0xFF,
                }
            }
        }

        fn refresh_px(&mut self, r: PxRect, wave: PxWave) {
            let (ox, oy) = self.view_origin;
            let x = (r.x.max(0) as u32 + ox).min(self.view.0 + ox);
            let y = (r.y.max(0) as u32 + oy).min(self.view.1 + oy);
            let w = (r.w.max(0) as u32).min(self.view.0 + ox - x);
            let h = (r.h.max(0) as u32).min(self.view.1 + oy - y);
            if w == 0 || h == 0 {
                return;
            }
            let mut cfg = self.cfg;
            cfg.wfm_mode = match wave {
                PxWave::Fast => fb::WFM_MODE_INDEX_E_WFM_DU,
                PxWave::Quality => fb::WFM_MODE_INDEX_E_WFM_GL16,
                PxWave::Full => fb::WFM_MODE_INDEX_E_WFM_GC16,
            };
            cfg.is_flashing = wave == PxWave::Full;
            unsafe {
                let rv = fb::fbink_refresh(self.fd, y, x, w, h, &cfg);
                if rv < 0 {
                    eprintln!("fbink_refresh px({y},{x},{w},{h}) failed: {rv}");
                }
            }
            self.refreshes[match wave {
                PxWave::Fast => 0,
                PxWave::Quality => 1,
                PxWave::Full => 2,
            }] += 1;
        }
    }

    impl Drop for FbinkPanel {
        fn drop(&mut self) {
            unsafe {
                fb::fbink_close(self.fd);
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::{FbinkPanel, Region};
