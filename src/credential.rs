//! The two secrets this CLI holds, and how they are parsed, masked, and kept
//! out of logs.
//!
//! 1. **The app credential** — the app key and app secret issued by
//!    developer.schwab.com when an application is registered. Long-lived;
//!    changes only when the owner rotates it in the portal.
//! 2. **The token set** — the OAuth access token (~30 minutes) and refresh
//!    token (7 days) obtained by `schw auth login`. Short-lived, rewritten
//!    every time the access token is refreshed.
//!
//! Both live in the OS keychain under `piekstra.schw`, in separate entries, so
//! `auth logout` can drop the session without discarding the app registration —
//! re-registering an app costs days of Schwab's approval queue, and no CLI
//! command should ever be able to throw that away by accident.
//!
//! Every type here implements [`std::fmt::Debug`] **by hand** so that a stray
//! `{:?}` in a future debug session cannot print a bearer token to a terminal
//! or a log file.

use pk_cli_core::CliError;
use pk_cli_secrets::{CredentialStore, Secret};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::config::{KEYCHAIN_APP, KEYCHAIN_TOKENS};

/// The app key/secret pair from developer.schwab.com.
#[derive(Clone, Serialize, Deserialize)]
pub struct AppCredential {
    pub app_key: String,
    pub app_secret: String,
}

impl std::fmt::Debug for AppCredential {
    /// Hand-written: the derived impl would print the secret.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppCredential")
            .field("app_key", &self.masked_key())
            .field("app_secret", &"<redacted>")
            .finish()
    }
}

impl AppCredential {
    /// Parse the app credential from whatever shape the owner pasted.
    ///
    /// Schwab's portal shows the two values in a web page rather than offering
    /// a download, so there is no canonical file format to standardize on and
    /// every user arrives with a different transcription. Three shapes are
    /// accepted:
    ///
    /// - JSON: `{"app_key": "...", "app_secret": "..."}`, also accepting the
    ///   OAuth spellings `client_id`/`client_secret` and the portal's own
    ///   `appKey`/`appSecret` casing.
    /// - `KEY:SECRET` on one line.
    /// - The key and the secret on two lines.
    ///
    /// Being liberal here is not sloppiness: the alternative is a user who
    /// stores a subtly wrong credential and then debugs `invalid_client`
    /// against Schwab, which tells them nothing about which half was wrong.
    pub fn parse(raw: &str) -> Result<Self, CliError> {
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(CliError::Usage("empty app credential".into()));
        }

        if raw.starts_with('{') {
            let v: Value = serde_json::from_str(raw)
                .map_err(|e| CliError::Usage(format!("app credential is not valid JSON: {e}")))?;
            let key = first_str(&v, &["app_key", "appKey", "client_id", "clientId", "key"]);
            let secret = first_str(
                &v,
                &[
                    "app_secret",
                    "appSecret",
                    "client_secret",
                    "clientSecret",
                    "secret",
                ],
            );
            return match (key, secret) {
                (Some(k), Some(s)) => Self::checked(k, s),
                _ => Err(CliError::Usage(
                    "app credential JSON needs both an app key and an app secret \
                     (keys: app_key/app_secret, client_id/client_secret)"
                        .into(),
                )),
            };
        }

        let lines: Vec<&str> = raw
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        match lines.as_slice() {
            // Two lines: key then secret.
            [k, s] => Self::checked(k.to_string(), s.to_string()),
            // One line: KEY:SECRET.
            [one] => match one.split_once(':') {
                Some((k, s)) => Self::checked(k.trim().to_string(), s.trim().to_string()),
                None => Err(CliError::Usage(
                    "app credential looks like a key with no secret — supply \
                     `APPKEY:APPSECRET`, two lines, or the JSON form"
                        .into(),
                )),
            },
            _ => Err(CliError::Usage(
                "could not read an app key and app secret from that input".into(),
            )),
        }
    }

    fn checked(app_key: String, app_secret: String) -> Result<Self, CliError> {
        if app_key.is_empty() || app_secret.is_empty() {
            return Err(CliError::Usage(
                "both the app key and the app secret must be non-empty".into(),
            ));
        }
        // Schwab rejects the portal's placeholder text with a bare
        // `invalid_client`, which reads like the credential is wrong rather
        // than absent. Catch the common placeholders locally, where the message
        // can actually say what happened.
        for placeholder in ["PUT_APP_KEY_HERE", "PUT_APP_SECRET_HERE", "YOUR_APP_KEY"] {
            if app_key.eq_ignore_ascii_case(placeholder)
                || app_secret.eq_ignore_ascii_case(placeholder)
            {
                return Err(CliError::Usage(format!(
                    "`{placeholder}` is a placeholder, not a credential — copy the real \
                     values from your app's page on developer.schwab.com"
                )));
            }
        }
        Ok(Self {
            app_key,
            app_secret,
        })
    }

    /// The app key with all but its last four characters hidden, for display.
    pub fn masked_key(&self) -> String {
        mask(&self.app_key)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }
}

