//! `schw transactions list` — the realized-gains estimation feed.
//!
//! This is the command the tax work is actually for. Within a tax year it
//! answers "what did I sell, when, how many, at what price" — the raw material
//! for a Form 8949 line.
//!
//! # Two things it is not
//!
//! **It is not a realized gain/loss report.** Schwab's API publishes no cost
//! basis, no proceeds, no realized gain, no acquisition date, and no wash-sale
//! adjustment. `transferItems[].cost` is the cash value of *this* trade's leg,
//! not an acquisition basis. Realized figures would have to be reconstructed by
//! lot-matching, and such a reconstruction cannot reproduce Schwab's own
//! lot-relief method, so it would not reconcile to the 1099-B. Schwab's own
//! Realized Gain/Loss report exists only in the web portal. See docs/api.md.
//!
//! **It is not a complete history.** Schwab limits how far back transactions
//! can be retrieved. The exact ceiling is not officially documented; live runs
//! (2026-08-12) retrieved data 121 days old and had 13-month-old requests
//! answered without error, so the community's "60 days" reading is wrong, but
//! how far short of forever the ceiling sits is still unknown (docs/api.md). A
//! request for older data does not error; it just returns less. That is
//! precisely the failure this module is built to make visible rather than
//! silent.
//!
//! # Why the range is chunked, then bisected
//!
//! Two separate silent-truncation mechanisms are layered here:
//!
//! 1. **Calendar-month windows.** Schwab's documentation is ambiguous about
//!    the maximum span of a single transactions query — community clients
//!    variously report 60 days and one year. Rather than bet a tax feed on the
//!    more generous reading, the range is split into month windows (≤31 days),
//!    which sit inside either interpretation.
//! 2. **The 3,000-row response cap.** Live-verified 2026-08-12: a single
//!    response carries at most [`RESPONSE_CAP`] rows, *keeping the newest and
//!    silently dropping the oldest* — a full-month window over an active
//!    account came back with its early days missing and one day partially cut,
//!    with no error and no marker. So any window that comes back at the cap is
//!    treated as truncated and bisected in time, recursively, until every
//!    sub-window is under the cap.
//!
//! Being wrong in this direction costs a few extra HTTP requests; being wrong
//! in the other direction returns a short list that looks exactly like a
//! complete one.

use clap::Subcommand;
use pk_cli_core::dates::{self, Civil};
use pk_cli_core::{CliError, Paged, RangeArgs};
use serde_json::Value;

use super::{account_transactions, Ctx};
use crate::parse;

/// Schwab's `types` values. Sent as-is; the CLI does not validate against this
/// list, because prior work found live responses carrying enum values absent
/// from the published schema and a client that rejects the unknown is a client
/// that breaks on Schwab's next release.
pub const KNOWN_TYPES: &[&str] = &[
    "TRADE",
    "RECEIVE_AND_DELIVER",
    "DIVIDEND_OR_INTEREST",
    "ACH_RECEIPT",
    "ACH_DISBURSEMENT",
    "CASH_RECEIPT",
    "CASH_DISBURSEMENT",
    "ELECTRONIC_FUND",
    "WIRE_OUT",
    "WIRE_IN",
    "JOURNAL",
    "MEMORANDUM",
    "MARGIN_CALL",
    "MONEY_MARKET",
    "SMA_ADJUSTMENT",
];

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List transactions, newest first (brokerage-transaction-list/v1).
    #[command(visible_alias = "ls")]
    List(ListArgs),
}

#[derive(clap::Args, Debug)]
pub struct ListArgs {
    /// Account hash (from `schw accounts list`). Defaults to the account set
    /// with `schw config set account <HASH>`.
    #[arg(long, value_name = "HASH")]
    pub account: Option<String>,

    /// Transaction type to fetch. Repeat for several. Defaults to `TRADE` —
    /// the trades that realize gains and losses.
    #[arg(long = "type", value_name = "TYPE")]
    pub types: Vec<String>,

    /// Fetch every transaction type by omitting the `types` filter entirely.
    /// Live-verified 2026-08-12: Schwab accepts the omission and answers with
    /// every type, despite its published schema marking `types` required.
    #[arg(long, conflicts_with = "types")]
    pub all_types: bool,

    #[command(flatten)]
    pub range: RangeArgs,
}

