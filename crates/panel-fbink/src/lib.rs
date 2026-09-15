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

    pub struct FbinkPanel {
        fd: core::ffi::c_int,
        cfg: fb::FBInkConfig,
        buf: *mut u8,
        buf_len: usize,
        stride: usize,
        bpp: u32,
        inverted_gray: bool,
        font: Font,
        geo: Geometry,
        x0: u32,
        y0: u32,
        margin: u32,
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
                let (cols, rows, x0, y0) = Self::layout(&font, margin, st.view_width, st.view_height, st.view_hori_origin as u32, st.view_vert_origin as u32);
                Ok(FbinkPanel {
                    fd,
                    cfg,
                    buf,
                    buf_len,
                    stride: st.scanline_stride as usize,
                    bpp: st.bpp,
                    inverted_gray: st.inverted_grayscale,
                    font,
                    geo: Geometry { cols, rows },
                    x0,
                    y0,
                    margin,
                    view_origin: (st.view_hori_origin as u32, st.view_vert_origin as u32),
                    device_name: cstr(&st.device_name),
                    view: (st.view_width, st.view_height),
                    refreshes: [0; 3],
                    touch_swap_axes: st.touch_swap_axes,
                    touch_mirror_x: st.touch_mirror_x,
                    touch_mirror_y: st.touch_mirror_y,
                })
            }
        }

        fn layout(font: &Font, margin: u32, vw: u32, vh: u32, ox: u32, oy: u32) -> (u16, u16, u32, u32) {
            let (fw, fh) = (font.width as u32, font.height as u32);
            let cols = (vw.saturating_sub(2 * margin) / fw) as u16;
            let rows = (vh.saturating_sub(2 * margin) / fh) as u16;
            let x0 = ox + (vw - cols as u32 * fw) / 2;
            let y0 = oy + (vh - rows as u32 * fh) / 2;
            (cols, rows, x0, y0)
        }

        /// Switch font (text size) and recompute the grid. Screen content is not redrawn.
        pub fn set_font(&mut self, font: Font, margin: u32) {
            let (cols, rows, x0, y0) = Self::layout(&font, margin, self.view.0, self.view.1, self.view_origin.0, self.view_origin.1);
            self.font = font;
            self.margin = margin;
            self.geo = Geometry { cols, rows };
            self.x0 = x0;
            self.y0 = y0;
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

        pub fn font(&self) -> &Font {
            &self.font
        }

        /// Copy of the whole framebuffer, to hand the screen back on exit.
        pub fn save_screen(&self) -> Vec<u8> {
            unsafe { std::slice::from_raw_parts(self.buf, self.buf_len).to_vec() }
        }

        pub fn restore_screen(&mut self, saved: &[u8]) {
            let n = saved.len().min(self.buf_len);
            unsafe { std::ptr::copy_nonoverlapping(saved.as_ptr(), self.buf, n) };
            let (cols, rows) = (self.geo.cols, self.geo.rows);
            // Refresh the full view, not just the cell grid, so margins are covered too.
            let mut cfg = self.cfg;
            cfg.wfm_mode = fb::WFM_MODE_INDEX_E_WFM_GC16;
            cfg.is_flashing = true;
            unsafe {
                fb::fbink_refresh(self.fd, 0, 0, self.view.0, self.view.1, &cfg);
                fb::fbink_wait_for_complete(self.fd, fb::fbink_get_last_marker());
            }
            let _ = (cols, rows);
        }

        /// Cell under a screen pixel, if inside the grid.
        pub fn cell_at(&self, x: i32, y: i32) -> Option<(u16, u16)> {
            let (fw, fh) = (self.font.width as i32, self.font.height as i32);
            let cx = (x - self.x0 as i32) / fw;
            let cy = (y - self.y0 as i32) / fh;
            if x < self.x0 as i32 || y < self.y0 as i32 || cx >= self.geo.cols as i32 || cy >= self.geo.rows as i32 {
                None
            } else {
                Some((cx as u16, cy as u16))
            }
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
            let (fw, fh) = (self.font.width as u32, self.font.height as u32);
            (self.y0 + r.row as u32 * fh, self.x0 + r.col as u32 * fw, r.cols as u32 * fw, r.rows as u32 * fh)
        }
    }

    impl Panel for FbinkPanel {
        fn geometry(&self) -> Geometry {
            self.geo
        }

        fn draw(&mut self, col: u16, row: u16, cell: &Cell) {
            let (fw, fh) = (self.font.width as u32, self.font.height as u32);
            let (mut fg, mut bg) = (0x00u8, 0xFFu8);
            if cell.dim {
                fg = 0x60;
            }
            if cell.inverse {
                core::mem::swap(&mut fg, &mut bg);
            }
            let rows: Vec<u32> = if cell.ch == '\0' {
                vec![0; fh as usize]
            } else {
                let g = &self.font.glyph(cell.ch).rows;
                if cell.bold { g.iter().map(|r| r | (r >> 1)).collect() } else { g.clone() }
            };
            let px = self.x0 + col as u32 * fw;
            let py = self.y0 + row as u32 * fh;
            for (dy, bits) in rows.iter().enumerate() {
                let underline = cell.underline && dy as u32 >= fh - 2;
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

    impl Drop for FbinkPanel {
        fn drop(&mut self) {
            unsafe {
                fb::fbink_close(self.fd);
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use imp::FbinkPanel;
