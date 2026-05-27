use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::Semaphore;
use tracing::{debug, warn};

use crate::source::decoder::decode_hook_payload;
use crate::source::{TaggedSender, Transport};

const MAX_CONCURRENT_CONNS: usize = 128;
const CONN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(1);

pub struct HookSocketListener {
    listener: UnixListener,
    path: PathBuf,
}

impl HookSocketListener {
    pub async fn bind(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        if Path::new(&path).exists() {
            let _ = tokio::fs::remove_file(&path).await;
        }
        // Set umask to 0077 before bind so the socket is created 0700
        // from the start, closing the TOCTOU window between bind and chmod.
        #[cfg(unix)]
        let _restore_umask = {
            let old = unsafe { libc::umask(0o077) };
            struct RestoreUmask(libc::mode_t);
            impl Drop for RestoreUmask {
                fn drop(&mut self) {
                    unsafe {
                        libc::umask(self.0);
                    }
                }
            }
            RestoreUmask(old)
        };
        let listener = UnixListener::bind(&path)
            .with_context(|| format!("binding hook socket at {}", path.display()))?;
        Ok(Self { listener, path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub async fn run(self, tx: TaggedSender) -> Result<()> {
        let sem = Arc::new(Semaphore::new(MAX_CONCURRENT_CONNS));
        loop {
            let permit = match Arc::clone(&sem).acquire_owned().await {
                Ok(p) => p,
                Err(_) => {
                    anyhow::bail!("hook socket semaphore closed unexpectedly");
                }
            };
            match self.listener.accept().await {
                Ok((stream, _addr)) => {
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let _permit = permit;
                        let _ = tokio::time::timeout(CONN_TIMEOUT, handle_conn(stream, tx)).await;
                    });
                }
                Err(e) => {
                    warn!("hook socket accept error: {e}");
                }
            }
        }
    }
}

async fn handle_conn(stream: UnixStream, tx: TaggedSender) {
    let reader = BufReader::new(stream);
    let mut lines = reader.lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                if line.trim().is_empty() {
                    continue;
                }
                let v: serde_json::Value = match serde_json::from_str(&line) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("malformed hook line skipped: {e}");
                        continue;
                    }
                };
                match decode_hook_payload(v) {
                    Ok(ev) => {
                        debug!("hook event: {ev:?}");
                        if tx.send((Transport::Hook, ev)).await.is_err() {
                            return;
                        }
                    }
                    Err(e) => warn!("hook decode error: {e}"),
                }
            }
            Ok(None) => return,
            Err(e) => {
                warn!("hook conn read error: {e}");
                return;
            }
        }
    }
}
