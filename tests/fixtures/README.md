# Test fixtures

**These are scrubbed live captures** (with two documented exceptions below).

Capture date: **2026-08-12**, against a real individual Trader API app in
`Ready For Use` state, over two linked accounts (one CASH, one MARGIN, the
latter with heavy options activity). Structure — keys, nesting, types,
null-vs-absent — is copied from what Schwab actually sent; every value was
then replaced under the scrubbing policy below.

The captures confirmed the pattern prior work warned about: the live shapes
disagree with Schwab's published schema in several places. The ones these
fixtures now encode:

- A TRADE row carries **no** `user`, `description`, `settlementDate`, or
  `activityType` — all promised by the published schema. A
  `DIVIDEND_OR_INTEREST` row *does* carry `description` and `settlementDate`,
  plus an undocumented `qualifiedDividend` boolean.
- Every trade has five `transferItems`: four fee legs (`COMMISSION`,
  `SEC_FEE`, `OPT_REG_FEE`, `TAF_FEE`) against a `CURRENCY_USD` instrument,
  and the security leg. Fee legs have no `price` key; the dividend's cash leg
  has `price` but no `feeType`.
- Timestamps come back as `2026-07-16T14:25:19+0000` — no milliseconds, no
  `Z` — despite the documented `yyyy-MM-dd'T'HH:mm:ss.SSSZ` grammar.
- An ETF position reports `assetType: COLLECTIVE_INVESTMENT` (with
  `instrument.type: EXCHANGE_TRADED_FUND` and a `uniformSymbol`), not
  `EQUITY`.
- The accounts payload wraps each account as `{securitiesAccount,
  aggregatedBalance}`; balance key sets differ sharply between CASH and
  MARGIN accounts (a CASH `currentBalances` has no `equity` key at all).

## Still synthetic, not captured

- **`error_401.json`** — no live 401 was provoked; the shape remains
  doc-derived. (A live HTTP 400 was observed and its `message` text surfaced
  correctly through the client's error mapping.)
- **`token_response.json`** — the live token body was not persisted, but its
  field set (`access_token`, `refresh_token`, `expires_in`, `token_type`,
  `scope`, `id_token`) matches what the live login and refresh of 2026-08-12
  parsed successfully.
- The **short position** row in `positions.json` — no live short position
  existed to capture; its `short*` field spellings are doc-derived, mirrored
  from the captured long row's structure.

## Scrubbing policy

Every identity-bearing or financial value is an obvious dummy:

| Field | Dummy shape |
|---|---|
| `accountNumber` | exactly 8 digits, drawn from `11111111` / `22222222` |
| `hashValue` | begins with `DUMMYHASH`, padded to the live 64-character length |
| `access_token`, `refresh_token`, `id_token` | begins with `DUMMY` |
| symbols / CUSIPs / descriptions | `ETFX`, `AAPL`, `TSLA`, `111111111`-style CUSIPs, `DUMMY …` descriptions |
| ids (`activityId`, `orderId`, `positionId`) | synthetic `9…`/`8…`/`7…` ranges |
| balances, prices, amounts | round synthetic numbers, no real position sizes |
| timestamps | synthetic dates in the captured `+0000` format |

`tests/fixture_shapes.rs` enforces this, and it enforces it **positively** —
"every account number must match the dummy shape" rather than "no account
number may be one of these real values". A denylist of real values would
publish, in the test file, exactly the data the rule exists to keep out.

If you refresh these from new captures: scrub under this policy, update the
capture date here, and update `docs/api.md`'s verification status in the same
commit. Never commit a raw capture.

## Files

| File | Stands for |
|---|---|
| `account_numbers.json` | `GET /trader/v1/accounts/accountNumbers` |
| `accounts.json` | `GET /trader/v1/accounts` (balances) |
| `positions.json` | `GET /trader/v1/accounts/{hash}?fields=positions` |
| `transactions.json` | `GET /trader/v1/accounts/{hash}/transactions` (TRADE + dividend rows) |
| `token_response.json` | `POST /v1/oauth/token` success body |
| `error_401.json` | a rejected-token error body |
