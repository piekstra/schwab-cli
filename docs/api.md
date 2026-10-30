# The Schwab Trader API, as `schw` understands it

Everything here carries a source. Claims verified against a real account by
this CLI are marked **[live]** (with the date); claims taken from Schwab's own
documentation are marked **[official]**; claims taken from third-party
clients, wrappers, and community write-ups are marked **[community]**;
anything none of those confirms is marked **UNVERIFIED**. Do not add an
uncited line to this file.

---

## 0. Verification status — read this first

**Live-verified 2026-08-12** — a new claim class, **[live]**, joins
**[official]** and **[community]** below. On that date every read command in
this CLI ran against a real individual Trader API app (`Ready For Use`,
reused from prior in-house work) and two linked accounts, one CASH and one
MARGIN, the MARGIN one carrying thousands of option trades per month —
a stress case that exposed truncation behavior no documentation mentions.

Verified live: the OAuth authorization URL, the code exchange (including the
`%40` decode), a refresh round-trip (and that it does **not** extend the
7-day deadline), `accounts list`, `positions list`, `transactions list`
(default, `--type`, and `--all-types`), the `schw api` passthrough, and the
HTTP-error → exit-code mapping. `tests/fixtures/` is now **scrubbed from
those captures** (see `tests/fixtures/README.md`, including the short list of
shapes that remain doc-derived).

The prediction that Schwab's published schema and its live responses disagree
was confirmed in force — §5 records the differences, of which the most
consequential is the **silent 3,000-row response cap** on `/transactions`.
§7 keeps the first-live-run checklist with each item's answer.

Anything below not marked **[live]** retains exactly the confidence of its
citation: a doc, or another client's code, but not a response this code
received.

---

## 1. Can an individual get access? Yes — and it takes days

Schwab publishes two tiers:

| Product | For | Link |
|---|---|---|
| **Trader API – Individual** | your *own* brokerage accounts | <https://developer.schwab.com/products/trader-api--individual> |
| Trader API – Commercial | serving *other* Schwab clients' accounts | <https://developer.schwab.com/products/trader-api--commercial> |

**[official]** The Individual product is described as: "As a self-directed
trader, the Individual Trader APIs provide your application with the ability to
perform trading activities using your own brokerage trading account."
<https://developer.schwab.com/products/trader-api--individual>

The individual track is **open** as of 2026 — not waitlisted, not closed.
**[community]** Wealth-Lab's Schwab extension, with builds dated August 2026,
tells users to register as an Individual Developer precisely because commercial
onboarding is backlogged: <https://www.wealth-lab.com/extension/detail/Schwab>.
Wrapper libraries are actively maintained against it, e.g. `schwabr` (May 2026)
<https://cran.r-project.org/web/packages/schwabr/schwabr.pdf>.

