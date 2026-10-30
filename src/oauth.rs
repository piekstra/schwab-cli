//! Schwab's OAuth2 authorization-code flow: build the authorization URL,
//! exchange a code for tokens, and refresh an access token.
//!
//! # The flow, and why it is shaped like this
//!
//! Schwab registers each app against an **https** callback URL. A command-line
//! tool therefore cannot simply listen on `http://localhost:<port>` the way
//! most OAuth CLIs do — the browser will open a TLS connection, and a plain
//! TCP listener cannot answer it. The two ways out are (a) terminate TLS
//! locally with a self-signed certificate the user must click past, or (b) let
//! the browser fail to connect and have the user copy the redirected URL out of
//! the address bar, which contains the authorization code either way.
//!
//! This CLI does (b). The redirect **always** carries the code in the URL,
//! whether or not anything is listening, so (b) has no failure mode that (a)
//! doesn't also have, needs no TLS stack, no certificate generation, and no
//! browser security warning that trains the user to click through warnings.
//! `docs/api.md` records the recipe for (a) if it is ever wanted.
//!
//! # Token lifetimes
//!
//! - Access token: ~30 minutes (`expires_in` in the response).
//! - Refresh token: **7 days, and refreshing does not extend it.** At the end
//!   of the week the user must complete the browser flow again. This is a
//!   Schwab policy, not a limitation here, and it is the single most annoying
//!   fact about automating against this API.

use pk_cli_core::CliError;
use serde_json::Value;

use crate::credential::{AppCredential, TokenSet, REFRESH_TOKEN_LIFETIME_SECS};
use crate::urlenc;

/// Path of the authorization endpoint, relative to the API base URL.
pub const AUTHORIZE_PATH: &str = "/v1/oauth/authorize";
/// Path of the token endpoint, relative to the API base URL.
pub const TOKEN_PATH: &str = "/v1/oauth/token";

/// Build the URL the user opens in a browser to authorize the app.
///
/// `response_type=code` is included because it is what Schwab's published
/// OAuth security scheme specifies. `scope` is deliberately **not** sent:
/// Schwab's spec advertises `scope=readonly`, but the token it issues comes
/// back with `scope=api` regardless, so requesting a read-only scope buys no
/// real guarantee. This CLI's read-only property is enforced in its own client
/// (no verb but GET) rather than delegated to a scope that may not be honored.
/// See `docs/api.md`.
pub fn authorize_url(base_url: &str, app_key: &str, redirect_uri: &str) -> String {
    format!(
        "{}{AUTHORIZE_PATH}?{}",
        base_url.trim_end_matches('/'),
        urlenc::query_string(&[
            ("client_id", app_key),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
        ])
    )
}

/// Extract the authorization code from whatever the user pasted.
///
/// Accepts the full redirected URL (what the browser's address bar holds), a
/// bare query string, or the code on its own. **The code is percent-decoded
/// exactly once** — see [`crate::urlenc`] for why that matters and what breaks
/// when it is skipped.
pub fn code_from_input(pasted: &str) -> Result<String, CliError> {
    let pasted = pasted.trim();
    if pasted.is_empty() {
        return Err(CliError::Usage("no authorization code given".into()));
    }
    if let Some(code) = urlenc::query_param(pasted, "code") {
        if code.is_empty() {
            return Err(CliError::Usage(
                "the pasted URL has an empty `code=` parameter".into(),
            ));
        }
        return Ok(code);
    }
    // No `code=` anywhere. If it looks like a URL, the user pasted the wrong
    // one — usually the authorization URL rather than the redirected one, which
    // is an easy mistake to make and an impossible error to diagnose from
    // Schwab's side.
    if pasted.contains("://") {
        return Err(CliError::Usage(
            "no `code=` parameter in that URL — paste the URL your browser was \
             redirected *to* (it starts with your app's callback URL), not the \
             authorization URL you opened"
                .into(),
        ));
    }
    // A bare code, pasted by hand. It may still be percent-encoded if it was
    // copied out of an address bar by selecting only part of it.
    Ok(urlenc::decode(pasted))
}

/// Exchange an authorization code for a token set.
pub fn exchange_code(
    http: &reqwest::blocking::Client,
    base_url: &str,
    app: &AppCredential,
    code: &str,
    redirect_uri: &str,
    now: u64,
) -> Result<TokenSet, CliError> {
    let form = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
    ];
    let body = post_token(http, base_url, app, &form)?;
    token_set_from_response(&body, now, None)
}

/// Trade a refresh token for a fresh access token.
///
/// `issued_refresh_expiry` carries the **original** refresh-token deadline
/// forward. Schwab's refresh response looks identical to the initial one, which
/// makes it tempting to restart the 7-day clock from each refresh — but Schwab
/// does not extend it, so doing that would report a healthy session right up to
/// the moment it stops working, with no warning that a browser round trip was
/// coming. Carrying the original deadline means `auth status` can say
/// truthfully how many days are left.
pub fn refresh_tokens(
    http: &reqwest::blocking::Client,
    base_url: &str,
    app: &AppCredential,
    refresh_token: &str,
    now: u64,
    issued_refresh_expiry: u64,
) -> Result<TokenSet, CliError> {
    let form = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
    ];
    let body = post_token(http, base_url, app, &form)?;
    token_set_from_response(&body, now, Some(issued_refresh_expiry))
}

