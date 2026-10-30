//! Contract tests over the fixtures in `tests/fixtures/`.
//!
//! Two jobs:
//!
//! 1. **Shape.** Assert the exact fields the table views and DTOs read, so a
//!    Schwab rename fails here loudly instead of silently emptying a column.
//!    A blank column in a tax feed is the kind of bug that gets noticed in
//!    April.
//! 2. **Hygiene.** Assert, *positively*, that every identity-bearing value in
//!    the fixtures matches an obvious dummy shape. Written this way round on
//!    purpose: a denylist of real account numbers would publish the very
//!    values it exists to exclude.
//!
//! These fixtures are scrubbed from responses captured live on 2026-08-12 —
//! see `tests/fixtures/README.md` for what was captured, what remains
//! doc-derived, and the scrubbing policy. Passing here proves the parsers
//! read the shape Schwab actually sent that day.

use std::path::PathBuf;

use schw::parse;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("parsing {}: {e}", path.display()))
}

fn array(name: &str) -> Vec<Value> {
    match fixture(name) {
        Value::Array(a) => a,
        other => panic!("{name} should be a JSON array, got {other}"),
    }
}

// ---------------------------------------------------------------- shape ----

#[test]
fn account_numbers_carry_both_the_number_and_the_hash() {
    let rows = array("account_numbers.json");
    assert_eq!(rows.len(), 2);
    for row in &rows {
        assert!(row.get("accountNumber").is_some(), "accountNumber missing");
        // The hash is the field every other endpoint needs; losing it makes
        // the whole CLI unusable, not just this command.
        assert!(row.get("hashValue").is_some(), "hashValue missing");
    }
}

