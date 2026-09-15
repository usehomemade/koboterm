//! Small device facts and knobs read from sysfs: clock, battery, frontlight.

use std::fs;

const BATTERY_DIRS: &[&str] = &["/sys/class/power_supply/bd71827_bat", "/sys/class/power_supply/mc13892_bat", "/sys/class/power_supply/battery"];
const FRONTLIGHT: &str = "/sys/class/backlight/mxc_msp430.0/brightness";

fn read_trim(path: &str) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

pub fn clock() -> String {
    let mut t: libc::time_t = 0;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::time(&mut t);
        libc::localtime_r(&t, &mut tm);
    }
    format!("{:02}:{:02}", tm.tm_hour, tm.tm_min)
}

/// (percent, charging)
pub fn battery() -> (Option<u8>, bool) {
    for d in BATTERY_DIRS {
        if let Some(cap) = read_trim(&format!("{d}/capacity")) {
            let pct = cap.parse::<u8>().ok().map(|p| p.min(100));
            let charging = read_trim(&format!("{d}/status")).map(|s| s.eq_ignore_ascii_case("charging") || s.eq_ignore_ascii_case("full")).unwrap_or(false);
            return (pct, charging);
        }
    }
    (None, false)
}

pub fn brightness() -> Option<u8> {
    read_trim(FRONTLIGHT)?.parse::<u8>().ok()
}

pub fn set_brightness(pct: u8) -> std::io::Result<()> {
    fs::write(FRONTLIGHT, format!("{}\n", pct.min(100)))
}
