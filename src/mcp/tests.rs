//! The tools over the scrubbed fixtures, offline: no network, no keychain. Every
//! request a tool makes goes through [`Fixtures`], which records it, so the
//! read-only guarantee is tested against the requests themselves.

use std::collections::HashMap;
use std::sync::Mutex;

use pk_cli_mcp::conformance::{assert_conforms, assert_fits, call, definitions_size};
use pk_cli_mcp::{tools_list_json, CallToolResult};
use serde_json::{json, Value};

use super::*;
use crate::commands::transactions::RESPONSE_CAP;

const HASH_MARGIN: &str = "DUMMYHASH11111111111111111111111111111111111111111111111111111AA";
const HASH_CASH: &str = "DUMMYHASH22222222222222222222222222222222222222222222222222222BB";

fn fixture(raw: &str) -> Value {
    serde_json::from_str(raw).unwrap()
}

/// Schwab, as the fixtures have it. An entry in `answers` replaces what a path
/// answers, and `fail` fails every later request.
#[derive(Default)]
struct Fixtures {
    /// Every request, as `path?key=value&…`.
    calls: Mutex<Vec<String>>,
    answers: Mutex<HashMap<String, Value>>,
    fail: Mutex<Option<CliError>>,
    /// How to answer a transactions request, in place of the fixture.
    windows: Mutex<Option<Windows>>,
}

type Windows = Box<dyn Fn(&str, &str) -> Vec<Value> + Send>;

impl Fixtures {
    fn answering(self, path: &str, body: Value) -> Self {
        self.answers.lock().unwrap().insert(path.into(), body);
        self
    }

    fn failing(self, e: CliError) -> Self {
        *self.fail.lock().unwrap() = Some(e);
        self
    }

    fn with_windows(self, f: impl Fn(&str, &str) -> Vec<Value> + Send + 'static) -> Self {
        *self.windows.lock().unwrap() = Some(Box::new(f));
        self
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn transactions(&self, query: &[(&str, String)]) -> Value {
        let value = |key: &str| {
            query
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| v.as_str())
        };
        let (start, end) = (value("startDate").unwrap(), value("endDate").unwrap());
        if let Some(f) = &*self.windows.lock().unwrap() {
            return Value::Array(f(start, end));
        }
        let all = fixture(include_str!("../../tests/fixtures/transactions.json"));
        let rows = all.as_array().unwrap().iter().filter(|row| {
            let date = row["tradeDate"].as_str().unwrap();
            let day = &date[..10];
            day >= &start[..10]
                && day <= &end[..10]
                && value("types").is_none_or(|t| row["type"] == t)
        });
        Value::Array(rows.cloned().collect())
    }
}

impl Reads for &Fixtures {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, CliError> {
        let pairs: Vec<String> = query.iter().map(|(k, v)| format!("{k}={v}")).collect();
        self.calls
            .lock()
            .unwrap()
            .push(format!("{path}?{}", pairs.join("&")));
        if let Some(e) = self.fail.lock().unwrap().take() {
            return Err(e);
        }
        if let Some(body) = self.answers.lock().unwrap().get(path) {
            return Ok(body.clone());
        }
        let margin = format!("{ACCOUNTS}/{HASH_MARGIN}");
        let cash = format!("{ACCOUNTS}/{HASH_CASH}");
        match path {
            ACCOUNT_NUMBERS => Ok(fixture(include_str!(
                "../../tests/fixtures/account_numbers.json"
            ))),
            ACCOUNTS => Ok(fixture(include_str!("../../tests/fixtures/accounts.json"))),
            p if p == margin => Ok(fixture(include_str!(
                "../../tests/fixtures/positions_margin.json"
            ))),
            p if p == cash => Ok(fixture(include_str!("../../tests/fixtures/positions.json"))),
            p if p == format!("{margin}/transactions") => Ok(self.transactions(query)),
            p if p == format!("{cash}/transactions") => Ok(Value::Array(Vec::new())),
            other => Err(CliError::NotFound(format!("no fixture for {other}"))),
        }
    }
}

impl Source for Fixtures {
    fn open(&self) -> Result<Box<dyn Reads + '_>, CliError> {
        Ok(Box::new(self))
    }
}