A Schwab **brokerage account is required** **[community]**
(<https://altanalytics.github.io/schwabr/>), and the developer account should
use the same email as the brokerage login **[community]**
(<https://github.com/tylerebowers/Schwabdev/blob/main/docs/pages-raw/setupguide.md>).

### History: this is not the TD Ameritrade API

**[community]** TD Ameritrade's developer API was shut down after market close
on 10 May 2024, and **TDA apps did not carry over** — a new Schwab app is
required: <https://schwab-py.readthedocs.io/en/latest/tda-transition.html>

Three regressions from TDA persist and are worth knowing before building on
this **[community]** (same source):

1. Token lifetime fell from up to 90 days with indefinite refresh to **7 days
   with no renewal mechanism**.
2. Saved orders and watchlists were removed and never returned.
3. Symbol formats changed (equity indices lost the `.X` suffix: `$SPX`, not
   `$SPX.X`).

---

## 2. Registration — the owner's walkthrough

This is the part only the account owner can do, and it is the gate on
everything else. Budget **several days**.

1. **Create a developer account** at <https://developer.schwab.com/login>.
   Use the same email as the Schwab brokerage login. **[community]**

2. **Request product access.** On
   <https://developer.schwab.com/products/trader-api--individual>, click
   **Request Access** and follow the prompts. There is an agreement to accept
   at this step; its exact text could not be retrieved — **UNVERIFIED**.
   **[community]** <https://www.wattcollc.com/Schwab/SchwabInstr.html>

3. **Create the app** at <https://developer.schwab.com/dashboard/apps> →
   *Create App*. Supply a name and description. **[community]**

4. **Select the API products.** The exact names are:
   - **`Accounts and Trading Production`** ← this is the one `schw` needs
   - `Market Data Production` (quotes, chains, price history — `schw` does not
     use these)

   **[community]** Schwabdev advises adding **both**; schwab-py notes that
   "Accounts and Trading Production" alone covers the account/trading surface.
   If missed at creation: *Apps Dashboard → View Details → Modify App → APIs*.
   <https://github.com/tylerebowers/Schwabdev/blob/main/docs/pages-raw/setupguide.md>,
   <https://schwab-py.readthedocs.io/en/latest/getting-started.html>

5. **Set the Order Limit.** An app-level orders-per-minute throttle, maximum
   **120**. `schw` never places orders, so this value does not matter to it —
   but the field is mandatory. **[official]** Schwab's docs: "Throttle limits
   for orders can be set from zero (0) to 120 requests per minute per account."
   <https://archive.org/stream/schwabtraderapi/schwabapi_djvu.txt>

6. **Set the Callback URL** — see §3. Use exactly:

   ```
   https://127.0.0.1
   ```

   and then set the same string with `schw config set redirect_uri …` if you
   choose anything different.

7. **Wait for the status to become `Ready For Use`.**

### The status trap

The lifecycle is `Approved - Pending` → `Ready For Use`.

**[community]** "Don't be fooled by the word `Approved`: your app is not yet
ready for use."
<https://schwab-py.readthedocs.io/en/latest/getting-started.html> — and
Schwabdev is blunter: "Note that 'Approved - Pending' will not work."
<https://github.com/tylerebowers/Schwabdev/blob/main/docs/pages-raw/setupguide.md>

An app in `Approved - Pending` fails the token exchange with **`invalid_client`
even when the key and secret are correct**. `schw` says so in that error
message, because otherwise it reads like a typo.

### How long

**[community]**, and highly variable — plan for days, budget for weeks:

| Report | Source |
|---|---|
| "a few days to process" | <https://medium.com/@carstensavage/the-unofficial-guide-to-charles-schwabs-trader-apis-14c1f5bc1d57> |
| "this can take a couple days" | <https://github.com/tylerebowers/Schwabdev/blob/main/docs/pages-raw/setupguide.md> |
| "multiple days" in `Approved - Pending` | <https://schwab-py.readthedocs.io/en/latest/tda-transition.html> |
| "up to a month if you don't press them" — emailing **traderapi@schwab.com** expedites | <https://www.wattcollc.com/Schwab/SchwabInstr.html> |

### Before registering: you may already have an app

Steps 1–7 are only needed if there is no existing registration. **Check the
[apps dashboard](https://developer.schwab.com/dashboard/apps) first.** An app
registered for earlier work — a trading client, a data script — is the *same*
kind of app this CLI needs, and reusing it skips the entire approval wait.

Two things to check if you reuse one:

1. **The product must include `Accounts and Trading Production`.** If it only
   has `Market Data Production`, add it via *View Details → Modify App → APIs*
   — and note that modifying an app can re-trigger approval.
2. **Match the callback URL exactly.** An existing app is likely registered
   with something other than this CLI's default of `https://127.0.0.1` — a
   loopback URL with a port and a path is a common choice. Point `schw` at
   whatever is registered rather than changing the app (changing it re-triggers
   approval):

   ```console
   $ schw config set redirect_uri 'https://127.0.0.1:8443/callback'
   ```

   It must match byte-for-byte, trailing slash included (§3).

Reusing an app does **not** avoid the browser login: the 7-day refresh-token
wall applies per token, not per app, so any previously stored token more than a
week old is dead and `schw auth login` is required regardless.

**[live]** This is exactly how this repo went live on 2026-08-12: the app
registered for prior in-house work was reused as-is, callback
`https://127.0.0.1:8443/callback`, no portal changes and no approval wait.
The credential observed was a **48-character app key and 64-character
secret** — community write-ups describe 32/16, so expect either.

### Then, locally

```console
$ schw auth set-credential --stdin       # paste APPKEY:APPSECRET, or the JSON form
$ schw auth login                        # browser round trip
$ schw accounts list                     # copy your account hash
$ schw config set account <HASH>
```

---

## 3. The callback URL

**[official]** Rules page:
<https://developer.schwab.com/user-guides/apis-and-apps/app-callback-url-requirements>
(also as an FAQ:
<https://developer.schwab.com/faqs/app-callback-url-requirements-and-limitations>).
Neither page could be retrieved by automated fetch — developer.schwab.com
answers 403 to non-browser clients — so the specifics below are
**[community]** and the official text is **UNVERIFIED**.

- **HTTPS is mandatory**, and loopback addresses are allowed.
  <https://github.com/tylerebowers/Schwabdev/blob/main/docs/pages-raw/setupguide.md>
- schwab-py: only `127.0.0.1` as a host, and a port **above 1024** is strongly
  recommended — omitting the port means 443, which the OS will usually refuse
  to bind. <https://schwab-py.readthedocs.io/en/latest/auth.html>
- **It must match byte-for-byte** at every step: registration, the
  authorization URL, and the token exchange. "Any deviation (including adding
  or removing a trailing slash!) can cause difficult-to-debug issues."
  <https://schwab-py.readthedocs.io/en/latest/getting-started.html>
  This is why `schw`'s `Config::redirect_uri()` returns the stored string
  verbatim and never normalizes it.
- Multiple callbacks are comma-separated in the portal. **[community]**
- **Flaky:** users have reported Schwab intermittently refusing to create apps
  with `127.0.0.1` callbacks; the workaround offered is `https://localhost:PORT`.
  <https://schwab-py.readthedocs.io/en/latest/getting-started.html>
- Changing the callback URL later **re-triggers approval**. **[community]**

### Why `schw` has no local redirect listener

Because the callback must be `https`, a plain TCP listener cannot answer the
browser — it would receive a TLS ClientHello it cannot decrypt. The two options
are:

1. Terminate TLS locally with a self-signed certificate, which the user must
   click past. This is what schwab-py and prior in-house work do
   (`rcgen` + `rustls`, SANs `localhost` and `127.0.0.1`, one-shot server).
2. Let the browser fail to connect and read the code out of the address bar.

`schw` does (2). The redirect carries the code in its URL whether or not
anything is listening, so (2) has no failure mode that (1) doesn't also have,
needs no TLS stack, and does not train the user to click through certificate
warnings. If (1) is ever wanted, the recipe above is the whole of it.

---

## 4. OAuth2

**[official]** unless marked. Source for the official lines: Schwab's own
Trader API documentation, mirrored at
<https://archive.org/stream/schwabtraderapi/schwabapi_djvu.txt> (the
developer.schwab.com originals are browser-only).

| Item | Value |
|---|---|
| Authorize | `GET https://api.schwabapi.com/v1/oauth/authorize` |
| Token | `POST https://api.schwabapi.com/v1/oauth/token` |
| Grants | `authorization_code`, `refresh_token` |
| Client auth | `Authorization: Basic base64(AppKey:AppSecret)` |
| Body | `application/x-www-form-urlencoded` |
| API auth | `Authorization: Bearer <access_token>` |
| Access token TTL | **30 minutes** |
| Refresh token TTL | **7 days**, not renewable |
| Scope issued | `api` |

Authorization URL:

```
https://api.schwabapi.com/v1/oauth/authorize?client_id={APP_KEY}&redirect_uri={CALLBACK}
```

Token bodies:

```
grant_type=authorization_code&code={CODE}&redirect_uri={CALLBACK}
grant_type=refresh_token&refresh_token={REFRESH_TOKEN}
```

Token response fields: `access_token`, `refresh_token`, `expires_in`,
`token_type`, `scope`, `id_token`.

**[live]** All of the above verified 2026-08-12: the authorization URL (sent
with only `client_id`, `redirect_uri`, and `response_type=code` — no `scope`,
no `state`) opened Schwab's login and consent flow; the pasted redirect's
code exchanged successfully; the resulting session survived a
`schw auth refresh` round-trip; and every subsequent API call authenticated.
The token response parsed with exactly the field set above.

### Trap: the authorization code must be URL-decoded

**[official], verbatim:** *"The 'code' within this request must be URL decoded
prior to making the request."*
<https://archive.org/stream/schwabtraderapi/schwabapi_djvu.txt>

Mechanically: the code Schwab returns ends in `%40`, which decodes to `@`. Post
it still-encoded and the exchange fails with an error that looks like a
malformed request rather than a mis-decoded one. **[community]** reference
implementations slice on the `%40` explicitly and re-append `"@"`:
<https://medium.com/@carstensavage/the-unofficial-guide-to-charles-schwabs-trader-apis-14c1f5bc1d57>

`schw` handles this by decoding exactly once with a real query-pair parser
(`src/urlenc.rs`) and letting the HTTP layer re-encode once when it builds the
form body. **Never hand-glue the form body from a raw query string** — that
double-encodes, which is the same bug wearing a different hat.

**[live]** Verified 2026-08-12: the redirect's code ended in `%40` exactly as
described, and the decode-once path exchanged it successfully on the first
attempt.

### Trap: the code expires in ~30 seconds

**[community]** The authorization code "must be used within 30 seconds".
<https://github.com/slimandslam/schwab-client-js/blob/main/docs/SchwabConfig.md>
It is also single-use. `schw auth login` therefore prints the URL and waits for
the paste rather than doing anything slow in between.

### Trap: `scope=readonly` is advertised but not honored

Schwab's published OAuth security scheme includes `response_type=code&
scope=readonly`, but the token comes back with `"scope": "api"` regardless.
Requesting a read-only scope therefore buys no guarantee. `schw` does not send
`scope`, and enforces read-only in its own client instead — `client::Schwab`
has no HTTP verb but GET. **UNVERIFIED** whether Schwab honors `scope=readonly`
in any form.

### The 7-day wall

This is the single most important operational fact about this API.

**[official]** "Upon expiration, a new set refresh token must be recreated
using the authorization_code Grant Type authentication flow."
<https://archive.org/stream/schwabtraderapi/schwabapi_djvu.txt>

**[community]**, corroborating:
- "requests for a new access token using a refresh token older than seven days
  are rejected" — <https://schwab-py.readthedocs.io/en/latest/auth.html>
- "a hard 7-day expiration limit that cannot be extended" —
  <https://github.com/sudowealth/schwab-api>
- "after 7 days you have to manually log in again" —
  <https://cran.r-project.org/web/packages/schwabr/schwabr.pdf> (May 2026)

Refreshing the access token **does not extend** the refresh token's 7-day
clock. `schw` models this explicitly: `oauth::refresh_tokens` carries the
*original* deadline forward, and `auth status` reports days remaining, so the
tool never claims a healthy session that is about to require a human.

**[live]** Verified 2026-08-12: `schw auth refresh` succeeded and
`auth status` reported the identical re-authorization deadline before and
after.

**Consequence for a once-a-year tax job:** there is no unattended path. Any
run more than 7 days after the last login needs a browser. Plan for
`schw auth login` as a manual step every time.

### Other notes

- **[community]** The consent screen includes **account selection** — accounts
  not ticked there are invisible to the API.
  <https://github.com/tylerebowers/Schwabdev/blob/main/docs/pages-raw/setupguide.md>
- **[community]** Don't use the same token from two machines.
  <https://schwab-py.readthedocs.io/en/latest/auth.html>
- **[community]** thinkorswim enablement is reported as required for the
  account. Whether it gates *read-only* endpoints is **UNVERIFIED**.
  <https://github.com/tylerebowers/Schwabdev/blob/main/docs/pages-raw/setupguide.md>
- `state` parameter support is **UNVERIFIED**; `schw` does not send one and
  does not rely on CSRF protection here, since the code is pasted by hand
  rather than accepted by a listener.

---

## 5. Endpoints

Base URLs: `https://api.schwabapi.com/trader/v1` and
`https://api.schwabapi.com/marketdata/v1`. Paths below verified against
schwab-py's literal path constants
<https://github.com/alexgolec/schwab-py/blob/main/schwab/client/base.py> and
prior in-house work against Schwab's own OpenAPI document.

**The entire Trader surface is seven resources:**

| Method | Path | `schw` |
|---|---|---|
| GET | `/trader/v1/accounts/accountNumbers` | `accounts list` |
| GET | `/trader/v1/accounts` (`?fields=positions`) | `accounts list` |
| GET | `/trader/v1/accounts/{hash}` (`?fields=positions`) | `positions list` |
| GET | `/trader/v1/accounts/{hash}/orders` | — |
| POST/PUT/DELETE | `…/orders…` | **never** (read-only) |
| GET | `/trader/v1/orders` | — |
| POST | `/trader/v1/accounts/{hash}/previewOrder` | **never** |
| GET | `/trader/v1/accounts/{hash}/transactions` | `transactions list` |
| GET | `/trader/v1/accounts/{hash}/transactions/{id}` | — |
| GET | `/trader/v1/userPreference` | — |

Independently corroborated by an API catalog listing exactly ten published
Schwab APIs — Accounts, Instruments, Market Hours, Movers, Options, Orders,
Price History, Quotes, Transactions, User Preferences:
<https://apis.io/providers/charles-schwab/>

### Account hashes, not account numbers

`GET /accounts/accountNumbers` returns `{accountNumber, hashValue}` pairs.
**Every other account-scoped endpoint requires the `hashValue`.**
**[community]** <https://schwab-py.readthedocs.io/en/latest/client.html>,
<https://medium.com/@carstensavage/the-unofficial-guide-to-charles-schwabs-trader-apis-14c1f5bc1d57>

The hash is **not an anonymization** — it is a stable handle to one specific
brokerage account. `schw` stores it in config and prints it in `accounts list`
because it is unavoidable, and masks the real account number beside it.

**[live]** Verified 2026-08-12: `accountNumber` is an 8-digit string and
`hashValue` a 64-character string.

### The accounts payload, as actually sent

**[live]** 2026-08-12. Each element of `GET /accounts` wraps the documented
account in an **undocumented `aggregatedBalance`** sibling:

```
{ "securitiesAccount": { … }, "aggregatedBalance": {
    "currentLiquidationValue", "liquidationValue",
    "intradayBuyingPowerAmount", "currentIntradayBuyingPowerAmount" } }
```

`securitiesAccount` carries `type` (`CASH` | `MARGIN`), `accountNumber`,
`roundTrips`, `isDayTrader`, `isClosingOnlyRestricted`, `isPortfolioMargin`,
`pfcbFlag`, and three balance objects — plus `isIntradayMargin` **on MARGIN
accounts only**. The balance key sets differ sharply by account type:

| | MARGIN | CASH |
|---|---|---|
| `currentBalances` | `equity`, `buyingPower`, `availableFunds`, `marginBalance`, `sma`, … (29 keys) | `cashAvailableForTrading`, `cashAvailableForWithdrawal`, `totalCash`, `unsettledCash`, `cashCall`, … (20 keys) |
| `equity` key present? | yes | **no** — use `liquidationValue` |
| `projectedBalances` | 10 keys | just `cashAvailableForTrading`, `cashAvailableForWithdrawal` |

All balances arrive as JSON numbers. This is why `parse::accounts` reads
`equity` with a `liquidationValue` fallback.

### Trap: `fields=positions`

Without `?fields=positions`, the account payload carries balances only. The
positions array is simply absent — which looks exactly like "you hold
nothing".

**[live]** Verified 2026-08-12, both directions: with the parameter every
position appeared; without it the `positions` key was absent. A position row
carries `longQuantity`/`shortQuantity`, `settledLongQuantity`,
`previousSessionLongQuantity`, `averagePrice`/`averageLongPrice`/
`taxLotAverageLongPrice`, `currentDayProfitLoss(Percentage)`,
`currentDayCost`, `marketValue`, `maintenanceRequirement`,
`longOpenProfitLoss`, and an `instrument` with `assetType`, `cusip`,
`symbol`, `uniformSymbol`, `description`, `type`. Note: an ETF reports
`assetType: COLLECTIVE_INVESTMENT` with `type: EXCHANGE_TRADED_FUND` — not
`EQUITY`.

### `/transactions`

Query parameters: `startDate`, `endDate`, `symbol`, `types`.

**Dates.** **[official]** Schwab's schema documents
`yyyy-MM-dd'T'HH:mm:ss.SSSZ`, e.g. `2024-03-28T21:10:42.000Z` — milliseconds
and the `Z`. **[live]** Verified 2026-08-12: millisecond-bearing bounds
(`.000Z` starts, `.999Z` ends) are accepted on every request. Both bounds must
be supplied together. Note the asymmetry: **request** timestamps carry
milliseconds and `Z`, while **response** timestamps arrive as
`2026-07-16T14:25:19+0000` — no milliseconds, `+0000` instead of `Z`.

**Trap: a zero-width window returns nothing.** `startDate == endDate` yields no
rows. **[community]** <https://github.com/GeneBO98/tradetally/issues/274>
**[live]** A same-day window spanning `T00:00:00.000Z`–`T23:59:59.999Z`
returned that day's rows fine, so the trap bites on identical *instants*, not
identical dates. `schw` ends every window at `T23:59:59.999Z` — `.999`
because the millisecond-precision grammar would otherwise leave
`(23:59:59.000, 00:00:00.000)` covered by no window at all.

**Trap: the 3,000-row response cap — silent, newest-first.** **[live]**
2026-08-12, previously documented nowhere. A single response holds at most
**3,000 rows**. An over-full window is answered with the **newest 3,000** of
its rows; everything older is dropped with no error, no HTTP signal, and no
body marker. A capped response is byte-for-byte indistinguishable from a
complete one, except for the suspicious row count. Observed directly: a
full-month window over an options-heavy account (true count 8,000+) returned
exactly 3,000 rows — the month's early days missing entirely and the oldest
surviving day cut partway through, mid-day; narrower probes then returned the
missing rows.

`schw` therefore treats *any* response of ≥3,000 rows as truncated and
**bisects the window in time**, recursively, until every sub-window comes
back under the cap (`transactions.rs::fetch_window`). Combined with the
month chunking below, a `transactions list` is complete regardless of volume.

**Trap: the history ceiling, and it is silent.** Sources conflict:

| Claim | Source |
|---|---|
| "Date must be within 60 days of the current date" | **[community]** <https://schwab-py.readthedocs.io/en/latest/client.html> — **disproved [live]** |
| default 180 days; "will not pull back transactions older than 1 year" | **[community]** <https://cran.r-project.org/web/packages/schwabr/schwabr.pdf>, <https://cran.r-project.org/web/packages/charlesschwabapi/charlesschwabapi.pdf> |

**[live]** 2026-08-12: data **121 days old** (April 2026) was returned in
full, so the 60-day reading is wrong. Requests reaching 13 months back were
**accepted without error** and returned empty arrays — but the probed
accounts' oldest activity was only ~4 months old, so whether a 1-year (or
any) ceiling exists could not be settled. The exact maximum remains
**UNVERIFIED**, and a too-old request still does not error — it returns
nothing. So `schw`:

1. Splits any range into **calendar-month windows** (≤31 days), which sit
   inside every claimed reading of the maximum query span.
2. Warns on stderr when `--since` is more than 12 months back, because the
   answer may be short and Schwab will not say so.

**`types`.** The 15 documented values: `TRADE`, `RECEIVE_AND_DELIVER`,
`DIVIDEND_OR_INTEREST`, `ACH_RECEIPT`, `ACH_DISBURSEMENT`, `CASH_RECEIPT`,
`CASH_DISBURSEMENT`, `ELECTRONIC_FUND`, `WIRE_OUT`, `WIRE_IN`, `JOURNAL`,
`MEMORANDUM`, `MARGIN_CALL`, `MONEY_MARKET`, `SMA_ADJUSTMENT`. **[community]**
<https://schwab-py.readthedocs.io/en/latest/client.html>

**[live]** Verified 2026-08-12, settling what the schema left ambiguous:
`types` is **optional** — omitting it returns every type (`--all-types`
works) — and it **accepts a comma-separated list**
(`types=TRADE,DIVIDEND_OR_INTEREST` returned both types). `schw` still
defaults to the single value `TRADE`. Types observed in the wild: `TRADE`,
`DIVIDEND_OR_INTEREST`, `JOURNAL`, `SMA_ADJUSTMENT`.

**Response shape.** **[live]** 2026-08-12 — and it is *not* the published
one. Field presence varies by row type:

- A `TRADE` row: `activityId` (number), `time`, `accountNumber`, `type`,
  `status`, `subAccount`, `tradeDate`, `positionId`, `orderId`, `netAmount`,
  `transferItems[]`. **None of the documented `user`, `description`,
  `settlementDate`, `activityType` fields exist.**
- A `DIVIDEND_OR_INTEREST` row: `activityId`, `time`, `description`,
  `accountNumber`, `type`, `status`, `subAccount`, `tradeDate`,
  `settlementDate`, `netAmount`, an **undocumented `qualifiedDividend`**
  boolean, and a single cash `transferItem`. No `positionId`/`orderId`.
- Every trade carries **five `transferItems`**: four fee legs (`COMMISSION`,
  `SEC_FEE`, `OPT_REG_FEE`, `TAF_FEE` — present even when `0.0`) against the
  `CURRENCY_USD` instrument, plus the security leg
  (`amount` = signed quantity, `cost` = signed cash value, `price`,
  `positionEffect`). Fee legs have no `price` key; the dividend cash leg has
  `price` but no `feeType`.
- An OPTION instrument carries `expirationDate`, `putCall`, `strikePrice`,
  `optionPremiumMultiplier`, `underlyingSymbol`, `underlyingCusip`,
  `uniformSymbol`, `type: VANILLA`, and an `optionDeliverables[]` array
  (`rootSymbol`, `strikePercent`, `deliverableNumber`, `deliverableUnits`,
  nested `deliverable{}` instrument).

`tests/fixtures/transactions.json` is the scrubbed capture of all of this.

---

## 6. Tax documents: **not in this API. Portal only.**

This is the finding that shapes what `schw` is for.

**There is no endpoint for tax documents, the consolidated 1099, Form 1099-B,
statements, cost basis, or realized gain/loss.** No `documents` command exists
in this CLI, and none should be added against a guessed endpoint — this has
been looked for.

The evidence is exhaustive-enumeration rather than a Schwab statement:

1. The complete Trader surface is the seven resources in §5, derived from
   Schwab's own path constants. None is document- or tax-related.
2. An independent catalog lists all ten published Schwab APIs; no document or
   tax API appears. <https://apis.io/providers/charles-schwab/>
3. The most complete third-party client states its gaps explicitly: "No
   endpoints documented for: Tax documents, Statements, Historical account
   data, Portfolio analytics." <https://github.com/sudowealth/schwab-api>
4. No wrapper in any language (schwab-py, Schwabdev, schwabr,
   charlesschwabapi, schwab-client-js, …) implements a tax or document call.
5. The transactions schema has no cost-basis, proceeds, realized-gain,
   acquisition-date, or wash-sale field.

**UNVERIFIED:** no explicit Schwab statement saying "tax documents are out of
scope for the API" was found. The *absence of endpoints* is well established;
the *stated policy* is not.

This is the same pattern already recorded for Lofty and Coinbase in this CLI
family: the money data has an API, the tax paperwork does not.

### Where the tax data actually lives

| What | Where | Notes |
|---|---|---|
| Consolidated 1099 / 1099-B PDFs | **<https://schwab.com/1099dashboard>** | View/save/print from the landing page. **[community]** <https://5280associates.com/retrieving-tax-documents-for-your-schwab-accounts/> |
| Realized Gain/Loss report | Log in → **Accounts → History → Realized Gain/Loss** | Has an **Export → CSV** button; custom date ranges; per-account or all accounts. **[community]** <https://marottaonmoney.com/view-realized-capital-gains-and-losses-on-schwab-com/> |
| Official explainers | <https://www.schwab.com/content/how-to-find-my-tax-documents>, <https://www.schwab.com/taxes/resources>, <https://www.schwab.com/resource/scfr_gain_loss> | **[official]** |
| 1099-B and cost basis background | <https://www.schwab.com/learn/story/form-1099-b-cost-basis-and-options-trading> | **[official]** |

**[community]** The Realized Gain/Loss CSV export is capped at **1,500 records**
per download — chunk by quarter for a high-volume account. Timing: 1099s arrive
in waves, with a second wave around mid-February and Form 5498 around mid-May.
<https://5280associates.com/retrieving-tax-documents-for-your-schwab-accounts/>

### So can this API serve the realized-gain estimation feed?

**Partly, and with caveats that must not be glossed over.**

What it *can* do: give you every `TRADE` in the current tax year, with symbol,
quantity, price, `positionEffect` (`OPENING`/`CLOSING`), fees, and net amount.
That is a genuine intra-year trading feed and it is what `schw transactions
list` returns.

What it *cannot* do:

- **No cost basis.** `transferItems[].cost` is the cash value of *this* trade's
  leg, not an acquisition basis. The name has misled people; the field is
  labelled accordingly in `schw`'s output docs.
- **No proceeds, no realized gain, no acquisition date, no wash-sale
  adjustment.** None of it is in any response.
- **Basis would have to be reconstructed** by lot-matching `TRADE` history —
  and that reconstruction is bounded by the ~1-year history ceiling, so
  anything held longer than a year cannot be matched at all.
- **Even a perfect reconstruction would not reconcile to the 1099-B**, because
  it cannot reproduce Schwab's lot-relief method or its wash-sale adjustments.

**Recommendation:** treat `schw transactions list` as an in-year *activity*
feed — "what did I sell this year, roughly what did it come to" — and treat the
portal's Realized Gain/Loss CSV as the authority for actual realized figures,
and the 1099 dashboard as the authority at filing time.

---

## 7. First-live-run checklist — completed 2026-08-12

Kept with its answers, since each line was once an open question:

- [x] `schw auth set-credential --stdin` accepts the portal's key/secret
      format — yes (a 48-char key / 64-char secret; community said 32/16)
- [x] `schw auth login` — the authorization URL opened Schwab's login and
      consent flow; no rejection for the missing `scope`/`state`
- [x] The pasted redirect URL's code exchanged successfully (the `%40` decode
      worked first try)
