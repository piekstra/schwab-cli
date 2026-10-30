//! Turning Schwab's JSON into the DTOs this CLI publishes.
//!
//! Every function here is **total**: a missing field, a renamed field, or a
//! string where a number was expected yields an omitted value, never a panic
//! and never a fabricated one. That is a deliberate response to what prior work
//! against this API found — Schwab's published schema and its live responses
//! disagree in a dozen documented places (enum values absent from the spec,
//! numbers typed as strings, singular objects where arrays were promised). A
//! parser that insists on the documented shape fails on real data; one that
//! omits what it cannot read keeps working and shows a gap.
//!
//! # Numbers
//!
//! Money and quantities are emitted as **strings**, per the family output
//! contract, and are produced by re-rendering the JSON number rather than by
//! doing arithmetic on it. Rust's `f64` Display prints the shortest
//! representation that round-trips, so a value that arrived as `1234.56` is
//! emitted as `"1234.56"` — not `1234.5600000000001`. This holds only because
//! nothing here computes: no summing, no netting, no lot matching. The moment
//! this CLI grows a realized-gain calculation, that work must move to a decimal
//! type, not an f64. See docs/api.md.

use pk_cli_core::Money;
use serde::Serialize;
use serde_json::Value;

/// One brokerage account.
#[derive(Debug, Serialize)]
pub struct Account {
    /// Schwab's `hashValue` — the handle every other endpoint wants.
    pub id: String,
    /// The real account number, masked to its last four digits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub account_number: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub r#type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equity: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cash: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_day_trader: Option<bool>,
}

/// One open position.
#[derive(Debug, Serialize)]
pub struct Position {
    pub symbol: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_value: Option<Money>,
    /// Unrealized P/L on the long side. **Not** a realized figure — see
    /// docs/api.md; nothing in this API reports realized gain or loss.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub open_pl: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day_pl: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One transaction — the tax feed's raw material.
#[derive(Debug, Serialize)]
pub struct Transaction {
    pub id: String,
    /// Trade date as ISO `YYYY-MM-DD`.
    pub date: String,
    pub r#type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub net_amount: Option<Money>,
    /// `OPENING` or `CLOSING` — the flag that says whether this trade realized
    /// anything. Schwab does not say *how much*; see docs/api.md.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_effect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Every leg of the transaction, as Schwab sent it. The security leg is
    /// summarized above; fees and cash legs live only here, and a caller doing
    /// its own basis arithmetic needs all of them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<TransferItem>,
}

#[derive(Debug, Serialize)]
pub struct TransferItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    /// The cash value of *this leg of this transaction* — **not** an
    /// acquisition cost basis. Naming it `cost` is Schwab's choice and it has
    /// misled people; see docs/api.md.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_effect: Option<String>,
}

/// Build accounts from `/accounts/accountNumbers` plus, optionally, the richer
/// `/accounts` payload.
///
/// The two are joined on the account hash. The hash list is the authority for
/// *which* accounts exist, so an account missing from the balances payload
/// still appears — with balances omitted rather than zeroed. A zero balance and
/// an unknown balance are very different claims to make about someone's money.
pub fn accounts(hashes: &[Value], detail: &[Value]) -> Vec<Account> {
    hashes
        .iter()
        .map(|h| {
            let id = string(h.get("hashValue")).unwrap_or_default();
            let number = string(h.get("accountNumber"));
            let sec = detail.iter().find_map(|d| {
                let s = d.get("securitiesAccount")?;
                // Live payloads key the detail by the plain account number.
                let matches = string(s.get("accountNumber")) == number && number.is_some();
                matches.then_some(s)
            });
            let balances = sec.and_then(|s| s.get("currentBalances"));
            Account {
                id,
                account_number: number.as_deref().map(mask_account),
                r#type: sec.and_then(|s| string(s.get("type"))),
                // `initialBalances` and `currentBalances` disagree on field
                // names across account types; accept either spelling.
                equity: balances
                    .and_then(|b| first_number(b, &["equity", "liquidationValue"]))
                    .map(Money::usd),
                cash: balances
                    .and_then(|b| first_number(b, &["cashBalance", "cashAvailableForTrading"]))
                    .map(Money::usd),
                is_day_trader: sec
                    .and_then(|s| s.get("isDayTrader"))
                    .and_then(Value::as_bool),
            }
        })
        .collect()
}

