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
| `self-update`, `completions`, `info` | family standard |

Exit codes: `0` ok · `2` usage · `3` auth · `4` not found · `5` upstream ·
`6` refused (a write verb).

## Read-only, enforced in code

`schw` cannot place, replace, or cancel an order. This is not a promise in a
README — Schwab's write endpoints share the same host and the same token as the
reads, so the guarantee is structural:

- `client::Schwab` exposes no HTTP verb but `GET`. There is no method to call.
- `schw api` refuses every verb but `GET`, with exit code 6.
- A test asserts both.

Changing any one of those means changing all three, in the same commit.

## Security

Secrets live only in the macOS/Linux keychain under `piekstra.schw`, in two
entries: the app key/secret, and the OAuth tokens. Never on argv, never in a
file, never in a log. `--verbose` prints method, path, and query parameters —
never the token. See [`SECURITY.md`](SECURITY.md).

`make install` re-signs the binary with a stable code identity so macOS
keychain "Always Allow" grants survive reinstalls; a plain `cargo install`
ad-hoc signs and silently revokes them.

## Development

```console
$ make verify     # fmt-check + clippy -D warnings + tests + smoke — the CI gate
$ make test       # fully offline: no network, no keychain, no credential
```

See [`CONTRIBUTING.md`](CONTRIBUTING.md) and [`AGENTS.md`](AGENTS.md).

## License

MIT or Apache-2.0, at your option.
