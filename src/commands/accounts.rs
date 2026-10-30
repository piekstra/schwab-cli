//! `schw accounts list` — the brokerage accounts linked to the authorized app,
//! and the hash each one is addressed by.
//!
//! This is the command everything else depends on. Schwab's account-scoped
//! endpoints take an opaque `hashValue`, not the account number printed on a
//! statement, and there is no way to derive one from the other — so the first
//! thing a new user must do is run this and copy a hash.

use clap::Subcommand;
use pk_cli_core::{CliError, Paged};
use serde_json::Value;

use super::{table_view, Ctx, ACCOUNTS, ACCOUNT_NUMBERS};
use crate::parse;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List linked accounts and their hashes (brokerage-account-list/v1).
    #[command(visible_alias = "ls")]
    List,
}

pub fn run(ctx: &Ctx, cmd: &Cmd) -> Result<(), CliError> {
    let Cmd::List = cmd;
    let client = ctx.client()?;

    let hashes = as_array(client.get(ACCOUNT_NUMBERS, &[])?);
    // Balances are a second request and a nicety, not the point of the command.
    // If it fails — a partial entitlement, a transient 500 — still print the
    // hashes, because a user who cannot see their account hash cannot use any
    // other command at all.
    let detail = match client.get(ACCOUNTS, &[]) {
        Ok(v) => as_array(v),
        Err(e) => {
            if !ctx.common.quiet {
                eprintln!("note: balances unavailable ({e}); listing accounts without them");
            }
            Vec::new()
        }
    };

    let accounts = parse::accounts(&hashes, &detail);
    if accounts.is_empty() {
        return Err(CliError::NotFound(
            "Schwab returned no accounts for this app. If the app was only just approved, \
             confirm on developer.schwab.com that it is \"Ready For Use\" and that you \
             selected the account during `schw auth login`."
                .into(),
        ));
    }

    let page = Paged::new("brokerage-account", accounts);
    if ctx.common.json {
        page.emit(true);
    } else {
        let v = serde_json::to_value(&page).unwrap_or(Value::Null);
        let rows = super::items(&v, "items");
        pk_cli_core::output::table(&table_view(
            &rows,
            &["id", "account_number", "type", "equity", "cash"],
        ));
    }
    Ok(())
}

/// Schwab's list endpoints answer with a bare JSON array.
fn as_array(v: Value) -> Vec<Value> {
    match v {
        Value::Array(a) => a,
        // A single object where an array was documented is a shape Schwab has
        // shipped before (`/userPreference`), so tolerate it rather than
        // reporting no accounts.
        Value::Object(_) => vec![v],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_array_payload_passes_through() {
        assert_eq!(as_array(json!([{"a": 1}, {"b": 2}])).len(), 2);
    }

    /// Schwab has documented an array and shipped a bare object before; that
    /// must not read as "you have no accounts".
    #[test]
    fn a_bare_object_is_treated_as_a_single_row() {
        assert_eq!(as_array(json!({"hashValue": "H"})).len(), 1);
    }

    #[test]
    fn an_unexpected_payload_yields_no_rows_rather_than_a_panic() {
        assert!(as_array(json!(null)).is_empty());
        assert!(as_array(json!("nope")).is_empty());
    }
}
