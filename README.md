# schwab-cli (`schw`)

Charles Schwab brokerage accounts, positions, and transaction history from the
command line — Keychain-secured, agent-friendly, `--json` on every command.

**Read-only. Unofficial.** Not affiliated with or endorsed by Charles Schwab.

Conforms to the [piekstra-cli spec v1](https://github.com/piekstra/cli-common).

---

## Status: live-verified

**First verified live 2026-08-12** against a real individual Trader API app
and two linked accounts: the OAuth login and refresh flows, every read
command, and the response shapes in `tests/fixtures/` (scrubbed from
captures). The live run also uncovered — and this CLI now defends against —
Schwab's **silent 3,000-row-per-response cap** on transaction history, which
truncates with no error and no marker (see
[`docs/api.md`](docs/api.md) §5).

Schwab's Trader API requires an application registered at
[developer.schwab.com](https://developer.schwab.com) and approved by hand,
which takes days; and its refresh tokens die after 7 days, unrenewably, so
every week of use starts with a browser login.

[`docs/api.md`](docs/api.md) records every claim with a citation and marks
which are live-verified versus documented-only. Read it before trusting any
output.

## What it's for

Tax work. Two things Schwab has that matter at tax time:

| Want | Where it is |
|---|---|
| What did I sell this year? | **This CLI** — `schw transactions list` |
| The consolidated 1099 / realized gain-loss report | **Not in the API.** Portal only — see below |

### The tax-documents finding

**Schwab's API exposes no tax documents and no realized gain/loss data.** There
is no 1099 endpoint, no statements endpoint, no cost basis, no proceeds, no
acquisition date, and no wash-sale field — anywhere. The whole Trader API is
seven resources, and none of them is paperwork. `docs/api.md` §6 shows the
enumeration and the citations.

What that leaves:

- **`schw transactions list`** gives you the in-year activity feed: every trade,
  with symbol, quantity, price, `OPENING`/`CLOSING`, fees, and net amount. Good
  for "roughly where am I this year".
- **Realized gain/loss figures** live in the portal: log in → *Accounts →
  History → Realized Gain/Loss*, which has a CSV export (capped at 1,500 rows
  per download).
- **The consolidated 1099** lives at
  [schwab.com/1099dashboard](https://schwab.com/1099dashboard).

Do not try to reconcile a reconstruction from this CLI to a 1099-B; it will not
match, because Schwab's lot-relief method and wash-sale adjustments are not in
the API. `docs/api.md` §6 explains why in detail.

## Install

```console
$ git clone https://github.com/piekstra/schwab-cli && cd schwab-cli
$ make install          # cargo install + stable code signature (see below)
```

## Setup

This is the slow part, and only the account owner can do it.

1. Register an app at [developer.schwab.com](https://developer.schwab.com) —
   product **`Accounts and Trading Production`**, callback URL
   `https://127.0.0.1`.
2. **Wait for the app to reach `Ready For Use`.** `Approved - Pending` is not
   ready, and fails with a misleading `invalid_client`. Expect several days.
3. Store the credential and log in:

```console
$ schw auth set-credential --stdin      # paste APPKEY:APPSECRET
$ schw auth login                       # prints a URL; paste back the redirect
$ schw accounts list                    # copy your account hash
$ schw config set account <HASH>
```

The full walkthrough, with every trap, is in [`docs/api.md`](docs/api.md) §2.

### The 7-day wall

Schwab's refresh token lasts **7 days and cannot be renewed** — refreshing the
access token does not extend it. After that, `schw auth login` and a browser
are mandatory. There is no unattended path; this is Schwab policy, not a
limitation here. `schw auth status` tells you how many days are left.

## Use

```console
$ schw accounts list                    # accounts and the hashes they're addressed by
$ schw positions list                   # open positions and unrealized P/L
$ schw transactions list                # this year's trades (the tax feed)
$ schw transactions list --since 2026-01-01 --until 2026-06-30
$ schw transactions list --type DIVIDEND_OR_INTEREST
$ schw api GET /trader/v1/userPreference    # raw passthrough (GET only)
```

Every command takes `--json` and emits a single `schema`-tagged DTO:

```console
$ schw transactions list --json | jq '.items[0]'
{
  "id": "90000000001",
  "date": "2026-03-14",
  "type": "TRADE",
  "symbol": "AAPL",
  "quantity": "-100",
  "price": "175.25",
  "net_amount": { "amount": "17518.05", "currency": "USD" },
  "position_effect": "CLOSING"
}
```

`transactions list` defaults to the current year and to `--type TRADE`. It
splits the range into calendar-month windows because Schwab's per-request date
cap is undocumented and an over-long request returns *fewer rows rather than an
error* — see `docs/api.md` §5.

### Commands

| Command | |
|---|---|
| `auth set-credential` (`set-app`) | store the app key/secret |
| `auth login` | browser authorization; `--print-url` for the two-step form |
| `auth status` | credential + session state, offline |
| `auth refresh` | renew the access token now |
| `auth logout [--forget]` | clear session; `--forget` also drops the app credential |
| `config path\|show\|set\|unset` | `base_url`, `redirect_uri`, `account` |
| `accounts list` | linked accounts and hashes |
| `positions list` | open positions |
| `transactions list` | transaction history |
| `api GET <PATH>` | raw passthrough |
| `mcp` | serve read-only tools to Claude ([below](#use-with-claude)) |
| `self-update`, `completions`, `info` | family standard |

Exit codes: `0` ok · `2` usage · `3` auth · `4` not found · `5` upstream ·
`6` refused (a write verb). Exit `5` also covers another `schw` process holding
the token-refresh lock past a 75-second wait, in which case nothing was sent.

## Read-only, enforced in code

`schw` cannot place, replace, or cancel an order. This is not a promise in a
README — Schwab's write endpoints share the same host and the same token as the
reads, so the guarantee is structural:

- `client::Schwab` exposes no HTTP verb but `GET`. There is no method to call.
- `schw api` refuses every verb but `GET`, with exit code 6.
- `schw mcp`'s tools reach Schwab through a seam with `get` alone, on paths built
  from fixed constants and the hashes in Schwab's own account list.
- Tests assert all three.

Changing any one of those means changing all of them, in the same commit.

## Use with Claude

`schw mcp` serves two read-only tools over MCP, so Claude can answer questions
about your own Schwab accounts. There is no hosted service: the tools run on your
computer, in your copy of `schw`, and call Schwab's API with the session in your OS
keychain.

| Tool | Answers |
| --- | --- |
| `schwab_portfolio` | What do I hold at Schwab, and what is it worth? Each account's equity and cash, and the positions, narrowed by `account` or `symbol` (a symbol also matches options on it) |
| `schwab_transactions` | What happened over a period? Trades, dividends, deposits and withdrawals by `since`/`until` (year to date by default, 24 months at most), `account`, `type` (`TRADE`, `DIVIDEND_OR_INTEREST`, …) and `symbol`; every linked account unless `account` names one |

No tool can place, change or cancel an order, move money or change anything: there
is no such tool, the tools have no verb but `GET`, and `--allow-writes` adds nothing.
Schwab's API has no tax documents, cost basis or realized gains, so neither do the
tools.

Results are concise by default (`response_format: "detailed"` adds each transaction's
legs and fees, and provider detail), 25 records a page with a cursor for more, and
never larger than about 32,000 characters. An account goes by its type and last four
digits (`MARGIN …1234`); no result carries the full number or the hash. A transaction
list that Schwab's per-response cap cut short says `complete: false`, and one reaching
back more than 12 months carries a `note`, because Schwab returns less without an
error when its history runs out. Each page reads Schwab again, so a row posted between
pages can repeat or be skipped: for a total, read the range in one call, with `limit` up
to 200. Descriptions come from Schwab and other parties; the server tells Claude to
treat them as data, not instructions.

The tools are worded for a model, so two things differ from `--json`. Amounts are plain
decimal strings, with `unrealized_gain`, `day_gain` and `amount` where `positions --json`
and `transactions list --json` have `open_pl`, `day_pl` and `net_amount`. And
`schwab_transactions` covers every type unless `type` names one, where `schw transactions
list` covers `TRADE` unless `--all-types` is passed.

**Sign in first.** Sign-in is through a browser, so no settings form can hold it. Run
`schw auth set-credential --stdin` and `schw auth login` in a terminal (see
[Setup](#setup)). Both ways of connecting Claude below then use that session
from the OS keychain (service `piekstra.schw`), and renew the access token in place, as
the commands do, under the same lock as every other `schw`. Schwab ends a session after
7 days, whatever you do: the tools then say so and ask you to run `schw auth login
--overwrite` in a terminal. They never sign in by themselves.

**Claude Code.** Add the `schwab-cli` plugin from the
[piekstra plugin marketplace](https://github.com/piekstra/claude-plugins):

```console
$ claude plugin marketplace add piekstra/claude-plugins
$ claude plugin install schwab-cli@piekstra-plugins
```

or register the server yourself:

```console
$ claude mcp add schwab -- schw mcp
```

**Claude Desktop.** Download `schw-<version>.mcpb` from the
[latest release](https://github.com/piekstra/schwab-cli/releases/latest) and open it.
It asks for nothing: the bundled `schw` reads what your installed `schw` stored. On
macOS the bundled binary is a different program to the keychain, so its first call
raises a keychain prompt for each `piekstra.schw` item; choose **Always Allow**, or
every call will ask again.

Any other MCP client that launches a local server can run `schw mcp`.

**Claude on the web and mobile.** claude.ai reaches tools through a remote connector,
and your Schwab session stays on your computer, so `schw` serves the same tools from
your computer to a connector you deploy into your own Cloudflare account:

```console
$ schw mcp --new-token            # once: a bearer token, saved in the keychain and printed for the Worker
$ schw mcp --http --port 8736     # serve on 127.0.0.1 only
```

The connector, [cli-common's `connector/`](https://github.com/piekstra/cli-common/tree/main/connector#tools-from-your-computer-home-modules),
reaches that port through a Cloudflare Tunnel in your account; its README walks through
the tunnel, the VPC service, a launchd agent that keeps the server running, and the
`HOME_SCHWAB_TOKEN` secret, with the threat model. `--http` binds 127.0.0.1 only,
refuses to start without the token, refuses any request whose `Host` is not loopback
or that carries an `Origin`, and serves the read-only tools only. The Schwab session
never leaves your computer, but tool results do: your balances, positions and
transactions pass through Cloudflare and reach Claude ([`SECURITY.md`](SECURITY.md)).
While the computer sleeps, the tools answer that it is offline.

After `schw self-update`, restart the server: a running one keeps the old code, and its
launchd agent restarts it only when it exits. Its start line names the version.

```console
$ launchctl kickstart -k gui/$(id -u)/com.piekstra.schw-mcp-http
```

`self-update` does not reach a Claude Desktop extension, which runs its own copy.
Install the new release's `.mcpb` too.

## Security

Secrets live in the macOS/Linux keychain under `piekstra.schw`, in two entries: the
app key/secret, and the OAuth tokens (`schw mcp --http` adds a bearer token, which can
also come from `$SCHW_MCP_TOKEN`). Never on argv, never in a file, never in a log. `--verbose` prints method, path, and query parameters —
never the token. See [`SECURITY.md`](SECURITY.md).

`make install` re-signs the binary with a stable code identity so macOS
keychain "Always Allow" grants survive reinstalls; a plain `cargo install`
ad-hoc signs and silently revokes them.

## Development

```console
$ make verify     # fmt-check + clippy -D warnings + tests + smoke — the local gate
$ make test       # fully offline: no network, no credential, never the real keychain entries
```

CI runs the same fmt check, clippy, tests and release build, but not `smoke`,
plus a cargo-audit / cargo-deny / gitleaks job. `smoke` never reads the real
keychain entries.

See [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`AGENTS.md`](AGENTS.md).

## License

MIT or Apache-2.0, at your option.
