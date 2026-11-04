//! `schw mcp`: Schwab balances, positions and transactions as MCP tools
//! (cli-common DESIGN.md §1.9, the MCP profile), for Claude Code, Claude Desktop
//! and any MCP client.
//!
//! Two read-only tools over the reads and parsers the commands use:
//! `schwab_portfolio` and `schwab_transactions`.
//!
//! # Nothing here can place an order
//!
//! Schwab's order endpoints share a host and a bearer token with these reads,
//! so the guarantee is structural, in three layers the tests hold:
//!
//! - [`Reads`], the one way a tool reaches Schwab, has `get` and nothing else,
//!   and [`Schwab`] has no other verb to give it.
//! - Every path a tool requests is built from `commands`' constants and a hash
//!   from Schwab's own account list. An `account` argument only selects from that
//!   list and never reaches a path, so a model cannot ask for
//!   `<hash>/orders` even as a read.
//! - No definition mentions the order surface, and `--allow-writes` adds nothing,
//!   because every tool is read-only.
//!
//! # A long-running server and the token
//!
//! [`LiveSource::open`] builds a new [`Schwab`] for each tool call, reading the
//! config and both keychain items afresh, so a sign-in or a renewal by any other
//! `schw` process applies on the next call and no token is held between calls. A
//! renewal goes through `client.rs`'s `RefreshLock`, shared with every other
//! process over the keychain item. A tool never signs in: a missing or dead
//! session is an auth error naming the command to run. Listing the tools touches
//! no credential and no file.

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use pk_cli_config::ConfigStore;
use pk_cli_core::dates::{self, Civil};
use pk_cli_core::CliError;
use pk_cli_mcp::{
    hold_within, parse_args, ListArgs, Order, Page, PageRequest, PositionKey, PositionRequest,
    Server, ToolDef, ToolError, ToolOutput, Toolset,
};
use pk_cli_secrets::CredentialStore;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::client::{has_status, Schwab};
use crate::commands::transactions::{
    beyond_history, dedupe_by_activity_id, fetch_window_noting, month_windows, query, rows_of,
    HISTORY_MONTHS,
};
use crate::commands::{account, account_transactions, ACCOUNTS, ACCOUNT_NUMBERS};
use crate::config::Config;
use crate::oauth::is_oauth_error;
use crate::parse::{self, Position, Transaction, TransferItem};
use crate::{credential, BIN};

pub const PORTFOLIO_TOOL: &str = "schwab_portfolio";
pub const TRANSACTIONS_TOOL: &str = "schwab_transactions";

/// What a person's next step is when the session is missing or lapsed. It takes
/// `--overwrite` because a lapsed session is still stored, and `auth login`
/// refuses to replace one without it.
const SIGN_IN: &str = "schw auth login --overwrite";

/// The fix when the app registration is what is missing or refused: a new
/// browser sign-in cannot help, and `auth login` needs the registration first.
const STORE_THE_APP: &str = "Ask the user to store the key and secret from their app on \
                             developer.schwab.com with `schw auth set-credential --stdin \
                             --overwrite`, then run `schw auth login --overwrite`, both in a \
                             terminal, then call the tool again. Tools never sign in or create \
                             credentials themselves";

/// The fix for a refusal a new token gets too.
const CHECK_THE_APP: &str = "A new sign-in will not help. Ask the user to check on \
                             developer.schwab.com that their app has the \"Accounts and Trading \
                             Production\" product and is \"Ready For Use\", then call the tool \
                             again";

/// A call reads at most this many calendar months, one request each per account.
const MAX_MONTHS: i64 = 24;

/// A call sends at most this many transaction requests: Schwab's reported
/// sustained budget of 120 a minute. Past it the call is refused rather than cut
/// short, and [`CALL_BUDGET`] stops it sooner if Schwab is slow.
const MAX_REQUESTS: usize = 120;

/// How long one call may keep asking Schwab, its wait for another call included:
/// just under the two minutes `--http` gives a call before it answers "try again"
/// and stops listening. The work is not cancelled then, so this is what ends it.
/// A request already sent still finishes, within the client's own timeout.
const CALL_BUDGET: Duration = Duration::from_secs(110);