fn post_token(
    http: &reqwest::blocking::Client,
    base_url: &str,
    app: &AppCredential,
    form: &[(&str, &str)],
) -> Result<Value, CliError> {
    let url = format!("{}{TOKEN_PATH}", base_url.trim_end_matches('/'));
    let resp = http
        .post(url)
        // The app authenticates with HTTP Basic, not with body parameters.
        // Sending client_id/client_secret in the form instead is a documented
        // way to get an opaque `invalid_client` out of this endpoint.
        .header("Authorization", format!("Basic {}", basic_auth(app)))
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .form(form)
        .send()?;
    token_response(resp)
}

/// `base64(app_key:app_secret)` for the `Authorization: Basic` header.
pub fn basic_auth(app: &AppCredential) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", app.app_key, app.app_secret))
}

/// Map the token endpoint's answer onto the family's exit-code contract,
/// translating Schwab's OAuth error slugs into something a human can act on.
fn token_response(resp: reqwest::blocking::Response) -> Result<Value, CliError> {
    let status = resp.status().as_u16();
    let body = resp.text().unwrap_or_default();
    let parsed: Option<Value> = serde_json::from_str(&body).ok();

    if (200..300).contains(&status) {
        return parsed.ok_or_else(|| {
            CliError::Upstream(format!(
                "Schwab's token endpoint returned HTTP {status} with a non-JSON body"
            ))
        });
    }

    let slug = parsed
        .as_ref()
        .and_then(|v| v.get("error"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let detail = parsed
        .as_ref()
        .and_then(|v| v.get("error_description"))
        .and_then(Value::as_str)
        .map(str::to_string);

    Err(CliError::Auth(explain_oauth_error(
        &slug,
        detail.as_deref(),
        status,
    )))
}

/// Turn an OAuth error slug into the sentence that actually unblocks the user.
///
/// These three slugs cover essentially every failure of this flow, and each one
/// has a cause that Schwab's own message does not mention.
pub fn explain_oauth_error(slug: &str, detail: Option<&str>, status: u16) -> String {
    let base = match slug {
        "invalid_client" => {
            "Schwab rejected the app credential (`invalid_client`). Either the app key/secret \
             are wrong — re-copy them from developer.schwab.com and run \
             `schw auth set-app --stdin --overwrite` — or the app has not finished approval \
             yet. An app still showing \"Approved - Pending\" fails here with exactly this \
             error even when the credentials are correct."
        }
        "unsupported_token_type" | "invalid_grant" => {
            "Schwab rejected the authorization code. The usual causes, in order of \
             likelihood: (1) the code was already used — each one works exactly once, so \
             re-run `schw auth login` for a fresh one; (2) it expired — codes are good for \
             about 30 seconds, so paste promptly; (3) the `redirect_uri` sent here does not \
             byte-for-byte match the one registered with the app — check \
             `schw config show`."
        }
        "unauthorized_client" => {
            "The app is not authorized for this grant. Check on developer.schwab.com that \
             the app has the \"Accounts and Trading Production\" API product and has reached \
             \"Ready For Use\"."
        }
        _ => "Schwab's token endpoint refused the request.",
    };
    match (slug.is_empty(), detail) {
        (false, Some(d)) => format!("{base} [{slug}: {d}] (HTTP {status})"),
        (false, None) => format!("{base} [{slug}] (HTTP {status})"),
        (true, Some(d)) => format!("{base} [{d}] (HTTP {status})"),
        (true, None) => format!("{base} (HTTP {status})"),
    }
}

/// Build a [`TokenSet`] from a token response body.
pub fn token_set_from_response(
    body: &Value,
    now: u64,
    keep_refresh_expiry: Option<u64>,
) -> Result<TokenSet, CliError> {
    let access_token = str_field(body, "access_token")?;
    let refresh_token = str_field(body, "refresh_token")?;
    // Schwab documents ~1800s. A missing or unreadable `expires_in` is treated
    // as a very short life rather than a long one: being wrong in that
    // direction costs one extra refresh, while the other direction serves a
    // dead token and produces a mystery 401.
    let expires_in = body.get("expires_in").and_then(Value::as_u64).unwrap_or(60);
    Ok(TokenSet {
        access_token,
        refresh_token,
        expires_at: now + expires_in,
        refresh_expires_at: keep_refresh_expiry.unwrap_or(now + REFRESH_TOKEN_LIFETIME_SECS),
        scope: body
            .get("scope")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn str_field(body: &Value, name: &str) -> Result<String, CliError> {
    body.get(name)
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CliError::Upstream(format!(
                "Schwab's token response has no `{name}` — the response shape has changed; \
                 see docs/api.md"
            ))
        })
}

/// Current Unix time in seconds.
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn app() -> AppCredential {
        AppCredential {
            app_key: "APPKEY".into(),
            app_secret: "APPSECRET".into(),
        }
    }

    #[test]
    fn the_authorize_url_carries_the_encoded_callback() {
        let url = authorize_url("https://api.schwabapi.com", "APPKEY", "https://127.0.0.1");
        assert!(url.starts_with("https://api.schwabapi.com/v1/oauth/authorize?"));
        assert!(url.contains("client_id=APPKEY"));
        assert!(url.contains("redirect_uri=https%3A%2F%2F127.0.0.1"));
        assert!(url.contains("response_type=code"));
        // Scope is deliberately absent — see the function docs.
        assert!(!url.contains("scope="));
    }

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        let url = authorize_url("https://api.schwabapi.com/", "K", "https://127.0.0.1");
        assert!(!url.contains("com//v1"));
    }

    #[test]
    fn basic_auth_is_the_base64_of_key_colon_secret() {
        // base64("APPKEY:APPSECRET")
        assert_eq!(basic_auth(&app()), "QVBQS0VZOkFQUFNFQ1JFVA==");
    }

    /// The headline decode case, end to end: a full redirect URL as the browser
    /// shows it, with the code's trailing `@` still percent-encoded.
    #[test]
    fn the_code_is_decoded_once_out_of_a_full_redirect_url() {
        let pasted = "https://127.0.0.1/?code=C0.abcdEFGH1234%40&session=9f8e";
        assert_eq!(code_from_input(pasted).unwrap(), "C0.abcdEFGH1234@");
    }

    #[test]
    fn a_bare_query_string_or_a_bare_code_also_works() {
        assert_eq!(code_from_input("code=ABC%40").unwrap(), "ABC@");
        assert_eq!(code_from_input("  ABC@ ").unwrap(), "ABC@");
    }

    /// Pasting the authorization URL instead of the redirected one is the most
    /// common user error; it must not be mistaken for a code.
    #[test]
    fn pasting_the_wrong_url_says_which_url_was_wanted() {
        let err = code_from_input("https://api.schwabapi.com/v1/oauth/authorize?client_id=K")
            .unwrap_err();
        assert_eq!(err.exit_code(), 2);
        assert!(err.to_string().contains("redirected"));
    }

    #[test]
    fn an_empty_or_codeless_input_is_a_usage_error() {
        assert!(matches!(code_from_input(""), Err(CliError::Usage(_))));
        assert!(matches!(
            code_from_input("https://127.0.0.1/?code=&session=1"),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn a_token_response_becomes_absolute_deadlines() {
        let body = json!({
            "access_token": "AT", "refresh_token": "RT",
            "expires_in": 1800, "token_type": "Bearer", "scope": "api"
        });
        let t = token_set_from_response(&body, 1_000, None).unwrap();
        assert_eq!(t.expires_at, 2_800);
        assert_eq!(t.refresh_expires_at, 1_000 + REFRESH_TOKEN_LIFETIME_SECS);
        assert_eq!(t.scope.as_deref(), Some("api"));
    }

    /// The rail that keeps `auth status` honest: refreshing must not silently
    /// restart the seven-day clock, or the CLI would promise a working session
    /// right up to the moment the browser flow becomes mandatory.
    #[test]
    fn refreshing_carries_the_original_refresh_deadline_forward() {
        let body = json!({"access_token": "AT2", "refresh_token": "RT2", "expires_in": 1800});
        let original_deadline = 500_000;
        let t = token_set_from_response(&body, 400_000, Some(original_deadline)).unwrap();
        assert_eq!(t.refresh_expires_at, original_deadline);
        // A rotated refresh token is still stored.
        assert_eq!(t.refresh_token, "RT2");
    }

    /// Being wrong about `expires_in` must cost a refresh, not a mystery 401.
    #[test]
    fn a_missing_expires_in_is_treated_as_a_short_life() {
        let body = json!({"access_token": "AT", "refresh_token": "RT"});
        let t = token_set_from_response(&body, 1_000, None).unwrap();
        assert!(t.expires_at - 1_000 <= 60);
    }

    #[test]
    fn a_token_response_missing_a_token_is_an_upstream_error() {
        let err = token_set_from_response(&json!({"refresh_token": "RT"}), 0, None).unwrap_err();
        assert_eq!(err.exit_code(), 5);
        assert!(err.to_string().contains("access_token"));
    }

    /// The error messages are the whole value of this module for a user who is
    /// stuck, so their content is asserted, not just their existence.
    #[test]
    fn invalid_client_mentions_the_pending_approval_case() {
        let msg = explain_oauth_error("invalid_client", None, 401);
        assert!(msg.contains("Approved - Pending"));
        assert!(msg.contains("invalid_client"));
    }

    #[test]
    fn a_rejected_code_mentions_single_use_expiry_and_redirect_mismatch() {
        let msg = explain_oauth_error("unsupported_token_type", Some("bad code"), 400);
        assert!(msg.contains("exactly once"));
        assert!(msg.contains("redirect_uri"));
        assert!(msg.contains("bad code"));
    }

    #[test]
    fn an_unknown_slug_still_produces_a_sentence() {
        let msg = explain_oauth_error("", None, 500);
        assert!(msg.contains("HTTP 500"));
        assert!(!msg.is_empty());
    }
}
