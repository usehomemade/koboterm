//! Touch input. `TouchParser` turns raw evdev packets into screen-space
//! events and is host-tested; `TouchDevice` (Linux) owns the device node and
//! can grab it exclusively so Nickel stops seeing taps while koboterm runs.

#[allow(unused_imports)]
use std::time::Duration;

/// How raw panel coordinates map to screen pixels. Kobo panels are commonly
/// mounted rotated; FBInk reports the quirks per device.
#[derive(Clone, Copy, Debug)]
pub struct TouchMap {
    pub swap_axes: bool,
    pub mirror_x: bool,
    pub mirror_y: bool,
    pub width: i32,
    pub height: i32,
}

impl TouchMap {
    pub fn to_screen(&self, raw_x: i32, raw_y: i32) -> (i32, i32) {
        let (mut x, mut y) = if self.swap_axes { (raw_y, raw_x) } else { (raw_x, raw_y) };
        if self.mirror_x {
            x = self.width - 1 - x;
        }
        if self.mirror_y {
            y = self.height - 1 - y;
        }
        (x.clamp(0, self.width - 1), y.clamp(0, self.height - 1))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TouchEvent {
    Down { x: i32, y: i32 },
    Move { x: i32, y: i32 },
    Up { x: i32, y: i32 },
    /// Emitted together with `Up` when the finger travelled less than `TAP_SLOP` px.
    Tap { x: i32, y: i32 },
}

pub const TAP_SLOP: i32 = 40;

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const EV_ABS: u16 = 3;
const BTN_TOUCH: u16 = 0x14a;
const ABS_MT_POSITION_X: u16 = 0x35;
const ABS_MT_POSITION_Y: u16 = 0x36;
const ABS_MT_TRACKING_ID: u16 = 0x39;

pub struct TouchParser {
    map: TouchMap,
    raw_x: i32,
    raw_y: i32,
    down: bool,
    pending_down: bool,
    pending_up: bool,
    moved: bool,
    start: (i32, i32),
    last: (i32, i32),
}

impl TouchParser {
    pub fn new(map: TouchMap) -> Self {
        TouchParser { map, raw_x: 0, raw_y: 0, down: false, pending_down: false, pending_up: false, moved: false, start: (0, 0), last: (0, 0) }
    }

    /// Feed one 16-byte `input_event` (32-bit ABI: 8 bytes timeval, u16 type,
    /// u16 code, i32 value). Returns events completed by a SYN_REPORT.
    pub fn feed(&mut self, ev: &[u8]) -> Vec<TouchEvent> {
        let mut out = Vec::new();
        if ev.len() < 16 {
            return out;
        }
        let ty = u16::from_le_bytes([ev[8], ev[9]]);
        let code = u16::from_le_bytes([ev[10], ev[11]]);
        let val = i32::from_le_bytes([ev[12], ev[13], ev[14], ev[15]]);
        match (ty, code) {
            (EV_ABS, ABS_MT_POSITION_X) => {
                self.raw_x = val;
                self.moved = true;
            }
            (EV_ABS, ABS_MT_POSITION_Y) => {
                self.raw_y = val;
                self.moved = true;
            }
            (EV_KEY, BTN_TOUCH) => {
                if val != 0 { self.pending_down = true } else { self.pending_up = true }
            }
            (EV_ABS, ABS_MT_TRACKING_ID) => {
                if val >= 0 { self.pending_down = true } else { self.pending_up = true }
            }
            (EV_SYN, 0) => {
                let (x, y) = self.map.to_screen(self.raw_x, self.raw_y);
                if self.pending_down && !self.down {
                    self.down = true;
                    self.start = (x, y);
                    self.last = (x, y);
                    out.push(TouchEvent::Down { x, y });
                } else if self.down && self.moved && !self.pending_up && (x, y) != self.last {
                    self.last = (x, y);
                    out.push(TouchEvent::Move { x, y });
                }
                if self.pending_up && self.down {
                    self.down = false;
                    let (x, y) = if self.moved { (x, y) } else { self.last };
                    out.push(TouchEvent::Up { x, y });
                    let (sx, sy) = self.start;
                    if (x - sx).abs() < TAP_SLOP && (y - sy).abs() < TAP_SLOP {
                        out.push(TouchEvent::Tap { x, y });
                    }
                }
                self.pending_down = false;
                self.pending_up = false;
                self.moved = false;
            }
            _ => {}
        }
        out
    }
}

#[cfg(target_os = "linux")]
mod device {
    use super::*;
    use anyhow::{Context, Result};
    use std::os::unix::io::{AsRawFd, RawFd};

    pub struct TouchDevice {
        file: std::fs::File,
        parser: TouchParser,
        grabbed: bool,
    }

    impl TouchDevice {
        pub fn open(path: &str, map: TouchMap) -> Result<Self> {
            use std::os::unix::fs::OpenOptionsExt;
            let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(path).with_context(|| path.to_string())?;
            Ok(TouchDevice { file, parser: TouchParser::new(map), grabbed: false })
        }

        fn fd(&self) -> RawFd {
            self.file.as_raw_fd()
        }

        /// Exclusive access: other readers (Nickel) stop receiving events.
        pub fn grab(&mut self, on: bool) -> Result<()> {
            const EVIOCGRAB: libc::c_ulong = 0x4004_4590; // _IOW('E', 0x90, int)
            let v: libc::c_int = if on { 1 } else { 0 };
            if unsafe { libc::ioctl(self.fd(), EVIOCGRAB as _, v) } < 0 {
                return Err(std::io::Error::last_os_error()).context("EVIOCGRAB");
            }
            self.grabbed = on;
            Ok(())
        }

        /// Wait up to `timeout` and return every event that became ready.
        pub fn poll(&mut self, timeout: Duration) -> Vec<TouchEvent> {
            let mut out = Vec::new();
            let mut pfd = libc::pollfd { fd: self.fd(), events: libc::POLLIN, revents: 0 };
            let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
            if unsafe { libc::poll(&mut pfd, 1, ms) } <= 0 {
                return out;
            }
            let mut buf = [0u8; 16 * 64];
            loop {
                let n = unsafe { libc::read(self.fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
                if n <= 0 {
                    break;
                }
                for ev in buf[..n as usize].chunks_exact(16) {
                    out.extend(self.parser.feed(ev));
                }
                if (n as usize) < buf.len() {
                    break;
                }
            }
            out
        }
    }

    impl Drop for TouchDevice {
        fn drop(&mut self) {
            if self.grabbed {
                let _ = self.grab(false);
            }
        }
    }
}

#[cfg(target_os = "linux")]
pub use device::TouchDevice;

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(ty: u16, code: u16, val: i32) -> [u8; 16] {
        let mut b = [0u8; 16];
        b[8..10].copy_from_slice(&ty.to_le_bytes());
        b[10..12].copy_from_slice(&code.to_le_bytes());
        b[12..16].copy_from_slice(&val.to_le_bytes());
        b
    }

    /// Clara BW: swapped axes, mirrored x. Raw (106, 988) was a top-left tap.
    fn clara() -> TouchMap {
        TouchMap { swap_axes: true, mirror_x: true, mirror_y: false, width: 1072, height: 1448 }
    }

    #[test]
    fn clara_mapping_matches_recorded_taps() {
        let m = clara();
        assert_eq!(m.to_screen(106, 988), (83, 106)); // top-left
        assert_eq!(m.to_screen(1158, 53), (1018, 1158)); // bottom-right
        assert_eq!(m.to_screen(708, 510), (561, 708)); // centre
    }

    #[test]
    fn a_tap_yields_down_up_tap() {
        let mut p = TouchParser::new(clara());
        let mut out = Vec::new();
        for e in [ev(1, 0x14a, 1), ev(3, 0x39, 0), ev(3, 0x35, 708), ev(3, 0x36, 510), ev(0, 0, 0)] {
            out.extend(p.feed(&e));
        }
        assert_eq!(out, vec![TouchEvent::Down { x: 561, y: 708 }]);
        out.clear();
        for e in [ev(3, 0x35, 709), ev(3, 0x36, 511), ev(0, 0, 0)] {
            out.extend(p.feed(&e));
        }
        assert_eq!(out, vec![TouchEvent::Move { x: 560, y: 709 }]);
        out.clear();
        for e in [ev(1, 0x14a, 0), ev(3, 0x39, -1), ev(0, 0, 0)] {
            out.extend(p.feed(&e));
        }
        assert_eq!(out, vec![TouchEvent::Up { x: 560, y: 709 }, TouchEvent::Tap { x: 560, y: 709 }]);
    }

    #[test]
    fn a_swipe_is_not_a_tap() {
        let mut p = TouchParser::new(clara());
        let mut out = Vec::new();
        for e in [ev(1, 0x14a, 1), ev(3, 0x35, 100), ev(3, 0x36, 100), ev(0, 0, 0), ev(3, 0x35, 600), ev(0, 0, 0), ev(1, 0x14a, 0), ev(0, 0, 0)] {
            out.extend(p.feed(&e));
        }
        assert!(out.iter().any(|e| matches!(e, TouchEvent::Move { .. })));
        assert!(!out.iter().any(|e| matches!(e, TouchEvent::Tap { .. })), "{out:?}");
    }
}