/// How long a call waits for the one before it. Parallel calls from one model turn
/// are normal and short; a call still running past this is a long walk, and the
/// model is better told than left to retry into a queue.
const QUEUE_WAIT: Duration = Duration::from_secs(45);

/// One tool call's way to Schwab: GET, and nothing else.
pub trait Reads {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, CliError>;
}

/// Where the tools get Schwab's data: the live API, or fixtures. One [`Reads`] per
/// tool call, which is what a call's several requests share.
pub trait Source {
    fn open(&self) -> Result<Box<dyn Reads + '_>, CliError>;
}

/// Stops a call's requests once its time is up, so a call nobody is waiting for
/// ends, rather than walking on and spending the app's request budget.
struct Timed<R> {
    reads: R,
    deadline: Instant,
}

impl<R: Reads> Reads for Timed<R> {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, CliError> {
        if Instant::now() >= self.deadline {
            return Err(CliError::Upstream(format!(
                "this call stopped after {} seconds without finishing; narrow since/until or \
                 name an account",
                CALL_BUDGET.as_secs()
            )));
        }
        self.reads.get(path, query)
    }
}

/// The live API, through the client the commands use.
pub struct LiveSource {
    creds: CredentialStore,
    one_at_a_time: Mutex<()>,
}

impl LiveSource {
    pub fn new(creds: CredentialStore) -> Self {
        Self {
            creds,
            one_at_a_time: Mutex::new(()),
        }
    }
}

impl Source for LiveSource {
    /// One call at a time, so parallel tool calls keep the client's request
    /// pacing. The config comes first: a malformed one reads no keychain item.
    fn open(&self) -> Result<Box<dyn Reads + '_>, CliError> {
        let deadline = Instant::now() + CALL_BUDGET;
        let one = hold_within(&self.one_at_a_time, QUEUE_WAIT).ok_or_else(|| {
            CliError::Upstream(
                "another call to Schwab is still running in this server; try again in a minute"
                    .into(),
            )
        })?;
        let cfg: Config = ConfigStore::new(BIN).load()?;
        Ok(Box::new(LiveReads {
            _one: one,
            client: Timed {
                reads: Schwab::new(&cfg, &self.creds, false)?,
                deadline,
            },
        }))
    }
}

struct LiveReads<'a> {
    _one: MutexGuard<'a, ()>,
    client: Timed<Schwab<'a>>,
}

impl Reads for LiveReads<'_> {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, CliError> {
        self.client.get(path, query)
    }
}

impl Reads for Schwab<'_> {
    fn get(&self, path: &str, query: &[(&str, String)]) -> Result<Value, CliError> {
        Schwab::get(self, path, query)
    }
}

/// The toolset `schw mcp` serves.
pub struct SchwabTools<S: Source> {
    source: S,
}

impl<S: Source> SchwabTools<S> {
    pub fn new(source: S) -> Self {
        Self { source }
    }

    #[cfg(test)]
    pub fn source(&self) -> &S {
        &self.source
    }
}

/// A provider failure as a tool error, with the fix that one has: the family's
/// standard remedy for an auth failure is a new sign-in, which is wrong for an app
/// registration that is missing, refused or not entitled.
fn tool_err(e: CliError) -> ToolError {
    match e {
        CliError::Auth(m) if m.starts_with(credential::NO_APP_CREDENTIAL) => {
            ToolError::auth(credential::NO_APP_CREDENTIAL).with_fix(STORE_THE_APP)
        }
        CliError::Auth(m) if is_oauth_error(&m, "invalid_client") => {
            ToolError::auth("Schwab refused the stored app key and secret (invalid_client)")
                .with_fix(STORE_THE_APP)
        }
        CliError::Auth(m) if is_oauth_error(&m, "unauthorized_client") || has_status(&m, 403) => {
            ToolError::auth(m).with_fix(CHECK_THE_APP)
        }
        // The token endpoint's wording is about a code pasted at sign-in; what a
        // tool presented was the stored refresh token.
        CliError::Auth(m)
            if is_oauth_error(&m, "invalid_grant")
                || is_oauth_error(&m, "unsupported_token_type") =>
        {
            ToolError::auth("Schwab refused the stored refresh token")
        }
        // Schwab calls the request malformed: a type it does not know, or a date
        // it will not take. The arguments are what to change.
        CliError::Upstream(m) if has_status(&m, 400) => ToolError::usage(m),
        other => other.into(),
    }
}

