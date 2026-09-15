//! In-process SSH transport (russh). Runs a small tokio runtime on a
//! background thread so the rest of koboterm stays synchronous; the
//! `Transport` methods talk to it over channels.

use crate::Transport;
use anyhow::{bail, Context, Result};
use russh::client;
use russh::keys::{load_secret_key, PrivateKeyWithHashAlg};
use russh::ChannelMsg;
use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::mpsc as tmpsc;

#[derive(Clone, Debug)]
pub struct SshTarget {
    pub user: String,
    pub host: String,
    pub port: u16,
    pub key_path: std::path::PathBuf,
    /// Command to run instead of a login shell (e.g. `tmux new -A -s kobo`).
    pub command: Option<String>,
}

impl SshTarget {
    /// Parse `user@host[:port]`.
    pub fn parse(spec: &str, key_path: &Path, command: Option<String>) -> Result<Self> {
        let (user, rest) = spec.split_once('@').context("expected user@host[:port]")?;
        let (host, port) = match rest.rsplit_once(':') {
            Some((h, p)) => (h.to_string(), p.parse::<u16>().context("bad port")?),
            None => (rest.to_string(), 22),
        };
        if user.is_empty() || host.is_empty() {
            bail!("expected user@host[:port]");
        }
        Ok(SshTarget { user: user.to_string(), host, port, key_path: key_path.to_path_buf(), command })
    }
}

enum Cmd {
    Data(Vec<u8>),
    Resize(u16, u16),
}

pub struct SshTransport {
    from_server: mpsc::Receiver<Vec<u8>>,
    to_server: tmpsc::UnboundedSender<Cmd>,
    status: Arc<Mutex<Option<i32>>>,
    _rt_thread: std::thread::JoinHandle<()>,
}

struct Handler;

impl client::Handler for Handler {
    type Error = russh::Error;

    // Host keys are not verified yet. TODO: known_hosts under the config dir.
    async fn check_server_key(&mut self, _key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        Ok(true)
    }
}

impl SshTransport {
    /// Connect, authenticate with the key file, open a pty of the given size
    /// and start the shell (or `command`). Blocks until the session is up.
    pub fn connect(target: SshTarget, cols: u16, rows: u16) -> Result<Self> {
        let (from_tx, from_rx) = mpsc::channel::<Vec<u8>>();
        let (to_tx, to_rx) = tmpsc::unbounded_channel::<Cmd>();
        let status = Arc::new(Mutex::new(None));
        let (ready_tx, ready_rx) = mpsc::channel::<Result<()>>();
        let status2 = status.clone();
        let thread = std::thread::Builder::new().name("ssh".into()).spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = ready_tx.send(Err(e.into()));
                    return;
                }
            };
            rt.block_on(async move {
                let outcome = session(target, cols, rows, from_tx, to_rx, ready_tx, status2.clone()).await;
                let mut st = status2.lock().unwrap();
                if st.is_none() {
                    *st = Some(match outcome {
                        Ok(()) => 0,
                        Err(e) => {
                            eprintln!("ssh: {e:#}");
                            255
                        }
                    });
                }
            });
        })?;
        ready_rx.recv().context("ssh thread died")??;
        Ok(SshTransport { from_server: from_rx, to_server: to_tx, status, _rt_thread: thread })
    }
}

