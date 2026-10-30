//! Non-secret settings, in `~/.config/schw/config.toml`.
//!
//! Nothing credential-bearing is kept here. The Schwab app key and app secret
//! live in the OS keychain as one blob, and so do the OAuth tokens (see
//! [`crate::credential`]) — the refresh token in particular is a multi-day key
//! to a brokerage account and has no business in a plaintext dotfile.
//!
//! Config holds only what is safe to `cat`: which host to talk to, which
//! redirect URI the app was registered with, and which account a bare command
//! defaults to.
//!
//! # On `account`
//!
//! Schwab addresses an account in requests by an opaque `hashValue`, not by its
//! account number. That hash is **not** an anonymization — it is a stable
//! handle to one specific brokerage account, and anyone holding it plus a token
//! can read that account. It is stored here because it is the only way to have
//! a usable default, and because it is strictly less sensitive than the account
//! number it stands for. Treat the file accordingly.

use serde::{Deserialize, Serialize};

/// Keychain account under `piekstra.schw` holding the app key/secret JSON.
pub const KEYCHAIN_APP: &str = "app-credentials";

/// Keychain account under `piekstra.schw` holding the OAuth token set.
pub const KEYCHAIN_TOKENS: &str = "oauth-tokens";

/// Schwab's API host. Both the Trader API and the Market Data API are served
/// from here.
pub const DEFAULT_BASE_URL: &str = "https://api.schwabapi.com";

/// The redirect URI this CLI assumes the app was registered with.
///
/// Schwab requires the callback to be an **https** URL and accepts a loopback
/// address, so `https://127.0.0.1` is the conventional choice for a local tool
/// and the one the ecosystem's other clients default to. It must match the
/// value registered on developer.schwab.com **exactly** — including the port
/// and any trailing path — or Schwab rejects the authorization request before
/// the user ever sees a login page.
pub const DEFAULT_REDIRECT_URI: &str = "https://127.0.0.1";

pub const KNOWN_KEYS: &[&str] = &["base_url", "redirect_uri", "account"];

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Config {
    /// Override the API host (for a proxy or a replay fixture server).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// The callback URL registered with the Schwab app.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    /// Default account hash for commands that take `--account`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
}

impl Config {
    pub fn base_url(&self) -> String {
        self.base_url
            .clone()
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string())
            .trim_end_matches('/')
            .to_string()
    }

    pub fn redirect_uri(&self) -> String {
        self.redirect_uri
            .clone()
            .unwrap_or_else(|| DEFAULT_REDIRECT_URI.to_string())
    }

    pub fn account(&self) -> Option<String> {
        self.account.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_url_defaults_and_strips_trailing_slash() {
        assert_eq!(Config::default().base_url(), DEFAULT_BASE_URL);
        let cfg = Config {
            base_url: Some("https://example.test/".into()),
            ..Default::default()
        };
        assert_eq!(cfg.base_url(), "https://example.test");
    }

    /// The redirect URI must survive round-tripping *unchanged*: Schwab
    /// compares it byte-for-byte against the registered value, so "helpfully"
    /// trimming a trailing slash here would break an app registered with one.
    #[test]
    fn redirect_uri_is_returned_verbatim() {
        let cfg = Config {
            redirect_uri: Some("https://127.0.0.1:8182/".into()),
            ..Default::default()
        };
        assert_eq!(cfg.redirect_uri(), "https://127.0.0.1:8182/");
        assert_eq!(Config::default().redirect_uri(), DEFAULT_REDIRECT_URI);
    }

    /// An empty config must serialize to an empty table, not a wall of nulls —
    /// `config show` renders this straight to the user.
    #[test]
    fn empty_config_serializes_empty() {
        let v = serde_json::to_value(Config::default()).unwrap();
        assert_eq!(v, serde_json::json!({}));
    }
}
