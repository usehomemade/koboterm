//! Small device facts and knobs read from sysfs: clock, battery, frontlight.

use std::fs;

const BATTERY_DIRS: &[&str] = &["/sys/class/power_supply/bd71827_bat", "/sys/class/power_supply/mc13892_bat", "/sys/class/power_supply/battery"];
const FRONTLIGHT: &str = "/sys/class/backlight/mxc_msp430.0/brightness";
/// Warm/cool LED mix, 0..10. The file counts the other way round from the
/// slider (10 = coolest), like Nickel's "Natural light".
const NATURAL_LIGHT: &str = "/sys/class/backlight/lm3630a_led/color";
pub const NATURAL_LIGHT_MAX: u8 = 10;

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

/// Warmth 0 (cool) .. 10 (warm), None if the device has no natural light.
pub fn natural_light() -> Option<u8> {
    let color = read_trim(NATURAL_LIGHT)?.parse::<u8>().ok()?;
    Some(NATURAL_LIGHT_MAX.saturating_sub(color.min(NATURAL_LIGHT_MAX)))
}

pub fn set_natural_light(warmth: u8) -> std::io::Result<()> {
    fs::write(NATURAL_LIGHT, format!("{}\n", NATURAL_LIGHT_MAX - warmth.min(NATURAL_LIGHT_MAX)))
}