/// Pull the positions out of an `/accounts/{hash}?fields=positions` payload.
pub fn positions(payload: &Value) -> Vec<Position> {
    payload
        .get("securitiesAccount")
        .and_then(|s| s.get("positions"))
        .and_then(Value::as_array)
        .map(|rows| rows.iter().map(position).collect())
        .unwrap_or_default()
}

fn position(row: &Value) -> Position {
    let instrument = row.get("instrument");
    // Schwab reports long and short quantities in separate fields; a short
    // position is presented as a negative quantity so one column reads
    // correctly for both.
    let long = number(row.get("longQuantity"));
    let short = number(row.get("shortQuantity"));
    let quantity = match (&long, &short) {
        (Some(l), _) if l != "0" => Some(l.clone()),
        (_, Some(s)) if s != "0" => Some(format!("-{s}")),
        _ => long.or(short),
    };
    Position {
        symbol: instrument
            .and_then(|i| string(i.get("symbol")))
            .unwrap_or_else(|| "?".into()),
        asset_type: instrument.and_then(|i| string(i.get("assetType"))),
        quantity,
        average_price: number(row.get("averagePrice")),
        market_value: number(row.get("marketValue")).map(Money::usd),
        open_pl: first_number(row, &["longOpenProfitLoss", "shortOpenProfitLoss"]).map(Money::usd),
        day_pl: number(row.get("currentDayProfitLoss")).map(Money::usd),
        description: instrument.and_then(|i| string(i.get("description"))),
    }
}

pub fn transactions(rows: &[Value]) -> Vec<Transaction> {
    rows.iter().map(transaction).collect()
}

fn transaction(row: &Value) -> Transaction {
    let items: Vec<TransferItem> = row
        .get("transferItems")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(transfer_item).collect())
        .unwrap_or_default();
    // The security leg: the first item that names a non-cash instrument and is
    // not a fee. Fee legs carry `feeType`, but a cash leg (live: dividends pay
    // into a `CURRENCY_USD` instrument with no `feeType`) does not — filtering
    // on the asset type keeps `CURRENCY_USD` from masquerading as the row's
    // security.
    let leg = items.iter().find(|i| {
        i.fee_type.is_none() && i.symbol.is_some() && i.asset_type.as_deref() != Some("CURRENCY")
    });
    Transaction {
        id: first_string(row, &["activityId", "transactionId", "orderId"]).unwrap_or_default(),
        date: iso_date(row.get("tradeDate").or_else(|| row.get("time"))).unwrap_or_default(),
        r#type: string(row.get("type")).unwrap_or_else(|| "UNKNOWN".into()),
        symbol: leg.and_then(|l| l.symbol.clone()),
        quantity: leg.and_then(|l| l.amount.clone()),
        price: leg.and_then(|l| l.price.clone()),
        net_amount: number(row.get("netAmount")).map(Money::usd),
        position_effect: leg.and_then(|l| l.position_effect.clone()),
        activity_type: string(row.get("activityType")),
        status: string(row.get("status")),
        settlement_date: iso_date(row.get("settlementDate")),
        order_id: first_string(row, &["orderId"]),
        description: string(row.get("description")),
        items,
    }
}

fn transfer_item(row: &Value) -> TransferItem {
    let instrument = row.get("instrument");
    TransferItem {
        symbol: instrument.and_then(|i| string(i.get("symbol"))),
        asset_type: instrument.and_then(|i| string(i.get("assetType"))),
        amount: number(row.get("amount")),
        price: number(row.get("price")),
        cost: number(row.get("cost")),
        fee_type: string(row.get("feeType")),
        position_effect: string(row.get("positionEffect")),
    }
}

