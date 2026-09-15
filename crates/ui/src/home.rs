//! Home screen: saved machines, an add form, the device's public key.

use crate::draw;
use anyhow::{Context, Result};
use panel::{CellRect, Panel, Waveform};
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostEntry {
    pub name: String,
    /// `user@host[:port]`
    pub spec: String,
    /// Remote command instead of a login shell, e.g. `tmux new -A -s kobo`.
    pub command: Option<String>,
}

impl HostEntry {
    /// The tmux binary this entry uses (first word of its command), if any.
    pub fn tmux_bin(&self) -> Option<&str> {
        let c = self.command.as_deref()?;
        let first = c.split_whitespace().next()?;
        if first.ends_with("tmux") { Some(first) } else { None }
    }

    /// Same entry but attaching to tmux session `name` (`-s NAME` replaced).
    pub fn with_session(&self, name: &str) -> HostEntry {
        let mut e = self.clone();
        if let Some(c) = &self.command {
            let mut words: Vec<String> = c.split(' ').map(|w| w.to_string()).collect();
            let mut i = 0;
            while i + 1 < words.len() {
                if words[i] == "-s" {
                    words[i + 1] = name.to_string();
                    break;
                }
                i += 1;
            }
            e.command = Some(words.join(" "));
        }
        e
    }

    /// File format: one entry per line, tab separated: name, spec, command.
    pub fn load(path: &Path) -> Result<Vec<HostEntry>> {
        let Ok(s) = std::fs::read_to_string(path) else { return Ok(Vec::new()) };
        Ok(s.lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
            .filter_map(|l| {
                let mut it = l.split('\t');
                let name = it.next()?.trim().to_string();
                let spec = it.next()?.trim().to_string();
                let command = it.next().map(|c| c.trim().to_string()).filter(|c| !c.is_empty());
                Some(HostEntry { name, spec, command })
            })
            .collect())
    }

    pub fn save(path: &Path, hosts: &[HostEntry]) -> Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let mut s = String::from("# koboterm machines: name<TAB>user@host[:port]<TAB>command\n");
        for h in hosts {
            s.push_str(&format!("{}\t{}\t{}\n", h.name.replace('\t', " "), h.spec.replace('\t', " "), h.command.clone().unwrap_or_default()));
        }
        std::fs::write(path, s).with_context(|| path.display().to_string())
    }
}

