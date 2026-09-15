//! On-screen keyboard: 5 rows of 13 slots, each key 5 cells wide and 3 tall
//! (about 8 mm on a 300 dpi panel). Modifiers are one-shot.

use crate::draw;
use panel::{CellRect, Panel, Waveform};

pub const OSK_ROWS: u16 = 15;
const SLOT_W: u16 = 5;
const KEY_H: u16 = 3;
const SLOTS: u16 = 13;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char, char), // normal, shifted
    Esc,
    Tab,
    Enter,
    Backspace,
    Space,
    Up,
    Down,
    Left,
    Right,
    Shift,
    Ctrl,
    Alt,
    Hide,
    Home,
}

#[derive(Clone, Copy, Debug)]
struct Slot {
    key: Key,
    span: u16,
}

const fn c(a: char, b: char) -> Slot {
    Slot { key: Key::Char(a, b), span: 1 }
}
const fn k(key: Key) -> Slot {
    Slot { key, span: 1 }
}

const ROWS: [&[Slot]; 5] = [
    &[c('`', '~'), c('1', '!'), c('2', '@'), c('3', '#'), c('4', '$'), c('5', '%'), c('6', '^'), c('7', '&'), c('8', '*'), c('9', '('), c('0', ')'), c('-', '_'), c('=', '+')],
    &[c('q', 'Q'), c('w', 'W'), c('e', 'E'), c('r', 'R'), c('t', 'T'), c('y', 'Y'), c('u', 'U'), c('i', 'I'), c('o', 'O'), c('p', 'P'), c('[', '{'), c(']', '}'), k(Key::Backspace)],
    &[c('a', 'A'), c('s', 'S'), c('d', 'D'), c('f', 'F'), c('g', 'G'), c('h', 'H'), c('j', 'J'), c('k', 'K'), c('l', 'L'), c(';', ':'), c('\'', '"'), c('\\', '|'), k(Key::Enter)],
    &[k(Key::Shift), c('z', 'Z'), c('x', 'X'), c('c', 'C'), c('v', 'V'), c('b', 'B'), c('n', 'N'), c('m', 'M'), c(',', '<'), c('.', '>'), c('/', '?'), k(Key::Shift), k(Key::Up)],
    &[k(Key::Esc), k(Key::Tab), k(Key::Ctrl), k(Key::Alt), Slot { key: Key::Space, span: 4 }, k(Key::Left), k(Key::Down), k(Key::Right), k(Key::Hide), k(Key::Home)],
];