fn tools() -> SchwabTools<Fixtures> {
    SchwabTools::new(Fixtures::default())
}

fn with(source: Fixtures) -> SchwabTools<Fixtures> {
    SchwabTools::new(source)
}

fn text(result: &CallToolResult) -> String {
    result.text().to_string()
}

/// A call's error text, asserting it is an error.
fn error(tools: &SchwabTools<Fixtures>, tool: &str, args: Value) -> String {
    let result = call(tools, tool, args);
    assert!(result.is_error(), "{}", text(&result));
    text(&result)
}

fn ok(tools: &SchwabTools<Fixtures>, tool: &str, args: Value) -> Value {
    let result = call(tools, tool, args);
    assert_fits(tools, tool, &result).clone()
}

// ---- the profile -------------------------------------------------------------

#[test]
fn the_tools_conform_to_the_profile() {
    assert_conforms(&tools());
    assert!(definitions_size(&tools()) < 8_000);
}

#[test]
fn listing_the_tools_opens_no_session() {
    struct Refuses;
    impl Source for Refuses {
        fn open(&self) -> Result<Box<dyn Reads + '_>, CliError> {
            panic!("listing the tools must not read a credential");
        }
    }
    let tools = SchwabTools::new(Refuses);
    let names: Vec<String> = tools.tools().into_iter().map(|t| t.name).collect();
    assert_eq!(names, [PORTFOLIO_TOOL, TRANSACTIONS_TOOL]);
    let _ = tools_list_json(&tools);
}

// ---- read-only ---------------------------------------------------------------

/// Every path a tool can ask for, over every filter it takes.
#[test]
fn every_request_any_tool_makes_is_a_listed_read() {
    let allowed = [
        ACCOUNT_NUMBERS.to_string(),
        ACCOUNTS.to_string(),
        format!("{ACCOUNTS}/{HASH_MARGIN}"),
        format!("{ACCOUNTS}/{HASH_CASH}"),
        format!("{ACCOUNTS}/{HASH_MARGIN}/transactions"),
        format!("{ACCOUNTS}/{HASH_CASH}/transactions"),
    ];
    let tools = tools();
    for (tool, args) in [
        (PORTFOLIO_TOOL, json!({})),
        (PORTFOLIO_TOOL, json!({"account": "1111"})),
        (PORTFOLIO_TOOL, json!({"symbol": "AAPL"})),
        (PORTFOLIO_TOOL, json!({"response_format": "detailed"})),
        (TRANSACTIONS_TOOL, json!({})),
        (
            TRANSACTIONS_TOOL,
            json!({"since": "2026-03-01", "until": "2026-03-31", "account": "MARGIN",
                   "type": "TRADE", "symbol": "AAPL", "response_format": "detailed"}),
        ),
    ] {
        ok(&tools, tool, args);
    }
    let calls = tools.source().calls();
    assert!(!calls.is_empty());
    for request in calls {
        let path = request.split('?').next().unwrap();
        assert!(allowed.iter().any(|a| a == path), "{request}");
    }
}

/// `account` is compared with Schwab's list, never put in a path: a model cannot
/// reach another endpoint, even as a read, by what it passes.
#[test]
fn a_call_cannot_steer_a_request_to_another_path() {
    for account in [
        "x/orders",
        "../orders",
        "/trader/v1/orders",
        "%2Forders",
        &format!("{HASH_MARGIN}/orders"),
        &format!("{HASH_MARGIN}/../orders"),
    ] {
        for tool in [PORTFOLIO_TOOL, TRANSACTIONS_TOOL] {
            let tools = tools();
            let says = error(&tools, tool, json!({ "account": account }));
            assert!(says.starts_with("not_found:"), "{says}");
            for request in tools.source().calls() {
                assert!(
                    request.starts_with(ACCOUNT_NUMBERS) || request.starts_with(ACCOUNTS),
                    "{request}"
                );
                assert!(!request.contains("orders"), "{request}");
            }
        }
    }
}