async fn session(
    target: SshTarget,
    cols: u16,
    rows: u16,
    from_tx: mpsc::Sender<Vec<u8>>,
    mut to_rx: tmpsc::UnboundedReceiver<Cmd>,
    ready_tx: mpsc::Sender<Result<()>>,
    status: Arc<Mutex<Option<i32>>>,
) -> Result<()> {
    let setup = async {
        let config = Arc::new(client::Config {
            inactivity_timeout: None,
            keepalive_interval: Some(Duration::from_secs(15)),
            keepalive_max: 3,
            ..Default::default()
        });
        let mut handle = client::connect(config, (target.host.as_str(), target.port), Handler)
            .await
            .with_context(|| format!("connect {}:{}", target.host, target.port))?;
        let key = load_secret_key(&target.key_path, None).with_context(|| format!("load key {}", target.key_path.display()))?;
        let hash = handle.best_supported_rsa_hash().await?.flatten();
        let auth = handle.authenticate_publickey(&target.user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await?;
        if !auth.success() {
            bail!("public key authentication rejected for {}", target.user);
        }
        let channel = handle.channel_open_session().await?;
        // Best effort: sshd only accepts these if AcceptEnv allows them (macOS does for LANG/LC_*).
        let _ = channel.set_env(false, "LANG", "en_US.UTF-8").await;
        let _ = channel.set_env(false, "LC_CTYPE", "en_US.UTF-8").await;
        channel.request_pty(false, "xterm-256color", cols as u32, rows as u32, 0, 0, &[]).await?;
        match &target.command {
            Some(c) => channel.exec(true, c.as_str()).await?,
            None => channel.request_shell(true).await?,
        }
        Ok::<_, anyhow::Error>((handle, channel))
    };
    let (handle, mut channel) = match setup.await {
        Ok(v) => v,
        Err(e) => {
            let _ = ready_tx.send(Err(anyhow::anyhow!("{e:#}")));
            return Ok(());
        }
    };
    let _ = ready_tx.send(Ok(()));

    loop {
        tokio::select! {
            msg = channel.wait() => {
                match msg {
                    Some(ChannelMsg::Data { data }) => {
                        if from_tx.send(data.to_vec()).is_err() { break; }
                    }
                    Some(ChannelMsg::ExtendedData { data, .. }) => {
                        if from_tx.send(data.to_vec()).is_err() { break; }
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => {
                        *status.lock().unwrap() = Some(exit_status as i32);
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
            cmd = to_rx.recv() => {
                match cmd {
                    Some(Cmd::Data(bytes)) => channel.data(&bytes[..]).await?,
                    Some(Cmd::Resize(c, r)) => channel.window_change(c as u32, r as u32, 0, 0).await?,
                    None => break,
                }
            }
        }
    }
    let _ = channel.close().await;
    let _ = handle.disconnect(russh::Disconnect::ByApplication, "", "en").await;
    Ok(())
}

impl Transport for SshTransport {
    fn read(&mut self, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
        match self.from_server.recv_timeout(timeout) {
            Ok(data) => {
                let n = data.len().min(buf.len());
                buf[..n].copy_from_slice(&data[..n]);
                // Anything beyond `buf` is rare (8 KB reads); push it back for the next call.
                if n < data.len() {
                    // A bounded local queue would be cleaner; this keeps ordering correct.
                    let rest = data[n..].to_vec();
                    let (tx, rx) = mpsc::channel();
                    let _ = tx.send(rest);
                    while let Ok(d) = self.from_server.try_recv() {
                        let _ = tx.send(d);
                    }
                    self.from_server = rx;
                }
                Ok(n)
            }
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(0),
            Err(mpsc::RecvTimeoutError::Disconnected) => Ok(0),
        }
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.to_server.send(Cmd::Data(bytes.to_vec())).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "ssh session closed"))
    }

    fn resize(&mut self, cols: u16, rows: u16) -> io::Result<()> {
        self.to_server.send(Cmd::Resize(cols, rows)).map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "ssh session closed"))
    }

    fn exit_status(&mut self) -> Option<i32> {
        *self.status.lock().unwrap()
    }
}

/// Generate an ed25519 key pair in OpenSSH format. Returns the public key line.
pub fn keygen(path: &Path, comment: &str) -> Result<String> {
    use russh::keys::ssh_key::{Algorithm, LineEnding, PrivateKey};
    let mut key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).map_err(|e| anyhow::anyhow!("{e}"))?;
    key.set_comment(comment);
    let pem = key.to_openssh(LineEnding::LF).map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(path, pem.as_bytes()).with_context(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // FAT partitions ignore this; harmless there.
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    let public = key.public_key().to_openssh().map_err(|e| anyhow::anyhow!("{e}"))?;
    std::fs::write(path.with_extension("pub"), format!("{public}\n"))?;
    Ok(public)
}