// ---- schwab_portfolio -------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PortfolioArgs {
    /// Only this account: its last four digits, or its type (CASH, MARGIN).
    #[serde(default)]
    pub account: Option<String>,
    /// Only this symbol, and options on it.
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(flatten)]
    pub list: ListArgs,
}

#[derive(Serialize)]
struct PortfolioFilter<'a> {
    account: Option<&'a str>,
    symbol: Option<&'a str>,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct AccountTotal {
    /// The account's type and the last four digits of its number.
    pub name: String,
    /// CASH or MARGIN, as Schwab says.
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Absent where Schwab did not state it, which is not zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub equity: Option<String>,
    /// Absent where Schwab did not state it, which is not zero.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cash: Option<String>,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct PositionItem {
    pub symbol: String,
    pub account: String,
    /// Negative for a short position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub market_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unrealized_gain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_price: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day_gain: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PortfolioOut {
    pub accounts: Vec<AccountTotal>,
    #[serde(flatten)]
    pub page: Page<PositionItem>,
}

// ---- schwab_transactions ----------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TransactionsArgs {
    /// First day, YYYY-MM-DD. Default: Jan 1 of until's year.
    #[serde(default)]
    pub since: Option<String>,
    /// Last day, YYYY-MM-DD. Default: today. At most 24 months after since.
    #[serde(default)]
    pub until: Option<String>,
    /// Only this account: its last four digits, or its type (CASH, MARGIN).
    #[serde(default)]
    pub account: Option<String>,
    /// Only this Schwab type: TRADE, DIVIDEND_OR_INTEREST, ACH_RECEIPT,
    /// ACH_DISBURSEMENT, WIRE_IN, WIRE_OUT, JOURNAL and others. Default: all.
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
    /// Only this symbol, and options on it. A row paid in cash, such as a
    /// dividend, names no symbol and is left out.
    #[serde(default)]
    pub symbol: Option<String>,
    #[serde(flatten)]
    pub list: ListArgs,
}

#[derive(Serialize)]
struct TransactionsFilter<'a> {
    since: &'a str,
    until: &'a str,
    account: Option<&'a str>,
    kind: Option<&'a str>,
    symbol: Option<&'a str>,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct Leg {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    /// This leg's cash value in this transaction, not a cost basis.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cost: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fee_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_effect: Option<String>,
}

#[derive(Debug, Default, Serialize, JsonSchema)]
pub struct TransactionItem {
    /// Absent where Schwab's row states none.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    /// Signed cash, after fees.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    /// OPENING or CLOSING.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position_effect: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub account: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settlement_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub activity_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Every leg of the transaction: the security, and each fee.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub legs: Vec<Leg>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct TransactionsOut {
    pub since: String,
    pub until: String,
    /// False when one second held more rows than Schwab returns at once, so rows
    /// are missing and no narrower call can recover them. It covers this call's
    /// read only: each page reads Schwab again.
    pub complete: bool,
    /// Present when since is far enough back that Schwab may return less with no
    /// error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    #[serde(flatten)]
    pub page: Page<TransactionItem>,
}

// ---- the toolset ------------------------------------------------------------

impl<S: Source> Toolset for SchwabTools<S> {
    fn server(&self) -> Server {
        Server::new("schw", env!("CARGO_PKG_VERSION"), "Schwab", "schwab")
            .sign_in_command(SIGN_IN)
            .instructions(
                "Schwab access here is read-only: no tool can place, change or cancel an order, \
                 move money or change anything. Schwab's API has no tax documents, cost basis or \
                 realized gains, so these tools cannot state a realized gain. A Schwab sign-in \
                 lasts 7 days and then needs a person at a browser. Amounts are USD decimal \
                 strings; keep every digit when adding them. A transaction posted between \
                 pages repeats nothing and skips nothing, and is not seen itself until the list \
                 is read again from the start.",
            )
    }

