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
use transport::{SshTarget, SshTransport, Transport, LOST};
use ui::{draw, AddForm, FormAction, Home, HomeAction, HostEntry, Osk, OskAction, Status, TextSize, TopAction, TopBar, TOPBAR_ROWS};

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

    fn next_size(&mut self) {
        self.size = match self.size {
            TextSize::Small => TextSize::Medium,
            TextSize::Medium => TextSize::Large,
            TextSize::Large => TextSize::Small,
        };
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
                match session(panel, touch, &entry, settings, ctx) {
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
fn session(panel: &mut FbinkPanel, touch: &mut TouchDevice, entry: &HostEntry, settings: &mut Settings, ctx: &Ctx) -> Result<()> {
    match session_with(panel, touch, entry, settings, ctx, entry.command.clone())? {
        Some(127) if entry.command.is_some() => {
            eprintln!("remote command {:?} not found; falling back to a login shell", entry.command);
            session_with(panel, touch, entry, settings, ctx, None).map(|_| ())
        }
        _ => Ok(()),
    }
}

/// Screen arrangement of a session, in pixel bands. Keyboard visible:
/// terminal band on top, keyboard (UI font) below. Keyboard hidden: top bar
/// (UI font) on top, terminal band below. The terminal band uses the
/// terminal font, so its cell grid differs from the UI's.
struct Layout {
    osk_visible: bool,
    term: Region,
    osk: Region,
    top: Region,
}

fn layout(panel: &FbinkPanel, margin: u32, osk_visible: bool) -> Layout {
    let ui = panel.region_full(UI_FONT, margin);
    let vh = panel.view.1;
    let osk_h_px = ui::osk_rows(ui.cols, ui.rows) as u32 * ui.fh;
    let top_h_px = TOPBAR_ROWS as u32 * ui.fh;
    let osk = panel.region(UI_FONT, margin, vh - margin - osk_h_px, vh - margin);
    let top = panel.region(UI_FONT, margin, margin, margin + top_h_px);
    let term = if osk_visible {
        panel.region(TERM_FONT, margin, margin, osk.y0 - ui.fh / 2)
    } else {
        panel.region(TERM_FONT, margin, top.y0 + top_h_px + ui.fh / 2, vh - margin)
    };
    Layout { osk_visible, term, osk, top }
}

fn status(entry: &HostEntry) -> Status {
    let (battery_pct, charging) = crate::device::battery();
    Status { clock: crate::device::clock(), name: entry.name.clone(), battery_pct, charging, brightness_pct: crate::device::brightness() }
}

/// Apply a layout: resize the terminal, redraw the chrome and the terminal with one full refresh.
fn relayout(panel: &mut FbinkPanel, lay: &Layout, osk: &Osk, topbar: &TopBar, entry: &HostEntry, term: &mut Terminal, tr: &mut dyn Transport, now: u64) -> Result<Renderer> {
    let g = lay.term.geometry();
    term.resize(g.cols, g.rows);
    tr.resize(g.cols, g.rows)?;
    let mut renderer = Renderer::new(Config::default(), g.cols, g.rows);
    let ui = panel.region_full(UI_FONT, 0);
    panel.use_region(ui);
    draw::fill(panel, CellRect { col: 0, row: 0, cols: ui.cols, rows: ui.rows }, ' ', false);
    if lay.osk_visible {
        panel.use_region(lay.osk);
        osk.draw(panel);
    } else {
        panel.use_region(lay.top);
        topbar.draw(panel, &status(entry));
    }
    panel.use_region(lay.term);
    let grid = term.snapshot();
    renderer.redraw_full(now, &grid, panel);
    Ok(renderer)
}

/// Returns the remote exit status if the far end ended the session quickly
/// (used for the tmux fallback), None otherwise.
fn session_with(panel: &mut FbinkPanel, touch: &mut TouchDevice, entry: &HostEntry, settings: &mut Settings, ctx: &Ctx, command: Option<String>) -> Result<Option<i32>> {
    let margin = ctx.margin;
    let ui = panel.region_full(UI_FONT, margin);
    panel.use_region(ui);
    panel.clear()?;
    draw::text(panel, 1, 1, &format!("connecting to {} ({})...", entry.name, entry.spec), true, false);
    panel.refresh(CellRect { col: 0, row: 0, cols: ui.cols, rows: 3 }, Waveform::Fast);

    let mut lay = layout(panel, margin, true);
    let mut osk = Osk::top(ui.cols, ui.rows);
    let topbar = TopBar::new(ui.cols);
    let tg = lay.term.geometry();
    let target = SshTarget::parse(&entry.spec, &ctx.key_path, command)?;
    let mut tr = SshTransport::connect(target, tg.cols, tg.rows)?;
    let mut term = Terminal::new(tg.cols, tg.rows, 5000);
    let t0 = Instant::now();
    let ms = |t0: Instant| t0.elapsed().as_millis() as u64;
    let mut renderer = relayout(panel, &lay, &osk, &topbar, entry, &mut term, &mut tr, ms(t0))?;

    let mut buf = [0u8; 8192];
    let mut pending_release: Vec<(usize, u64)> = Vec::new();
    let mut finger_down: Option<(i32, i32)> = None;
    let mut last_status = status(entry);
    let mut next_status_check = 5000u64;
    loop {
        let n = tr.read(&mut buf, Duration::from_millis(10))?;
        if n > 0 {
            term.feed(&buf[..n]);
        }
        panel.use_region(lay.term);
        let grid = term.snapshot();
        renderer.tick(ms(t0), &grid, panel);

        let now = ms(t0);
        if lay.osk_visible && pending_release.iter().any(|&(_, at)| now >= at) {
            panel.use_region(lay.osk);
            pending_release.retain(|&(k, at)| {
                if now >= at {
                    osk.release(panel, k);
                    false
                } else {
                    true
                }
            });
        }
        if !lay.osk_visible && now >= next_status_check {
            next_status_check = now + 5000;
            let st = status(entry);
            if st != last_status {
                panel.use_region(lay.top);
                topbar.update_status(panel, &st);
                last_status = st;
            }
        }

        for ev in touch.poll(Duration::from_millis(0)) {
            match ev {
                TouchEvent::Down { x, y } => finger_down = Some((x, y)),
                TouchEvent::Up { x, y } => {
                    let Some((sx, sy)) = finger_down.take() else { continue };
                    let dy = y - sy;
                    let dx = x - sx;
                    if dy.abs() >= SWIPE_MIN_PX && dy.abs() > dx.abs() {
                        // Swipe on the terminal: finger down = older content.
                        let lines = (dy.abs() / lay.term.fh as i32).max(1);
                        let older = dy > 0;
                        let (col, row) = panel.cell_in(&lay.term, x, y).unwrap_or((0, 0));
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
                    if !lay.osk_visible {
                        if let Some((col, row)) = panel.cell_in(&lay.top, x, y) {
                            match topbar.hit(col, row) {
                                TopAction::Home => return Ok(None),
                                TopAction::Keyboard => {
                                    lay = layout(panel, margin, true);
                                    renderer = relayout(panel, &lay, &osk, &topbar, entry, &mut term, &mut tr, ms(t0))?;
                                }
                                a @ (TopAction::BrightnessDown | TopAction::BrightnessUp) => {
                                    let cur = crate::device::brightness().unwrap_or(0) as i32;
                                    let next = if a == TopAction::BrightnessUp { cur + 10 } else { cur - 10 };
                                    let _ = crate::device::set_brightness(next.clamp(0, 100) as u8);
                                    last_status = status(entry);
                                    panel.use_region(lay.top);
                                    topbar.update_status(panel, &last_status);
                                }
                                TopAction::TextSize => {
                                    settings.next_size();
                                    settings.save(&ctx.settings_path);
                                    panel.replace_font(TERM_FONT, settings.term_font());
                                    lay = layout(panel, margin, false);
                                    renderer = relayout(panel, &lay, &osk, &topbar, entry, &mut term, &mut tr, ms(t0))?;
                                }
                                TopAction::Nothing => {}
                            }
                            continue;
                        }
                    }
                    let osk_hit = if lay.osk_visible { panel.cell_in(&lay.osk, x, y).and_then(|(c, r)| osk.hit(c, r)) } else { None };
                    let Some(k) = osk_hit else {
                        // Tap on the terminal (or anywhere else): toggle keyboard / top bar.
                        pending_release.clear();
                        lay = layout(panel, margin, !lay.osk_visible);
                        renderer = relayout(panel, &lay, &osk, &topbar, entry, &mut term, &mut tr, ms(t0))?;
                        continue;
                    };
                    panel.use_region(lay.osk);
                    osk.flash(panel, k);
                    match osk.press(k) {
                        OskAction::Home => return Ok(None),
                        OskAction::Hide => {
                            pending_release.clear();
                            lay = layout(panel, margin, false);
                            renderer = relayout(panel, &lay, &osk, &topbar, entry, &mut term, &mut tr, ms(t0))?;
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
            panel.use_region(lay.term);
            let grid = term.snapshot();
            renderer.tick(ms(t0) + 10_000, &grid, panel);
            let tg = lay.term.geometry();
            let r = tg.rows.saturating_sub(1);
            let msg = if st == LOST { "[connection lost; tap to go home]".to_string() } else { format!("[session ended ({st}); tap to go home]") };
            draw::text(panel, 1, r, &msg, true, true);
            panel.refresh(CellRect { col: 0, row: r, cols: tg.cols, rows: 1 }, Waveform::Fast);
            wait_tap(panel, touch, Duration::from_secs(3600));
            return Ok(Some(st));
        }
    }
}
