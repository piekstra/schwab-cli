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

use pk_cli_auth::refresh_lock::RefreshLock;
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
            http: http()?,
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
            Err(CliError::Auth(first)) if worth_a_renewal(&first) => {
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
            .send()
            .map_err(network_failure)?;
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

    /// Renew because the access token this process holds is expired or refused,
    /// unless another process has renewed it since.
    fn refresh(&self) -> Result<(), CliError> {
        let presented = self.tokens.borrow().access_token.clone();
        self.renew(|stored| {
            stored.access_token == presented || stored.access_expired(oauth::now_unix())
        })
    }

    /// Renew now, whatever the stored access token's state: `auth refresh`.
    pub fn refresh_now(&self) -> Result<(), CliError> {
        self.renew(|_| true)
    }

    /// Trade the refresh token for a new access token and persist the result.
    ///
    /// Persisting matters: the next invocation is a separate process, and a
    /// refresh whose result was never written would make every command pay for
    /// its own refresh — burning requests and, worse, hiding the fact that the
    /// seven-day refresh window is running out.
    fn renew(&self, due: impl FnOnce(&TokenSet) -> bool) -> Result<(), CliError> {
        let fresh = renew_session(
            &RefreshLock::for_binary(crate::BIN)?,
            || credential::read_tokens(self.creds),
            due,
            |stored| {
                let fresh = oauth::refresh_tokens(
                    &self.http,
                    &self.base_url,
                    &self.app,
                    &stored.refresh_token,
                    oauth::now_unix(),
                    stored.refresh_expires_at,
                )?;
                if self.verbose {
                    eprintln!("access token refreshed");
                }
                Ok(fresh)
            },
            |fresh| credential::write_tokens(self.creds, fresh),
        )?;
        *self.tokens.borrow_mut() = fresh;
        Ok(())
    }
}

/// One renewal, safe against every other `schw` process over this keychain item.
/// The session is read again under the lock, so a grant always presents the newest
/// refresh token and checks the newest deadline, and `due` can see that another
/// process renewed while this one waited: then there is no grant, and its tokens
/// are the answer. `store` runs before the lock is released, which is what the next
/// process reads.
fn renew_session(
    lock: &RefreshLock,
    read: impl FnOnce() -> Result<Option<TokenSet>, CliError>,
    due: impl FnOnce(&TokenSet) -> bool,
    grant: impl FnOnce(&TokenSet) -> Result<TokenSet, CliError>,
    store: impl FnOnce(&TokenSet) -> Result<(), CliError>,
) -> Result<TokenSet, CliError> {
    lock.renew(
        // Logged out while this process waited: there is nothing to renew.
        || read()?.ok_or_else(|| CliError::Auth("not logged in — run `schw auth login`".into())),
        due,
        |stored| {
            if stored.refresh_expired(oauth::now_unix()) {
                return Err(CliError::Auth(
                    "the Schwab refresh token has expired (they last 7 days and refreshing does \
                     not extend them) — run `schw auth login` to authorize again in a browser"
                        .into(),
                ));
            }
            let fresh = grant(&stored)?;
            store(&fresh)?;
            Ok(fresh)
        },
    )
}

/// Whether a refused request can be fixed by renewing the token. Only a 401 can:
/// a 403 is an entitlement refusal that a new token gets too, and a long-running
/// server asked again and again must not spend a grant on each.
fn worth_a_renewal(refusal: &str) -> bool {
    has_status(refusal, 401)
}

/// The client for every request here. Schwab answers on its own host, and the token endpoint's
/// body holds a code or a refresh token, which reqwest would re-send on a 307 or 308 to any host.
pub fn http() -> Result<reqwest::blocking::Client, CliError> {
    pk_cli_http::builder(crate::BIN, env!("CARGO_PKG_VERSION"))
        .redirect(pk_cli_core::transport::same_host_redirects())
        .build()
        .map_err(|e| CliError::Other(format!("failed to build HTTP client: {e}")))
}

