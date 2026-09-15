//! The interactive app: home screen with saved machines, an on-screen
//! keyboard, and SSH sessions. Runs on top of Nickel: grabs the touch device
//! while active and hands the screen back on exit.

use anyhow::{Context, Result};
use input::{TouchDevice, TouchEvent, TouchMap};
use panel::{CellRect, Panel, Waveform};
use panel_fbink::FbinkPanel;
use render::{Config, Renderer};
use std::path::PathBuf;
use std::time::{Duration, Instant};
use term::Terminal;
use transport::{SshTarget, SshTransport, Transport};
use ui::{draw, AddForm, FormAction, Home, HomeAction, HostEntry, Osk, OskAction, OSK_ROWS};

use crate::pair::PairServer;

const TOUCH_DEV: &str = "/dev/input/event1";

fn data_dir() -> PathBuf {
    let onboard = PathBuf::from("/mnt/onboard/.adds/koboterm");
    if onboard.parent().map(|p| p.exists()).unwrap_or(false) {
        onboard
    } else {
        PathBuf::from("koboterm-data")
    }
}

pub fn run() -> Result<()> {
    let dir = data_dir();
    std::fs::create_dir_all(&dir)?;
    let key_path = dir.join("id_ed25519");
    if !key_path.exists() {
        eprintln!("generating device key at {}", key_path.display());
        transport::keygen(&key_path, "koboterm")?;
    }
    let pubkey = std::fs::read_to_string(key_path.with_extension("pub")).unwrap_or_default();
    let hosts_path = dir.join("hosts");
    let mut hosts = HostEntry::load(&hosts_path)?;

    let font = font::Font::from_bdf(font::SPLEEN_16X32);
    let mut panel = FbinkPanel::open(font)?;
    let saved = stable_screen(&panel);
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
    let result = main_loop(&mut panel, &mut touch, &mut hosts, &hosts_path, &key_path, &pubkey, pair.as_ref());

    let _ = touch.grab(false);
    panel.restore_screen(&saved);
    result
}

