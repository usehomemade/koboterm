//! The interactive app: home screen with saved machines, an on-screen
//! keyboard, and SSH sessions. Nickel is frozen while we run (see nickel.rs);
//! the touch device is grabbed and the screen handed back on exit.

use anyhow::{Context, Result};
use input::{TouchDevice, TouchEvent, TouchMap};
use panel::{CellRect, Panel, Waveform};
use panel_fbink::{FbinkPanel, Region};
use render::{Config, Renderer};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use term::Terminal;
use transport::{SshTarget, SshTransport, Transport};
use ui::{draw, AddForm, FormAction, Home, HomeAction, HostEntry, Osk, OskAction, TextSize};

use crate::pair::PairServer;

const TOUCH_DEV: &str = "/dev/input/event1";
const SWIPE_MIN_PX: i32 = 60;
const UI_FONT: usize = 0;
const TERM_FONT: usize = 1;

fn data_dir() -> PathBuf {
    let onboard = PathBuf::from("/mnt/onboard/.adds/koboterm");
    if onboard.parent().map(|p| p.exists()).unwrap_or(false) {
        onboard
    } else {
        PathBuf::from("koboterm-data")
    }
}

#[derive(Clone, Copy, Debug)]
struct Settings {
    size: TextSize,
    margin: u32,
}

impl Settings {
    fn load(path: &Path) -> Settings {
        let mut s = Settings { size: TextSize::Medium, margin: 12 };
        if let Ok(txt) = std::fs::read_to_string(path) {
            for line in txt.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    match k.trim() {
                        "size" => s.size = TextSize::parse(v.trim()),
                        "margin" => s.margin = v.trim().parse().unwrap_or(12),
                        _ => {}
                    }
                }
            }
        }
        s
    }

    fn save(&self, path: &Path) {
        let _ = std::fs::write(path, format!("size={}\nmargin={}\n", self.size.name(), self.margin));
    }

    fn term_font(&self) -> font::Font {
        match self.size {
            TextSize::Small => font::Font::from_bdf(font::SPLEEN_12X24),
            TextSize::Medium => font::Font::from_bdf(font::SPLEEN_16X32),
            TextSize::Large => font::Font::from_bdf(font::SPLEEN_12X24).scaled_2x(),
        }
    }
}

pub fn run(args: Vec<String>) -> Result<()> {
    let mut nickel_mode = crate::nickel::Mode::Leave;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--nickel" {
            nickel_mode = crate::nickel::Mode::parse(&it.next().unwrap_or_default());
        }
    }
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let key_path = dir.join("id_ed25519");
    if !key_path.exists() {
        eprintln!("generating device key at {}", key_path.display());
        transport::keygen(&key_path, "koboterm")?;
    }
    let pubkey = std::fs::read_to_string(key_path.with_extension("pub")).unwrap_or_default();
    let hosts_path = dir.join("hosts");
    let settings_path = dir.join("settings");
    let mut hosts = HostEntry::load(&hosts_path)?;
    let mut settings = Settings::load(&settings_path);

    // Font 0 is the UI font (home screen, keyboard); font 1 is the terminal's.
    let mut panel = FbinkPanel::open_with_margin(font::Font::from_bdf(font::SPLEEN_16X32), settings.margin)?;
    let term_font_idx = panel.add_font(settings.term_font());
    debug_assert_eq!(term_font_idx, TERM_FONT);
    let saved = stable_screen(&panel);
    let mut paused = crate::nickel::pause(nickel_mode);
    let map = TouchMap {
        swap_axes: panel.touch_swap_axes,
        mirror_x: panel.touch_mirror_x,
        mirror_y: panel.touch_mirror_y,
        width: panel.view.0 as i32,
        height: panel.view.1 as i32,
    };
    let mut touch = TouchDevice::open(TOUCH_DEV, map).context("touch device")?;
    touch.grab(true).context("grab touch")?;
    let pair = match PairServer::start(&pubkey) {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("pairing server unavailable: {e}");
            None
        }
    };

    let ctx = Ctx { hosts_path, settings_path, key_path, pubkey, pair, margin: settings.margin };
    let result = main_loop(&mut panel, &mut touch, &mut hosts, &mut settings, &ctx);

    let _ = touch.grab(false);
    panel.restore_screen(&saved);
    paused.resume();
    result
}

struct Ctx {
    hosts_path: PathBuf,
    settings_path: PathBuf,
    key_path: PathBuf,
    pubkey: String,
    pair: Option<PairServer>,
    margin: u32,
}