/// "kobo", then "kobo-2", "kobo-3", ... skipping names already in use.
pub fn next_session_name(existing: &[String]) -> String {
    if !existing.iter().any(|s| s == "kobo") {
        return "kobo".into();
    }
    (2..).map(|n| format!("kobo-{n}")).find(|n| !existing.contains(n)).unwrap()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextSize {
    Small,
    Medium,
    Large,
}

impl TextSize {
    pub fn parse(s: &str) -> TextSize {
        match s {
            "small" => TextSize::Small,
            "large" => TextSize::Large,
            _ => TextSize::Medium,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            TextSize::Small => "small",
            TextSize::Medium => "medium",
            TextSize::Large => "large",
        }
    }
}

/// tmux sessions known for a machine, filled in asynchronously.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Sessions {
    #[default]
    Unknown,
    Fetching,
    /// Names of live tmux sessions on the machine (may be empty).
    Known(Vec<String>),
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HomeAction {
    /// Machine header: connect with its default command.
    Connect(usize),
    /// A listed tmux session on machine `i`.
    ConnectSession(usize, String),
    /// "+ new session" on machine `i`.
    NewSession(usize),
    Add,
    Quit,
    Text,
    Brightness,
    Nothing,
}

const ENTRY_H: u16 = 3;
const LIST_TOP: u16 = 5;

pub struct Home {
    cols: u16,
    rows: u16,
    n: usize,
    /// Per machine: header rect, then one rect per sub-row (sessions, then "+ new").
    blocks: Vec<(CellRect, Vec<(CellRect, SubRow)>)>,
    add: CellRect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SubRow {
    Session(String),
    New,
    Note(String),
}

impl Home {
    pub fn new(cols: u16, rows: u16) -> Self {
        Home { cols, rows, n: 0, blocks: Vec::new(), add: CellRect { col: 1, row: LIST_TOP, cols: cols - 2, rows: ENTRY_H } }
    }

    /// Lay out machines with their session rows; call before drawing/hit-testing.
    pub fn layout(&mut self, hosts: &[HostEntry], sessions: &[Sessions]) {
        self.n = hosts.len();
        self.blocks.clear();
        let mut row = LIST_TOP;
        for (i, h) in hosts.iter().enumerate() {
            let header = CellRect { col: 1, row, cols: self.cols - 2, rows: ENTRY_H };
            row += ENTRY_H;
            let mut subs = Vec::new();
            let uses_tmux = h.command.as_deref().map(|c| c.contains("tmux")).unwrap_or(false);
            if uses_tmux {
                let mut push = |sr: SubRow, row: &mut u16| {
                    subs.push((CellRect { col: 1, row: *row, cols: self.cols - 2, rows: 1 }, sr));
                    *row += 1;
                };
                match sessions.get(i).cloned().unwrap_or_default() {
                    Sessions::Unknown | Sessions::Fetching => push(SubRow::Note("looking for tmux sessions...".into()), &mut row),
                    Sessions::Unavailable => push(SubRow::Note("(machine not reachable)".into()), &mut row),
                    Sessions::Known(names) => {
                        for n in names {
                            push(SubRow::Session(n), &mut row);
                        }
                    }
                }
                if !matches!(sessions.get(i), Some(Sessions::Unavailable)) {
                    push(SubRow::New, &mut row);
                }
                row += 1;
            }
            self.blocks.push((header, subs));
        }
        self.add = CellRect { col: 1, row, cols: self.cols - 2, rows: ENTRY_H };
    }

    fn add_rect(&self) -> CellRect {
        self.add
    }

    fn quit_rect(&self) -> CellRect {
        CellRect { col: self.cols - 9, row: self.rows - 4, cols: 8, rows: 3 }
    }

    fn text_rect(&self) -> CellRect {
        CellRect { col: self.cols - 13, row: 0, cols: 6, rows: 3 }
    }

    fn brightness_rect(&self) -> CellRect {
        CellRect { col: self.cols - 6, row: 0, cols: 6, rows: 3 }
    }

    pub fn draw(&mut self, panel: &mut dyn Panel, hosts: &[HostEntry], sessions: &[Sessions], pubkey: &str, pair_url: &str, status: &str) {
        self.layout(hosts, sessions);
        let full = CellRect { col: 0, row: 0, cols: self.cols, rows: self.rows };
        draw::fill(panel, full, ' ', false);
        draw::text(panel, 1, 1, "koboterm", true, false);
        let status: String = status.chars().take(self.cols.saturating_sub(28) as usize).collect();
        draw::text(panel, 11, 1, &status, false, false);
        draw::button(panel, self.text_rect(), "Aa", false);
        draw::button(panel, self.brightness_rect(), "☼", false);
        draw::text(panel, 1, 4, "Machines", false, false);
        for (i, h) in hosts.iter().enumerate() {
            let (r, subs) = &self.blocks[i];
            draw::frame(panel, *r, false);
            draw::text(panel, r.col + 2, r.row + 1, &h.name, true, false);
            let spec_col = r.col + r.cols - 2 - h.spec.chars().count() as u16;
            draw::text(panel, spec_col.max(r.col + 3 + h.name.chars().count() as u16), r.row + 1, &h.spec, false, false);
            for (sr, sub) in subs {
                match sub {
                    SubRow::Session(n) => draw::text(panel, sr.col + 4, sr.row, &format!("▸ tmux: {n}"), false, false),
                    SubRow::New => draw::text(panel, sr.col + 4, sr.row, "+ new tmux session", false, false),
                    SubRow::Note(t) => draw::text(panel, sr.col + 4, sr.row, t, false, false),
                }
            }
        }
        draw::button(panel, self.add_rect(), "+ add a machine", false);
        let width = (self.cols - 2) as usize;
        let wrap = |s: &str| -> Vec<String> { s.chars().collect::<Vec<_>>().chunks(width).map(|c| c.iter().collect()).collect() };
        let key_lines = wrap(pubkey.trim());
        let curl_lines = wrap(&format!("curl -fsSL {pair_url}/install.sh | sh"));
        let block = 2 + curl_lines.len() + 1 + key_lines.len();
        let mut row = self.rows.saturating_sub(5 + block as u16).max(self.add_rect().row + ENTRY_H + 1);
        draw::text(panel, 1, row, "To add a machine automatically, run this on it:", false, false);
        row += 1;
        for l in &curl_lines {
            draw::text(panel, 1, row, l, true, false);
            row += 1;
        }
        row += 1;
        draw::text(panel, 1, row, "Or add this device's key to ~/.ssh/authorized_keys:", false, false);
        row += 1;
        for l in key_lines.iter().take(3) {
            draw::text(panel, 1, row, l, false, false);
            row += 1;
        }
        draw::button(panel, self.quit_rect(), "Quit", false);
        panel.refresh(full, Waveform::Full);
    }

    pub fn hit(&self, col: u16, row: u16) -> HomeAction {
        for (i, (header, subs)) in self.blocks.iter().enumerate() {
            if header.contains(col, row) {
                return HomeAction::Connect(i);
            }
            for (sr, sub) in subs {
                if sr.contains(col, row) {
                    return match sub {
                        SubRow::Session(n) => HomeAction::ConnectSession(i, n.clone()),
                        SubRow::New => HomeAction::NewSession(i),
                        SubRow::Note(_) => HomeAction::Nothing,
                    };
                }
            }
        }
        if self.add_rect().contains(col, row) {
            return HomeAction::Add;
        }
        if self.quit_rect().contains(col, row) {
            return HomeAction::Quit;
        }
        if self.text_rect().contains(col, row) {
            return HomeAction::Text;
        }
        if self.brightness_rect().contains(col, row) {
            return HomeAction::Brightness;
        }
        HomeAction::Nothing
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Name,
    Spec,
    Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormAction {
    Save,
    Cancel,
    Focus(Field),
    Nothing,
}

/// "Add a machine" form. Occupies the rows above the keyboard.
pub struct AddForm {
    cols: u16,
    pub name: String,
    pub spec: String,
    pub command: String,
    pub focus: Field,
    pub error: String,
}

impl AddForm {
    pub fn new(cols: u16) -> Self {
        AddForm { cols, name: String::new(), spec: String::new(), command: String::new(), focus: Field::Name, error: String::new() }
    }

    fn field_rect(&self, f: Field) -> CellRect {
        let row = match f {
            Field::Name => 3,
            Field::Spec => 8,
            Field::Command => 13,
        };
        CellRect { col: 1, row, cols: self.cols - 2, rows: 3 }
    }

    fn save_rect(&self) -> CellRect {
        CellRect { col: 1, row: 18, cols: 12, rows: 3 }
    }

    fn cancel_rect(&self) -> CellRect {
        CellRect { col: 15, row: 18, cols: 12, rows: 3 }
    }

    pub fn draw(&self, panel: &mut dyn Panel, rows_available: u16) {
        let area = CellRect { col: 0, row: 0, cols: self.cols, rows: rows_available };
        draw::fill(panel, area, ' ', false);
        draw::text(panel, 1, 1, "Add a machine", true, false);
        for (f, label, value) in [
            (Field::Name, "Name (e.g. MacBook Pro)", &self.name),
            (Field::Spec, "Address: user@host[:port]", &self.spec),
            (Field::Command, "Command (optional, e.g. tmux new -A -s kobo)", &self.command),
        ] {
            let r = self.field_rect(f);
            let focused = self.focus == f;
            draw::text(panel, r.col, r.row - 1, label, focused, false);
            draw::frame(panel, r, focused);
            let shown: String = value.chars().rev().take((r.cols - 4) as usize).collect::<Vec<_>>().into_iter().rev().collect();
            draw::text(panel, r.col + 2, r.row + 1, &shown, false, false);
            if focused {
                draw::put(panel, r.col + 2 + shown.chars().count() as u16, r.row + 1, '_', true, false);
            }
        }
        draw::button(panel, self.save_rect(), "Save", false);
        draw::button(panel, self.cancel_rect(), "Cancel", false);
        draw::text(panel, 1, 22, &self.error, true, false);
        panel.refresh(area, Waveform::Partial);
    }

    pub fn hit(&self, col: u16, row: u16) -> FormAction {
        for f in [Field::Name, Field::Spec, Field::Command] {
            if self.field_rect(f).contains(col, row) {
                return FormAction::Focus(f);
            }
        }
        if self.save_rect().contains(col, row) {
            return FormAction::Save;
        }
        if self.cancel_rect().contains(col, row) {
            return FormAction::Cancel;
        }
        FormAction::Nothing
    }

    fn field_mut(&mut self) -> &mut String {
        match self.focus {
            Field::Name => &mut self.name,
            Field::Spec => &mut self.spec,
            Field::Command => &mut self.command,
        }
    }

    /// Feed keyboard bytes (from the OSK). Enter advances or saves; Tab advances.
    pub fn input(&mut self, bytes: &[u8]) -> FormAction {
        for &b in bytes {
            match b {
                0x7f | 0x08 => {
                    self.field_mut().pop();
                }
                b'\r' | b'\n' | b'\t' => {
                    self.focus = match self.focus {
                        Field::Name => Field::Spec,
                        Field::Spec => Field::Command,
                        Field::Command => {
                            if b == b'\t' { Field::Name } else { return FormAction::Save }
                        }
                    };
                }
                0x1b => return FormAction::Cancel,
                b if b >= 0x20 => self.field_mut().push(b as char),
                _ => {}
            }
        }
        FormAction::Nothing
    }

    pub fn entry(&self) -> Result<HostEntry> {
        let name = self.name.trim().to_string();
        let spec = self.spec.trim().to_string();
        if name.is_empty() {
            anyhow::bail!("name is required");
        }
        if !spec.contains('@') {
            anyhow::bail!("address must be user@host");
        }
        let command = Some(self.command.trim().to_string()).filter(|c| !c.is_empty());
        Ok(HostEntry { name, spec, command })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use panel::FakePanel;

    #[test]
    fn hosts_round_trip_through_the_file() {
        let dir = std::env::temp_dir().join(format!("koboterm-hosts-{}", std::process::id()));
        let path = dir.join("hosts");
        let hosts = vec![
            HostEntry { name: "MacBook Pro M5".into(), spec: "tunc@192.168.0.199".into(), command: None },
            HostEntry { name: "vps".into(), spec: "tunc@server.wust.co:22".into(), command: Some("tmux new -A -s kobo".into()) },
        ];
        HostEntry::save(&path, &hosts).unwrap();
        assert_eq!(HostEntry::load(&path).unwrap(), hosts);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn home_draws_entries_and_hit_tests_them() {
        let mut fp = FakePanel::new(67, 45);
        let mut h = Home::new(67, 45);
        let hosts = vec![HostEntry { name: "MacBook".into(), spec: "tunc@10.0.0.2".into(), command: None }];
        h.draw(&mut fp, &hosts, &[Sessions::Unknown], "ssh-ed25519 AAAAtest koboterm", "http://10.0.0.9:8080", "");
        assert!((0..45).any(|r| fp.row_text(r).contains("curl -fsSL http://10.0.0.9:8080/install.sh | sh")));
        assert!(fp.row_text(6).contains("MacBook"));
        assert!(fp.row_text(6).contains("tunc@10.0.0.2"));
        assert_eq!(h.hit(10, 6), HomeAction::Connect(0));
        assert_eq!(h.hit(10, 9), HomeAction::Add);
        assert_eq!(h.hit(62, 42), HomeAction::Quit);
        assert_eq!(h.hit(55, 1), HomeAction::Text);
        assert_eq!(h.hit(63, 1), HomeAction::Brightness);
        assert_eq!(h.hit(30, 30), HomeAction::Nothing);
        // Small grid (large text) still lays out without panicking.
        let mut fp2 = FakePanel::new(43, 29);
        let mut h2 = Home::new(43, 29);
        h2.draw(&mut fp2, &hosts, &[Sessions::Unknown], "ssh-ed25519 AAAAtest koboterm", "http://10.0.0.9:8080", "");
        assert!(fp2.row_text(6).contains("MacBook"));
    }

    #[test]
    fn session_substitution_and_naming() {
        let e = HostEntry { name: "MBP".into(), spec: "t@h".into(), command: Some("/opt/homebrew/bin/tmux -u new -A -s kobo \\; set -g mouse on".into()) };
        assert_eq!(e.tmux_bin(), Some("/opt/homebrew/bin/tmux"));
        assert_eq!(e.with_session("work").command.as_deref(), Some("/opt/homebrew/bin/tmux -u new -A -s work \\; set -g mouse on"));
        assert_eq!(next_session_name(&[]), "kobo");
        assert_eq!(next_session_name(&["kobo".into()]), "kobo-2");
        assert_eq!(next_session_name(&["kobo".into(), "kobo-2".into()]), "kobo-3");
        let plain = HostEntry { name: "x".into(), spec: "t@h".into(), command: None };
        assert_eq!(plain.tmux_bin(), None);
    }

    #[test]
    fn tmux_machines_list_their_sessions() {
        let mut fp = FakePanel::new(67, 45);
        let mut h = Home::new(67, 45);
        let hosts = vec![HostEntry { name: "MBP".into(), spec: "tunc@10.0.0.2".into(), command: Some("/opt/homebrew/bin/tmux -u new -A -s kobo".into()) }];
        h.draw(&mut fp, &hosts, &[Sessions::Known(vec!["kobo".into(), "work".into()])], "k", "u", "");
        assert!(fp.row_text(8).contains("tmux: kobo"), "{:?}", fp.row_text(8));
        assert!(fp.row_text(9).contains("tmux: work"));
        assert!(fp.row_text(10).contains("new tmux session"));
        assert_eq!(h.hit(10, 9), HomeAction::ConnectSession(0, "work".into()));
        assert_eq!(h.hit(10, 10), HomeAction::NewSession(0));
        assert_eq!(h.hit(10, 6), HomeAction::Connect(0));
        assert!(fp.row_text(13).contains("add a machine"), "{:?}", fp.row_text(13));
        assert_eq!(fp.count(Waveform::Full), 1);
    }

    #[test]
    fn form_typing_and_validation() {
        let mut f = AddForm::new(67);
        assert_eq!(f.input(b"MacBook\r"), FormAction::Nothing);
        assert_eq!(f.focus, Field::Spec);
        f.input(b"tunc@10.0.0.2\x7f2");
        assert_eq!(f.spec, "tunc@10.0.0.2");
        assert_eq!(f.input(b"\r"), FormAction::Nothing);
        assert_eq!(f.focus, Field::Command);
        assert_eq!(f.input(b"\r"), FormAction::Save);
        let e = f.entry().unwrap();
        assert_eq!(e.name, "MacBook");
        assert_eq!(e.command, None);
        let mut bad = AddForm::new(67);
        bad.input(b"x");
        assert!(bad.entry().is_err());
        let mut fp = FakePanel::new(67, 45);
        f.draw(&mut fp, 29);
        assert!(fp.row_text(1).contains("Add a machine"));
        assert_eq!(f.hit(3, 19), FormAction::Save);
    }
}
