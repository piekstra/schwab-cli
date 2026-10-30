//! The Schwab HTTP client: keep an access token fresh, send a GET, map the
//! answer onto the family's exit-code contract.
//!
//! Deliberately GET-only. There is no `post`/`put`/`delete` on this type, so
//! "this CLI is read-only" is a property of the code rather than a promise in a
//! README. That matters more here than for most providers: the same base URL
//! and the same bearer token that read a position will also **place a live
//! order** (`POST /trader/v1/accounts/{hash}/orders`) or cancel one. The only
//! thing standing between this binary and someone's money is the absence of a
//! method on this struct, so adding a write means adding one here — a visible
//! change, in the same commit as the docs.

use std::cell::RefCell;
use std::time::{Duration, Instant};

use pk_cli_core::CliError;
use pk_cli_secrets::CredentialStore;
use serde_json::Value;

use crate::config::Config;
use crate::credential::{self, AppCredential, TokenSet};
use crate::oauth;

/// Minimum spacing between requests — a burst guard, not the whole budget.
///
/// Two different numbers are in play and they answer different questions. The
/// *sustained* budget is widely reported as 120 requests per minute per app
/// (Schwab publishes no rate-limit page; see docs/api.md), and no CLI
/// invocation here comes close — even a five-year transaction walk is a few
/// dozen requests inside a few seconds. The *burst* ceiling is what this
/// constant guards, and 10/second is the figure prior in-house work against
/// this API settled on. Pacing costs nothing measurable and removes the one
/// way a date-chunked walk could look like abuse.
const MIN_REQUEST_INTERVAL: Duration = Duration::from_millis(100);

pub struct Schwab<'a> {
    http: reqwest::blocking::Client,
    base_url: String,
    app: AppCredential,
    creds: &'a CredentialStore,
    tokens: RefCell<TokenSet>,
    last_request: RefCell<Option<Instant>>,
    verbose: bool,
}

impl<'a> Schwab<'a> {
    /// Build a client from the app credential and token set in the keychain.
    ///
    /// Both absences are [`CliError::Auth`] (exit 3) carrying the command that
    /// fixes them, and they are *different* commands — `auth set-app` for a
    /// missing registration, `auth login` for a missing session — so a driver
    /// that branches on exit code 3 still shows the user the right next step.
    pub fn new(cfg: &Config, creds: &'a CredentialStore, verbose: bool) -> Result<Self, CliError> {
        let app = credential::require_app(creds)?;
        let tokens = credential::read_tokens(creds)?.ok_or_else(|| {
            CliError::Auth("not logged in to Schwab — run `schw auth login`".into())
        })?;
        Ok(Self {
            http: pk_cli_http::client("schw", env!("CARGO_PKG_VERSION"))?,
            base_url: cfg.base_url(),
            app,
            creds,
            tokens: RefCell::new(tokens),
            last_request: RefCell::new(None),
            verbose,
        })
    }

    /// When the current access token expires, for `auth status`.
    pub fn access_expires_at(&self) -> u64 {
        self.tokens.borrow().expires_at
    }

    /// When the refresh token expires — i.e. the deadline for the next browser
    /// round trip.
    pub fn refresh_expires_at(&self) -> u64 {
        self.tokens.borrow().refresh_expires_at
    }

    /// GET a path, with optional query parameters.
    ///
    /// `path` carries the leading slash and is relative to the API host, e.g.
    /// `/trader/v1/accounts`.
    pub fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, CliError> {
        self.ensure_fresh()?;
        match self.send(path, query) {
            // The local expiry estimate can be wrong — a token can be revoked
            // in the portal, and the clock can drift. A 401 is the provider's
            // authoritative answer, so honour it: refresh once, retry once.
            // `refresh` fails outright when the refresh token is spent, so
            // this cannot loop.
            Err(CliError::Auth(first)) => {
                if self.verbose {
                    eprintln!("access token rejected; refreshing and retrying once");
                }
                match self.refresh() {
                    Ok(()) => self.send(path, query),
                    // The refresh failure is the actionable message — it says
                    // "run auth login", where the original only said "401".
                    Err(CliError::Auth(_)) if first.contains("auth login") => {
                        Err(CliError::Auth(first))
                    }
                    Err(e) => Err(e),
                }
            }
            other => other,
        }
    }

    fn send(&self, path: &str, query: &[(&str, String)]) -> Result<Value, CliError> {
        self.pace();
        if self.verbose {
            // The path and parameters, never the token.
            eprintln!("GET {}{path} {query:?}", self.base_url);
        }
        let token = self.tokens.borrow().access_token.clone();
        let resp = self
            .http
            .get(format!("{}{path}", self.base_url))
            .query(query)
            .bearer_auth(token)
            .header("Accept", "application/json")
            .send()?;
        response_json(resp)
    }

    /// Sleep just long enough to keep under [`MIN_REQUEST_INTERVAL`].
    fn pace(&self) {
        let mut last = self.last_request.borrow_mut();
        if let Some(prev) = *last {
            let elapsed = prev.elapsed();
            if elapsed < MIN_REQUEST_INTERVAL {
                std::thread::sleep(MIN_REQUEST_INTERVAL - elapsed);
            }
        }
        *last = Some(Instant::now());
    }

    /// Refresh the access token if it is at or near expiry.
    fn ensure_fresh(&self) -> Result<(), CliError> {
        let now = oauth::now_unix();
        if self.tokens.borrow().access_expired(now) {
            self.refresh()?;
        }
        Ok(())
    }