/// A hash from Schwab goes into a path too, so one that could add a segment is an
/// error rather than a request.
#[test]
fn a_hash_that_could_add_a_path_segment_is_never_requested() {
    let tools = with(Fixtures::default().answering(
        ACCOUNT_NUMBERS,
        json!([{"accountNumber": "11111111", "hashValue": "DUMMYHASH/orders"}]),
    ));
    let says = error(&tools, PORTFOLIO_TOOL, json!({}));
    assert!(says.starts_with("upstream:"), "{says}");
    assert!(!says.contains("DUMMYHASH"), "{says}");
    assert!(tools.source().calls().iter().all(|c| !c.contains("orders")));
}

/// `--allow-writes` serves what is not read-only too. There is nothing of that
/// kind, and no definition speaks of the order surface, so none invites it.
#[test]
fn no_definition_reaches_the_order_surface_or_is_a_write() {
    let list = tools_list_json(&tools());
    for tool in list["tools"].as_array().unwrap() {
        let hints = &tool["annotations"];
        assert_eq!(hints["readOnlyHint"], true, "{tool}");
        assert_eq!(hints["destructiveHint"], false, "{tool}");
        let body = tool.to_string().to_ascii_lowercase();
        for word in ["order", "preview", "cancel", "replace", "place", "transfer"] {
            assert!(!body.contains(word), "`{word}` in {}", tool["name"]);
        }
    }
}

/// Only `oauth.rs` sends anything but a GET, to the token endpoint. The tools'
/// way out and the client under it have no other verb.
#[test]
fn neither_the_tools_nor_the_client_have_a_write_verb() {
    for source in [include_str!("../client.rs"), include_str!("../mcp.rs")] {
        for verb in [".post(", ".put(", ".patch(", ".delete(", ".request("] {
            assert!(!source.contains(verb), "`{verb}`");
        }
    }
    let trait_body = include_str!("../mcp.rs")
        .split("pub trait Reads {")
        .nth(1)
        .unwrap()
        .split('}')
        .next()
        .unwrap();
    assert_eq!(trait_body.matches("fn ").count(), 1, "{trait_body}");
    assert!(trait_body.contains("fn get("), "{trait_body}");
}

// ---- schwab_portfolio --------------------------------------------------------

#[test]
fn the_portfolio_names_accounts_and_lists_positions() {
    let tools = tools();
    let out = ok(&tools, PORTFOLIO_TOOL, json!({}));
    let accounts = out["accounts"].as_array().unwrap();
    assert_eq!(accounts.len(), 2);
    assert_eq!(accounts[0]["name"], "MARGIN …1111");
    assert_eq!(accounts[0]["equity"], "30000");
    assert_eq!(accounts[0]["cash"], "5000");
    assert_eq!(accounts[1]["name"], "CASH …2222");
    assert_eq!(accounts[1]["equity"], "42000", "liquidationValue for CASH");

    let items = out["items"].as_array().unwrap();
    assert_eq!(out["returned"], 3);
    let etf = items.iter().find(|p| p["symbol"] == "ETFX").unwrap();
    assert_eq!(etf["account"], "CASH …2222");
    assert_eq!(etf["quantity"], "100");
    assert_eq!(etf["market_value"], "17550");
    assert_eq!(etf["unrealized_gain"], "2525");
    assert_eq!(etf["average_price"], "150.25");
    assert!(etf.get("description").is_none(), "concise");

    let short = items.iter().find(|p| p["symbol"] == "TSLA").unwrap();
    assert_eq!(short["quantity"], "-50");
    assert_eq!(short["unrealized_gain"], "-250");
}

#[test]
fn detailed_adds_provider_detail() {
    let out = ok(
        &tools(),
        PORTFOLIO_TOOL,
        json!({"response_format": "detailed", "symbol": "ETFX"}),
    );
    let etf = &out["items"][0];
    assert_eq!(etf["description"], "DUMMY EXCHANGE TRADED FUND");
    assert_eq!(etf["asset_type"], "COLLECTIVE_INVESTMENT");
    assert_eq!(etf["day_gain"], "125.5");
}