- [x] `schw auth status` showed the correct 7-day refresh deadline
- [x] `schw auth refresh` works and does **not** extend the 7-day deadline
      (identical `expires_at` before and after)
- [x] `schw accounts list` — `hashValue` (64 chars) and balances appeared;
      MARGIN uses `currentBalances.equity`, CASH has no `equity` key and
      needs `liquidationValue` (§5 table)
- [x] `schw positions list` — `fields=positions` behaves as documented, both
      with and without the parameter
- [x] `schw transactions list` — `types` is **optional**; `--all-types`
      works; comma-separated lists work
- [x] `schw transactions list --since <13 months ago>` — accepted without
      error; 121-day-old data returned in full, so the 60-day claim is dead;
      the true ceiling is older than the probed accounts' history and remains
      unknown (§5)
- [x] `startDate`/`endDate` accepted with milliseconds — yes, on every call
- [x] Response fields disagreeing with the old fixtures — many; the fixtures
      are now scrubbed captures and §5 records every difference. The one
      nobody predicted: the silent 3,000-row response cap.

---

## 8. Rate limits

**[community]** ~**120 requests per minute** per app, corroborated across
independent clients:
<https://github.com/slimandslam/schwab-client-js/blob/main/docs/SchwabConfig.md>,
<https://github.com/sudowealth/schwab-api>. Orders are additionally capped at
4,000/day **[community]**
(<https://www.quantconnect.com/docs/v2/cloud-platform/live-trading/brokerages/charles-schwab>).
Over-limit returns **HTTP 429** with codes `429-001` (rate) and `429-005`
(burst); back off ~60s.

**The authoritative official rate-limit page could not be retrieved — treat 120/min
as well-corroborated community consensus, not a quoted official figure.
UNVERIFIED.**

`schw` paces requests at ≥100ms apart as a burst guard. A CLI invocation makes
a few dozen requests at most, so the per-minute budget is never the binding
constraint.

**[live]** 2026-08-12: several hundred requests at the 100ms pacing across
the session produced zero 429s.

Prior in-house work against this API used a 10 requests/second client-side
limiter and observed no 429s; it also encoded **no retry logic**, which is
worth knowing if you add any.

---

## 9. Terms

**[official]** The Individual product is scoped to "your own brokerage trading
account". <https://developer.schwab.com/products/trader-api--individual>
Serving other people's Schwab accounts requires the Commercial product and a
separate review. <https://developer.schwab.com/products/trader-api--commercial>

**[community]** There is **no sandbox and no paper trading**. Applications
flagged as institutional use are reported to be rejected on the individual
track.

**UNVERIFIED:** the Data Accessor Agreement and market-data redistribution
terms could not be retrieved. Assume standard exchange redistribution
prohibitions apply to anything from the Market Data product (which `schw` does
not use).