/// Keep only rows whose date falls within an inclusive ISO bound.
pub fn within<T>(
    rows: Vec<T>,
    since: Option<&str>,
    until: Option<&str>,
    date_of: impl Fn(&T) -> &str,
) -> Vec<T> {
    rows.into_iter()
        .filter(|r| {
            let d = date_of(r);
            // An undated row is kept: dropping it would silently shrink a tax
            // feed, and a visible oddity beats an invisible omission.
            d.is_empty() || (since.is_none_or(|s| d >= s) && until.is_none_or(|u| d <= u))
        })
        .collect()
}

/// First 10 characters of a Schwab timestamp: `2026-03-14T00:00:00+0000` →
/// `2026-03-14`.
fn iso_date(v: Option<&Value>) -> Option<String> {
    let s = string(v)?;
    let date: String = s.chars().take(10).collect();
    // Only accept something that actually looks like a date, so a surprise
    // format yields "no date" rather than a convincing-looking wrong one.
    let looks_iso = date.len() == 10
        && date.as_bytes()[4] == b'-'
        && date.as_bytes()[7] == b'-'
        && date
            .bytes()
            .enumerate()
            .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit());
    looks_iso.then_some(date)
}

/// Show only the last four digits of an account number.
pub fn mask_account(number: &str) -> String {
    let n = number.chars().count();
    if n <= 4 {
        return "…".into();
    }
    format!("…{}", number.chars().skip(n - 4).collect::<String>())
}

fn string(v: Option<&Value>) -> Option<String> {
    v.and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
}

fn first_string(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| {
        v.get(*k).and_then(|x| {
            // Schwab sends ids as both strings and numbers depending on the
            // field; accept either rather than losing the id.
            string(Some(x)).or_else(|| x.as_i64().map(|n| n.to_string()))
        })
    })
}

/// Render a JSON number as a decimal string. See the module docs on why this
/// is safe and when it stops being safe.
fn number(v: Option<&Value>) -> Option<String> {
    let v = v?;
    if let Some(i) = v.as_i64() {
        return Some(i.to_string());
    }
    if let Some(f) = v.as_f64() {
        return Some(f.to_string());
    }
    // Prior work found Schwab sending numbers as strings on some fields
    // (option strike prices, notably). Pass those through rather than dropping
    // the value.
    string(Some(v)).filter(|s| s.parse::<f64>().is_ok())
}