/// Neither format carries the hash or the full number: the hash plus a token reads
/// the account, and the number is on nobody's list of what these tools return.
#[test]
fn no_result_carries_an_account_number_or_a_hash() {
    let tools = tools();
    for format in ["concise", "detailed"] {
        let args = json!({"response_format": format, "since": "2026-03-01", "until": "2026-03-31"});
        let transactions = ok(&tools, TRANSACTIONS_TOOL, args);
        let portfolio = ok(&tools, PORTFOLIO_TOOL, json!({"response_format": format}));
        for out in [transactions, portfolio] {
            let body = out.to_string();
            assert!(!body.contains("DUMMYHASH"), "{body}");
            assert!(!body.contains("11111111"), "{body}");
            assert!(!body.contains("22222222"), "{body}");
        }
    }
    let says = error(&tools, PORTFOLIO_TOOL, json!({"account": "nope"}));
    assert!(says.contains("MARGIN …1111, CASH …2222"), "{says}");
    assert!(!says.contains("DUMMYHASH"), "{says}");
}

#[test]
fn an_account_is_named_by_its_tail_label_type_or_hash() {
    for (account, expected) in [
        ("1111", "MARGIN …1111"),
        ("…2222", "CASH …2222"),
        ("x2222", "CASH …2222"),
        ("margin …1111", "MARGIN …1111"),
        ("cash", "CASH …2222"),
        (HASH_MARGIN, "MARGIN …1111"),
    ] {
        let out = ok(&tools(), PORTFOLIO_TOOL, json!({ "account": account }));
        let accounts = out["accounts"].as_array().unwrap();
        assert_eq!(accounts.len(), 1, "{account}");
        assert_eq!(accounts[0]["name"], expected, "{account}");
    }
}

#[test]
fn a_symbol_matches_itself_and_options_on_it() {
    let out = ok(&tools(), PORTFOLIO_TOOL, json!({"symbol": "aapl"}));
    assert_eq!(out["returned"], 1);
    assert_eq!(out["items"][0]["symbol"], "AAPL  260320C00180000");
}

#[test]
fn an_account_holding_nothing_is_an_empty_list() {
    let path = format!("{ACCOUNTS}/{HASH_CASH}");
    let tools = with(Fixtures::default().answering(
        &path,
        json!({"securitiesAccount": {"type": "CASH", "currentBalances": {
            "cashBalance": 10.0, "longMarketValue": 0.0, "shortMarketValue": 0.0
        }}}),
    ));
    let out = ok(&tools, PORTFOLIO_TOOL, json!({"account": "2222"}));
    assert_eq!(out["returned"], 0);
}

/// "You hold nothing" must not come from an answer that says otherwise, or says
/// nothing.
#[test]
fn an_answer_schwab_did_not_give_is_an_error_not_an_empty_portfolio() {
    let path = format!("{ACCOUNTS}/{HASH_CASH}");
    for (body, why) in [
        (json!({"renamed": {}}), "has no securitiesAccount"),
        (
            json!({"securitiesAccount": {"type": "CASH"}}),
            "states neither positions nor balances",
        ),
        (
            json!({"securitiesAccount": {"currentBalances": {"longMarketValue": 5.0}}}),
            "states holdings in its balances and no positions",
        ),
        (
            json!({"securitiesAccount": {"positions": {"not": "a list"}}}),
            "has positions that are not a list",
        ),
        (
            json!({"securitiesAccount": {"positions": [{"renamed": {}}]}}),
            "has positions and none can be read",
        ),
    ] {
        let tools = with(Fixtures::default().answering(&path, body));
        let says = error(&tools, PORTFOLIO_TOOL, json!({"account": "2222"}));
        assert!(says.starts_with("upstream:"), "{says}");
        assert!(says.contains(why), "{says}");
    }
}

#[test]
fn an_account_list_that_is_not_a_list_is_an_error_not_no_accounts() {
    let tools = with(Fixtures::default().answering(ACCOUNT_NUMBERS, json!("nope")));
    let says = error(&tools, PORTFOLIO_TOOL, json!({}));
    assert!(says.starts_with("upstream:"), "{says}");

    let tools = with(Fixtures::default().answering(ACCOUNTS, Value::Null));
    let says = error(&tools, PORTFOLIO_TOOL, json!({}));
    assert!(says.starts_with("upstream:"), "{says}");
}

