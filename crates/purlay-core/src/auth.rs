//! Native sign-in (server D-188, D-191): the system browser signs the person in; the app
//! receives a one-time code on `purlay://auth` and exchanges it, with the PKCE verifier,
//! for the same session cookie the web uses.
//!
//! 1. `Handoff::new()` makes a verifier (32 random bytes, base64url), its S256 challenge and
//!    a state, and `sign_in_url` is opened in the browser.
//! 2. The server's page calls `POST /api/native/authorize` and sends the browser to
//!    `purlay://auth?code=<64 hex>&state=<state>`; Windows hands that URL to this app.
//! 3. `Handoff::read_callback` checks it (state first), and `exchange` calls
//!    `POST /api/native/token` once — the code is single-use — and reads the cookie.

use crate::api::{to_body, ApiClient, ApiError, SESSION_COOKIE};
use base64::Engine;
use purlay_contract::constants::{NATIVE_CODE_PATTERN, NATIVE_REDIRECT};
use purlay_contract::endpoints;
use purlay_contract::models::{NativeTokenRequest, NativeTokenRequestDevice};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

#[derive(Debug, Clone)]
pub struct Handoff {
    pub verifier: String,
    pub challenge: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CallbackError {
    #[error("not a sign-in address")]
    NotSignIn,
    #[error("the sign-in answer does not belong to this attempt")]
    StateMismatch,
    #[error("the sign-in answer is malformed")]
    Malformed,
}

impl Handoff {
    pub fn new() -> Self {
        let verifier = b64url(&rand::random::<[u8; 32]>());
        let challenge = b64url(&Sha256::digest(verifier.as_bytes()));
        let state = b64url(&rand::random::<[u8; 16]>());
        Handoff { verifier, challenge, state }
    }

    /// `https://<server>/sign-in/native?challenge=…&state=…`
    pub fn sign_in_url(&self, origin: &url::Url) -> url::Url {
        let mut u = origin.join("/sign-in/native").expect("path");
        u.query_pairs_mut().append_pair("challenge", &self.challenge).append_pair("state", &self.state);
        u
    }

    /// Checks `purlay://auth?code=…&state=…` and returns the code.
    pub fn read_callback(&self, raw: &str) -> Result<String, CallbackError> {
        let url = url::Url::parse(raw.trim()).map_err(|_| CallbackError::NotSignIn)?;
        let expected = url::Url::parse(NATIVE_REDIRECT).expect("redirect");
        if url.scheme() != expected.scheme() || url.host_str() != expected.host_str() {
            return Err(CallbackError::NotSignIn);
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(CallbackError::Malformed);
        }
        let mut code = None;
        let mut state = None;
        for (k, v) in url.query_pairs() {
            let slot = match &*k {
                "code" => &mut code,
                "state" => &mut state,
                _ => continue,
            };
            if slot.is_some() {
                return Err(CallbackError::Malformed);
            }
            *slot = Some(v.into_owned());
        }
        // State first: an answer for another attempt is not this one's, whatever its code.
        if state.as_deref() != Some(self.state.as_str()) {
            return Err(CallbackError::StateMismatch);
        }
        let code = code.ok_or(CallbackError::Malformed)?;
        let re = regex::Regex::new(NATIVE_CODE_PATTERN).expect("pattern");
        if !re.is_match(&code) {
            return Err(CallbackError::Malformed);
        }
        Ok(code)
    }
}

impl Default for Handoff {
    fn default() -> Self {
        Self::new()
    }
}

/// Whether a URL the OS handed us is a sign-in answer (as opposed to `purlay://open?...`).
pub fn is_sign_in_callback(raw: &str) -> bool {
    url::Url::parse(raw).is_ok_and(|u| u.scheme() == "purlay" && u.host_str() == Some("auth"))
}

/// The session as kept in the credential store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredSession {
    pub cookie: String,
    /// Milliseconds since the epoch, from the cookie's `Max-Age`.
    pub expires_at_ms: Option<i64>,
    /// The server origin this session belongs to.
    pub origin: String,
}

/// Reads `__Host-purlay_session=<value>; Max-Age=<s>; …` → (value, max-age seconds).
pub fn parse_session_cookie(header: &str) -> Option<(String, Option<i64>)> {
    let mut parts = header.split(';');
    let first = parts.next()?.trim();
    let (name, value) = first.split_once('=')?;
    if name.trim() != SESSION_COOKIE || value.trim().is_empty() {
        return None;
    }
    let mut max_age = None;
    for p in parts {
        if let Some((k, v)) = p.trim().split_once('=') {
            if k.trim().eq_ignore_ascii_case("max-age") {
                max_age = v.trim().parse::<i64>().ok();
            }
        }
    }
    Some((value.trim().trim_matches('"').to_string(), max_age))
}

