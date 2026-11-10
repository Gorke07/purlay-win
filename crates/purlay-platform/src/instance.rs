//! One running Purlay per user session: the first process listens on a named pipe; any
//! later one (started by a `purlay:` link or a second launch) sends its arguments there and
//! exits, and the first brings its window forward.

use tokio::sync::mpsc;

/// Per-user pipe name: two people signed in to the same machine have separate instances.
pub fn pipe_name() -> String {
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    let clean: String = user.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    // A development profile (`PURLAY_PROFILE`) is its own instance.
    let profile: String = std::env::var("PURLAY_PROFILE").unwrap_or_default().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if profile.is_empty() { format!(r"\\.\pipe\purlay-instance-{clean}") } else { format!(r"\\.\pipe\purlay-instance-{clean}-{profile}") }
}

/// A message from a later instance: its arguments (empty when it was a plain launch).
#[derive(Debug, Clone)]
pub struct Activation {
    pub args: Vec<String>,
}

pub enum Claim {
    /// This process is the first; activations arrive on the receiver.
    Primary(mpsc::UnboundedReceiver<Activation>),
    /// Another instance is running and has been handed our arguments.
    Forwarded,
}

#[cfg(windows)]
pub fn claim(rt: &tokio::runtime::Handle, args: Vec<String>) -> Claim {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

    let name = pipe_name();
    // Try to be the server first: `first_pipe_instance` fails when another process owns it.
    let first = {
        let _g = rt.enter();
        ServerOptions::new().first_pipe_instance(true).create(&name)
    };
    match first {
        Ok(server) => {
            let (tx, rx) = mpsc::unbounded_channel();
            let name2 = name.clone();
            rt.spawn(async move {
                let mut server = server;
                loop {
                    if server.connect().await.is_err() {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    loop {
                        match server.read(&mut chunk).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&chunk[..n]);
                                if buf.len() > 64 * 1024 {
                                    break;
                                }
                            }
                        }
                    }
                    let text = String::from_utf8_lossy(&buf);
                    let args: Vec<String> = text.split('\0').filter(|s| !s.is_empty()).map(String::from).collect();
                    let _ = tx.send(Activation { args });
                    match ServerOptions::new().create(&name2) {
                        Ok(next) => server = next,
                        Err(e) => {
                            tracing::warn!("instance pipe: {e}");
                            return;
                        }
                    }
                }
            });
            Claim::Primary(rx)
        }
        Err(_) => {
            let payload = args.join("\0");
            let sent = rt.block_on(async move {
                for _ in 0..20 {
                    match ClientOptions::new().open(&name) {
                        Ok(mut c) => {
                            let _ = c.write_all(payload.as_bytes()).await;
                            let _ = c.shutdown().await;
                            return true;
                        }
                        Err(_) => tokio::time::sleep(std::time::Duration::from_millis(100)).await,
                    }
                }
                false
            });
            if sent {
                Claim::Forwarded
            } else {
                // The other instance did not answer: run anyway rather than do nothing.
                let (_tx, rx) = mpsc::unbounded_channel();
                Claim::Primary(rx)
            }
        }
    }
}

#[cfg(not(windows))]
pub fn claim(_rt: &tokio::runtime::Handle, _args: Vec<String>) -> Claim {
    let (_tx, rx) = mpsc::unbounded_channel();
    Claim::Primary(rx)
}