/// The default type filter: the realized-gains feed.
const DEFAULT_TYPE: &str = "TRADE";

pub fn run(ctx: &Ctx, cmd: &Cmd) -> Result<(), CliError> {
    let Cmd::List(args) = cmd;
    // Everything that can fail on the user's input fails here, before the
    // keychain is opened or a token is spent.
    args.range.validate()?;
    let (since, until) = resolve_range(&args.range)?;
    let account = ctx.account(args.account.as_deref())?;

    if !ctx.common.quiet {
        warn_if_beyond_history(since, dates::today());
    }

    let types = if args.all_types {
        None
    } else if args.types.is_empty() {
        Some(DEFAULT_TYPE.to_string())
    } else {
        Some(args.types.join(","))
    };

    let client = ctx.client()?;
    let path = account_transactions(&account);
    let mut fetch = |start: &str, end: &str| -> Result<Vec<Value>, CliError> {
        let mut query: Vec<(&str, String)> = vec![
            ("startDate", start.to_string()),
            ("endDate", end.to_string()),
        ];
        if let Some(t) = &types {
            query.push(("types", t.clone()));
        }
        match client.get(&path, &query)? {
            Value::Array(page) => Ok(page),
            // A window with nothing in it answers with an empty array; anything
            // else is a shape change worth failing loudly on rather than
            // silently treating as "no transactions".
            other => Err(CliError::Upstream(format!(
                "expected a JSON array of transactions, got {} — the response shape has \
                 changed; see docs/api.md",
                kind_of(&other)
            ))),
        }
    };
    let mut rows: Vec<Value> = Vec::new();
    for window in month_windows(since, until) {
        rows.extend(fetch_window(&mut fetch, window, ctx.common.quiet)?);
    }
    dedupe_by_activity_id(&mut rows);

    let mut txns = parse::transactions(&rows);
    // Newest first, matching every other list in the family.
    txns.sort_by(|a, b| b.date.cmp(&a.date));
    if let Some(n) = args.range.limit {
        txns.truncate(n as usize);
    }

    Paged::new("brokerage-transaction", txns).emit(ctx.common.json);
    Ok(())
}

fn kind_of(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Resolve the range flags into concrete dates.
///
/// The defaults are the tax use case: **year to date**. `--since` defaults to
/// 1 January of the current year and `--until` to today, so a bare
/// `schw transactions list` answers "what have I realized this year".
fn resolve_range(range: &RangeArgs) -> Result<(Civil, Civil), CliError> {
    let today = dates::today();
    let until = match &range.until {
        Some(u) => dates::parse_iso(u)?,
        None => today,
    };
    let since = match &range.since {
        Some(s) => dates::parse_iso(s)?,
        None => (until.0, 1, 1),
    };
    Ok((since, until))
}

/// Schwab's retrievable history, in whole months, used only to decide whether
/// to warn. Deliberately generous: the point is to flag a request that is
/// *certainly* beyond reach, not to argue about the boundary.
const HISTORY_MONTHS: i64 = 12;

/// Warn when the requested start is beyond what Schwab will return.
///
/// Schwab does not error on an over-long lookback — it returns fewer rows. For
/// a tax feed that is the worst possible behaviour, so the CLI says out loud
/// that the answer may be short. On stderr, so it never pollutes `--json`.
fn warn_if_beyond_history(since: Civil, today: Civil) {
    let months = (today.0 - since.0) * 12 + (today.1 as i64 - since.1 as i64);
    if months > HISTORY_MONTHS {
        eprintln!(
            "warning: --since {} is more than {HISTORY_MONTHS} months ago. Schwab's \
             transactions endpoint may not return history that old, and it reports no \
             error when history runs out — it just returns nothing older — so this list \
             may be incomplete. For older data use the Realized Gain/Loss CSV export on \
             schwab.com (see docs/api.md).",
            dates::fmt_iso(since)
        );
    }
}

/// The most rows Schwab puts in one transactions response, silently.
///
/// Live-verified 2026-08-12: windows over an active account came back with
/// exactly 3,000 rows, holding the **newest** 3,000 of the window and dropping
/// the rest — one full-month response was missing its early days entirely and
/// had the oldest surviving day partially cut, mid-day. No error, no header,
/// no marker distinguishes a capped response from a complete one, so a
/// response *at* the cap must be treated as truncated.
pub const RESPONSE_CAP: usize = 3000;

/// A fetch window inside a single calendar month, at second precision.
///
/// Seconds are offsets from midnight UTC on the first of the month, which
/// makes bisection plain integer arithmetic — no cross-month day math. Windows
/// never span a month boundary; `month_windows` guarantees that.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    y: i64,
    m: u32,
    /// Inclusive start, seconds since the month began.
    start_s: i64,
    /// Inclusive end, seconds since the month began.
    end_s: i64,
}