#[derive(Clone, Copy, Debug)]
struct Placed {
    key: Key,
    rect: CellRect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OskAction {
    Send(&'static [u8]),
    SendChar(char),
    Hide,
    Home,
    ModifierChanged,
    Nothing,
}

pub struct Osk {
    keys: Vec<Placed>,
    pub area: CellRect,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

impl Osk {
    pub fn new(cols: u16, top_row: u16) -> Self {
        let width = SLOTS * SLOT_W;
        let margin = cols.saturating_sub(width) / 2;
        let mut keys = Vec::new();
        for (ri, row) in ROWS.iter().enumerate() {
            let mut slot = 0u16;
            for s in row.iter() {
                keys.push(Placed {
                    key: s.key,
                    rect: CellRect { col: margin + slot * SLOT_W, row: top_row + ri as u16 * KEY_H, cols: s.span * SLOT_W, rows: KEY_H },
                });
                slot += s.span;
            }
        }
        Osk { keys, area: CellRect { col: 0, row: top_row, cols, rows: OSK_ROWS }, shift: false, ctrl: false, alt: false }
    }

    fn label(&self, key: Key) -> String {
        match key {
            Key::Char(a, b) => (if self.shift { b } else { a }).to_string(),
            Key::Esc => "Esc".into(),
            Key::Tab => "Tab".into(),
            Key::Enter => "Ent".into(),
            Key::Backspace => "Bks".into(),
            Key::Space => "space".into(),
            Key::Up => "↑".into(),
            Key::Down => "↓".into(),
            Key::Left => "←".into(),
            Key::Right => "→".into(),
            Key::Shift => "Sft".into(),
            Key::Ctrl => "Ctl".into(),
            Key::Alt => "Alt".into(),
            Key::Hide => "Kbd".into(),
            Key::Home => "Hom".into(),
        }
    }

    fn is_active(&self, key: Key) -> bool {
        matches!(key, Key::Shift if self.shift) || matches!(key, Key::Ctrl if self.ctrl) || matches!(key, Key::Alt if self.alt)
    }

    fn draw_key(&self, panel: &mut dyn Panel, i: usize, pressed: bool) {
        let p = self.keys[i];
        draw::button(panel, p.rect, &self.label(p.key), pressed || self.is_active(p.key));
    }

    /// Draw every key and refresh the keyboard area once.
    pub fn draw(&self, panel: &mut dyn Panel) {
        draw::fill(panel, self.area, ' ', false);
        for i in 0..self.keys.len() {
            self.draw_key(panel, i, false);
        }
        panel.refresh(self.area, Waveform::Partial);
    }

    /// Redraw only the modifier keys (after a one-shot modifier fired or was set).
    pub fn draw_modifiers(&self, panel: &mut dyn Panel) {
        for (i, p) in self.keys.iter().enumerate() {
            if matches!(p.key, Key::Shift | Key::Ctrl | Key::Alt) {
                self.draw_key(panel, i, false);
                panel.refresh(p.rect, Waveform::Fast);
            }
        }
    }

    pub fn hit(&self, col: u16, row: u16) -> Option<usize> {
        self.keys.iter().position(|p| p.rect.contains(col, row))
    }

    /// Visual feedback: invert the key (call `release` afterwards).
    pub fn flash(&self, panel: &mut dyn Panel, i: usize) {
        self.draw_key(panel, i, true);
        panel.refresh(self.keys[i].rect, Waveform::Fast);
    }

    pub fn release(&self, panel: &mut dyn Panel, i: usize) {
        self.draw_key(panel, i, false);
        panel.refresh(self.keys[i].rect, Waveform::Fast);
    }

    pub fn press(&mut self, i: usize) -> OskAction {
        let key = self.keys[i].key;
        let action = match key {
            Key::Shift => {
                self.shift = !self.shift;
                return OskAction::ModifierChanged;
            }
            Key::Ctrl => {
                self.ctrl = !self.ctrl;
                return OskAction::ModifierChanged;
            }
            Key::Alt => {
                self.alt = !self.alt;
                return OskAction::ModifierChanged;
            }
            Key::Hide => return OskAction::Hide,
            Key::Home => return OskAction::Home,
            Key::Char(a, b) => OskAction::SendChar(if self.shift { b } else { a }),
            Key::Space => OskAction::SendChar(' '),
            Key::Esc => OskAction::Send(b"\x1b"),
            Key::Tab => OskAction::Send(b"\t"),
            Key::Enter => OskAction::Send(b"\r"),
            Key::Backspace => OskAction::Send(b"\x7f"),
            Key::Up => OskAction::Send(b"\x1b[A"),
            Key::Down => OskAction::Send(b"\x1b[B"),
            Key::Right => OskAction::Send(b"\x1b[C"),
            Key::Left => OskAction::Send(b"\x1b[D"),
        };
        action
    }

    /// Bytes to send for an action, applying and clearing one-shot modifiers.
    pub fn bytes_for(&mut self, action: OskAction) -> Vec<u8> {
        let mut out = Vec::new();
        match action {
            OskAction::SendChar(ch) => {
                if self.alt {
                    out.push(0x1b);
                }
                if self.ctrl {
                    let c = ch.to_ascii_lowercase() as u32;
                    if (0x40..=0x7f).contains(&c) {
                        out.push((c & 0x1f) as u8);
                    } else {
                        let mut b = [0u8; 4];
                        out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                    }
                } else {
                    let mut b = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut b).as_bytes());
                }
            }
            OskAction::Send(seq) => {
                if self.alt {
                    out.push(0x1b);
                }
                out.extend_from_slice(seq);
            }
            _ => {}
        }
        self.shift = false;
        self.ctrl = false;
        self.alt = false;
        out
    }

    pub fn modifiers_active(&self) -> bool {
        self.shift || self.ctrl || self.alt
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel::FakePanel;

    #[test]
    fn layout_fits_67_columns_and_15_rows() {
        let o = Osk::new(67, 30);
        for p in &o.keys {
            assert!(p.rect.col + p.rect.cols <= 67, "{p:?}");
            assert!(p.rect.row >= 30 && p.rect.row + p.rect.rows <= 45, "{p:?}");
        }
        assert_eq!(o.keys.len(), 13 * 4 + 10);
    }

    #[test]
    fn draw_labels_land_in_the_grid() {
        let mut fp = FakePanel::new(67, 45);
        let o = Osk::new(67, 30);
        o.draw(&mut fp);
        let all: String = (30..45).map(|r| fp.row_text(r)).collect::<Vec<_>>().join("\n");
        for l in ["q", "Ent", "Bks", "space", "Sft", "Ctl", "Esc", "Hom"] {
            assert!(all.contains(l), "missing {l}\n{all}");
        }
        assert_eq!(fp.total_refreshes(), 1, "one refresh for the whole keyboard");
    }

    #[test]
    fn tapping_q_then_shift_q_then_ctrl_c() {
        let mut o = Osk::new(67, 30);
        let q = o.keys.iter().position(|p| p.key == Key::Char('q', 'Q')).unwrap();
        let (qc, qr) = (o.keys[q].rect.col + 2, o.keys[q].rect.row + 1);
        assert_eq!(o.hit(qc, qr), Some(q));
        let a = o.press(q);
        assert_eq!(o.bytes_for(a), b"q");
        let sh = o.keys.iter().position(|p| p.key == Key::Shift).unwrap();
        assert_eq!(o.press(sh), OskAction::ModifierChanged);
        let a = o.press(q);
        assert_eq!(o.bytes_for(a), b"Q");
        assert!(!o.shift, "shift is one-shot");
        let ctl = o.keys.iter().position(|p| p.key == Key::Ctrl).unwrap();
        o.press(ctl);
        let cc = o.keys.iter().position(|p| p.key == Key::Char('c', 'C')).unwrap();
        let a = o.press(cc);
        assert_eq!(o.bytes_for(a), b"\x03");
    }

    #[test]
    fn arrows_and_enter_send_sequences() {
        let mut o = Osk::new(67, 30);
        let up = o.keys.iter().position(|p| p.key == Key::Up).unwrap();
        let a = o.press(up);
        assert_eq!(o.bytes_for(a), b"\x1b[A");
        let ent = o.keys.iter().position(|p| p.key == Key::Enter).unwrap();
        let a = o.press(ent);
        assert_eq!(o.bytes_for(a), b"\r");
    }
}