/// Snapshot of the framebuffer taken once Nickel has stopped drawing (the
/// launcher's menu is closing when we start). Waits for 1.2 s of no change,
/// at most 8 s.
fn stable_screen(panel: &FbinkPanel) -> Vec<u8> {
    let start = Instant::now();
    let mut last = panel.save_screen();
    let mut stable_since = Instant::now();
    loop {
        std::thread::sleep(Duration::from_millis(300));
        let now = panel.save_screen();
        if now != last {
            last = now;
            stable_since = Instant::now();
        }
        if stable_since.elapsed() >= Duration::from_millis(1200) || start.elapsed() >= Duration::from_secs(8) {
            return last;
        }
    }
}

enum Wait {
    Tap(u16, u16),
    Registered(HostEntry),
    Timeout,
}

fn wait_tap(panel: &FbinkPanel, touch: &mut TouchDevice, timeout: Duration) -> Option<(u16, u16)> {
    match wait_event(panel, touch, None, timeout) {
        Wait::Tap(c, r) => Some((c, r)),
        _ => None,
    }
}

fn wait_event(panel: &FbinkPanel, touch: &mut TouchDevice, pair: Option<&PairServer>, timeout: Duration) -> Wait {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if let Some(e) = pair.and_then(|p| p.try_recv()) {
            return Wait::Registered(e);
        }
        for ev in touch.poll(Duration::from_millis(50)) {
            if let TouchEvent::Tap { x, y } = ev {
                if let Some((c, r)) = panel.cell_at(x, y) {
                    return Wait::Tap(c, r);
                }
            }
        }
    }
    Wait::Timeout
}

fn main_loop(panel: &mut FbinkPanel, touch: &mut TouchDevice, hosts: &mut Vec<HostEntry>, settings: &mut Settings, ctx: &Ctx) -> Result<()> {
    let mut status = String::new();
    let pair_url = ctx.pair.as_ref().map(|p| p.url.clone()).unwrap_or_else(|| "http://<kobo-ip>:8080".into());
    loop {
        let ui = panel.region_full(UI_FONT, ctx.margin);
        panel.use_region(ui);
        let g = ui.geometry();
        let mut home = Home::new(g.cols, g.rows);
        home.draw(panel, hosts, &ctx.pubkey, &pair_url, &status, settings.size);
        status.clear();
        let (col, row) = match wait_event(panel, touch, ctx.pair.as_ref(), Duration::from_secs(3600)) {
            Wait::Tap(c, r) => (c, r),
            Wait::Registered(e) => {
                match hosts.iter().position(|h| h.name == e.name) {
                    Some(i) => hosts[i] = e.clone(),
                    None => hosts.push(e.clone()),
                }
                HostEntry::save(&ctx.hosts_path, hosts)?;
                status = format!("added {}", e.name);
                continue;
            }
            Wait::Timeout => continue,
        };
        match home.hit(col, row) {
            HomeAction::Quit => return Ok(()),
            HomeAction::Size(size) => {
                if size != settings.size {
                    settings.size = size;
                    settings.save(&ctx.settings_path);
                    panel.replace_font(TERM_FONT, settings.term_font());
                }
            }
            HomeAction::Add => {
                if let Some(entry) = add_form(panel, touch)? {
                    hosts.push(entry);
                    HostEntry::save(&ctx.hosts_path, hosts)?;
                }
            }
            HomeAction::Connect(i) => {
                let entry = hosts[i].clone();
                match session(panel, touch, &entry, &ctx.key_path, ctx.margin) {
                    Ok(()) => status = format!("{}: session ended", entry.name),
                    Err(e) => status = format!("{}: {e:#}", entry.name),
                }
            }
            HomeAction::Nothing => {}
        }
    }
}

