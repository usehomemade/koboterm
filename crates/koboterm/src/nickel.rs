//! Keeping Nickel (the Kobo UI) out of the way while koboterm runs.
//!
//! `pause` freezes the whole UI stack with SIGSTOP and thaws it on exit:
//! instant resume, Nickel's state untouched, no repaints, no idle timer.
//! `restart` is the KOReader-style kill + relaunch, done by the launcher
//! script; the app then does nothing here. `leave` is for development.

use std::fs;

const UI_PROCS: &[&str] = &["nickel", "hindenburg", "fickel", "strickel", "fontickel", "adobehost", "iink", "fmon"];
/// The watchdog that restarts Nickel when it stops answering. Frozen first, thawed last.
const WATCHDOG: &str = "sickel";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Pause,
    Leave,
}

impl Mode {
    pub fn parse(s: &str) -> Mode {
        match s {
            "pause" => Mode::Pause,
            _ => Mode::Leave,
        }
    }
}

fn pids_named(name: &str) -> Vec<i32> {
    let mut out = Vec::new();
    let Ok(rd) = fs::read_dir("/proc") else { return out };
    for e in rd.flatten() {
        let Ok(pid) = e.file_name().to_string_lossy().parse::<i32>() else { continue };
        if let Ok(comm) = fs::read_to_string(e.path().join("comm")) {
            if comm.trim() == name {
                out.push(pid);
            }
        }
    }
    out
}

fn signal(names: &[&str], sig: i32) -> usize {
    let mut n = 0;
    for name in names {
        for pid in pids_named(name) {
            if unsafe { libc::kill(pid, sig) } == 0 {
                n += 1;
            }
        }
    }
    n
}

pub struct Paused {
    active: bool,
}

/// Freeze the UI stack. Call only once the screen is stable (menus closed).
pub fn pause(mode: Mode) -> Paused {
    if mode != Mode::Pause {
        return Paused { active: false };
    }
    unsafe { libc::sync() };
    let w = signal(&[WATCHDOG], libc::SIGSTOP);
    let n = signal(UI_PROCS, libc::SIGSTOP);
    eprintln!("nickel: froze {n} UI processes and {w} watchdog");
    Paused { active: n > 0 }
}

impl Paused {
    pub fn resume(&mut self) {
        if !self.active {
            return;
        }
        let n = signal(UI_PROCS, libc::SIGCONT);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let w = signal(&[WATCHDOG], libc::SIGCONT);
        eprintln!("nickel: thawed {n} UI processes and {w} watchdog");
        self.active = false;
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        self.resume();
    }
}