    fn tools(&self) -> Vec<ToolDef> {
        vec![
            ToolDef::read(
                PORTFOLIO_TOOL,
                "Schwab balances and positions",
                "Shows each linked Schwab account's equity and cash, and the positions held. Use \
                 it for what the user owns at Schwab and its value. Returns accounts (name, type, \
                 equity, cash) and position items (symbol, account, quantity, market_value, \
                 unrealized_gain, average_price). Narrow with account or symbol.",
            )
            .input::<PortfolioArgs>()
            .output::<PortfolioOut>(),
            ToolDef::read(
                TRANSACTIONS_TOOL,
                "Schwab transactions",
                "Lists Schwab transactions in a date range, newest first: trades, dividends, \
                 deposits, withdrawals. Use it for what happened in a period or what was sold in \
                 a year. Items carry date, type, symbol, quantity, price, amount, position_effect \
                 (OPENING or CLOSING), description and account, and the result says whether it is \
                 complete. It holds no cost basis. Defaults to the year to date; narrow with \
                 account, type or symbol.",
            )
            .input::<TransactionsArgs>()
            .output::<TransactionsOut>(),
        ]
    }

    async fn call(&self, name: &str, args: Value) -> Result<ToolOutput, ToolError> {
        match name {
            PORTFOLIO_TOOL => self.portfolio(parse_args(args)?),
            TRANSACTIONS_TOOL => self.transactions(parse_args(args)?),
            other => Err(ToolError::other(format!("no handler for {other}"))),
        }
    }
}

impl<S: Source> SchwabTools<S> {
    fn portfolio(&self, args: PortfolioArgs) -> Result<ToolOutput, ToolError> {
        let account_arg = non_empty(args.account.as_deref());
        let symbol = non_empty(args.symbol.as_deref());
        let page = PageRequest::new(
            &args.list,
            &PortfolioFilter {
                account: account_arg,
                symbol,
            },
        )?;
        let detailed = args.list.format().is_detailed();

        let reads = self.source.open().map_err(tool_err)?;
        let all = linked(reads.as_ref())?;
        let scope = pick(&all, account_arg)?;

        let mut matched = Vec::new();
        for acct in &scope {
            // `fields=positions` is what makes the payload include holdings at all.
            let payload = reads
                .get(&account(&acct.hash), &[("fields", "positions".into())])
                .map_err(tool_err)?;
            for position in held(&payload, &acct.label)? {
                if matches_symbol(Some(&position.symbol), symbol) {
                    matched.push(position_item(position, acct, detailed));
                }
            }
        }
        ToolOutput::new(&PortfolioOut {
            accounts: scope.iter().map(|a| a.total()).collect(),
            page: page.page(matched, "account or symbol")?,
        })
    }

