//! koboterm entry point. For now only `probe`, which reports what the device
//! looks like from userland so the toolchain and framebuffer assumptions can be
//! checked before any drawing code exists.

use anyhow::Result;

#[cfg(target_os = "linux")]
mod app;
#[cfg(target_os = "linux")]
mod pair;

fn main() -> Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        #[cfg(target_os = "linux")]
        "" | "app" => app::run(),
        "probe" => probe(),
        #[cfg(target_os = "linux")]
        "demo" => demo::run(),
        #[cfg(target_os = "linux")]
        "run" => session::run(session::Kind::Pty, std::env::args().skip(2).collect()),
        #[cfg(target_os = "linux")]
        "ssh" => session::run(session::Kind::Ssh, std::env::args().skip(2).collect()),
        "keygen" => keygen(std::env::args().nth(2)),
        #[cfg(target_os = "linux")]
        "touch-dump" => touchdump::run(std::env::args().nth(2).unwrap_or_else(|| "/dev/input/event1".into())),
        _ => {
            eprintln!("usage: koboterm probe | demo | keygen [PATH]");
            eprintln!("       koboterm run [--type TEXT]... [--hold SECS] -- CMD [ARGS...]");
            eprintln!("       koboterm ssh user@host[:port] [--key PATH] [--cmd COMMAND] [--type TEXT]... [--hold SECS]");
            std::process::exit(2);
        }
    }
}

fn default_key_path() -> std::path::PathBuf {
    let onboard = std::path::Path::new("/mnt/onboard/.adds/koboterm");
    if onboard.parent().map(|p| p.exists()).unwrap_or(false) {
        onboard.join("id_ed25519")
    } else {
        std::path::PathBuf::from("koboterm_id_ed25519")
    }
}

fn keygen(path: Option<String>) -> Result<()> {
    let path = path.map(std::path::PathBuf::from).unwrap_or_else(default_key_path);
    if path.exists() {
        anyhow::bail!("{} already exists; delete it first to regenerate", path.display());
    }
    let public = transport::keygen(&path, "koboterm")?;
    println!("{public}");
    eprintln!("private key: {}", path.display());
    Ok(())
}

/// Run a command under a pty and show it on the panel until it exits.
/// `--type TEXT` queues keystrokes (with `\n` escapes) sent 1 s after start,
/// which stands in for a keyboard until we have one.
#[cfg(target_os = "linux")]
mod session {
    use anyhow::{bail, Result};
    use panel::Panel;
    use render::{Config, Renderer};
    use std::time::{Duration, Instant};
    use term::Terminal;
    use transport::{PtyTransport, SshTarget, SshTransport, Transport};

    pub enum Kind {
        Pty,
        Ssh,
    }

    pub fn run(kind: Kind, args: Vec<String>) -> Result<()> {
        let mut typed: Vec<String> = Vec::new();
        let mut hold = 2u64;
        let mut cmd: Vec<String> = Vec::new();
        let mut key: Option<String> = None;
        let mut remote_cmd: Option<String> = None;
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            match a.as_str() {
                "--type" => typed.push(it.next().unwrap_or_default().replace("\\n", "\n").replace("\\t", "\t")),
                "--hold" => hold = it.next().unwrap_or_default().parse().unwrap_or(2),
                "--key" => key = it.next(),
                "--cmd" => remote_cmd = it.next(),
                "--" => {
                    cmd.extend(it.by_ref());
                    break;
                }
                other => cmd.push(other.to_string()),
            }
        }
        let font = font::Font::from_bdf(font::SPLEEN_16X32);
        let mut panel = panel_fbink::FbinkPanel::open(font)?;
        let g = panel.geometry();
        panel.clear()?;
        let mut tr: Box<dyn Transport> = match kind {
            Kind::Pty => {
                if cmd.is_empty() {
                    cmd.push("/bin/sh".into());
                }
                let argv: Vec<&str> = cmd.iter().map(|s| s.as_str()).collect();
                Box::new(PtyTransport::spawn(&argv, g.cols, g.rows)?)
            }
            Kind::Ssh => {
                let spec = cmd.first().cloned().unwrap_or_default();
                let key_path = key.map(std::path::PathBuf::from).unwrap_or_else(super::default_key_path);
                let target = SshTarget::parse(&spec, &key_path, remote_cmd)?;
                eprintln!("connecting to {}@{}:{} with {}", target.user, target.host, target.port, key_path.display());
                Box::new(SshTransport::connect(target, g.cols, g.rows)?)
            }
        };
        let mut term = Terminal::new(g.cols, g.rows, 0);
        let mut r = Renderer::new(Config::default(), g.cols, g.rows);
        let t0 = Instant::now();
        let ms = |t0: Instant| t0.elapsed().as_millis() as u64;
        let mut buf = [0u8; 8192];
        let mut next_type_at = 1000u64;
        let mut typed = typed.into_iter();
        let mut exited_at: Option<u64> = None;
        loop {
            let n = tr.read(&mut buf, Duration::from_millis(10))?;
            if n > 0 {
                term.feed(&buf[..n]);
            }
            if exited_at.is_none() && ms(t0) >= next_type_at {
                if let Some(s) = typed.next() {
                    tr.write_all(s.as_bytes())?;
                    next_type_at = ms(t0) + 1500;
                }
            }
            let grid = term.snapshot();
            r.tick(ms(t0), &grid, &mut panel);
            if exited_at.is_none() {
                if let Some(st) = tr.exit_status() {
                    eprintln!("child exited with {st}");
                    exited_at = Some(ms(t0));
                }
            }
            if let Some(t) = exited_at {
                if ms(t0) - t > hold * 1000 {
                    break;
                }
            }
        }
        eprintln!("refreshes [fast, partial, full]: {:?}", panel.refreshes);
        if panel.refreshes[0] == 0 && panel.refreshes[2] == 0 {
            bail!("nothing was ever drawn");
        }
        Ok(())
    }
}

