//! Pairing server: while the app runs, the Kobo serves a host-setup script
//! (with this device's public key baked in) and accepts registrations from
//! machines that ran it. Plain HTTP on the LAN, no dependencies.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc;
use ui::HostEntry;

pub const PORT: u16 = 8080;
const SCRIPT: &str = include_str!("../../../host/install-host.sh");

pub struct PairServer {
    pub url: String,
    rx: mpsc::Receiver<HostEntry>,
}

impl PairServer {
    pub fn start(pubkey: &str) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("0.0.0.0", PORT))?;
        let ip = local_ipv4().unwrap_or_else(|| "kobo".into());
        let url = format!("http://{ip}:{PORT}");
        let (tx, rx) = mpsc::channel();
        let script = SCRIPT.replace("__KOBO_KEY__", pubkey.trim()).replace("__KOBO_URL__", &url);
        let pubkey = pubkey.trim().to_string();
        std::thread::Builder::new().name("pair".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = handle(stream, &script, &pubkey, &tx);
            }
        })?;
        Ok(PairServer { url, rx })
    }

    pub fn try_recv(&self) -> Option<HostEntry> {
        self.rx.try_recv().ok()
    }
}

fn handle(mut s: TcpStream, script: &str, pubkey: &str, tx: &mpsc::Sender<HostEntry>) -> std::io::Result<()> {
    s.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let (head_end, mut content_len) = loop {
        let n = s.read(&mut tmp)?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&buf[..pos]).to_string();
            let cl = head
                .lines()
                .find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("content-length")).map(|(_, v)| v.trim().parse::<usize>().unwrap_or(0)))
                .unwrap_or(0);
            break (pos + 4, cl);
        }
        if buf.len() > 65536 {
            return Ok(());
        }
    };
    while buf.len() < head_end + content_len {
        let n = s.read(&mut tmp)?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
        content_len = content_len.min(65536);
    }
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let body = String::from_utf8_lossy(&buf[head_end..(head_end + content_len).min(buf.len())]).to_string();
    let mut parts = head.lines().next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("");
    let path = parts.next().unwrap_or("/");

    let (status, ctype, out) = match (method, path) {
        ("GET", "/install.sh") => ("200 OK", "text/x-shellscript", script.to_string()),
        ("GET", "/key") => ("200 OK", "text/plain", format!("{pubkey}\n")),
        ("GET", "/") => ("200 OK", "text/plain", "koboterm pairing server\n  GET /install.sh   host setup script\n  GET /key          this device's public key\n  POST /register    name=&spec=user@host&cmd=\n".into()),
        ("POST", "/register") => {
            let mut name = String::new();
            let mut spec = String::new();
            let mut cmd = String::new();
            for kv in body.split('&') {
                if let Some((k, v)) = kv.split_once('=') {
                    let v = urldecode(v);
                    match k {
                        "name" => name = v,
                        "spec" => spec = v,
                        "cmd" => cmd = v,
                        _ => {}
                    }
                }
            }
            if name.trim().is_empty() || !spec.contains('@') {
                ("400 Bad Request", "text/plain", "need name and spec=user@host\n".into())
            } else {
                let entry = HostEntry { name: name.trim().to_string(), spec: spec.trim().to_string(), command: Some(cmd.trim().to_string()).filter(|c| !c.is_empty()) };
                let _ = tx.send(entry);
                ("200 OK", "text/plain", "registered\n".into())
            }
        }
        _ => ("404 Not Found", "text/plain", "not found\n".into()),
    };
    let resp = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{out}", out.len());
    s.write_all(resp.as_bytes())?;
    Ok(())
}

pub fn urldecode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() || i + 2 == b.len() => {
                if i + 2 < b.len() + 1 {
                    if let Ok(v) = u8::from_str_radix(&s[i + 1..(i + 3).min(s.len())], 16) {
                        out.push(v);
                        i += 3;
                        continue;
                    }
                }
                out.push(b'%');
            }
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// First non-loopback IPv4 address of this machine.
pub fn local_ipv4() -> Option<String> {
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 {
            return None;
        }
        let mut cur = ifap;
        let mut found = None;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = std::net::Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                if !ip.is_loopback() && !ip.is_link_local() {
                    found = Some(ip.to_string());
                    break;
                }
            }
            cur = ifa.ifa_next;
        }
        libc::freeifaddrs(ifap);
        found
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urldecode_handles_plus_and_percent() {
        assert_eq!(urldecode("MacBook+Pro+M5"), "MacBook Pro M5");
        assert_eq!(urldecode("tunc%40192.168.0.199"), "tunc@192.168.0.199");
        assert_eq!(urldecode("tmux%20new%20-A%20-s%20kobo"), "tmux new -A -s kobo");
        assert_eq!(urldecode("100%"), "100%");
    }

    #[test]
    fn script_has_placeholders_and_local_ip_is_found() {
        assert!(SCRIPT.contains("__KOBO_KEY__") && SCRIPT.contains("__KOBO_URL__"));
        assert!(local_ipv4().is_some());
    }
}
