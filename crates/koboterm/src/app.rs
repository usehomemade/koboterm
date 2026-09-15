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
use font::Family;
use px::{PxCanvas, PxRect, PxWave};
use pxui::popup::{EndIcon, Popup, PopupHit};
use pxui::topbar::{BarAction, BarStatus, TopBar};
use pxui::Fonts;
use ui::{draw, AddForm, FormAction, Home, HomeAction, HostEntry, Osk, OskAction};

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Settings {
    family: Family,
    size_idx: usize,
    line_gap: u32,
    margin: u32,
}

impl Settings {
    fn load(path: &Path) -> Settings {
        let mut s = Settings { family: Family::Spleen, size_idx: 2, line_gap: 0, margin: 12 };
        if let Ok(txt) = std::fs::read_to_string(path) {
            for line in txt.lines() {
                if let Some((k, v)) = line.split_once('=') {
                    let v = v.trim();
                    match k.trim() {
                        "family" => s.family = Family::parse(v),
                        "size_idx" => s.size_idx = v.parse().unwrap_or(2),
                        // Legacy S/M/L setting.
                        "size" => s.size_idx = match v { "small" => 1, "large" => 3, _ => 2 },
                        "line_gap" => s.line_gap = v.parse().unwrap_or(0),
                        "margin" => s.margin = v.parse().unwrap_or(12),
                        _ => {}
                    }
                }
            }
        }
        s.size_idx = s.size_idx.min(s.family.size_count() - 1);
        s
    }

    fn save(&self, path: &Path) {
        let _ = std::fs::write(path, format!("family={}\nsize_idx={}\nline_gap={}\nmargin={}\n", self.family.name().to_ascii_lowercase(), self.size_idx, self.line_gap, self.margin));
    }

    fn term_font(&self) -> font::Font {
        self.family.load(self.size_idx)
    }