    fn transactions(&self, args: TransactionsArgs) -> Result<ToolOutput, ToolError> {
        let today = dates::today();
        let (since, until) = window(
            non_empty(args.since.as_deref()),
            non_empty(args.until.as_deref()),
            today,
        )?;
        let (since_text, until_text) = (dates::fmt_iso(since), dates::fmt_iso(until));
        let account_arg = non_empty(args.account.as_deref());
        let kind = type_arg(non_empty(args.kind.as_deref()))?;
        let symbol = non_empty(args.symbol.as_deref());
        let page = PositionRequest::new(
            &args.list,
            &TransactionsFilter {
                since: &since_text,
                until: &until_text,
                account: account_arg,
                kind: kind.as_deref(),
                symbol,
            },
        )?;
        let detailed = args.list.format().is_detailed();

        let reads = self.source.open().map_err(tool_err)?;
        let all = linked(reads.as_ref())?;

        let mut complete = true;
        let mut requests = 0;
        let mut matched = Vec::new();
        for acct in pick(&all, account_arg)? {
            let path = account_transactions(&acct.hash);
            let mut fetch = |start: &str, end: &str| -> Result<Vec<Value>, CliError> {
                requests += 1;
                if requests > MAX_REQUESTS {
                    return Err(CliError::Usage(format!(
                        "this range needs more than {MAX_REQUESTS} requests to Schwab; narrow \
                         since/until or name an account"
                    )));
                }
                rows_of(reads.get(&path, &query(start, end, kind.as_deref()))?)
            };
            let mut rows = Vec::new();
            for month in month_windows(since, until) {
                rows.extend(
                    fetch_window_noting(&mut fetch, month, &mut |_, _| complete = false)
                        .map_err(tool_err)?,
                );
            }
            dedupe_by_activity_id(&mut rows);
            let parsed = parse::transactions(&rows);
            if !parsed.is_empty() && parsed.iter().all(|t| t.date.is_empty()) {
                return Err(ToolError::upstream(format!(
                    "Schwab returned {} transactions for {} and none states a date; its answer \
                     has changed",
                    parsed.len(),
                    acct.label
                )));
            }
            for t in parsed {
                if matches_symbol(t.symbol.as_deref(), symbol) {
                    let key = PositionKey::new(t.date.as_str(), &t.id);
                    matched.push((key, transaction_item(t, acct, detailed)));
                }
            }
        }
        let note = beyond_history(since, today).then(|| {
            format!(
                "since is more than {HISTORY_MONTHS} months ago. Schwab returns less with no \
                 error when its history runs out, so this list may be short; its Realized \
                 Gain/Loss export on schwab.com goes further back."
            )
        });
        ToolOutput::new(&TransactionsOut {
            since: since_text,
            until: until_text,
            complete,
            note,
            // Newest first across accounts, an undated row last. A row posted between pages
            // repeats nothing and skips nothing.
            page: page.page(
                matched,
                Order::Descending,
                "since/until, account, type or symbol",
            )?,
        })
    }
}

// ---- accounts ---------------------------------------------------------------

/// A linked account as the tools see it. The hash is what a path is built from
/// and never leaves in a result; the plain account number is not held at all.
struct AccountRef {
    hash: String,
    /// `MARGIN …1111`: the one name a model sees.
    label: String,
    kind: Option<String>,
    /// The last four digits.
    tail: Option<String>,
    equity: Option<String>,
    cash: Option<String>,
}

impl AccountRef {
    fn total(&self) -> AccountTotal {
        AccountTotal {
            name: self.label.clone(),
            kind: self.kind.clone(),
            equity: self.equity.clone(),
            cash: self.cash.clone(),
        }
    }
}

/// Every account linked to the app. `accountNumbers` is the authority for which
/// exist, and the only source of the hashes every other path is built from.
fn linked(reads: &dyn Reads) -> Result<Vec<AccountRef>, ToolError> {
    let hashes = list_of(
        reads.get(ACCOUNT_NUMBERS, &[]).map_err(tool_err)?,
        "accounts",
    )?;
    let detail = list_of(reads.get(ACCOUNTS, &[]).map_err(tool_err)?, "balances")?;
    let accounts = parse::accounts(&hashes, &detail);
    if accounts.is_empty() {
        return Err(
            ToolError::not_found("Schwab returned no accounts for this app").with_fix(
                "Ask the user to run `schw auth login --overwrite` in a terminal and select their \
             accounts on Schwab's consent page; an app only just approved may need its \"Ready \
             For Use\" state on developer.schwab.com first",
            ),
        );
    }
    accounts
        .into_iter()
        .map(|a| {
            if !path_safe(&a.id) {
                return Err(ToolError::upstream(
                    "Schwab's account list has a row with no usable hashValue; its answer has \
                     changed",
                ));
            }
            let masked = a.account_number.unwrap_or_default();
            let kind = a.r#type;
            let label = format!("{} {masked}", kind.as_deref().unwrap_or_default());
            let label = match label.trim() {
                "" => "unnamed account".to_string(),
                named => named.to_string(),
            };
            Ok(AccountRef {
                hash: a.id,
                label,
                kind,
                tail: masked
                    .strip_prefix('…')
                    .filter(|t| !t.is_empty())
                    .map(String::from),
                equity: a.equity.map(|m| m.amount),
                cash: a.cash.map(|m| m.amount),
            })
        })
        .collect()
}