/// Scripted terminal session on the real panel: proves glyphs, attributes,
/// scrolling and the refresh scheduler end to end, and prints refresh counts.
#[cfg(target_os = "linux")]
mod demo {
    use anyhow::Result;
    use panel::Panel;
    use render::{Config, Renderer};
    use std::time::{Duration, Instant};
    use term::Terminal;

    pub fn run() -> Result<()> {
        let font = font::Font::from_bdf(font::SPLEEN_16X32);
        let mut panel = panel_fbink::FbinkPanel::open(font)?;
        let g = panel.geometry();
        eprintln!("panel: {} {}x{} px, {}x{} cells", panel.device_name, panel.view.0, panel.view.1, g.cols, g.rows);
        panel.clear()?;
        let mut term = Terminal::new(g.cols, g.rows, 0);
        let mut r = Renderer::new(Config::default(), g.cols, g.rows);
        let t0 = Instant::now();
        let now = |t0: Instant| t0.elapsed().as_millis() as u64;

        let step = |term: &mut Terminal, r: &mut Renderer, panel: &mut panel_fbink::FbinkPanel, bytes: &[u8], wait_ms: u64| {
            term.feed(bytes);
            let end = now(t0) + wait_ms;
            loop {
                let grid = term.snapshot();
                r.tick(now(t0), &grid, panel);
                if now(t0) >= end {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        };

        let w = g.cols as usize;
        let top = format!("\u{250c}{}\u{2510}\r\n", "\u{2500}".repeat(w - 2));
        let bot = format!("\u{2514}{}\u{2518}\r\n", "\u{2500}".repeat(w - 2));
        let line = |s: &str| format!("\u{2502} {:<width$}\u{2502}\r\n", s, width = w - 4);
        step(&mut term, &mut r, &mut panel, top.as_bytes(), 0);
        step(&mut term, &mut r, &mut panel, line("koboterm demo").as_bytes(), 0);
        step(&mut term, &mut r, &mut panel, line(&format!("{}x{} cells, Spleen 16x32", g.cols, g.rows)).as_bytes(), 0);
        step(&mut term, &mut r, &mut panel, line("\x1b[1mbold\x1b[0m \x1b[7minverse\x1b[0m \x1b[4munderline\x1b[0m \x1b[2mdim\x1b[0m").as_bytes(), 0);
        step(&mut term, &mut r, &mut panel, bot.as_bytes(), 800);

        // Streaming: 120 lines at 40 ms each. The scheduler should cap this at ~4 refreshes/s.
        for i in 0..120 {
            step(&mut term, &mut r, &mut panel, format!("{i:4}  the quick brown fox jumps over the lazy dog 0123456789\r\n").as_bytes(), 40);
        }
        let after_stream = panel.refreshes;
        // Spinner for 2 s.
        step(&mut term, &mut r, &mut panel, b"working ", 0);
        for _ in 0..50 {
            for f in [b"\x08|", b"\x08/", b"\x08-", b"\x08\\"] {
                step(&mut term, &mut r, &mut panel, f, 10);
            }
        }
        step(&mut term, &mut r, &mut panel, b"\x08done.\r\n$ ", 6000);
        eprintln!("refreshes [fast, partial, full]: after stream {:?}, total {:?}", after_stream, panel.refreshes);
        Ok(())
    }
}

fn probe() -> Result<()> {
    let mut u: libc::utsname = unsafe { std::mem::zeroed() };
    if unsafe { libc::uname(&mut u) } == 0 {
        let f = |a: &[libc::c_char]| unsafe { std::ffi::CStr::from_ptr(a.as_ptr()) }.to_string_lossy().into_owned();
        println!("uname: {} {} {} {}", f(&u.sysname), f(&u.release), f(&u.machine), f(&u.version));
    }
    println!("pointer width: {} bits", std::mem::size_of::<usize>() * 8);
    #[cfg(target_os = "linux")]
    fb::report()?;
    #[cfg(target_os = "linux")]
    inputs();
    Ok(())
}

/// Print absolute-axis ranges and then raw events for 10 s, for mapping touch.
#[cfg(target_os = "linux")]
mod touchdump {
    use anyhow::{Context, Result};
    use std::os::unix::io::AsRawFd;
    use std::time::{Duration, Instant};

    #[repr(C)]
    #[derive(Default, Debug)]
    struct AbsInfo {
        value: i32,
        minimum: i32,
        maximum: i32,
        fuzz: i32,
        flat: i32,
        resolution: i32,
    }

    pub fn run(path: String) -> Result<()> {
        let f = std::fs::File::open(&path).with_context(|| path.clone())?;
        let fd = f.as_raw_fd();
        // EVIOCGABS(code) = _IOR('E', 0x40 + code, struct input_absinfo)
        for (name, code) in [("ABS_X", 0x00u32), ("ABS_Y", 0x01), ("ABS_MT_SLOT", 0x2f), ("ABS_MT_POSITION_X", 0x35), ("ABS_MT_POSITION_Y", 0x36), ("ABS_MT_TRACKING_ID", 0x39)] {
            let mut ai = AbsInfo::default();
            let req: u32 = (2u32 << 30) | ((core::mem::size_of::<AbsInfo>() as u32) << 16) | ((b'E' as u32) << 8) | (0x40 + code);
            if unsafe { libc::ioctl(fd, req as _, &mut ai) } == 0 {
                println!("{name}: min={} max={} res={}", ai.minimum, ai.maximum, ai.resolution);
            }
        }
        println!("tap the screen; dumping events for 10 s");
        let mut buf = [0u8; 16 * 64];
        let start = Instant::now();
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        while start.elapsed() < Duration::from_secs(10) {
            if unsafe { libc::poll(&mut pfd, 1, 200) } <= 0 {
                continue;
            }
            let n = unsafe { libc::read(fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                break;
            }
            for ev in buf[..n as usize].chunks_exact(16) {
                let ty = u16::from_le_bytes([ev[8], ev[9]]);
                let code = u16::from_le_bytes([ev[10], ev[11]]);
                let val = i32::from_le_bytes([ev[12], ev[13], ev[14], ev[15]]);
                println!("t={:5} type={ty} code=0x{code:02x} val={val}", start.elapsed().as_millis());
            }
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn inputs() {
    if let Ok(rd) = std::fs::read_dir("/sys/class/input") {
        for e in rd.flatten() {
            let p = e.path();
            if let Ok(name) = std::fs::read_to_string(p.join("device/name")) {
                println!("input: {} = {}", e.file_name().to_string_lossy(), name.trim());
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod fb {
    use anyhow::{Context, Result};
    use std::os::unix::io::AsRawFd;

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct Bitfield {
        offset: u32,
        length: u32,
        msb_right: u32,
    }

    #[repr(C)]
    #[derive(Default, Clone, Copy)]
    struct VarScreenInfo {
        xres: u32,
        yres: u32,
        xres_virtual: u32,
        yres_virtual: u32,
        xoffset: u32,
        yoffset: u32,
        bits_per_pixel: u32,
        grayscale: u32,
        red: Bitfield,
        green: Bitfield,
        blue: Bitfield,
        transp: Bitfield,
        nonstd: u32,
        activate: u32,
        height: u32,
        width: u32,
        accel_flags: u32,
        pixclock: u32,
        left_margin: u32,
        right_margin: u32,
        upper_margin: u32,
        lower_margin: u32,
        hsync_len: u32,
        vsync_len: u32,
        sync: u32,
        vmode: u32,
        rotate: u32,
        colorspace: u32,
        reserved: [u32; 4],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct FixScreenInfo {
        id: [u8; 16],
        smem_start: libc::c_ulong,
        smem_len: u32,
        type_: u32,
        type_aux: u32,
        visual: u32,
        xpanstep: u16,
        ypanstep: u16,
        ywrapstep: u16,
        line_length: u32,
        mmio_start: libc::c_ulong,
        mmio_len: u32,
        accel: u32,
        capabilities: u16,
        reserved: [u16; 2],
    }

    const FBIOGET_VSCREENINFO: u32 = 0x4600;
    const FBIOGET_FSCREENINFO: u32 = 0x4602;

    pub fn report() -> Result<()> {
        let f = std::fs::File::open("/dev/fb0").context("open /dev/fb0")?;
        let fd = f.as_raw_fd();
        let mut v = VarScreenInfo::default();
        let mut x: FixScreenInfo = unsafe { std::mem::zeroed() };
        if unsafe { libc::ioctl(fd, FBIOGET_VSCREENINFO as _, &mut v) } != 0 {
            anyhow::bail!("FBIOGET_VSCREENINFO failed: {}", std::io::Error::last_os_error());
        }
        if unsafe { libc::ioctl(fd, FBIOGET_FSCREENINFO as _, &mut x) } != 0 {
            anyhow::bail!("FBIOGET_FSCREENINFO failed: {}", std::io::Error::last_os_error());
        }
        let id = String::from_utf8_lossy(&x.id).trim_end_matches('\0').to_string();
        println!("fb: id={id} {}x{} (virtual {}x{}) bpp={} grayscale={} rotate={} line_length={} smem_len={}",
            v.xres, v.yres, v.xres_virtual, v.yres_virtual, v.bits_per_pixel, v.grayscale, v.rotate, x.line_length, x.smem_len);
        Ok(())
    }
}