#[test]
fn accounts_parse_into_the_listed_columns() {
    let hashes = array("account_numbers.json");
    let detail = array("accounts.json");
    let accounts = parse::accounts(&hashes, &detail);

    assert_eq!(accounts.len(), 2, "one row per linked account");
    // The detail array lists CASH first while the hash list leads with MARGIN:
    // the join must go by account number, never by position.
    let margin = &accounts[0];
    assert_eq!(
        margin.id,
        "DUMMYHASH11111111111111111111111111111111111111111111111111111AA"
    );
    assert_eq!(margin.account_number.as_deref(), Some("…1111"));
    assert_eq!(margin.r#type.as_deref(), Some("MARGIN"));
    assert_eq!(margin.equity.as_ref().expect("equity").amount, "30000");
    assert_eq!(margin.cash.as_ref().expect("cash").amount, "5000");
    assert_eq!(margin.is_day_trader, Some(false));

    // Live-verified 2026-08-12: a CASH account's `currentBalances` carries no
    // `equity` key at all — the fallback chain must find `liquidationValue`
    // rather than leaving the column blank.
    let cash = &accounts[1];
    assert_eq!(cash.r#type.as_deref(), Some("CASH"));
    assert_eq!(
        cash.equity
            .as_ref()
            .expect("CASH account lost its balance")
            .amount,
        "42000"
    );
}

#[test]
fn positions_parse_into_the_listed_columns() {
    let positions = parse::positions(&fixture("positions.json"));
    assert_eq!(positions.len(), 2);

    // The long row mirrors a live capture: an ETF reports assetType
    // `COLLECTIVE_INVESTMENT` (with `instrument.type` EXCHANGE_TRADED_FUND),
    // not `EQUITY` as the docs suggested.
    let long = &positions[0];
    assert_eq!(long.symbol, "ETFX");
    assert_eq!(long.asset_type.as_deref(), Some("COLLECTIVE_INVESTMENT"));
    assert_eq!(long.quantity.as_deref(), Some("100"));
    assert_eq!(long.average_price.as_deref(), Some("150.25"));
    assert_eq!(
        long.market_value.as_ref().expect("market_value").amount,
        "17550"
    );
    assert_eq!(long.open_pl.as_ref().expect("open_pl").amount, "2525");

    // The short leg reads as a negative quantity and picks up the *short*
    // open-P/L field, which is a different key from the long one.
    let short = &positions[1];
    assert_eq!(short.symbol, "TSLA");
    assert_eq!(short.quantity.as_deref(), Some("-50"));
    assert_eq!(short.open_pl.as_ref().expect("open_pl").amount, "-250");
}

#[test]
fn transactions_parse_into_the_tax_relevant_columns() {
    let rows = array("transactions.json");
    let txns = parse::transactions(&rows);
    assert_eq!(txns.len(), 3);

    // A closing option sale, shaped as captured live: the OCC symbol must
    // survive, and the quantity is the (negative) contract count.
    let sale = &txns[0];
    assert_eq!(sale.id, "90000000001");
    assert_eq!(sale.date, "2026-03-14");
    assert_eq!(sale.r#type, "TRADE");
    assert_eq!(sale.symbol.as_deref(), Some("AAPL  260320C00180000"));
    assert_eq!(sale.quantity.as_deref(), Some("-1"));
    assert_eq!(sale.price.as_deref(), Some("2.5"));
    assert_eq!(
        sale.net_amount.as_ref().expect("net_amount").amount,
        "249.49"
    );
    // The flag that says this trade realized something. It is the closest the
    // API comes to a realized-gain signal, so it must never go missing.
    assert_eq!(sale.position_effect.as_deref(), Some("CLOSING"));
    assert_eq!(sale.order_id.as_deref(), Some("80000000001"));
    // Live-verified 2026-08-12: a TRADE row carries none of the fields the
    // published schema promised here. Their absence is the shape.
    assert_eq!(sale.settlement_date, None);
    assert_eq!(sale.activity_type, None);
    assert_eq!(sale.description, None);

    let buy = &txns[1];
    assert_eq!(buy.position_effect.as_deref(), Some("OPENING"));
    assert_eq!(buy.quantity.as_deref(), Some("2"));

    // A dividend, live-shaped: cash-only legs, so there is no security symbol
    // to summarize — and unlike TRADE rows it does carry a description and a
    // settlement date.
    let dividend = &txns[2];
    assert_eq!(dividend.r#type, "DIVIDEND_OR_INTEREST");
    assert_eq!(dividend.symbol, None);
    assert_eq!(
        dividend.net_amount.as_ref().expect("net_amount").amount,
        "12.34"
    );
    assert_eq!(
        dividend.description.as_deref(),
        Some("DUMMY EXCHANGE TRADED FUND DIVIDEND")
    );
    assert_eq!(dividend.settlement_date.as_deref(), Some("2026-03-11"));
}

/// Fee legs and the cash counter-leg are published but never summarized as the
/// trade itself — the commission must not become the row's symbol, quantity,
/// or price. Live trades carry five legs: four fees and the security.
#[test]
fn every_leg_survives_while_only_the_security_leg_is_summarized() {
    let txns = parse::transactions(&array("transactions.json"));
    let sale = &txns[0];
    assert_eq!(sale.items.len(), 5, "no fee leg may be discarded");
    let fee = sale
        .items
        .iter()
        .find(|i| i.fee_type.as_deref() == Some("COMMISSION"))
        .expect("commission leg");
    assert_eq!(fee.cost.as_deref(), Some("-0.5"));
    assert_ne!(sale.symbol.as_deref(), Some("CURRENCY_USD"));
}

#[test]
fn the_token_response_has_every_field_the_flow_depends_on() {
    let body = fixture("token_response.json");
    for field in ["access_token", "refresh_token", "expires_in", "token_type"] {
        assert!(body.get(field).is_some(), "token response missing {field}");
    }
    let tokens = schw::oauth::token_set_from_response(&body, 1_000, None).expect("parse");
    assert_eq!(tokens.expires_at, 1_000 + 1_800);
    assert_eq!(tokens.scope.as_deref(), Some("api"));
}

#[test]
fn the_error_body_shape_is_the_one_the_client_reads() {
    let body = fixture("error_401.json");
    assert!(body.get("message").is_some());
    assert!(body.get("errors").and_then(Value::as_array).is_some());
}

// -------------------------------------------------------------- hygiene ----

/// Walk every fixture and assert each identity-bearing value matches its dummy
/// shape. Stated positively — see the module docs.
#[test]
fn fixtures_contain_only_obvious_dummy_identifiers() {
    for name in [
        "account_numbers.json",
        "accounts.json",
        "positions.json",
        "transactions.json",
        "token_response.json",
        "error_401.json",
    ] {
        let mut checked = 0usize;
        walk(&fixture(name), &mut |key, value| {
            let Some(text) = value.as_str() else { return };
            match key {
                "accountNumber" => {
                    checked += 1;
                    assert!(
                        matches!(text, "11111111" | "22222222"),
                        "{name}: accountNumber `{text}` is not one of the documented dummies"
                    );
                }
                "hashValue" => {
                    checked += 1;
                    assert!(
                        text.starts_with("DUMMYHASH"),
                        "{name}: hashValue must start with DUMMYHASH"
                    );
                }
                "access_token" | "refresh_token" | "id_token" => {
                    checked += 1;
                    assert!(
                        text.starts_with("DUMMY"),
                        "{name}: {key} must start with DUMMY"
                    );
                }
                _ => {}
            }
        });
        // A fixture whose identity fields were all renamed would otherwise pass
        // this test by checking nothing at all.
        if matches!(
            name,
            "account_numbers.json"
                | "accounts.json"
                | "positions.json"
                | "transactions.json"
                | "token_response.json"
        ) {
            assert!(checked > 0, "{name}: no identity fields found to check");
        }
    }
}

/// Nothing in the fixtures may look like a real bearer token: a long opaque
/// string is exactly what a leaked credential looks like.
#[test]
fn no_fixture_contains_a_long_opaque_secret_like_string() {
    for name in [
        "account_numbers.json",
        "accounts.json",
        "positions.json",
        "transactions.json",
        "token_response.json",
    ] {
        walk(&fixture(name), &mut |key, value| {
            let Some(text) = value.as_str() else { return };
            if text.len() >= 20
                && !text.starts_with("DUMMY")
                && !is_timestamp(text)
                && !is_enum_constant(text)
            {
                // Descriptions and OCC option symbols are legitimately long;
                // they are human-readable, not opaque.
                let opaque = !text.contains(' ') && !text.contains('/');
                assert!(
                    !opaque,
                    "{name}: `{key}` holds a long opaque string that looks like a credential: \
                     `{text}`"
                );
            }
        });
    }
}

/// Schwab timestamps (`2026-03-14T14:31:02+0000`) are long and have no spaces,
/// which is exactly the shape the opaque-secret check looks for. They are not
/// secrets, so they are recognized and skipped rather than the check being
/// loosened for everything.
fn is_timestamp(text: &str) -> bool {
    let b = text.as_bytes();
    b.len() >= 10 && b[..4].iter().all(u8::is_ascii_digit) && b[4] == b'-' && b[7] == b'-'
}

/// Schwab enum constants (`COLLECTIVE_INVESTMENT`, `DIVIDEND_OR_INTEREST`)
/// are long and space-free too, but SCREAMING_SNAKE is not what a leaked
/// credential looks like.
fn is_enum_constant(text: &str) -> bool {
    text.bytes().all(|b| b.is_ascii_uppercase() || b == b'_')
}

fn walk(value: &Value, f: &mut impl FnMut(&str, &Value)) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                f(k, v);
                walk(v, f);
            }
        }
        Value::Array(items) => {
            for v in items {
                walk(v, f);
            }
        }
        _ => {}
    }
}