enum Wait {
    Tap(u16, u16),
    Registered(HostEntry),
    Timeout,
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

fn main_loop(
    panel: &mut FbinkPanel,
    touch: &mut TouchDevice,
    hosts: &mut Vec<HostEntry>,
    hosts_path: &std::path::Path,
    key_path: &std::path::Path,
    pubkey: &str,
    pair: Option<&PairServer>,
) -> Result<()> {
    let g = panel.geometry();
    let mut home = Home::new(g.cols, g.rows);
    let mut status = String::new();
    let pair_url = pair.map(|p| p.url.clone()).unwrap_or_else(|| "http://<kobo-ip>:8080".into());
    loop {
        home.draw(panel, hosts, pubkey, &pair_url, &status);
        status.clear();
        let (col, row) = match wait_event(panel, touch, pair, Duration::from_secs(3600)) {
            Wait::Tap(c, r) => (c, r),
            Wait::Registered(e) => {
                // Replace an entry with the same name, otherwise append.
                match hosts.iter().position(|h| h.name == e.name) {
                    Some(i) => hosts[i] = e.clone(),
                    None => hosts.push(e.clone()),
                }
                HostEntry::save(hosts_path, hosts)?;
                status = format!("added {}", e.name);
                continue;
            }
            Wait::Timeout => continue,
        };
        match home.hit(col, row) {
            HomeAction::Quit => return Ok(()),
            HomeAction::Add => {
                if let Some(entry) = add_form(panel, touch)? {
                    hosts.push(entry);
                    HostEntry::save(hosts_path, hosts)?;
                }
            }
            HomeAction::Connect(i) => {
                let entry = hosts[i].clone();
                match session(panel, touch, &entry, key_path) {
                    Ok(()) => status = format!("{}: session ended", entry.name),
                    Err(e) => status = format!("{}: {e:#}", entry.name).chars().take((g.cols - 12) as usize).collect(),
                }
            }
            HomeAction::Nothing => {}
        }
    }
}

/// Returns the new entry, or None on cancel.
fn add_form(panel: &mut FbinkPanel, touch: &mut TouchDevice) -> Result<Option<HostEntry>> {
    let g = panel.geometry();
    let top = g.rows - OSK_ROWS;
    let mut osk = Osk::new(g.cols, top);
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
                    osk.release(panel, k);
                    if osk.modifiers_active() {
                        osk.draw_modifiers(panel);
                    }
                    form.input(&bytes)
                }
            };
            if !matches!(action, OskAction::ModifierChanged) {
                osk.release(panel, k);
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
fn session(panel: &mut FbinkPanel, touch: &mut TouchDevice, entry: &HostEntry, key_path: &std::path::Path) -> Result<()> {
    match session_with(panel, touch, entry, key_path, entry.command.clone())? {
        Some(127) if entry.command.is_some() => {
            eprintln!("remote command {:?} not found; falling back to a login shell", entry.command);
            session_with(panel, touch, entry, key_path, None).map(|_| ())
        }
        _ => Ok(()),
    }
}

/// Returns the remote exit status if the far end ended the session quickly
/// (used for the tmux fallback), None otherwise.
fn session_with(panel: &mut FbinkPanel, touch: &mut TouchDevice, entry: &HostEntry, key_path: &std::path::Path, command: Option<String>) -> Result<Option<i32>> {
    let g = panel.geometry();
    panel.clear()?;
    draw::text(panel, 1, 1, &format!("connecting to {} ({})...", entry.name, entry.spec), true, false);
    panel.refresh(CellRect { col: 0, row: 0, cols: g.cols, rows: 3 }, Waveform::Fast);

    let mut osk_visible = true;
    let mut term_rows = g.rows - OSK_ROWS;
    let target = SshTarget::parse(&entry.spec, key_path, command)?;
    let mut tr = SshTransport::connect(target, g.cols, term_rows)?;
    let mut term = Terminal::new(g.cols, term_rows, 0);
    let mut renderer = Renderer::new(Config::default(), g.cols, term_rows);
    let mut osk = Osk::new(g.cols, g.rows - OSK_ROWS);
    panel.clear()?;
    osk.draw(panel);

    let t0 = Instant::now();
    let ms = |t0: Instant| t0.elapsed().as_millis() as u64;
    let mut buf = [0u8; 8192];
    let mut pending_release: Option<(usize, u64)> = None;
    loop {
        let n = tr.read(&mut buf, Duration::from_millis(10))?;
        if n > 0 {
            term.feed(&buf[..n]);
        }
        let grid = term.snapshot();
        renderer.tick(ms(t0), &grid, panel);

        if let Some((k, at)) = pending_release {
            if ms(t0) >= at {
                osk.release(panel, k);
                pending_release = None;
            }
        }

        for ev in touch.poll(Duration::from_millis(0)) {
            let TouchEvent::Tap { x, y } = ev else { continue };
            let Some((col, row)) = panel.cell_at(x, y) else { continue };
            let on_osk = osk_visible && osk.hit(col, row).is_some();
            if !on_osk {
                // A tap on the terminal toggles the keyboard, with a full-page refresh.
                osk_visible = !osk_visible;
                term_rows = if osk_visible { g.rows - OSK_ROWS } else { g.rows };
                term.resize(g.cols, term_rows);
                tr.resize(g.cols, term_rows)?;
                renderer = Renderer::new(Config::default(), g.cols, term_rows);
                draw::fill(panel, CellRect { col: 0, row: 0, cols: g.cols, rows: g.rows }, ' ', false);
                if osk_visible {
                    osk.draw(panel);
                }
                let grid = term.snapshot();
                renderer.redraw_full(ms(t0), &grid, panel);
                pending_release = None;
                continue;
            }
            let Some(k) = osk.hit(col, row) else { continue };
            osk.flash(panel, k);
            match osk.press(k) {
                OskAction::Home => return Ok(None),
                OskAction::Hide => {
                    osk_visible = false;
                    term_rows = g.rows;
                    term.resize(g.cols, term_rows);
                    tr.resize(g.cols, term_rows)?;
                    renderer = Renderer::new(Config::default(), g.cols, term_rows);
                    draw::fill(panel, CellRect { col: 0, row: 0, cols: g.cols, rows: g.rows }, ' ', false);
                    let grid = term.snapshot();
                    renderer.redraw_full(ms(t0), &grid, panel);
                    pending_release = None;
                }
                OskAction::ModifierChanged => {
                    osk.draw_modifiers(panel);
                }
                a => {
                    let bytes = osk.bytes_for(a);
                    tr.write_all(&bytes)?;
                    pending_release = Some((k, ms(t0) + 120));
                    if !osk.modifiers_active() {
                        osk.draw_modifiers(panel);
                    }
                }
            }
        }

        if let Some(st) = tr.exit_status() {
            if st == 127 && ms(t0) < 5000 {
                return Ok(Some(st));
            }
            let grid = term.snapshot();
            renderer.tick(ms(t0) + 10_000, &grid, panel);
            draw::text(panel, 1, term_rows.saturating_sub(1), &format!("[session ended ({st}); tap to go home]"), true, true);
            panel.refresh(CellRect { col: 0, row: term_rows.saturating_sub(1), cols: g.cols, rows: 1 }, Waveform::Fast);
            wait_tap(panel, touch, Duration::from_secs(3600));
            return Ok(Some(st));
        }
    }
}