/// An OAuth token set as persisted between runs.
///
/// `expires_at` and `refresh_expires_at` are absolute Unix seconds rather than
/// the `expires_in` durations Schwab returns, because a duration is meaningless
/// once it has been written to disk and read back an hour later.
#[derive(Clone, Serialize, Deserialize)]
pub struct TokenSet {
    pub access_token: String,
    pub refresh_token: String,
    /// When the access token stops working (Unix seconds).
    pub expires_at: u64,
    /// When the refresh token stops working (Unix seconds).
    ///
    /// **Computed locally, not reported by Schwab.** The token response carries
    /// `expires_in` for the access token only; the refresh token's lifetime is
    /// documented as 7 days and is not in the payload, so this is
    /// `issued_at + 7 days`. It is therefore an estimate, and the honest
    /// failure mode is that it says "valid" for a token Schwab has already
    /// retired — which surfaces as an ordinary auth error on next use.
    pub refresh_expires_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

impl std::fmt::Debug for TokenSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenSet")
            .field("access_token", &"<redacted>")
            .field("refresh_token", &"<redacted>")
            .field("expires_at", &self.expires_at)
            .field("refresh_expires_at", &self.refresh_expires_at)
            .finish()
    }
}

/// Treat an access token expiring within this many seconds as already gone, so
/// a command doesn't start work that will 401 halfway through a paged read.
pub const EXPIRY_SKEW_SECS: u64 = 60;

/// Schwab's documented refresh-token lifetime. Not in the token response — see
/// [`TokenSet::refresh_expires_at`].
pub const REFRESH_TOKEN_LIFETIME_SECS: u64 = 7 * 24 * 60 * 60;

impl TokenSet {
    pub fn access_expired(&self, now: u64) -> bool {
        self.expires_at.saturating_sub(EXPIRY_SKEW_SECS) <= now
    }