    /// Push font, spacing into the panel (margin is applied by layouts).
    fn apply(&self, panel: &mut FbinkPanel) {
        panel.replace_font(TERM_FONT, self.term_font());
        panel.set_line_gap(TERM_FONT, self.line_gap);
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
    panel.set_line_gap(TERM_FONT, settings.line_gap);
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

    let fonts = Fonts::load_device();
    if fonts.is_none() {
        eprintln!("UI fonts not found under {}; top bar and popups disabled", Fonts::DIR);
    }
    let ctx = Ctx { hosts_path, settings_path, key_path, pubkey, pair, fonts };
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
    fonts: Option<Fonts>,
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
        let ui = panel.region_full(UI_FONT, settings.margin);
        panel.use_region(ui);
        let g = ui.geometry();
        let mut home = Home::new(g.cols, g.rows);
        home.draw(panel, hosts, &ctx.pubkey, &pair_url, &status);
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
            HomeAction::Text => {
                text_popup(panel, touch, settings, ctx, 120, None)?;
            }
            HomeAction::Brightness => {
                brightness_popup(panel, touch, ctx, 120, None)?;
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

/// Drive a pixel popup until it is closed. `on_change` runs after every
/// slider or choice change and may adjust the popup (e.g. a slider's range).
fn run_popup(panel: &mut FbinkPanel, touch: &mut TouchDevice, fonts: &Fonts, popup: &mut Popup, on_change: &mut dyn FnMut(&mut Popup, PopupHit)) -> Result<()> {
    popup.draw(panel, fonts);
    let mut dragging: Option<usize> = None;
    let mut dragged = false;
    loop {
        for ev in touch.poll(Duration::from_millis(50)) {
            match ev {
                TouchEvent::Down { x, y } => {
                    dragging = popup.slider_at(x, y);
                    dragged = false;
                }
                TouchEvent::Move { x, .. } => {
                    if let Some(i) = dragging {
                        let v = popup.slider_value_at(i, x);
                        if v != popup.slider_value(i) {
                            popup.set_slider(i, v);
                            on_change(popup, PopupHit::Slider(i, v));
                            popup.refresh_slider(panel, fonts, i, PxWave::Fast);
                            dragged = true;
                        }
                    }
                }
                TouchEvent::Up { .. } => {
                    if let (Some(i), true) = (dragging, dragged) {
                        popup.refresh_slider(panel, fonts, i, PxWave::Quality);
                    }
                    dragging = None;
                }
                TouchEvent::Tap { x, y } => {
                    if dragged {
                        continue;
                    }
                    match popup.hit(x, y) {
                        PopupHit::Close => return Ok(()),
                        PopupHit::Slider(i, v) => {
                            popup.set_slider(i, v);
                            on_change(popup, PopupHit::Slider(i, v));
                            popup.refresh_slider(panel, fonts, i, PxWave::Quality);
                        }
                        PopupHit::Choice(ci, oi) => {
                            popup.set_choice(ci, oi);
                            on_change(popup, PopupHit::Choice(ci, oi));
                            popup.draw(panel, fonts);
                        }
                        PopupHit::Nothing => {}
                    }
                }
            }
        }
    }
}

/// Nickel's brightness panel: frontlight and natural light, applied live.
fn brightness_popup(panel: &mut FbinkPanel, touch: &mut TouchDevice, ctx: &Ctx, y: i32, notch_x: Option<i32>) -> Result<()> {
    let Some(fonts) = ctx.fonts.as_ref() else { return Ok(()) };
    let w = panel.view.0 as i32;
    let mut p = Popup::new(w, 40 * w / 1072, y, notch_x);
    p.title("Brightness").slider(0, 100, crate::device::brightness().unwrap_or(0) as i32, 1, EndIcon::SunSmall, EndIcon::SunLarge, Some(pxui::popup::pct));
    let has_nl = crate::device::natural_light().is_some();
    if let Some(warmth) = crate::device::natural_light() {
        p.divider().title("Natural light").slider(0, crate::device::NATURAL_LIGHT_MAX as i32, warmth as i32, 1, EndIcon::Gear, EndIcon::Moon, None);
    }
    p.finish();
    run_popup(panel, touch, fonts, &mut p, &mut |_, hit| {
        if let PopupHit::Slider(i, v) = hit {
            let _ = match (i, has_nl) {
                (0, _) => crate::device::set_brightness(v as u8),
                (1, true) => crate::device::set_natural_light(v as u8),
                _ => Ok(()),
            };
        }
    })
}

/// Nickel's text panel: font face, size, line spacing, margins. Returns true
/// if anything changed (callers re-lay out the screen).
fn text_popup(panel: &mut FbinkPanel, touch: &mut TouchDevice, settings: &mut Settings, ctx: &Ctx, y: i32, notch_x: Option<i32>) -> Result<bool> {
    let Some(fonts) = ctx.fonts.as_ref() else { return Ok(false) };
    let before = *settings;
    let w = panel.view.0 as i32;
    let names: Vec<&str> = Family::ALL.iter().map(|f| f.name()).collect();
    let fam_idx = Family::ALL.iter().position(|f| *f == settings.family).unwrap_or(0);
    let mut p = Popup::new(w, 40 * w / 1072, y, notch_x);
    p.labeled_choice("Font Face:", &names, fam_idx)
        .labeled_slider("Font Size:", 0, settings.family.size_count() as i32 - 1, settings.size_idx as i32, 1)
        .labeled_slider("Line Spacing:", 0, 16, settings.line_gap as i32, 2)
        .labeled_slider("Margins:", 0, 48, settings.margin as i32, 4);
    p.finish();
    let mut cur = *settings;
    run_popup(panel, touch, fonts, &mut p, &mut |p, hit| match hit {
        PopupHit::Choice(0, oi) => {
            cur.family = Family::ALL[oi];
            p.set_slider_max(0, cur.family.size_count() as i32 - 1);
            cur.size_idx = p.slider_value(0) as usize;
        }
        PopupHit::Slider(0, v) => cur.size_idx = v as usize,
        PopupHit::Slider(1, v) => cur.line_gap = v as u32,
        PopupHit::Slider(2, v) => cur.margin = v as u32,
        _ => {}
    })?;
    *settings = cur;
    if *settings != before {
        settings.save(&ctx.settings_path);
        settings.apply(panel);
    }
    Ok(*settings != before)
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
/// terminal band on top, keyboard (UI font) below. Keyboard hidden: the
/// reader-style top bar, then the terminal band.
struct Layout {
    osk_visible: bool,
    term: Region,
    osk: Region,
}

fn layout(panel: &FbinkPanel, margin: u32, top_px: i32, osk_visible: bool) -> Layout {
    let ui = panel.region_full(UI_FONT, margin);
    let vh = panel.view.1;
    let osk_h_px = ui::osk_rows(ui.cols, ui.rows) as u32 * ui.fh;
    let osk = panel.region(UI_FONT, margin, vh - margin - osk_h_px, vh - margin);
    let term = if osk_visible {
        panel.region(TERM_FONT, margin, margin, osk.y0 - ui.fh / 2)
    } else {
        panel.region(TERM_FONT, margin, top_px as u32 + 12, vh - margin)
    };
    Layout { osk_visible, term, osk }
}

fn bar_status() -> BarStatus {
    let (battery_pct, charging) = crate::device::battery();
    BarStatus { clock: crate::device::clock(), title: "Back to Home".into(), battery_pct, charging }
}

/// Apply a layout: resize the terminal, redraw the chrome and the terminal with one full refresh.
fn relayout(panel: &mut FbinkPanel, lay: &Layout, osk: &Osk, bar: &TopBar, fonts: Option<&Fonts>, term: &mut Terminal, tr: &mut dyn Transport, now: u64) -> Result<Renderer> {
    let g = lay.term.geometry();
    term.resize(g.cols, g.rows);
    tr.resize(g.cols, g.rows)?;
    let mut renderer = Renderer::new(Config::default(), g.cols, g.rows);
    let (vw, vh) = (panel.view.0 as i32, panel.view.1 as i32);
    PxCanvas::fill(panel, PxRect::new(0, 0, vw, vh), 0xFF);
    if lay.osk_visible {
        panel.use_region(lay.osk);
        osk.draw(panel);
    } else if let Some(f) = fonts {
        bar.draw(panel, f, &bar_status());
    }
    panel.use_region(lay.term);
    let grid = term.snapshot();
    renderer.redraw_full(now, &grid, panel);
    Ok(renderer)
}

enum End {
    /// Remote command finished with this status.
    Exit(i32),
    /// User asked to go back to the machine list.
    Home,
    /// Connection dropped.
    Lost,
}

struct SessionState {
    lay: Layout,
    osk: Osk,
    bar: TopBar,
    term: Terminal,
    renderer: Renderer,
    pending_release: Vec<(usize, u64)>,
    finger_down: Option<(i32, i32)>,
    t0: Instant,
    margin: u32,
    last_status: BarStatus,
    next_status_check: u64,
}

fn ms(t0: Instant) -> u64 {
    t0.elapsed().as_millis() as u64
}

/// One-line notice at the bottom of the terminal band.
fn banner(panel: &mut FbinkPanel, st: &SessionState, text: &str) {
    panel.use_region(st.lay.term);
    let r = st.lay.term.rows.saturating_sub(1);
    let cols = st.lay.term.cols;
    let line: String = format!("{text:<width$}", width = cols as usize).chars().take(cols as usize).collect();
    draw::text(panel, 0, r, &line, true, true);
    panel.refresh(CellRect { col: 0, row: r, cols, rows: 1 }, Waveform::Fast);
}

/// Returns the remote exit status if the far end ended the session quickly
/// (used for the tmux fallback), None otherwise. Reconnects on its own when
/// the link drops; a tap on Home / Back while waiting cancels.
fn session_with(panel: &mut FbinkPanel, touch: &mut TouchDevice, entry: &HostEntry, settings: &mut Settings, ctx: &Ctx, command: Option<String>) -> Result<Option<i32>> {
    let margin = settings.margin;
    let ui = panel.region_full(UI_FONT, margin);
    panel.use_region(ui);
    panel.clear()?;
    draw::text(panel, 1, 1, &format!("connecting to {} ({})...", entry.name, entry.spec), true, false);
    panel.refresh(CellRect { col: 0, row: 0, cols: ui.cols, rows: 3 }, Waveform::Fast);

    let bar = TopBar::new(panel.view.0 as i32);
    let lay = layout(panel, margin, bar.height(), true);
    let osk = Osk::top(ui.cols, ui.rows);
    let tg = lay.term.geometry();
    let target = SshTarget::parse(&entry.spec, &ctx.key_path, command)?;
    let term = Terminal::new(tg.cols, tg.rows, 5000);
    let renderer = Renderer::new(Config::default(), tg.cols, tg.rows);
    let mut st = SessionState {
        lay,
        osk,
        bar,
        term,
        renderer,
        pending_release: Vec::new(),
        finger_down: None,
        t0: Instant::now(),
        margin,
        last_status: bar_status(),
        next_status_check: 5000,
    };

    let mut attempt = 0u32;
    let mut first = true;
    loop {
        match SshTransport::connect(target.clone(), st.lay.term.cols, st.lay.term.rows) {
            Ok(mut tr) => {
                attempt = 0;
                first = false;
                st.renderer = relayout(panel, &st.lay, &st.osk, &st.bar, ctx.fonts.as_ref(), &mut st.term, &mut tr, ms(st.t0))?;
                let started = ms(st.t0);
                match run_session(panel, touch, &mut st, &mut tr, settings, ctx)? {
                    End::Home => return Ok(None),
                    End::Exit(code) => {
                        if code == 127 && ms(st.t0) - started < 5000 {
                            return Ok(Some(code));
                        }
                        banner(panel, &st, &format!("[session ended ({code}); tap to go home]"));
                        wait_tap(panel, touch, Duration::from_secs(3600));
                        return Ok(Some(code));
                    }
                    End::Lost => {}
                }
            }
            Err(e) => {
                if first {
                    // Never got in at all: report on the home screen instead of retrying.
                    return Err(e);
                }
                eprintln!("reconnect failed: {e:#}");
            }
        }
        attempt += 1;
        let delay = 2u64.pow(attempt.min(4)).min(30);
        banner(panel, &st, &format!("[connection lost; reconnecting in {delay}s, tap Home to stop]"));
        let until = Instant::now() + Duration::from_secs(delay);
        while Instant::now() < until {
            for ev in touch.poll(Duration::from_millis(50)) {
                if let TouchEvent::Tap { x, y } = ev {
                    if !st.lay.osk_visible && y < st.bar.height() && matches!(st.bar.hit(x, y), BarAction::Back | BarAction::More) {
                        return Ok(None);
                    }
                    if let Some((c, r)) = panel.cell_in(&st.lay.osk, x, y) {
                        if st.lay.osk_visible && st.osk.hit(c, r).map(|k| st.osk.key_is_home(k)).unwrap_or(false) {
                            return Ok(None);
                        }
                    }
                }
            }
        }
        banner(panel, &st, "[reconnecting...]");
    }
}

fn run_session(panel: &mut FbinkPanel, touch: &mut TouchDevice, st: &mut SessionState, tr: &mut SshTransport, settings: &mut Settings, ctx: &Ctx) -> Result<End> {
    let fonts = ctx.fonts.as_ref();
    let t0 = st.t0;
    let mut buf = [0u8; 8192];
    loop {
        let n = tr.read(&mut buf, Duration::from_millis(10))?;
        if n > 0 {
            st.term.feed(&buf[..n]);
        }
        panel.use_region(st.lay.term);
        let grid = st.term.snapshot();
        st.renderer.tick(ms(t0), &grid, panel);

        let now = ms(t0);
        if st.lay.osk_visible && st.pending_release.iter().any(|&(_, at)| now >= at) {
            panel.use_region(st.lay.osk);
            let osk = &st.osk;
            st.pending_release.retain(|&(k, at)| {
                if now >= at {
                    osk.release(panel, k);
                    false
                } else {
                    true
                }
            });
        }
        if !st.lay.osk_visible && now >= st.next_status_check {
            st.next_status_check = now + 5000;
            let status = bar_status();
            if status != st.last_status {
                if let Some(f) = fonts {
                    st.bar.update_status(panel, f, &status);
                }
                st.last_status = status;
            }
        }

        for ev in touch.poll(Duration::from_millis(0)) {
            match ev {
                TouchEvent::Down { x, y } => st.finger_down = Some((x, y)),
                TouchEvent::Up { x, y } => {
                    let Some((sx, sy)) = st.finger_down.take() else { continue };
                    let dy = y - sy;
                    let dx = x - sx;
                    if dy.abs() >= SWIPE_MIN_PX && dy.abs() > dx.abs() && panel.cell_in(&st.lay.term, sx, sy).is_some() {
                        // Swipe on the terminal: finger down = older content.
                        let lines = (dy.abs() / st.lay.term.fh as i32).max(1);
                        let older = dy > 0;
                        let (col, row) = panel.cell_in(&st.lay.term, x, y).unwrap_or((0, 0));
                        if let Some(bytes) = st.term.mouse_wheel_bytes(older, col, row) {
                            for _ in 0..lines {
                                tr.write_all(&bytes)?;
                            }
                        } else {
                            st.term.scroll_view(if older { lines } else { -lines });
                        }
                    }
                }
                TouchEvent::Tap { x, y } => {
                    if !st.lay.osk_visible && y < st.bar.height() {
                        let action = st.bar.hit(x, y);
                        let notch = bar_notch(&st.bar, action);
                        match action {
                            BarAction::Back | BarAction::More => return Ok(End::Home),
                            BarAction::Keyboard => {
                                st.lay = layout(panel, st.margin, st.bar.height(), true);
                                st.renderer = relayout(panel, &st.lay, &st.osk, &st.bar, fonts, &mut st.term, tr, ms(t0))?;
                            }
                            BarAction::Brightness => {
                                brightness_popup(panel, touch, ctx, st.bar.height() + 8, notch)?;
                                st.last_status = bar_status();
                                st.renderer = relayout(panel, &st.lay, &st.osk, &st.bar, fonts, &mut st.term, tr, ms(t0))?;
                            }
                            BarAction::Text | BarAction::Settings => {
                                text_popup(panel, touch, settings, ctx, st.bar.height() + 8, notch)?;
                                st.margin = settings.margin;
                                st.lay = layout(panel, st.margin, st.bar.height(), false);
                                st.renderer = relayout(panel, &st.lay, &st.osk, &st.bar, fonts, &mut st.term, tr, ms(t0))?;
                            }
                            BarAction::Nothing => {}
                        }
                        continue;
                    }
                    let osk_hit = if st.lay.osk_visible { panel.cell_in(&st.lay.osk, x, y).and_then(|(c, r)| st.osk.hit(c, r)) } else { None };
                    let Some(k) = osk_hit else {
                        // Tap on the terminal: toggle keyboard / top bar.
                        st.pending_release.clear();
                        st.lay = layout(panel, st.margin, st.bar.height(), !st.lay.osk_visible);
                        st.renderer = relayout(panel, &st.lay, &st.osk, &st.bar, fonts, &mut st.term, tr, ms(t0))?;
                        continue;
                    };
                    panel.use_region(st.lay.osk);
                    st.osk.flash(panel, k);
                    match st.osk.press(k) {
                        OskAction::Home => return Ok(End::Home),
                        OskAction::Hide => {
                            st.pending_release.clear();
                            st.lay = layout(panel, st.margin, st.bar.height(), false);
                            st.renderer = relayout(panel, &st.lay, &st.osk, &st.bar, fonts, &mut st.term, tr, ms(t0))?;
                        }
                        OskAction::ModifierChanged => {
                            st.osk.draw_modifiers(panel);
                        }
                        a => {
                            let bytes = st.osk.bytes_for(a);
                            tr.write_all(&bytes)?;
                            st.term.scroll_view(-100_000); // typing returns to the live view
                            st.pending_release.push((k, ms(t0) + 120));
                            st.osk.draw_modifiers(panel);
                        }
                    }
                }
                TouchEvent::Move { .. } => {}
            }
        }

        if let Some(code) = tr.exit_status() {
            panel.use_region(st.lay.term);
            let grid = st.term.snapshot();
            st.renderer.tick(ms(t0) + 10_000, &grid, panel);
            return Ok(if code == LOST { End::Lost } else { End::Exit(code) });
        }
    }
}

/// Centre x of a bar button, for the popup's pointer.
fn bar_notch(bar: &TopBar, action: BarAction) -> Option<i32> {
    let w = bar.area().w;
    let step = 72 * w / 1072;
    let right = w - 40 * w / 1072 - step / 2;
    match action {
        BarAction::More => Some(right),
        BarAction::Settings => Some(right - step),
        BarAction::Keyboard => Some(right - 2 * step),
        BarAction::Text => Some(right - 3 * step),
        BarAction::Brightness => Some(right - 4 * step),
        _ => None,
    }
}