#[test]
fn no_accounts_says_where_they_are_chosen() {
    let tools = with(Fixtures::default().answering(ACCOUNT_NUMBERS, json!([])));
    let says = error(&tools, TRANSACTIONS_TOOL, json!({}));
    assert!(says.starts_with("not_found:"), "{says}");
    assert!(says.contains("consent page"), "{says}");
    assert!(!says.contains("list tool"), "{says}");
}

// ---- schwab_transactions -----------------------------------------------------

fn march() -> Value {
    json!({"since": "2026-03-01", "until": "2026-03-31"})
}

#[test]
fn transactions_list_newest_first_with_the_concise_fields() {
    let out = ok(&tools(), TRANSACTIONS_TOOL, march());
    assert_eq!(out["since"], "2026-03-01");
    assert_eq!(out["until"], "2026-03-31");
    assert_eq!(out["complete"], true);
    assert!(out.get("note").is_none());
    let items = out["items"].as_array().unwrap();
    let dates: Vec<&str> = items.iter().map(|t| t["date"].as_str().unwrap()).collect();
    assert_eq!(dates, ["2026-03-14", "2026-03-13", "2026-03-11"]);

    let sale = &items[0];
    assert_eq!(sale["type"], "TRADE");
    assert_eq!(sale["symbol"], "AAPL  260320C00180000");
    assert_eq!(sale["amount"], "249.49");
    assert_eq!(sale["position_effect"], "CLOSING");
    assert_eq!(sale["account"], "MARGIN …1111");
    assert!(sale.get("legs").is_none(), "concise");
    assert!(sale.get("id").is_none(), "concise");
}

#[test]
fn detailed_adds_every_leg_and_the_id() {
    let mut args = march();
    args["response_format"] = json!("detailed");
    let out = ok(&tools(), TRANSACTIONS_TOOL, args);
    let sale = &out["items"][0];
    assert_eq!(sale["id"], "90000000001");
    let legs = sale["legs"].as_array().unwrap();
    assert_eq!(legs.len(), 5);
    assert!(legs.iter().any(|l| l.get("fee_type").is_some()));
    let dividend = &out["items"][2];
    assert_eq!(dividend["settlement_date"], "2026-03-11");
}

#[test]
fn type_and_symbol_narrow_and_a_lowercase_type_is_schwabs_spelling() {
    let mut args = march();
    args["type"] = json!("trade");
    let tools = tools();
    let out = ok(&tools, TRANSACTIONS_TOOL, args);
    assert_eq!(out["returned"], 2);
    assert!(tools
        .source()
        .calls()
        .iter()
        .any(|c| c.contains("types=TRADE")));

    let mut args = march();
    args["symbol"] = json!("AAPL");
    let out = ok(&with(Fixtures::default()), TRANSACTIONS_TOOL, args);
    assert_eq!(out["returned"], 2, "options on it");

    let mut args = march();
    args["symbol"] = json!("ETFX");
    let out = ok(&with(Fixtures::default()), TRANSACTIONS_TOOL, args);
    assert_eq!(
        out["returned"], 0,
        "a dividend paid in cash names no symbol"
    );
}

#[test]
fn no_type_asks_for_every_type() {
    let tools = tools();
    ok(&tools, TRANSACTIONS_TOOL, march());
    assert!(tools.source().calls().iter().all(|c| !c.contains("types=")));
}

#[test]
fn a_long_list_pages_and_its_cursor_is_bound_to_its_filters() {
    let rows = |n: u32| -> Vec<Value> {
        (0..n)
            .map(|i| {
                json!({"activityId": i, "type": "TRADE",
                       "tradeDate": "2026-03-14T14:31:02+0000", "netAmount": 1.5})
            })
            .collect()
    };
    let tools = with(Fixtures::default().with_windows(move |start, _| {
        if start.starts_with("2026-03") {
            rows(30)
        } else {
            Vec::new()
        }
    }));
    let mut args = march();
    args["account"] = json!("1111");
    let first = ok(&tools, TRANSACTIONS_TOOL, args.clone());
    assert_eq!(first["returned"], 25, "the default limit");
    let cursor = first["next_cursor"].as_str().unwrap().to_string();

    let mut next = args.clone();
    next["cursor"] = json!(cursor);
    let second = ok(&tools, TRANSACTIONS_TOOL, next.clone());
    assert_eq!(second["returned"], 5);
    assert!(second.get("next_cursor").is_none());

    next["since"] = json!("2026-03-02");
    let says = error(&tools, TRANSACTIONS_TOOL, next);
    assert!(says.starts_with("usage:"), "{says}");
}