fn first_number(v: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| number(v.get(*k)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn numbers_render_without_float_artifacts() {
        assert_eq!(number(Some(&json!(1234.56))).as_deref(), Some("1234.56"));
        assert_eq!(number(Some(&json!(100))).as_deref(), Some("100"));
        assert_eq!(number(Some(&json!(0.0))).as_deref(), Some("0"));
        assert_eq!(number(Some(&json!(-42.5))).as_deref(), Some("-42.5"));
        // A number sent as a string still reads.
        assert_eq!(number(Some(&json!("175.25"))).as_deref(), Some("175.25"));
        // Genuinely non-numeric text does not.
        assert_eq!(number(Some(&json!("N/A"))), None);
        assert_eq!(number(Some(&json!(null))), None);
        assert_eq!(number(None), None);
    }

    #[test]
    fn dates_are_truncated_to_iso_days() {
        assert_eq!(
            iso_date(Some(&json!("2026-03-14T00:00:00+0000"))).as_deref(),
            Some("2026-03-14")
        );
        assert_eq!(
            iso_date(Some(&json!("2026-03-14"))).as_deref(),
            Some("2026-03-14")
        );
    }

    /// A surprise date format must yield nothing rather than a plausible wrong
    /// answer — `03/14/2026` truncated blindly would read as `03/14/2026`.
    #[test]
    fn an_unrecognized_date_format_is_dropped_not_mangled() {
        assert_eq!(iso_date(Some(&json!("03/14/2026"))), None);
        assert_eq!(iso_date(Some(&json!("not a date"))), None);
        assert_eq!(iso_date(Some(&json!(""))), None);
        assert_eq!(iso_date(None), None);
    }

    #[test]
    fn account_numbers_are_masked_to_four_digits() {
        assert_eq!(mask_account("12345678"), "…5678");
        assert_eq!(mask_account("123"), "…");
    }

    #[test]
    fn accounts_join_hashes_to_balances_on_the_account_number() {
        let hashes = vec![json!({"accountNumber": "12345678", "hashValue": "HASH1"})];
        let detail = vec![json!({"securitiesAccount": {
            "accountNumber": "12345678", "type": "MARGIN", "isDayTrader": false,
            "currentBalances": {"equity": 10000.5, "cashBalance": 250.0}
        }})];
        let out = accounts(&hashes, &detail);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].id, "HASH1");
        assert_eq!(out[0].account_number.as_deref(), Some("…5678"));
        assert_eq!(out[0].r#type.as_deref(), Some("MARGIN"));
        assert_eq!(out[0].equity.as_ref().unwrap().amount, "10000.5");
    }

    /// An account with no balance payload must still be listed, with the money
    /// fields *absent* — never zero. "I don't know" and "it's zero" are
    /// different claims.
    #[test]
    fn an_account_without_balances_is_listed_with_no_money_fields() {
        let hashes = vec![json!({"accountNumber": "12345678", "hashValue": "HASH1"})];
        let out = accounts(&hashes, &[]);
        assert_eq!(out.len(), 1);
        assert!(out[0].equity.is_none());
        assert!(out[0].cash.is_none());
        let v = serde_json::to_value(&out[0]).unwrap();
        assert!(v.get("equity").is_none(), "must be omitted, not null");
    }

    /// The join must not pair an account with another account's money.
    #[test]
    fn balances_are_never_taken_from_a_different_account() {
        let hashes = vec![json!({"accountNumber": "11111111", "hashValue": "HASH1"})];
        let detail = vec![json!({"securitiesAccount": {
            "accountNumber": "99999999", "currentBalances": {"equity": 999.0}
        }})];
        let out = accounts(&hashes, &detail);
        assert!(out[0].equity.is_none());
    }

    #[test]
    fn a_short_position_reads_as_a_negative_quantity() {
        let payload = json!({"securitiesAccount": {"positions": [
            {"instrument": {"symbol": "TSLA", "assetType": "EQUITY"},
             "longQuantity": 0, "shortQuantity": 50, "averagePrice": 210.0},
            {"instrument": {"symbol": "AAPL", "assetType": "EQUITY"},
             "longQuantity": 100, "shortQuantity": 0, "averagePrice": 150.0}
        ]}});
        let out = positions(&payload);
        assert_eq!(out[0].quantity.as_deref(), Some("-50"));
        assert_eq!(out[1].quantity.as_deref(), Some("100"));
    }

    #[test]
    fn a_payload_without_positions_yields_an_empty_list_not_a_panic() {
        assert!(positions(&json!({})).is_empty());
        assert!(positions(&json!({"securitiesAccount": {}})).is_empty());
        assert!(positions(&json!({"securitiesAccount": {"positions": "nope"}})).is_empty());
    }

    fn trade() -> Value {
        json!({
            "activityId": 90210, "type": "TRADE", "status": "VALID",
            "tradeDate": "2026-03-14T14:31:02+0000",
            "settlementDate": "2026-03-16T00:00:00+0000",
            "activityType": "EXECUTION", "orderId": 12345,
            "netAmount": 17518.05, "description": "SELL TRADE",
            "transferItems": [
                {"instrument": {"symbol": "AAPL", "assetType": "EQUITY"},
                 "amount": -100, "price": 175.25, "cost": 17525.0,
                 "positionEffect": "CLOSING"},
                {"feeType": "COMMISSION", "amount": 1, "cost": -6.95}
            ]
        })
    }

    #[test]
    fn a_trade_summarizes_its_security_leg() {
        let t = transaction(&trade());
        assert_eq!(t.id, "90210");
        assert_eq!(t.date, "2026-03-14");
        assert_eq!(t.symbol.as_deref(), Some("AAPL"));
        assert_eq!(t.quantity.as_deref(), Some("-100"));
        assert_eq!(t.price.as_deref(), Some("175.25"));
        assert_eq!(t.position_effect.as_deref(), Some("CLOSING"));
        assert_eq!(t.net_amount.as_ref().unwrap().amount, "17518.05");
    }

    /// The fee leg must not be mistaken for the security leg, or the symbol and
    /// quantity columns silently describe the commission instead of the trade.
    #[test]
    fn the_fee_leg_is_never_chosen_as_the_security_leg() {
        let mut v = trade();
        // Put the fee first, which is how some payloads order them.
        let items = v["transferItems"].as_array().unwrap().clone();
        v["transferItems"] = json!([items[1], items[0]]);
        let t = transaction(&v);
        assert_eq!(t.symbol.as_deref(), Some("AAPL"));
        assert_eq!(t.quantity.as_deref(), Some("-100"));
        // But every leg is still published for a caller doing its own maths.
        assert_eq!(t.items.len(), 2);
        assert!(t
            .items
            .iter()
            .any(|i| i.fee_type.as_deref() == Some("COMMISSION")));
    }

    /// Live-verified 2026-08-12: a dividend's only leg is the `CURRENCY_USD`
    /// cash instrument, carrying no `feeType`. It must not be summarized as
    /// the row's security.
    #[test]
    fn a_cash_only_dividend_reports_no_security_symbol() {
        let t = transaction(&json!({
            "activityId": 3, "type": "DIVIDEND_OR_INTEREST",
            "netAmount": 12.34,
            "transferItems": [
                {"instrument": {"symbol": "CURRENCY_USD", "assetType": "CURRENCY"},
                 "amount": 12.34, "cost": 0.0, "price": 0.0}
            ]
        }));
        assert!(t.symbol.is_none());
        assert_eq!(t.items.len(), 1, "the cash leg is still published");
    }

    #[test]
    fn a_transaction_with_no_items_still_parses() {
        let t = transaction(&json!({"activityId": 1, "type": "JOURNAL"}));
        assert_eq!(t.id, "1");
        assert_eq!(t.r#type, "JOURNAL");
        assert!(t.symbol.is_none());
        assert!(t.items.is_empty());
        // An unreadable date must not become a fake one.
        assert!(t.date.is_empty());
    }

    #[test]
    fn an_unknown_transaction_type_is_reported_not_dropped() {
        let t = transaction(&json!({"activityId": 1, "type": "SOME_NEW_TYPE"}));
        assert_eq!(t.r#type, "SOME_NEW_TYPE");
        let t = transaction(&json!({"activityId": 1}));
        assert_eq!(t.r#type, "UNKNOWN");
    }

    #[test]
    fn date_bounds_are_inclusive_on_both_ends() {
        let rows = vec!["2026-01-01", "2026-06-15", "2026-12-31"];
        let kept = within(rows.clone(), Some("2026-01-01"), Some("2026-12-31"), |r| r);
        assert_eq!(kept.len(), 3);
        let kept = within(rows.clone(), Some("2026-02-01"), Some("2026-11-30"), |r| r);
        assert_eq!(kept, vec!["2026-06-15"]);
        let kept = within(rows, None, None, |r| r);
        assert_eq!(kept.len(), 3);
    }

    /// Dropping an undated row would quietly shorten a tax feed.
    #[test]
    fn an_undated_row_survives_filtering() {
        let kept = within(vec![""], Some("2026-01-01"), Some("2026-12-31"), |r| *r);
        assert_eq!(kept.len(), 1);
    }
}
