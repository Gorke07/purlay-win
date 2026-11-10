//! What Purlay needs from Windows itself.
//!
//! - **`purlay:` links.** The server's sign-in page sends the browser to
//!   `purlay://auth?code=…&state=…` (fixed by the server, `NATIVE_REDIRECT`), and message
//!   links arrive as `purlay://open?url=…`. The scheme is registered per user under
//!   `HKCU\Software\Classes\purlay`, pointing at this executable with the URL as `%1`.
//! - **One instance.** Windows starts a new process for every link; the first instance
//!   owns a named pipe and later ones hand their arguments to it and exit.
//! - **Toasts, the taskbar and launch at login.**

pub mod instance;

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::*;

#[cfg(not(windows))]
mod other {
    pub const AUMID: &str = "dev.purlay.windows";
    pub fn register_protocol() -> anyhow::Result<()> {
        Ok(())
    }
    pub fn register_app_identity(_icon_png: Option<&std::path::Path>) {}
    pub fn set_launch_at_login(_on: bool) -> anyhow::Result<()> {
        Ok(())
    }
    pub fn launch_at_login() -> bool {
        false
    }
    pub fn toast(_title: &str, _body: &str, _launch: Option<&str>, _silent: bool) {}
    pub fn flash_window(_hwnd: isize, _on: bool) {}
}
#[cfg(not(windows))]
pub use other::*;

/// The `purlay:` URL among the process arguments, if any.
pub fn url_from_args(args: &[String]) -> Option<String> {
    args.iter().skip(1).find(|a| a.to_ascii_lowercase().starts_with("purlay:")).cloned()
}