impl Window {
    fn days(y: i64, m: u32, start_day: u32, end_day: u32) -> Self {
        Self {
            y,
            m,
            start_s: (i64::from(start_day) - 1) * 86_400,
            end_s: (i64::from(end_day) - 1) * 86_400 + 86_399,
        }
    }

    fn instant(&self, s: i64, millis: &str) -> String {
        let (day, rem) = (s / 86_400 + 1, s % 86_400);
        format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{millis}Z",
            self.y,
            self.m,
            day,
            rem / 3600,
            rem % 3600 / 60,
            rem % 60
        )
    }

    /// `startDate` value: the window's first second, at `.000`.
    pub fn start(&self) -> String {
        self.instant(self.start_s, "000")
    }

    /// `endDate` value: the window's last second, at `.999`.
    ///
    /// `.999` rather than `.000` for two reasons. First, the zero-width trap:
    /// a window whose start and end are the same instant returns nothing, and
    /// bisection produces one-second windows whose start and end *seconds*
    /// coincide. Second, the boundary hole: Schwab's date grammar carries
    /// milliseconds, so ending one window at `X.000` and starting the next at
    /// `X+1s.000` would leave `(X.000, X+1s.000)` claimed by neither — a trade
    /// stamped inside it would be silently owned by no window at all.
    pub fn end(&self) -> String {
        self.instant(self.end_s, "999")
    }

    /// Split into two non-overlapping, gapless halves, or `None` when the
    /// window is already a single second and cannot shrink further.
    fn halves(&self) -> Option<(Window, Window)> {
        if self.end_s <= self.start_s {
            return None;
        }
        let mid = self.start_s + (self.end_s - self.start_s) / 2;
        Some((
            Window {
                end_s: mid,
                ..*self
            },
            Window {
                start_s: mid + 1,
                ..*self
            },
        ))
    }
}

/// Fetch one window completely: request it, and if the response comes back at
/// [`RESPONSE_CAP`] — which means "truncated", not "exactly that many" —
/// bisect and recurse until every sub-window is under the cap.
///
/// The fetch is injected so this logic is testable offline against a fake
/// server that truncates the way the real one does.
pub fn fetch_window(
    fetch: &mut impl FnMut(&str, &str) -> Result<Vec<Value>, CliError>,
    window: Window,
    quiet: bool,
) -> Result<Vec<Value>, CliError> {
    let rows = fetch(&window.start(), &window.end())?;
    if rows.len() < RESPONSE_CAP {
        return Ok(rows);
    }
    let Some((older, newer)) = window.halves() else {
        // A single second holding 3,000+ transactions. Nothing narrower can be
        // asked for, so say plainly that rows are missing rather than letting
        // the capped answer impersonate a complete one.
        if !quiet {
            eprintln!(
                "warning: {} .. {} returned {} rows — Schwab's silent per-response cap — and \
                 a one-second window cannot be subdivided further. Rows have been dropped and \
                 this list is incomplete.",
                window.start(),
                window.end(),
                rows.len()
            );
        }
        return Ok(rows);
    };
    let mut out = fetch_window(fetch, older, quiet)?;
    out.extend(fetch_window(fetch, newer, quiet)?);
    Ok(out)
}

/// Drop rows whose `activityId` has already been seen, keeping first
/// occurrence and keeping every row that carries no id at all.
///
/// The windows are constructed disjoint, so in principle this removes nothing;
/// it is insurance against the server treating a boundary inclusively on both
/// sides. Double-counting a trade overstates a tax feed exactly as silently as
/// truncation understates it.
fn dedupe_by_activity_id(rows: &mut Vec<Value>) {
    let mut seen = std::collections::HashSet::new();
    rows.retain(|row| match activity_id(row) {
        Some(id) => seen.insert(id),
        None => true,
    });
}