/// A hash goes into a URL path, so whatever Schwab sends may not add a segment.
fn path_safe(hash: &str) -> bool {
    !hash.is_empty()
        && hash
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Schwab's list endpoints answer with an array, and have answered with a bare
/// object where one was documented. Anything else is a changed answer, which must
/// not read as "you have no accounts".
fn list_of(answer: Value, what: &str) -> Result<Vec<Value>, ToolError> {
    match answer {
        Value::Array(rows) => Ok(rows),
        Value::Object(_) => Ok(vec![answer]),
        _ => Err(ToolError::upstream(format!(
            "Schwab's answer for {what} is not a list; its answer has changed"
        ))),
    }
}

/// The accounts `wanted` names, or every account when it names none. It is
/// compared with the list and never used itself, so nothing a call passes can reach
/// a path. An unknown name is not found, listing the names that exist.
fn pick<'a>(all: &'a [AccountRef], wanted: Option<&str>) -> Result<Vec<&'a AccountRef>, ToolError> {
    let Some(wanted) = wanted else {
        return Ok(all.iter().collect());
    };
    let tail = wanted.trim_start_matches(|c: char| !c.is_ascii_digit());
    let hit: Vec<&AccountRef> = all
        .iter()
        .filter(|a| {
            a.hash == wanted
                || a.label.eq_ignore_ascii_case(wanted)
                || a.tail.as_deref() == Some(tail)
                || a.kind
                    .as_deref()
                    .is_some_and(|k| k.eq_ignore_ascii_case(wanted))
        })
        .collect();
    if hit.is_empty() {
        let names: Vec<&str> = all.iter().map(|a| a.label.as_str()).collect();
        return Err(ToolError::not_found(format!(
            "no Schwab account matches that; the accounts are: {}",
            names.join(", ")
        ))
        .with_fix("Call again with one of those names, or with its last four digits"));
    }
    Ok(hit)
}

// ---- positions --------------------------------------------------------------

/// One account's positions. Schwab omits the array for an account holding
/// nothing, so a missing array is only an empty account if the balances beside it
/// agree; otherwise the answer changed, and "you hold nothing" would be untrue.
fn held(payload: &Value, label: &str) -> Result<Vec<Position>, ToolError> {
    let changed = |why: &str| {
        ToolError::upstream(format!(
            "Schwab's answer for {label} {why}; its answer has changed"
        ))
    };
    let account = payload
        .get("securitiesAccount")
        .ok_or_else(|| changed("has no securitiesAccount"))?;
    match account.get("positions") {
        Some(Value::Array(rows)) => {
            let positions = parse::positions(payload);
            let unreadable = |p: &Position| p.symbol == "?" && p.market_value.is_none();
            if !rows.is_empty() && positions.iter().all(unreadable) {
                return Err(changed("has positions and none can be read"));
            }
            Ok(positions)
        }
        None | Some(Value::Null) => {
            let balances = account
                .get("currentBalances")
                .ok_or_else(|| changed("states neither positions nor balances"))?;
            if holds_value(balances) {
                return Err(changed("states holdings in its balances and no positions"));
            }
            Ok(Vec::new())
        }
        Some(_) => Err(changed("has positions that are not a list")),
    }
}

/// Whether `currentBalances` says the account holds any security.
fn holds_value(balances: &Value) -> bool {
    [
        "longMarketValue",
        "shortMarketValue",
        "longOptionMarketValue",
        "shortOptionMarketValue",
        "mutualFundValue",
        "bondValue",
    ]
    .iter()
    .any(|k| balances.get(*k).and_then(Value::as_f64).unwrap_or(0.0) != 0.0)
}

