//! Before sign-in: what address the person typed, and whether that server and this app
//! can work together (`GET /api/client`).

use crate::api::{ApiClient, ApiError};
use purlay_contract::models::ClientInfo;
use purlay_contract::CONTRACT_VERSION;

/// The oldest server this client is built to work with.
pub const MIN_SERVER_VERSION: &str = "0.5.0";
/// Contract versions this build speaks.
pub const SUPPORTED_CONTRACTS: &[i64] = &[CONTRACT_VERSION];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    #[error("empty")]
    Empty,
    #[error("not a web address")]
    NotWeb,
    #[error("an address must not carry a user name or password")]
    Credentials,
    #[error("an address is only the server: no path, query or fragment")]
    HasPath,
    #[error("plain http is only allowed for this computer")]
    InsecureHttp,
    #[error("unreadable address")]
    Unreadable,
}

/// Turns what the person typed into a server origin: `chat.example.com` →
/// `https://chat.example.com`. Plain `http` only for loopback, and only in debug builds
/// (or when `allow_http` is set by a developer option).
pub fn parse_server_address(input: &str, allow_http: bool) -> Result<url::Url, AddressError> {
    let trimmed = input.trim().trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(AddressError::Empty);
    }
    let with_scheme = if trimmed.contains("://") { trimmed.to_string() } else { format!("https://{trimmed}") };
    let url = url::Url::parse(&with_scheme).map_err(|_| AddressError::Unreadable)?;
    match url.scheme() {
        "https" => {}
        "http" => {
            let loopback = matches!(url.host_str(), Some("localhost") | Some("127.0.0.1") | Some("[::1]"));
            if !(loopback && (allow_http || cfg!(debug_assertions))) {
                return Err(AddressError::InsecureHttp);
            }
        }
        _ => return Err(AddressError::NotWeb),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(AddressError::Credentials);
    }
    if url.path() != "/" || url.query().is_some() || url.fragment().is_some() {
        return Err(AddressError::HasPath);
    }
    if url.host_str().is_none() {
        return Err(AddressError::Unreadable);
    }
    // An origin: lower-case host (the parser does this), no default port, no path.
    let mut origin = url.clone();
    origin.set_path("/");
    Ok(origin)
}

/// The origin text `https://host[:port]` (no trailing slash).
pub fn origin_text(url: &url::Url) -> String {
    url.origin().ascii_serialization()
}

#[derive(Debug, Clone, PartialEq)]
pub enum Handshake {
    /// Ready to sign in. `origin` is the server's canonical address (`appUrl`): the one the
    /// socket upgrade must name as its `Origin`.
    Ready { info: ClientInfo, origin: url::Url },
    /// The server speaks a contract this build does not, or is older than this app supports.
    ServerTooOld { version: String },
    /// This app is older than the server allows.
    AppTooOld { download_url: Option<String> },
    /// `appUrl` is not a usable address.
    UnusableOrigin,
}

pub fn evaluate(info: ClientInfo) -> Handshake {
    if !SUPPORTED_CONTRACTS.contains(&info.contract_version) {
        if info.contract_version > CONTRACT_VERSION {
            return Handshake::AppTooOld { download_url: None };
        }
        return Handshake::ServerTooOld { version: info.server.version.clone() };
    }
    let min = semver::Version::parse(MIN_SERVER_VERSION).expect("valid");
    match semver::Version::parse(info.server.version.trim_start_matches('v')) {
        Ok(v) if v < min => return Handshake::ServerTooOld { version: info.server.version.clone() },
        Ok(_) => {}
        // A server that reports a version we cannot read is let through: the contract
        // version above already says whether we can talk.
        Err(_) => {}
    }
    let Ok(origin) = url::Url::parse(&info.app_url) else { return Handshake::UnusableOrigin };
    if !matches!(origin.scheme(), "https" | "http") || origin.host_str().is_none() {
        return Handshake::UnusableOrigin;
    }
    Handshake::Ready { info, origin }
}

pub async fn handshake(address: &url::Url, app_version: &str) -> Result<Handshake, ApiError> {
    let api = ApiClient::new(address.clone(), app_version);
    let info = api.get_client_info().await?;
    Ok(evaluate(info))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_host_gets_https() {
        assert_eq!(parse_server_address("Chat.Example.com", false).unwrap().as_str(), "https://chat.example.com/");
        assert_eq!(parse_server_address("https://chat.example.com:443/", false).unwrap().as_str(), "https://chat.example.com/");
        assert_eq!(parse_server_address("chat.example.com:8443", false).unwrap().as_str(), "https://chat.example.com:8443/");
    }

    #[test]
    fn refuses_what_is_not_an_origin() {
        assert_eq!(parse_server_address("", false), Err(AddressError::Empty));
        assert_eq!(parse_server_address("https://u:p@chat.example.com", false), Err(AddressError::Credentials));
        assert_eq!(parse_server_address("https://chat.example.com/w/x", false), Err(AddressError::HasPath));
        assert_eq!(parse_server_address("https://chat.example.com/?a=1", false), Err(AddressError::HasPath));
        assert_eq!(parse_server_address("ftp://chat.example.com", false), Err(AddressError::NotWeb));
        assert_eq!(parse_server_address("http://chat.example.com", true), Err(AddressError::InsecureHttp));
        assert!(parse_server_address("http://localhost:4390", true).is_ok());
    }

    fn info(version: &str, contract: i64, app_url: &str) -> ClientInfo {
        serde_json::from_value(serde_json::json!({
            "server": {"version": version}, "contractVersion": contract, "appUrl": app_url,
            "minClient": {"macos": {"contractVersion": null, "appVersion": null, "downloadUrl": null}},
            "auth": {"github": true, "google": false, "email": true, "oidc": null, "signUp": "open", "password": false}
        }))
        .unwrap()
    }

    #[test]
    fn evaluates_versions() {
        assert!(matches!(evaluate(info("0.6.1", 1, "https://chat.example.com")), Handshake::Ready { .. }));
        assert!(matches!(evaluate(info("0.4.9", 1, "https://chat.example.com")), Handshake::ServerTooOld { .. }));
        assert!(matches!(evaluate(info("0.9.0", 2, "https://chat.example.com")), Handshake::AppTooOld { .. }));
        assert!(matches!(evaluate(info("0.6.1", 1, "not a url")), Handshake::UnusableOrigin));
    }
}
