//! `schw api <METHOD> <PATH>` — raw passthrough to any Schwab API endpoint.
//!
//! The escape hatch for surfaces the typed commands don't cover, and the
//! quickest way to check whether a response shape has drifted. The bearer token
//! is attached (and refreshed) automatically.
//!
//! **GET only.** Schwab's Trader API places live orders, replaces them, and
//! cancels them; every one of those is a POST, PUT, or DELETE against the same
//! host with the same token. Rejecting the verbs outright is what makes
//! "read-only" a property of this binary rather than a claim in its README —
//! and it means the escape hatch cannot become the hole in the guarantee. Exit
//! 6 (confirmation required), not 2, so a caller can tell "this tool won't do
//! that" from "you typed it wrong".

use pk_cli_core::{output, CliError};
use pk_cli_http::ApiArgs;

use super::Ctx;

pub fn run(ctx: &Ctx, args: &ApiArgs) -> Result<(), CliError> {
    // Validate the whole invocation before the keychain or the network.
    let method = args.method.trim().to_uppercase();
    if method != "GET" {
        return Err(CliError::ConfirmationRequired(format!(
            "{method} is a write verb and schw is read-only — only GET is allowed through \
             `schw api`. Schwab's write endpoints place and cancel real orders; this tool \
             has no code path that can reach them."
        )));
    }
    if args.data.is_some() {
        return Err(CliError::Usage(
            "--data has no meaning for a GET; schw api is read-only".into(),
        ));
    }
    let (path, query) = split_query(&args.path)?;

    let payload = ctx.client()?.get(&path, &query)?;
    output::json(&payload);
    Ok(())
}

/// A path and its decomposed query parameters, ready for the client's `get`.
type PathAndQuery<'a> = (String, Vec<(&'a str, String)>);

/// Split `/trader/v1/accounts?fields=positions` into path and parameters.
///
/// They travel separately so `reqwest` does the encoding once and correctly —
/// a hand-glued query string double-encodes the `%40` in anything Schwab
/// round-trips, which is the same class of bug as the authorization-code
/// decode trap.
fn split_query(raw: &str) -> Result<PathAndQuery<'_>, CliError> {
    let raw = raw.trim();
    if !raw.starts_with('/') {
        return Err(CliError::Usage(format!(
            "path must start with `/` (e.g. /trader/v1/accounts), got `{raw}`"
        )));
    }
    let (path, qs) = match raw.split_once('?') {
        Some((p, q)) => (p, q),
        None => (raw, ""),
    };
    let query = qs
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (k, crate::urlenc::decode(v)),
            None => (pair, String::new()),
        })
        .collect();
    Ok((path.to_string(), query))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_query_string_is_split_off_the_path() {
        let (path, q) = split_query("/trader/v1/accounts?fields=positions").unwrap();
        assert_eq!(path, "/trader/v1/accounts");
        assert_eq!(q, vec![("fields", "positions".to_string())]);
    }

    #[test]
    fn a_bare_path_has_no_parameters() {
        let (path, q) = split_query("/trader/v1/accounts").unwrap();
        assert_eq!(path, "/trader/v1/accounts");
        assert!(q.is_empty());
    }

    /// Values are decoded here and re-encoded by the HTTP layer, so a date
    /// parameter typed with its colons escaped still arrives correctly.
    #[test]
    fn encoded_values_are_decoded_once_before_being_re_encoded() {
        let (_, q) = split_query(
            "/trader/v1/accounts/H/transactions?startDate=2026-01-01T00%3A00%3A00.000Z",
        )
        .unwrap();
        assert_eq!(q[0].1, "2026-01-01T00:00:00.000Z");
    }

    #[test]
    fn a_path_without_a_leading_slash_is_a_usage_error() {
        assert!(matches!(
            split_query("trader/v1/accounts"),
            Err(CliError::Usage(_))
        ));
        assert!(matches!(
            split_query("https://api.schwabapi.com/trader/v1/accounts"),
            Err(CliError::Usage(_))
        ));
    }

    #[test]
    fn a_valueless_parameter_does_not_panic() {
        let (_, q) = split_query("/trader/v1/accounts?flag&fields=positions").unwrap();
        assert_eq!(q[0], ("flag", String::new()));
    }
}