/// Returns the new entry, or None on cancel.
fn add_form(panel: &mut FbinkPanel, touch: &mut TouchDevice) -> Result<Option<HostEntry>> {
    let g = panel.geometry();
    let mut osk = Osk::bottom(g.cols, g.rows);
    let top = osk.area.row;
    let mut form = AddForm::new(g.cols);
    panel.clear()?;
    form.draw(panel, top);
    osk.draw(panel);
    loop {
        let Some((col, row)) = wait_tap(panel, touch, Duration::from_secs(3600)) else { continue };
        if let Some(k) = osk.hit(col, row) {
            osk.flash(panel, k);
            let action = osk.press(k);
            let outcome = match action {
                OskAction::Hide | OskAction::Home => FormAction::Cancel,
                OskAction::ModifierChanged => {
                    osk.draw_modifiers(panel);
                    FormAction::Nothing
                }
                a => {
                    let bytes = osk.bytes_for(a);
                    form.input(&bytes)
                }
            };
            if !matches!(action, OskAction::ModifierChanged) {
                osk.release(panel, k);
                osk.draw_modifiers(panel);
            }
            match outcome {
                FormAction::Save => match form.entry() {
                    Ok(e) => return Ok(Some(e)),
                    Err(e) => form.error = e.to_string(),
                },
                FormAction::Cancel => return Ok(None),
                _ => {}
            }
            form.draw(panel, top);
            continue;
        }
        match form.hit(col, row) {
            FormAction::Save => match form.entry() {
                Ok(e) => return Ok(Some(e)),
                Err(e) => {
                    form.error = e.to_string();
                    form.draw(panel, top);
                }
            },
            FormAction::Cancel => return Ok(None),
            FormAction::Focus(f) => {
                form.focus = f;
                form.draw(panel, top);
            }
            FormAction::Nothing => {}
        }
    }
}

/// Connect; if the remote command (tmux) is missing, fall back to a plain shell.
fn session(panel: &mut FbinkPanel, touch: &mut TouchDevice, entry: &HostEntry, key_path: &Path, margin: u32) -> Result<()> {
    match session_with(panel, touch, entry, key_path, entry.command.clone(), margin)? {
        Some(127) if entry.command.is_some() => {
            eprintln!("remote command {:?} not found; falling back to a login shell", entry.command);
            session_with(panel, touch, entry, key_path, None, margin).map(|_| ())
        }
        _ => Ok(()),
    }
}

/// Screen split for a session: terminal band on top (terminal font), keyboard
/// band at the bottom (UI font). With the keyboard hidden the terminal takes
/// the whole view.
struct Layout {
    osk_visible: bool,
    term: Region,
    osk: Region,
}

impl Layout {
    fn compute(panel: &FbinkPanel, margin: u32, osk_visible: bool, osk_rows: u16) -> Layout {
        let ui = panel.region_full(UI_FONT, margin);
        let osk_px = osk_rows as u32 * ui.fh;
        let bottom = panel.view.1.saturating_sub(margin);
        let osk_top = bottom.saturating_sub(osk_px);
        let osk = panel.region(UI_FONT, margin, osk_top, bottom);
        let term_bottom = if osk_visible { osk_top.saturating_sub(ui.fh / 2) } else { bottom };
        let term = panel.region(TERM_FONT, margin, margin, term_bottom);
        Layout { osk_visible, term, osk }
    }
}

/// Wipe the view, draw the keyboard if visible, redraw the terminal with a full refresh.
fn relayout(panel: &mut FbinkPanel, layout: &Layout, osk: &Osk, term: &mut Terminal, tr: &mut dyn Transport, now: u64) -> Result<Renderer> {
    term.resize(layout.term.cols, layout.term.rows);
    tr.resize(layout.term.cols, layout.term.rows)?;
    let full = panel.region_full(UI_FONT, 0);
    panel.use_region(full);
    draw::fill(panel, CellRect { col: 0, row: 0, cols: full.cols, rows: full.rows }, ' ', false);
    if layout.osk_visible {
        panel.use_region(layout.osk);
        osk.draw(panel);
    }
    panel.use_region(layout.term);
    let mut renderer = Renderer::new(Config::default(), layout.term.cols, layout.term.rows);
    let grid = term.snapshot();
    renderer.redraw_full(now, &grid, panel);
    Ok(renderer)
}