    /// Trade the refresh token for a new access token and persist the result.
    ///
    /// Persisting matters: the next invocation is a separate process, and a
    /// refresh whose result was never written would make every command pay for
    /// its own refresh — burning requests and, worse, hiding the fact that the
    /// seven-day refresh window is running out.
    fn refresh(&self) -> Result<(), CliError> {
        let now = oauth::now_unix();
        let (refresh_token, refresh_deadline) = {
            let t = self.tokens.borrow();
            (t.refresh_token.clone(), t.refresh_expires_at)
        };
        if now >= refresh_deadline {
            return Err(CliError::Auth(
                "the Schwab refresh token has expired (they last 7 days and refreshing does \
                 not extend them) — run `schw auth login` to authorize again in a browser"
                    .into(),
            ));
        }
        let fresh = oauth::refresh_tokens(
            &self.http,
            &self.base_url,
            &self.app,
            &refresh_token,
            now,
            refresh_deadline,
        )?;
        credential::write_tokens(self.creds, &fresh)?;
        *self.tokens.borrow_mut() = fresh;
        if self.verbose {
            eprintln!("access token refreshed");
        }
        Ok(())
    }
}

/// Map a response onto the exit-code contract, preferring Schwab's own message
/// over a bare status line.
///
/// Not `pk_cli_http::json_response`, for two reasons. It maps 403 to the same
/// message as 401, and here those mean genuinely different things — 401 is a
/// token problem the CLI can often fix itself, while 403 is an entitlement
/// problem on the Schwab side that no retry will help. And it calls `.json()`
/// on a body that is not reliably JSON: Schwab's gateway answers some rejected
/// requests with an HTML error page, which would surface as a deserialization
/// error instead of "your token was rejected".
pub fn response_json(resp: reqwest::blocking::Response) -> Result<Value, CliError> {
    let status = resp.status().as_u16();
    let body = resp.text().unwrap_or_default();
    let parsed: Option<Value> = serde_json::from_str(&body).ok();

    if (200..300).contains(&status) {
        return parsed.ok_or_else(|| {
            CliError::Upstream(format!(
                "Schwab returned HTTP {status} with a non-JSON body"
            ))
        });
    }

    let detail = parsed.as_ref().and_then(error_message);
    let say = |fallback: &str| match &detail {
        Some(m) => format!("{m} (HTTP {status})"),
        None => format!("{fallback} (HTTP {status})"),
    };

    Err(match status {
        401 => CliError::Auth(say(
            "Schwab rejected the access token — run `schw auth login`",
        )),
        403 => CliError::Auth(say(
            "Schwab refused the request. The token is valid but the app is not entitled to \
             this resource: check on developer.schwab.com that the app has the \"Accounts and \
             Trading Production\" product and is \"Ready For Use\"",
        )),
        404 => CliError::NotFound(say(
            "no such Schwab resource — if this is an account, note that Schwab addresses \
             accounts by their hashValue, not their account number (`schw accounts list`)",
        )),
        // Schwab distinguishes the sustained limit (429-001) from the burst
        // limit (429-005); both want the same response from a CLI, which is to
        // wait rather than retry immediately.
        429 => CliError::Upstream(say(
            "Schwab rate limit exceeded (roughly 120 requests/minute per app) — wait ~60s \
             and retry",
        )),
        // Schwab documents 500 and 503 on every Trader endpoint. They are
        // upstream conditions, not anything the caller did wrong.
        500 | 503 => CliError::Upstream(say("Schwab's API is unavailable — retry later")),
        _ => CliError::Upstream(say("Schwab returned an error")),
    })
}

/// Schwab's documented error body is `{"message": "...", "errors": [...]}`;
/// its OAuth endpoints answer `{"error": "...", "error_description": "..."}`;
/// and its gateway sometimes sends neither. Read whichever is present rather
/// than assuming.
fn error_message(body: &Value) -> Option<String> {
    if let Some(m) = body.get("message").and_then(Value::as_str) {
        // `errors` is an array of strings in the documented shape; append the
        // first, which usually names the offending parameter.
        let first = body
            .get("errors")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(Value::as_str);
        return Some(match first {
            Some(e) if e != m => format!("{m}: {e}"),
            _ => m.to_string(),
        });
    }
    body.get("error_description")
        .or_else(|| body.get("error"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_documented_error_shape_is_read_for_its_message() {
        let body = json!({"message": "Invalid date", "errors": ["startDate must be ISO-8601"]});
        assert_eq!(
            error_message(&body).as_deref(),
            Some("Invalid date: startDate must be ISO-8601")
        );
    }

    #[test]
    fn a_duplicate_detail_is_not_repeated() {
        let body = json!({"message": "boom", "errors": ["boom"]});
        assert_eq!(error_message(&body).as_deref(), Some("boom"));
    }

    #[test]
    fn the_oauth_error_dialect_is_read_too() {
        let body = json!({"error": "invalid_client", "error_description": "bad key"});
        assert_eq!(error_message(&body).as_deref(), Some("bad key"));
        assert_eq!(
            error_message(&json!({"error": "invalid_client"})).as_deref(),
            Some("invalid_client")
        );
    }

    /// Totality: none of these may panic, and none may invent a message.
    #[test]
    fn unfamiliar_bodies_yield_nothing_rather_than_a_panic() {
        assert!(error_message(&json!({})).is_none());
        assert!(error_message(&json!([1, 2, 3])).is_none());
        assert!(error_message(&json!({"message": 42})).is_none());
        assert!(error_message(&json!({"errors": "not an array"})).is_none());
        assert!(error_message(&json!(null)).is_none());
    }

    /// An HTML error page from the gateway must not be parsed as JSON.
    #[test]
    fn a_non_json_body_is_not_mistaken_for_a_payload() {
        assert!(serde_json::from_str::<Value>("<html>503</html>").is_err());
    }
}
