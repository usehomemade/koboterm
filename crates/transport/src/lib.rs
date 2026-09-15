//! Transport: the byte pipe between the terminal model and whatever produces
//! the session. `PtyTransport` runs a local command under a pseudo-terminal;
//! it is also how an external `ssh`/`mosh-client` would be driven. An
//! in-process SSH transport can implement the same trait later.

pub mod ssh;
pub use ssh::{keygen, SshTarget, SshTransport, LOST};

use anyhow::{bail, Context, Result};
use std::ffi::CString;
use std::io;
use std::os::unix::io::RawFd;
use std::time::Duration;

pub trait Transport {
    /// Wait up to `timeout` for data; return bytes read (0 = nothing yet).
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize>;
    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()>;
    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()>;
    /// `Some(status)` once the far end has gone away.
    fn exit_status(&mut self) -> Option<i32>;
}

pub struct PtyTransport {
    master: RawFd,
    pid: libc::pid_t,
    status: Option<i32>,
}

impl PtyTransport {
    /// Spawn `argv[0]` with `argv` under a new pty of the given size.
    pub fn spawn(argv: &[&str], cols: u16, rows: u16) -> Result<Self> {
        if argv.is_empty() {
            bail!("empty command");
        }
        let cargs: Vec<CString> = argv.iter().map(|a| CString::new(*a)).collect::<Result<_, _>>()?;
        let mut ptrs: Vec<*const libc::c_char> = cargs.iter().map(|c| c.as_ptr()).collect();
        ptrs.push(std::ptr::null());
        let mut ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        let mut master: RawFd = -1;
        let term = CString::new("TERM=xterm-256color").unwrap();
        // SAFETY: forkpty is async-signal-safe enough for our use: the child only
        // calls setenv/execvp, which is the documented pattern.
        let pid = unsafe { libc::forkpty(&mut master, std::ptr::null_mut(), std::ptr::null_mut(), &mut ws) };
        if pid < 0 {
            return Err(io::Error::last_os_error()).context("forkpty");
        }
        if pid == 0 {
            unsafe {
                libc::putenv(term.into_raw());
                libc::execvp(ptrs[0], ptrs.as_ptr());
                libc::_exit(127);
            }
        }
        Ok(PtyTransport { master, pid, status: None })
    }

    fn reap(&mut self, block: bool) {
        if self.status.is_some() {
            return;
        }
        let mut st: libc::c_int = 0;
        let flags = if block { 0 } else { libc::WNOHANG };
        let r = unsafe { libc::waitpid(self.pid, &mut st, flags) };
        if r == self.pid {
            self.status = Some(if libc::WIFEXITED(st) { libc::WEXITSTATUS(st) } else { 128 + libc::WTERMSIG(st) });
        }
    }
}

impl Transport for PtyTransport {
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        let mut pfd = libc::pollfd { fd: self.master, events: libc::POLLIN, revents: 0 };
        let ms = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
        let n = unsafe { libc::poll(&mut pfd, 1, ms) };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                return Ok(0);
            }
            return Err(e);
        }
        if n == 0 {
            self.reap(false);
            return Ok(0);
        }
        let r = unsafe { libc::read(self.master, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
        if r < 0 {
            let e = io::Error::last_os_error();
            // EIO on Linux means the slave side closed: the child is gone.
            if e.raw_os_error() == Some(libc::EIO) {
                self.reap(true);
                return Ok(0);
            }
            return Err(e);
        }
        if r == 0 {
            self.reap(true);
        }
        Ok(r as usize)
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let w = unsafe { libc::write(self.master, bytes.as_ptr() as *const libc::c_void, bytes.len()) };
            if w < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            bytes = &bytes[w as usize..];
        }
        Ok(())
    }

    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        if unsafe { libc::ioctl(self.master, libc::TIOCSWINSZ as _, &ws) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn exit_status(&mut self) -> Option<i32> {
        self.reap(false);
        self.status
    }
}

impl Drop for PtyTransport {
    fn drop(&mut self) {
        unsafe {
            libc::close(self.master);
            if self.status.is_none() {
                libc::kill(self.pid, libc::SIGHUP);
                self.reap(true);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(t: &mut PtyTransport, _want: &str, budget_ms: u64) -> (String, Option<i32>) {
        let mut out = String::new();
        let mut buf = [0u8; 4096];
        let start = std::time::Instant::now();
        loop {
            let n = t.read(&mut buf, Duration::from_millis(50)).unwrap();
            out.push_str(&String::from_utf8_lossy(&buf[..n]));
            if let Some(s) = t.exit_status() {
                // drain leftovers
                while let Ok(n) = t.read(&mut buf, Duration::from_millis(20)) {
                    if n == 0 { break; }
                    out.push_str(&String::from_utf8_lossy(&buf[..n]));
                }
                return (out, Some(s));
            }
            if start.elapsed().as_millis() as u64 > budget_ms {
                return (out, None);
            }
        }
    }

    #[test]
    fn spawns_a_command_and_reads_its_output() {
        let mut t = PtyTransport::spawn(&["sh", "-c", "echo hello-pty; exit 3"], 80, 24).unwrap();
        let (out, status) = drain(&mut t, "hello-pty", 3000);
        assert!(out.contains("hello-pty"), "{out:?}");
        assert_eq!(status, Some(3));
    }

    #[test]
    fn writes_reach_the_child_and_size_is_applied() {
        let mut t = PtyTransport::spawn(&["sh", "-c", "read x; stty size; echo got=$x"], 67, 45).unwrap();
        t.write_all(b"ping\n").unwrap();
        let (out, _) = drain(&mut t, "got=ping", 3000);
        assert!(out.contains("45 67"), "{out:?}");
        assert!(out.contains("got=ping"), "{out:?}");
    }
}