/// A failure that got no answer, without its URL. `reqwest` prints the URL it was
/// asked for, and a URL here holds an account hash, which must not reach a log, a
/// `--json` error or a tool result.
pub fn network_failure(e: reqwest::Error) -> CliError {
    e.without_url().into()
}

/// An error message for an answer with this status. [`has_status`] is how anything
/// that acts on the status reads it back, so the two cannot drift apart.
pub fn with_status(message: &str, status: u16) -> String {
    format!("{message} (HTTP {status})")
}

/// Whether a message built by [`with_status`] carries this status.
pub fn has_status(message: &str, status: u16) -> bool {
    message.ends_with(&format!("(HTTP {status})"))
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
    answer_json(status, &resp.text().unwrap_or_default())
}

/// [`response_json`] once the answer has been read.
pub fn answer_json(status: u16, body: &str) -> Result<Value, CliError> {
    let parsed: Option<Value> = serde_json::from_str(body).ok();

    if (200..300).contains(&status) {
        return parsed.ok_or_else(|| {
            CliError::Upstream(format!(
                "Schwab returned HTTP {status} with a non-JSON body"
            ))
        });
    }

    let detail = parsed.as_ref().and_then(error_message);
    let say = |fallback: &str| match &detail {
        Some(m) => with_status(m, status),
        None => with_status(fallback, status),
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
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Barrier, Mutex};

    fn session(refresh: &str, refresh_expires_at: u64) -> TokenSet {
        TokenSet {
            access_token: format!("dummy-access-for-{refresh}"),
            refresh_token: refresh.into(),
            expires_at: 0,
            refresh_expires_at,
            scope: None,
        }
    }

    const STILL_VALID: u64 = u64::MAX;

    fn lock_path(dir: &Path) -> PathBuf {
        dir.join("token-refresh.lock")
    }

    /// Two processes' worth of contenders: each has its own open of the lock
    /// file, which is what contends, and both saw the same access token expire.
    /// Offline, with the keychain and the token endpoint stood in for.
    #[test]
    fn two_contenders_post_one_grant_and_both_use_its_tokens() {
        let dir = tempfile::tempdir().unwrap();
        let path = lock_path(dir.path());
        let stored = Arc::new(Mutex::new(session("dummy-refresh-1", STILL_VALID)));
        let grants = Arc::new(Mutex::new(Vec::new()));
        let start = Arc::new(Barrier::new(2));
        let presented = "dummy-access-for-dummy-refresh-1";
        let contenders: Vec<_> = (0..2)
            .map(|_| {
                let (path, stored, grants, start) = (
                    path.clone(),
                    Arc::clone(&stored),
                    Arc::clone(&grants),
                    Arc::clone(&start),
                );
                std::thread::spawn(move || {
                    start.wait();
                    renew_session(
                        &RefreshLock::at(path),
                        || Ok(Some(stored.lock().unwrap().clone())),
                        |latest| latest.access_token == presented,
                        |latest| {
                            grants.lock().unwrap().push(latest.refresh_token.clone());
                            std::thread::sleep(Duration::from_millis(100));
                            Ok(session("dummy-refresh-2", STILL_VALID))
                        },
                        |fresh| {
                            *stored.lock().unwrap() = fresh.clone();
                            Ok(())
                        },
                    )
                })
            })
            .collect();
        for contender in contenders {
            let used = contender.join().unwrap().unwrap();
            assert_eq!(used.refresh_token, "dummy-refresh-2");
        }
        assert_eq!(*grants.lock().unwrap(), ["dummy-refresh-1"]);
    }

    #[test]
    fn a_session_another_process_renewed_is_used_without_a_grant() {
        let dir = tempfile::tempdir().unwrap();
        let used = renew_session(
            &RefreshLock::at(lock_path(dir.path())),
            || Ok(Some(session("dummy-refresh-2", STILL_VALID))),
            |_| false,
            |_| panic!("another process renewed: no grant"),
            |_| panic!("nothing new to store"),
        )
        .unwrap();
        assert_eq!(used.refresh_token, "dummy-refresh-2");
    }

    /// What is stored decides, not what this process read at startup: a login in
    /// another process may have replaced the session and restarted its 7 days.
    #[test]
    fn the_grant_presents_the_stored_token_and_stores_the_result() {
        let dir = tempfile::tempdir().unwrap();
        let stored = Mutex::new(None);
        let used = renew_session(
            &RefreshLock::at(lock_path(dir.path())),
            || Ok(Some(session("dummy-refresh-from-a-new-login", STILL_VALID))),
            |_| true,
            |latest| {
                assert_eq!(latest.refresh_token, "dummy-refresh-from-a-new-login");
                Ok(session("dummy-refresh-2", STILL_VALID))
            },
            |fresh| {
                *stored.lock().unwrap() = Some(fresh.refresh_token.clone());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(used.refresh_token, "dummy-refresh-2");
        assert_eq!(stored.lock().unwrap().as_deref(), Some("dummy-refresh-2"));
    }

    #[test]
    fn an_expired_refresh_token_is_an_auth_error_and_no_grant() {
        let dir = tempfile::tempdir().unwrap();
        let err = renew_session(
            &RefreshLock::at(lock_path(dir.path())),
            || Ok(Some(session("dummy-refresh-1", 0))),
            |_| true,
            |_| panic!("past the deadline: no grant"),
            |_| panic!("nothing to store"),
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(err.to_string().contains("schw auth login"), "{err}");
    }

    #[test]
    fn a_logout_while_waiting_is_an_auth_error_and_no_grant() {
        let dir = tempfile::tempdir().unwrap();
        let err = renew_session(
            &RefreshLock::at(lock_path(dir.path())),
            || Ok(None),
            |_| panic!("logged out: nothing to judge"),
            |_| panic!("logged out: no grant"),
            |_| panic!("logged out: nothing to store"),
        )
        .unwrap_err();
        assert_eq!(err.exit_code(), 3);
        assert!(err.to_string().contains("not logged in"), "{err}");
    }

    #[test]
    fn a_held_lock_is_a_bounded_error_and_no_grant() {
        let dir = tempfile::tempdir().unwrap();
        let path = lock_path(dir.path());
        RefreshLock::at(&path)
            .hold(|| {
                let err = renew_session(
                    &RefreshLock::at(&path).wait_at_most(Duration::from_millis(100)),
                    || panic!("no read without the lock"),
                    |_| panic!("nothing to judge"),
                    |_| panic!("no grant without the lock"),
                    |_| panic!("nothing to store"),
                )
                .unwrap_err();
                assert_eq!(err.exit_code(), 5);
                assert!(err.to_string().contains("try again"), "{err}");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn only_a_401_is_worth_a_renewal() {
        let refusal = |status| match answer_json(status, "{\"message\":\"no\"}") {
            Err(CliError::Auth(m)) => m,
            other => panic!("{status}: {other:?}"),
        };
        assert!(worth_a_renewal(&refusal(401)));
        assert!(!worth_a_renewal(&refusal(403)));
        assert!(!worth_a_renewal("not logged in to Schwab"));
    }

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

    /// Against a port nobody listens on: the failure is real, from `reqwest`.
    #[test]
    fn a_failure_with_no_answer_does_not_carry_its_url() {
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let raw = reqwest::blocking::Client::new()
            .get(format!(
                "http://127.0.0.1:{port}/trader/v1/accounts/DUMMYHASH/transactions"
            ))
            .send()
            .unwrap_err();
        assert!(raw.to_string().contains("DUMMYHASH"), "the premise: {raw}");
        let told = network_failure(raw).to_string();
        assert!(!told.contains("DUMMYHASH"), "{told}");
        assert!(!told.contains("/trader/"), "{told}");
    }
}
