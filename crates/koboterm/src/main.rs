//! koboterm entry point. For now only `probe`, which reports what the device
//! looks like from userland so the toolchain and framebuffer assumptions can be
//! checked before any drawing code exists.

use anyhow::Result;

fn main() -> Result<()> {
    let cmd = std::env::args().nth(1).unwrap_or_default();
    match cmd.as_str() {
        "probe" => probe(),
        _ => {
            eprintln!("usage: koboterm probe");
            std::process::exit(2);
        }
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