    /// Whether the refresh token is past its estimated life, in which case no
    /// silent recovery is possible and the user must run `auth login` again.
    pub fn refresh_expired(&self, now: u64) -> bool {
        self.refresh_expires_at <= now
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn parse(raw: &str) -> Result<Self, CliError> {
        serde_json::from_str(raw).map_err(|e| {
            CliError::Auth(format!(
                "stored token set is unreadable ({e}) — run `schw auth login`"
            ))
        })
    }
}

/// Read the stored app credential, if there is one.
pub fn read_app(creds: &CredentialStore) -> Result<Option<AppCredential>, CliError> {
    match creds.get(KEYCHAIN_APP)? {
        Some(s) => Ok(Some(AppCredential::parse(s.expose())?)),
        None => Ok(None),
    }
}

/// Read the stored app credential or explain how to create one.
pub fn require_app(creds: &CredentialStore) -> Result<AppCredential, CliError> {
    read_app(creds)?.ok_or_else(|| {
        CliError::Auth(
            "no Schwab app credential stored — register an app at https://developer.schwab.com, \
             wait for it to reach \"Ready For Use\", then run \
             `schw auth set-credential --stdin` (see docs/api.md)"
                .into(),
        )
    })
}

pub fn write_app(creds: &CredentialStore, cred: &AppCredential) -> Result<(), CliError> {
    creds.set(KEYCHAIN_APP, &Secret::new(cred.to_json()))
}

pub fn read_tokens(creds: &CredentialStore) -> Result<Option<TokenSet>, CliError> {
    match creds.get(KEYCHAIN_TOKENS)? {
        Some(s) => Ok(Some(TokenSet::parse(s.expose())?)),
        None => Ok(None),
    }
}

pub fn write_tokens(creds: &CredentialStore, tokens: &TokenSet) -> Result<(), CliError> {
    creds.set(KEYCHAIN_TOKENS, &Secret::new(tokens.to_json()))
}

/// Look at the keychain without failing when it is unreachable.
///
/// SPEC v1 §1.2 requires `auth status` to work logged out, and that has to
/// include the case where the keychain itself refuses — otherwise the one
/// command a driver uses to decide "do I need to log in?" is the command that
/// cannot answer. Returns the note to show the user alongside the DTO.
pub fn peek(creds: &CredentialStore, account: &str) -> (Option<Secret>, Option<String>) {
    match creds.get(account) {
        Ok(v) => (v, None),
        Err(e) => (None, Some(format!("could not read the keychain: {e}"))),
    }
}

/// Show only the tail of an identifier, e.g. `…7f2a`.
pub fn mask(s: &str) -> String {
    let n = s.chars().count();
    if n <= 4 {
        return "…".to_string();
    }
    format!("…{}", s.chars().skip(n - 4).collect::<String>())
}

fn first_str(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        v.get(*k)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_with_schwab_spelling_parses() {
        let c =
            AppCredential::parse(r#"{"app_key":"KEYVALUE1234","app_secret":"SECRETV"}"#).unwrap();
        assert_eq!(c.app_key, "KEYVALUE1234");
        assert_eq!(c.app_secret, "SECRETV");
    }

    #[test]
    fn json_with_oauth_and_portal_spellings_parses() {
        let c = AppCredential::parse(r#"{"client_id":"KEYVALUE1234","client_secret":"SECRETV"}"#)
            .unwrap();
        assert_eq!(c.app_key, "KEYVALUE1234");
        let c = AppCredential::parse(r#"{"appKey":"KEYVALUE1234","appSecret":"SECRETV"}"#).unwrap();
        assert_eq!(c.app_secret, "SECRETV");
    }

    #[test]
    fn the_colon_and_two_line_forms_parse() {
        let c = AppCredential::parse("KEYVALUE1234:SECRETV").unwrap();
        assert_eq!(
            (c.app_key.as_str(), c.app_secret.as_str()),
            ("KEYVALUE1234", "SECRETV")
        );
        let c = AppCredential::parse("KEYVALUE1234\nSECRETV\n").unwrap();
        assert_eq!(
            (c.app_key.as_str(), c.app_secret.as_str()),
            ("KEYVALUE1234", "SECRETV")
        );
    }

    #[test]
    fn half_a_credential_is_a_usage_error_not_a_silent_success() {
        assert!(matches!(
            AppCredential::parse("KEYONLY"),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(AppCredential::parse(""), Err(CliError::Usage(_))));
        assert!(matches!(
            AppCredential::parse(r#"{"app_key":"K"}"#),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            AppCredential::parse(r#"{"app_key":"K","app_secret":""}"#),
            Err(CliError::Usage(_))
        ));
    }

    /// Schwab answers the portal's placeholder text with `invalid_client`,
    /// which sends people hunting for a credential problem that is really a
    /// copy-paste problem.
    #[test]
    fn placeholders_are_rejected_locally_with_a_useful_message() {
        let err = AppCredential::parse("PUT_APP_KEY_HERE:SECRETV").unwrap_err();
        assert!(err.to_string().contains("placeholder"));
    }

    /// The whole point of the hand-written Debug impls.
    #[test]
    fn debug_output_never_contains_secret_material() {
        let c = AppCredential::parse("KEYVALUE1234:SUPERSECRET").unwrap();
        let shown = format!("{c:?}");
        assert!(!shown.contains("SUPERSECRET"), "{shown}");
        assert!(!shown.contains("KEYVALUE1234"), "{shown}");

        let t = TokenSet {
            access_token: "ACCESSTOKENVALUE".into(),
            refresh_token: "REFRESHTOKENVALUE".into(),
            expires_at: 100,
            refresh_expires_at: 200,
            scope: None,
        };
        let shown = format!("{t:?}");
        assert!(!shown.contains("ACCESSTOKENVALUE"), "{shown}");
        assert!(!shown.contains("REFRESHTOKENVALUE"), "{shown}");
    }

    #[test]
    fn masking_shows_only_the_tail() {
        assert_eq!(mask("ABCDEFGH"), "…EFGH");
        assert_eq!(mask("abc"), "…");
        assert_eq!(mask(""), "…");
    }

    #[test]
    fn expiry_uses_the_skew_so_a_doomed_request_is_not_started() {
        let t = TokenSet {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 1_000,
            refresh_expires_at: 100_000,
            scope: None,
        };
        assert!(!t.access_expired(900 - EXPIRY_SKEW_SECS));
        // Within the skew window: treated as already expired.
        assert!(t.access_expired(1_000 - EXPIRY_SKEW_SECS));
        assert!(t.access_expired(2_000));
        assert!(!t.refresh_expired(99_999));
        assert!(t.refresh_expired(100_000));
    }

    /// A token set round-trips through the keychain as JSON; the fields the
    /// refresh path depends on must survive.
    #[test]
    fn a_token_set_round_trips_through_json() {
        let t = TokenSet {
            access_token: "a".into(),
            refresh_token: "r".into(),
            expires_at: 1,
            refresh_expires_at: 2,
            scope: Some("api".into()),
        };
        let back = TokenSet::parse(&t.to_json()).unwrap();
        assert_eq!(back.refresh_token, "r");
        assert_eq!(back.refresh_expires_at, 2);
        assert_eq!(back.scope.as_deref(), Some("api"));
    }

    #[test]
    fn an_unreadable_token_set_is_an_auth_error_pointing_at_login() {
        let err = TokenSet::parse("not json").unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(err.to_string().contains("auth login"));
    }
}