/// Schwab sends id fields as both numbers and strings (`parse::first_string`
/// exists for the same reason); accept either here, or the dedupe silently
/// stops guarding the day the type flips.
fn activity_id(row: &Value) -> Option<String> {
    let v = row.get("activityId")?;
    v.as_i64()
        .map(|n| n.to_string())
        .or_else(|| v.as_str().map(String::from))
}

/// Split an inclusive date range into calendar-month windows.
///
/// Months are the outer chunking unit: at most 31 days, which sits inside
/// both community readings (60 days / one year) of Schwab's undocumented
/// maximum query span. The 3,000-row cap is handled separately, per window,
/// by [`fetch_window`].
pub fn month_windows(since: Civil, until: Civil) -> Vec<Window> {
    if (since.0, since.1, since.2) > (until.0, until.1, until.2) {
        return Vec::new();
    }
    let mut out = Vec::new();
    let (mut y, mut m) = (since.0, since.1);
    loop {
        let is_first = (y, m) == (since.0, since.1);
        let is_last = (y, m) == (until.0, until.1);
        let start_day = if is_first { since.2 } else { 1 };
        let end_day = if is_last {
            until.2
        } else {
            days_in_month(y, m)
        };
        out.push(Window::days(y, m, start_day, end_day));
        if is_last {
            break;
        }
        (y, m) = next_month(y, m);
        // Guard against a nonsense range producing an unbounded walk.
        if y > until.0 + 1 {
            break;
        }
    }
    out
}

fn next_month(y: i64, m: u32) -> (i64, u32) {
    if m == 12 {
        (y + 1, 1)
    } else {
        (y, m + 1)
    }
}