/// A device name the server accepts: 1–64 characters, no control, bidi or zero-width ones.
pub fn device_name() -> String {
    let raw = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "Windows PC".into());
    let clean: String = raw
        .chars()
        .filter(|c| {
            let u = *c as u32;
            !(u <= 0x1f || (0x7f..=0x9f).contains(&u) || (0x200b..=0x200f).contains(&u) || (0x2028..=0x202e).contains(&u) || (0x2066..=0x2069).contains(&u) || u == 0xfeff)
        })
        .take(64)
        .collect();
    if clean.trim().is_empty() { "Windows PC".into() } else { clean }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ExchangeError {
    /// `404 NATIVE_CODE_NOT_FOUND`: expired or used; start again.
    #[error("the sign-in code expired; start again")]
    CodeNotFound,
    #[error("too many attempts; wait and try again")]
    RateLimited { retry_after_s: Option<u64> },
    #[error("the server did not return a session")]
    NoCookie,
    #[error(transparent)]
    Api(#[from] ApiError),
}

/// `POST /api/native/token` — never retried, the code is single-use.
pub async fn exchange(api: &ApiClient, handoff: &Handoff, code: &str, now_ms: i64) -> Result<StoredSession, ExchangeError> {
    let body = NativeTokenRequest {
        code: code.to_string(),
        verifier: handoff.verifier.clone(),
        device: NativeTokenRequestDevice { name: device_name() },
    };
    let bytes = to_body(&body)?;
    let res = api.send(&endpoints::NATIVE_TOKEN, &[], None, Some((bytes, "application/json"))).await;
    let res = match res {
        Ok(r) => r,
        Err(e) if e.status() == Some(404) => return Err(ExchangeError::CodeNotFound),
        Err(ApiError::Server { status: 429, retry_after, .. }) => {
            return Err(ExchangeError::RateLimited { retry_after_s: retry_after.map(|d| d.as_secs()) });
        }
        Err(e) => return Err(e.into()),
    };
    for v in res.headers().get_all("set-cookie") {
        if let Some((cookie, max_age)) = v.to_str().ok().and_then(parse_session_cookie) {
            return Ok(StoredSession {
                cookie,
                expires_at_ms: max_age.map(|s| now_ms + s * 1000),
                origin: crate::handshake::origin_text(api.origin()),
            });
        }
    }
    Err(ExchangeError::NoCookie)
}

/// Where sessions are kept: Windows Credential Manager, one entry per server origin.
pub mod credentials {
    use super::StoredSession;

    const SERVICE: &str = "dev.purlay.windows.session";

    pub fn load(origin: &str) -> Option<StoredSession> {
        let entry = keyring::Entry::new(SERVICE, origin).ok()?;
        let text = entry.get_password().ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(session: &StoredSession) -> Result<(), String> {
        let entry = keyring::Entry::new(SERVICE, &session.origin).map_err(|e| e.to_string())?;
        let text = serde_json::to_string(session).map_err(|e| e.to_string())?;
        entry.set_password(&text).map_err(|e| e.to_string())
    }

    pub fn delete(origin: &str) {
        if let Ok(entry) = keyring::Entry::new(SERVICE, origin) {
            let _ = entry.delete_credential();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn challenge_is_s256_of_verifier() {
        let h = Handoff::new();
        assert_eq!(h.verifier.len(), 43);
        assert_eq!(h.challenge.len(), 43);
        assert!(regex::Regex::new(purlay_contract::constants::NATIVE_VERIFIER_PATTERN).unwrap().is_match(&h.verifier));
        assert!(regex::Regex::new(purlay_contract::constants::NATIVE_CHALLENGE_PATTERN).unwrap().is_match(&h.challenge));
        assert!(regex::Regex::new(purlay_contract::constants::NATIVE_STATE_PATTERN).unwrap().is_match(&h.state));
        assert_eq!(h.challenge, b64url(&Sha256::digest(h.verifier.as_bytes())));
    }

    #[test]
    fn reads_the_callback() {
        let h = Handoff::new();
        let code = "a".repeat(64);
        let ok = format!("purlay://auth?code={code}&state={}", h.state);
        assert_eq!(h.read_callback(&ok).unwrap(), code);
        assert_eq!(h.read_callback(&format!("purlay://auth?code={code}&state=other")), Err(CallbackError::StateMismatch));
        assert_eq!(h.read_callback(&format!("purlay://auth?code=XYZ&state={}", h.state)), Err(CallbackError::Malformed));
        assert_eq!(
            h.read_callback(&format!("purlay://auth?code={code}&code={code}&state={}", h.state)),
            Err(CallbackError::Malformed)
        );
        assert_eq!(h.read_callback("purlay://open?url=x"), Err(CallbackError::NotSignIn));
        assert_eq!(h.read_callback("https://auth?code=1"), Err(CallbackError::NotSignIn));
    }

    #[test]
    fn parses_cookie() {
        let (v, age) = parse_session_cookie("__Host-purlay_session=abc.def; Max-Age=2592000; Path=/; HttpOnly; Secure; SameSite=Lax").unwrap();
        assert_eq!(v, "abc.def");
        assert_eq!(age, Some(2592000));
        assert!(parse_session_cookie("other=1").is_none());
    }

    #[test]
    fn device_names_are_clean() {
        let n = device_name();
        assert!(!n.is_empty() && n.chars().count() <= 64);
    }
}