fn position_item(p: Position, acct: &AccountRef, detailed: bool) -> PositionItem {
    let money = |m: Option<pk_cli_core::Money>| m.map(|m| m.amount);
    PositionItem {
        symbol: p.symbol,
        account: acct.label.clone(),
        quantity: p.quantity,
        market_value: money(p.market_value),
        unrealized_gain: money(p.open_pl),
        average_price: p.average_price,
        day_gain: money(p.day_pl).filter(|_| detailed),
        asset_type: p.asset_type.filter(|_| detailed),
        description: p.description.filter(|_| detailed),
    }
}

// ---- transactions -----------------------------------------------------------

fn transaction_item(t: Transaction, acct: &AccountRef, detailed: bool) -> TransactionItem {
    TransactionItem {
        date: Some(t.date).filter(|d| !d.is_empty()),
        kind: t.r#type,
        symbol: t.symbol,
        quantity: t.quantity,
        price: t.price,
        amount: t.net_amount.map(|m| m.amount),
        position_effect: t.position_effect,
        description: t.description,
        account: acct.label.clone(),
        settlement_date: t.settlement_date.filter(|_| detailed),
        status: t.status.filter(|_| detailed),
        activity_type: t.activity_type.filter(|_| detailed),
        id: Some(t.id).filter(|i| detailed && !i.is_empty()),
        legs: if detailed {
            t.items.into_iter().map(leg).collect()
        } else {
            Vec::new()
        },
    }
}

fn leg(item: TransferItem) -> Leg {
    Leg {
        symbol: item.symbol,
        asset_type: item.asset_type,
        amount: item.amount,
        price: item.price,
        cost: item.cost,
        fee_type: item.fee_type,
        position_effect: item.position_effect,
    }
}

/// A type goes to Schwab as sent, since live responses carry values its schema
/// does not list; only its shape is checked. Upper case, so a cursor binds to the
/// same query however it was typed.
fn type_arg(kind: Option<&str>) -> Result<Option<String>, ToolError> {
    kind.map(|k| {
        let k = k.to_ascii_uppercase();
        if k.bytes().all(|b| b.is_ascii_uppercase() || b == b'_') {
            Ok(k)
        } else {
            Err(ToolError::usage(format!(
                "type must be one Schwab type such as TRADE or DIVIDEND_OR_INTEREST, got `{k}`"
            )))
        }
    })
    .transpose()
}

/// The symbol itself, or its underlying when it is an option: Schwab writes an
/// option as the root, spaces, then expiry and strike (`AAPL  260320C00180000`).
fn matches_symbol(symbol: Option<&str>, wanted: Option<&str>) -> bool {
    let Some(wanted) = wanted else { return true };
    symbol.is_some_and(|s| {
        s.eq_ignore_ascii_case(wanted)
            || s.split_whitespace()
                .next()
                .is_some_and(|root| root.eq_ignore_ascii_case(wanted))
    })
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

fn date_arg(name: &str, v: Option<&str>) -> Result<Option<Civil>, ToolError> {
    v.map(|d| {
        dates::parse_iso(d).map_err(|_| {
            ToolError::usage(format!("{name} must be a date as YYYY-MM-DD, got `{d}`"))
        })
    })
    .transpose()
}

/// The inclusive window, defaulting to the year to date.
fn window(
    since: Option<&str>,
    until: Option<&str>,
    today: Civil,
) -> Result<(Civil, Civil), ToolError> {
    let since = date_arg("since", since)?;
    let until = date_arg("until", until)?.unwrap_or(today);
    let since = since.unwrap_or((until.0, 1, 1));
    if since > until {
        return Err(ToolError::usage(format!(
            "since ({}) is after until ({})",
            dates::fmt_iso(since),
            dates::fmt_iso(until)
        )));
    }
    let months = (until.0 - since.0) * 12 + (i64::from(until.1) - i64::from(since.1)) + 1;
    if months > MAX_MONTHS {
        return Err(ToolError::usage(format!(
            "since to until spans {months} months and a call reads at most {MAX_MONTHS}; narrow \
             the range"
        )));
    }
    Ok((since, until))
}

#[cfg(test)]
mod tests;