/// A repeated row from a server that treats a boundary inclusively is one row.
/// The scenario this CLI's first cursor left open: a trade posts between pages.
#[test]
fn a_trade_that_posts_between_pages_repeats_nothing_and_skips_nothing() {
    let posted = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let seen_posted = posted.clone();
    let tools = with(Fixtures::default().with_windows(move |_, _| {
        let row = |id: u64, day: u32| {
            json!({"activityId": id, "type": "TRADE",
                   "tradeDate": format!("2026-03-{day:02}T14:31:02+0000"), "netAmount": 1.5})
        };
        let mut rows = vec![row(4, 14), row(3, 13), row(2, 12), row(1, 11)];
        if seen_posted.load(std::sync::atomic::Ordering::SeqCst) {
            rows.insert(0, row(5, 15));
        }
        rows
    }));
    let mut args = json!({"since": "2026-03-01", "until": "2026-03-31", "limit": 2,
                          "account": "MARGIN", "response_format": "detailed"});
    let first = ok(&tools, TRANSACTIONS_TOOL, args.clone());
    let ids = |page: &Value| -> Vec<String> {
        page["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["date"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(ids(&first), ["2026-03-14", "2026-03-13"]);
    posted.store(true, std::sync::atomic::Ordering::SeqCst);
    args["cursor"] = first["next_cursor"].clone();
    let second = ok(&tools, TRANSACTIONS_TOOL, args);
    assert_eq!(ids(&second), ["2026-03-12", "2026-03-11"]);
}

#[test]
fn a_row_two_windows_return_is_counted_once() {
    let tools = with(Fixtures::default().with_windows(|_, _| {
        vec![json!({"activityId": 7, "type": "TRADE",
                    "tradeDate": "2026-03-14T14:31:02+0000", "netAmount": 1.5})]
    }));
    let out = ok(
        &tools,
        TRANSACTIONS_TOOL,
        json!({"since": "2026-02-01", "until": "2026-03-31", "account": "1111"}),
    );
    assert_eq!(out["returned"], 1);
}

/// One second at Schwab's cap cannot be narrowed: the rows it returned are served,
/// and the result says some are missing.
#[test]
fn a_second_at_the_cap_is_reported_as_incomplete() {
    let the_second = "2026-03-14T14:31:02";
    let tools = with(Fixtures::default().with_windows(move |start, end| {
        if start[..19] <= *the_second && *the_second <= end[..19] {
            (0..RESPONSE_CAP)
                .map(|i| {
                    json!({"activityId": i, "type": "TRADE",
                           "tradeDate": "2026-03-14T14:31:02+0000", "netAmount": 1.5})
                })
                .collect()
        } else {
            Vec::new()
        }
    }));
    let mut args = march();
    args["account"] = json!("1111");
    let out = ok(&tools, TRANSACTIONS_TOOL, args);
    assert_eq!(out["complete"], false);
}

/// A server that never stops answering at the cap would bisect for days.
#[test]
fn a_range_that_needs_too_many_requests_is_refused() {
    let cap: Vec<Value> = (0..RESPONSE_CAP)
        .map(|i| json!({"activityId": i, "type": "TRADE"}))
        .collect();
    let tools = with(Fixtures::default().with_windows(move |_, _| cap.clone()));
    let says = error(&tools, TRANSACTIONS_TOOL, march());
    assert!(says.starts_with("usage:"), "{says}");
    assert!(says.contains("narrow"), "{says}");
    let requested = tools
        .source()
        .calls()
        .iter()
        .filter(|c| c.contains("/transactions"))
        .count();
    assert_eq!(requested, MAX_REQUESTS);
}

#[test]
fn history_schwab_may_not_hold_is_noted() {
    let out = ok(
        &tools(),
        TRANSACTIONS_TOOL,
        json!({"since": "2020-01-01", "until": "2020-01-31"}),
    );
    assert!(out["note"].as_str().unwrap().contains("may be short"));
}

#[test]
fn rows_no_one_can_date_are_an_error_not_a_list() {
    let tools = with(
        Fixtures::default()
            .with_windows(|_, _| vec![json!({"renamedDate": "2026-03-14", "type": "TRADE"})]),
    );
    let mut args = march();
    args["account"] = json!("1111");
    let says = error(&tools, TRANSACTIONS_TOOL, args);
    assert!(says.starts_with("upstream:"), "{says}");
    assert!(says.contains("MARGIN …1111"), "{says}");
}

#[test]
fn a_transactions_answer_that_is_not_a_list_is_an_error() {
    let path = format!("{ACCOUNTS}/{HASH_MARGIN}/transactions");
    let tools = with(Fixtures::default().answering(&path, json!({"message": "changed"})));
    let mut args = march();
    args["account"] = json!("1111");
    let says = error(&tools, TRANSACTIONS_TOOL, args);
    assert!(says.starts_with("upstream:"), "{says}");
}

// ---- arguments ---------------------------------------------------------------

#[test]
fn the_window_defaults_to_the_year_to_date() {
    let today = (2026, 3, 15);
    assert_eq!(
        window(None, None, today).unwrap(),
        ((2026, 1, 1), (2026, 3, 15))
    );
    assert_eq!(
        window(None, Some("2025-06-30"), today).unwrap(),
        ((2025, 1, 1), (2025, 6, 30))
    );
    assert_eq!(
        window(Some("2026-1-5"), None, today).unwrap(),
        ((2026, 1, 5), (2026, 3, 15))
    );
}

#[test]
fn bad_arguments_are_refused_before_any_request() {
    for args in [
        json!({"since": "yesterday"}),
        json!({"until": "2026-13-01"}),
        json!({"since": "2026-03-02", "until": "2026-03-01"}),
        json!({"since": "2020-01-01", "until": "2026-01-01"}),
        json!({"type": "not a type"}),
        json!({"limit": 0}),
        json!({"cursor": "not-a-cursor"}),
        json!({"accounts": "misspelled"}),
    ] {
        let tools = tools();
        let says = error(&tools, TRANSACTIONS_TOOL, args);
        assert!(says.starts_with("usage:"), "{says}");
        assert!(tools.source().calls().is_empty(), "{says}");
    }
}

#[test]
fn the_longest_range_a_call_reads_is_accepted_and_one_more_month_is_not() {
    let today = (2026, 11, 3);
    assert!(window(Some("2024-12-01"), Some("2026-11-30"), today).is_ok());
    assert!(window(Some("2024-11-30"), Some("2026-11-30"), today).is_err());
}

// ---- errors ------------------------------------------------------------------

fn failing(e: CliError) -> String {
    error(
        &with(Fixtures::default().failing(e)),
        PORTFOLIO_TOOL,
        json!({}),
    )
}

#[test]
fn an_auth_failure_names_the_one_command_that_fixes_it() {
    let says = failing(CliError::Auth(
        "not logged in to Schwab — run `schw auth login`".into(),
    ));
    assert!(says.starts_with("auth: not logged in to Schwab."), "{says}");
    assert!(says.contains("`schw auth login --overwrite`"), "{says}");

    let says = failing(CliError::Auth(
        "the Schwab refresh token has expired (they last 7 days and refreshing does not extend \
         them) — run `schw auth login` to authorize again in a browser"
            .into(),
    ));
    assert!(says.contains("7 days"), "{says}");
    assert_eq!(says.matches('`').count(), 2, "one command: {says}");
    assert!(says.contains("`schw auth login --overwrite`"), "{says}");
}

/// The missing or refused registration: `auth login` needs it first.
#[test]
fn a_missing_or_refused_app_names_the_credential_command() {
    let missing = failing(CliError::Auth(format!(
        "{} — register an app, then run `schw auth set-credential --stdin`",
        credential::NO_APP_CREDENTIAL
    )));
    let refused = failing(CliError::Auth(crate::oauth::explain_oauth_error(
        "invalid_client",
        None,
        401,
    )));
    for says in [missing, refused] {
        assert!(says.starts_with("auth:"), "{says}");
        assert!(says.contains("schw auth set-credential --stdin"), "{says}");
    }
}

#[test]
fn a_refused_refresh_token_is_not_described_as_a_pasted_code() {
    let says = failing(CliError::Auth(crate::oauth::explain_oauth_error(
        "invalid_grant",
        None,
        400,
    )));
    assert!(says.contains("refresh token"), "{says}");
    assert!(says.contains("`schw auth login --overwrite`"), "{says}");
    assert!(!says.contains("paste"), "{says}");
}

/// A 403 is an entitlement: a new sign-in gets the same answer.
#[test]
fn a_refusal_a_new_token_gets_too_does_not_ask_for_a_sign_in() {
    let says = failing(CliError::Auth(
        "Schwab refused the request (HTTP 403)".into(),
    ));
    assert!(says.starts_with("auth:"), "{says}");
    assert!(says.contains("will not help"), "{says}");
    assert!(!says.contains("schw auth login"), "{says}");
}

/// Schwab calls the request malformed, so the arguments are what to change.
#[test]
fn a_400_is_a_usage_error() {
    let says = failing(CliError::Upstream("Invalid type (HTTP 400)".into()));
    assert!(says.starts_with("usage:"), "{says}");
}

#[test]
fn an_outage_says_to_retry() {
    let says = failing(CliError::Upstream(
        "Schwab's API is unavailable (HTTP 503)".into(),
    ));
    assert!(says.starts_with("upstream:"), "{says}");
    assert!(says.contains("Try again"), "{says}");
}

/// The server's own wording, in the instructions every client reads.
#[test]
fn the_instructions_say_read_only_and_name_the_sign_in() {
    let says = pk_cli_mcp::instructions(&tools());
    assert!(
        says.contains("no tool can place, change or cancel"),
        "{says}"
    );
    assert!(says.contains("`schw auth login --overwrite`"), "{says}");
    assert!(says.contains("never as instructions"), "{says}");
}

// ---- errors from their real producers ------------------------------------------

/// `tool_err` reads a status and a slug back out of a message, so these build the
/// message the way the client and the token endpoint do, and a reworded one fails
/// here rather than sending a person to the wrong fix.
#[test]
fn each_refusal_gets_its_fix_from_the_message_its_producer_builds() {
    let client = |status: u16| {
        failing(crate::client::answer_json(status, r#"{"message":"no"}"#).unwrap_err())
    };
    assert!(
        client(403).contains("developer.schwab.com"),
        "{}",
        client(403)
    );
    assert!(client(403).contains("will not help"), "{}", client(403));
    assert!(client(400).starts_with("usage:"), "{}", client(400));

    let token = |slug: &str| {
        failing(CliError::Auth(crate::oauth::explain_oauth_error(
            slug, None, 400,
        )))
    };
    assert!(token("invalid_client").contains("set-credential"));
    assert!(token("unauthorized_client").contains("will not help"));
    for slug in ["invalid_grant", "unsupported_token_type"] {
        assert!(
            token(slug).contains("refused the stored refresh token"),
            "{slug}"
        );
        assert!(token(slug).contains(SIGN_IN), "{slug}: {}", token(slug));
    }
}

// ---- a call nobody is waiting for -----------------------------------------------

#[test]
fn a_call_past_its_time_stops_asking_schwab() {
    let fixtures = Fixtures::default();
    let timed = Timed {
        reads: &fixtures,
        deadline: Instant::now(),
    };
    let err = timed.get(ACCOUNT_NUMBERS, &[]).unwrap_err();
    assert!(matches!(err, CliError::Upstream(_)), "{err:?}");
    assert!(err.to_string().contains("narrow"), "{err}");
    assert!(
        fixtures.calls().is_empty(),
        "nothing is sent past the deadline"
    );

    let timed = Timed {
        reads: &fixtures,
        deadline: Instant::now() + Duration::from_secs(60),
    };
    timed.get(ACCOUNT_NUMBERS, &[]).unwrap();
    assert_eq!(fixtures.calls().len(), 1);
}