fn days_in_month(y: i64, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if is_leap(y) => 29,
        // February, and anything nonsensical: the short month is the safe
        // guess, since an over-long window is the failure that loses data.
        _ => 28,
    }
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_month_is_one_window_bounded_by_the_requested_days() {
        let w = month_windows((2026, 3, 5), (2026, 3, 20));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].start(), "2026-03-05T00:00:00.000Z");
        assert_eq!(w[0].end(), "2026-03-20T23:59:59.999Z");
    }

    /// The trap: a zero-width window returns nothing at all, so a single-day
    /// query must still span a whole day.
    #[test]
    fn a_single_day_query_spans_that_whole_day() {
        let w = month_windows((2026, 3, 5), (2026, 3, 5));
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].start(), "2026-03-05T00:00:00.000Z");
        assert_eq!(w[0].end(), "2026-03-05T23:59:59.999Z");
        assert_ne!(w[0].start(), w[0].end());
    }

    #[test]
    fn a_range_is_split_at_month_boundaries_with_no_gaps_and_no_overlap() {
        let w = month_windows((2026, 1, 15), (2026, 4, 10));
        assert_eq!(w.len(), 4);
        assert_eq!(w[0].start(), "2026-01-15T00:00:00.000Z");
        assert_eq!(w[0].end(), "2026-01-31T23:59:59.999Z");
        assert_eq!(w[1].start(), "2026-02-01T00:00:00.000Z");
        assert_eq!(w[2].start(), "2026-03-01T00:00:00.000Z");
        assert_eq!(w[3].start(), "2026-04-01T00:00:00.000Z");
        assert_eq!(w[3].end(), "2026-04-10T23:59:59.999Z");
        // Every day of the range is covered exactly once: each window starts
        // the day after the previous one ended.
        for pair in w.windows(2) {
            let prev_end = pair[0].end();
            let next_start = pair[1].start();
            assert!(
                prev_end[..10] < next_start[..10],
                "{prev_end} .. {next_start}"
            );
        }
    }

    #[test]
    fn february_length_follows_the_leap_rule() {
        let w = month_windows((2024, 2, 1), (2024, 3, 1));
        assert_eq!(w[0].end(), "2024-02-29T23:59:59.999Z");
        let w = month_windows((2026, 2, 1), (2026, 3, 1));
        assert_eq!(w[0].end(), "2026-02-28T23:59:59.999Z");
        // Centuries: 1900 is not a leap year, 2000 is.
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
    }

    #[test]
    fn a_range_crossing_a_year_boundary_keeps_going() {
        let w = month_windows((2025, 11, 1), (2026, 2, 15));
        assert_eq!(w.len(), 4);
        assert_eq!(w[1].start(), "2025-12-01T00:00:00.000Z");
        assert_eq!(w[2].start(), "2026-01-01T00:00:00.000Z");
        assert_eq!(w[3].end(), "2026-02-15T23:59:59.999Z");
    }

    /// No window may exceed 31 days, which is what keeps the walk inside both
    /// the 60-day and the one-year reading of Schwab's undocumented cap.
    #[test]
    fn no_window_is_longer_than_a_calendar_month() {
        for w in month_windows((2024, 1, 1), (2026, 12, 31)) {
            assert_eq!(
                &w.start()[..7],
                &w.end()[..7],
                "a window spanned two months"
            );
        }
    }

    // ------------------------------------------------ the 3,000-row cap ----

    /// A fake of the truncation observed live: hold a set of timestamped rows,
    /// answer each request with the rows inside the window — and when there
    /// are more than the cap, keep the **newest** and drop the oldest, exactly
    /// as Schwab does.
    fn truncating_server(
        times: Vec<i64>,
    ) -> impl FnMut(&str, &str) -> Result<Vec<Value>, CliError> {
        move |start: &str, end: &str| {
            let mut hits: Vec<i64> = times
                .iter()
                .copied()
                .filter(|t| {
                    let s = parse_secs(start);
                    let e = parse_secs(end);
                    *t >= s && *t <= e
                })
                .collect();
            hits.sort_unstable();
            if hits.len() > RESPONSE_CAP {
                hits.drain(..hits.len() - RESPONSE_CAP);
            }
            Ok(hits
                .into_iter()
                .map(|t| serde_json::json!({ "activityId": t, "type": "TRADE" }))
                .collect())
        }
    }

    /// Seconds-in-month of an instant string, millisecond-aware: `.999` counts
    /// as the same second, matching an inclusive server comparison.
    fn parse_secs(instant: &str) -> i64 {
        let day: i64 = instant[8..10].parse().unwrap();
        let h: i64 = instant[11..13].parse().unwrap();
        let mi: i64 = instant[14..16].parse().unwrap();
        let s: i64 = instant[17..19].parse().unwrap();
        (day - 1) * 86_400 + h * 3600 + mi * 60 + s
    }

    #[test]
    fn a_capped_window_is_bisected_until_every_row_is_recovered() {
        // 7,500 trades over three days: any single request over the whole
        // window is truncated, and the early rows are the ones dropped.
        let times: Vec<i64> = (0..7_500).map(|i| i * 30).collect();
        let mut fetch = truncating_server(times.clone());
        let w = month_windows((2026, 7, 1), (2026, 7, 3))[0];
        let rows = fetch_window(&mut fetch, w, true).unwrap();
        let mut got: Vec<i64> = rows
            .iter()
            .map(|r| r.get("activityId").unwrap().as_i64().unwrap())
            .collect();
        got.sort_unstable();
        assert_eq!(got, times, "every row must be recovered exactly once");
    }

    #[test]
    fn an_uncapped_window_is_fetched_in_one_request() {
        let mut calls = 0usize;
        let mut fetch = |_: &str, _: &str| {
            calls += 1;
            Ok(vec![serde_json::json!({"activityId": 1, "type": "TRADE"})])
        };
        let w = month_windows((2026, 7, 1), (2026, 7, 31))[0];
        let rows = fetch_window(&mut fetch, w, true).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(calls, 1, "an under-cap response must not trigger a split");
    }

    /// The pathological floor: a single second holding a full cap of rows
    /// cannot be subdivided, and must still return what it got rather than
    /// recurse forever or drop everything.
    #[test]
    fn a_capped_single_second_returns_its_rows_rather_than_recursing() {
        let times: Vec<i64> = vec![86_400; RESPONSE_CAP + 500];
        let mut fetch = truncating_server(times);
        let w = Window {
            y: 2026,
            m: 7,
            start_s: 86_400,
            end_s: 86_400,
        };
        let rows = fetch_window(&mut fetch, w, true).unwrap();
        assert_eq!(rows.len(), RESPONSE_CAP);
    }

    #[test]
    fn halves_are_gapless_and_disjoint() {
        let w = Window {
            y: 2026,
            m: 7,
            start_s: 0,
            end_s: 86_399,
        };
        let (a, b) = w.halves().unwrap();
        assert_eq!(a.start_s, w.start_s);
        assert_eq!(b.end_s, w.end_s);
        assert_eq!(a.end_s + 1, b.start_s, "no gap, no overlap");
        // The end instant of the older half and the start instant of the newer
        // half straddle the boundary at millisecond precision.
        assert!(a.end() < b.start());
    }

    #[test]
    fn a_one_second_window_does_not_split() {
        let w = Window {
            y: 2026,
            m: 7,
            start_s: 5,
            end_s: 5,
        };
        assert!(w.halves().is_none());
        assert_eq!(w.start(), "2026-07-01T00:00:05.000Z");
        assert_eq!(w.end(), "2026-07-01T00:00:05.999Z");
    }

    #[test]
    fn duplicate_activity_ids_are_dropped_and_idless_rows_are_kept() {
        let mut rows = vec![
            serde_json::json!({"activityId": 1}),
            serde_json::json!({"activityId": 2}),
            serde_json::json!({"activityId": 1}),
            serde_json::json!({"type": "TRADE"}),
            serde_json::json!({"type": "TRADE"}),
        ];
        dedupe_by_activity_id(&mut rows);
        assert_eq!(rows.len(), 4, "one duplicate gone, idless rows kept");
    }

    /// Schwab sends id fields as either numbers or strings; a string-typed
    /// `activityId` must still deduplicate — including against its numeric
    /// twin — rather than silently falling into the "no id" arm.
    #[test]
    fn string_typed_activity_ids_still_deduplicate() {
        let mut rows = vec![
            serde_json::json!({"activityId": "7"}),
            serde_json::json!({"activityId": "7"}),
            serde_json::json!({"activityId": 7}),
            serde_json::json!({"activityId": "8"}),
        ];
        dedupe_by_activity_id(&mut rows);
        assert_eq!(rows.len(), 2, "string and numeric spellings are one id");
    }

    #[test]
    fn an_inverted_range_yields_no_windows_rather_than_looping() {
        assert!(month_windows((2026, 5, 1), (2026, 1, 1)).is_empty());
        assert!(month_windows((2026, 1, 2), (2026, 1, 1)).is_empty());
    }

    #[test]
    fn a_long_range_is_covered_completely() {
        let w = month_windows((2020, 1, 1), (2026, 12, 31));
        assert_eq!(w.len(), 7 * 12);
        assert_eq!(w[0].start(), "2020-01-01T00:00:00.000Z");
        assert_eq!(w.last().unwrap().end(), "2026-12-31T23:59:59.999Z");
    }

    #[test]
    fn the_default_range_is_year_to_date() {
        let (since, until) = resolve_range(&RangeArgs::default()).unwrap();
        let today = dates::today();
        assert_eq!(since, (today.0, 1, 1));
        assert_eq!(until, today);
    }

    #[test]
    fn an_explicit_since_defaults_until_to_today() {
        let range = RangeArgs {
            since: Some("2025-06-01".into()),
            ..Default::default()
        };
        let (since, until) = resolve_range(&range).unwrap();
        assert_eq!(since, (2025, 6, 1));
        assert_eq!(until, dates::today());
    }

    /// With only `--until`, the year defaults to *that* date's year, so
    /// `--until 2024-12-31` means calendar 2024 rather than a range starting in
    /// the current year and ending in the past.
    #[test]
    fn an_explicit_until_anchors_the_default_since_to_its_own_year() {
        let range = RangeArgs {
            until: Some("2024-12-31".into()),
            ..Default::default()
        };
        let (since, until) = resolve_range(&range).unwrap();
        assert_eq!(since, (2024, 1, 1));
        assert_eq!(until, (2024, 12, 31));
        assert!(!month_windows(since, until).is_empty());
    }

    #[test]
    fn a_malformed_date_is_a_usage_error() {
        let range = RangeArgs {
            since: Some("not-a-date".into()),
            ..Default::default()
        };
        assert!(matches!(resolve_range(&range), Err(CliError::Usage(_))));
    }

    #[test]
    fn the_known_type_list_holds_the_documented_enum() {
        assert_eq!(KNOWN_TYPES.len(), 15);
        assert!(KNOWN_TYPES.contains(&DEFAULT_TYPE));
    }
}