/// Returns the remote exit status if the far end ended the session quickly
/// (used for the tmux fallback), None otherwise.
fn session_with(panel: &mut FbinkPanel, touch: &mut TouchDevice, entry: &HostEntry, key_path: &Path, command: Option<String>, margin: u32) -> Result<Option<i32>> {
    let ui = panel.region_full(UI_FONT, margin);
    panel.use_region(ui);
    panel.clear()?;
    draw::text(panel, 1, 1, &format!("connecting to {} ({})...", entry.name, entry.spec), true, false);
    panel.refresh(CellRect { col: 0, row: 0, cols: ui.cols, rows: 3 }, Waveform::Fast);

    let mut osk = Osk::new(ui.cols, 0);
    let mut layout = Layout::compute(panel, margin, true, osk.height());
    let target = SshTarget::parse(&entry.spec, key_path, command)?;
    let mut tr = SshTransport::connect(target, layout.term.cols, layout.term.rows)?;
    let mut term = Terminal::new(layout.term.cols, layout.term.rows, 5000);
    panel.clear()?;
    let t0 = Instant::now();
    let ms = |t0: Instant| t0.elapsed().as_millis() as u64;
    let mut renderer = relayout(panel, &layout, &osk, &mut term, &mut tr, ms(t0))?;

    let mut buf = [0u8; 8192];
    let mut pending_release: Vec<(usize, u64)> = Vec::new();
    let mut finger_down: Option<(i32, i32)> = None;
    loop {
        let n = tr.read(&mut buf, Duration::from_millis(10))?;
        if n > 0 {
            term.feed(&buf[..n]);
        }
        panel.use_region(layout.term);
        let grid = term.snapshot();
        renderer.tick(ms(t0), &grid, panel);

        let now = ms(t0);
        if pending_release.iter().any(|&(_, at)| now >= at) {
            panel.use_region(layout.osk);
            pending_release.retain(|&(k, at)| {
                if now >= at {
                    osk.release(panel, k);
                    false
                } else {
                    true
                }
            });
        }

        for ev in touch.poll(Duration::from_millis(0)) {
            match ev {
                TouchEvent::Down { x, y } => finger_down = Some((x, y)),
                TouchEvent::Up { x, y } => {
                    let Some((sx, sy)) = finger_down.take() else { continue };
                    let dy = y - sy;
                    let dx = x - sx;
                    if dy.abs() >= SWIPE_MIN_PX && dy.abs() > dx.abs() && panel.cell_in(&layout.term, sx, sy).is_some() {
                        // Swipe on the terminal: finger down = older content.
                        let lines = (dy.abs() / layout.term.fh as i32).max(1);
                        let older = dy > 0;
                        let (col, row) = panel.cell_in(&layout.term, x, y).unwrap_or((0, 0));
                        if let Some(bytes) = term.mouse_wheel_bytes(older, col, row) {
                            for _ in 0..lines {
                                tr.write_all(&bytes)?;
                            }
                        } else {
                            term.scroll_view(if older { lines } else { -lines });
                        }
                    }
                }
                TouchEvent::Tap { x, y } => {
                    let osk_hit = if layout.osk_visible { panel.cell_in(&layout.osk, x, y).and_then(|(c, r)| osk.hit(c, r)) } else { None };
                    let Some(k) = osk_hit else {
                        // Tap anywhere else toggles the keyboard with a full-page refresh.
                        pending_release.clear();
                        layout = Layout::compute(panel, margin, !layout.osk_visible, osk.height());
                        renderer = relayout(panel, &layout, &osk, &mut term, &mut tr, ms(t0))?;
                        continue;
                    };
                    panel.use_region(layout.osk);
                    osk.flash(panel, k);
                    match osk.press(k) {
                        OskAction::Home => return Ok(None),
                        OskAction::Hide => {
                            pending_release.clear();
                            layout = Layout::compute(panel, margin, false, osk.height());
                            renderer = relayout(panel, &layout, &osk, &mut term, &mut tr, ms(t0))?;
                        }
                        OskAction::ModifierChanged => {
                            osk.draw_modifiers(panel);
                        }
                        a => {
                            let bytes = osk.bytes_for(a);
                            tr.write_all(&bytes)?;
                            term.scroll_view(-100_000); // typing returns to the live view
                            pending_release.push((k, ms(t0) + 120));
                            osk.draw_modifiers(panel);
                        }
                    }
                }
                TouchEvent::Move { .. } => {}
            }
        }

        if let Some(st) = tr.exit_status() {
            if st == 127 && ms(t0) < 5000 {
                return Ok(Some(st));
            }
            panel.use_region(layout.term);
            let grid = term.snapshot();
            renderer.tick(ms(t0) + 10_000, &grid, panel);
            let r = layout.term.rows.saturating_sub(1);
            draw::text(panel, 1, r, &format!("[session ended ({st}); tap to go home]"), true, true);
            panel.refresh(CellRect { col: 0, row: r, cols: layout.term.cols, rows: 1 }, Waveform::Fast);
            wait_tap(panel, touch, Duration::from_secs(3600));
            return Ok(Some(st));
        }
    }
}
