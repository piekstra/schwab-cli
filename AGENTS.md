# AGENTS.md

Canonical guide for anyone — human or agent — working in this repo.

## What this is

`schw`, a piekstra-family CLI over Charles Schwab's Trader API
(`api.schwabapi.com`). Read-only. Conforms to
[piekstra-cli/1](https://github.com/piekstra/cli-common); the shared crates are
pinned to tag `v0.5.0`.

## The four facts that shape everything here

1. **Live verification happened on 2026-08-12** — OAuth login, refresh, and
   every read command ran against a real app and two real accounts, and the
   fixtures are scrubbed from those captures. `docs/api.md` marks what is
   live-verified versus documented-only; don't write "verified" about anything
   that wasn't actually run.
2. **Schwab silently caps a transactions response at 3,000 rows**, keeping the
   newest and dropping the oldest, with no error and no marker. Live-verified.
   `transactions.rs` bisects any capped window until complete; see the traps.
3. **Schwab's API has no tax documents and no realized gain/loss.** Not a gap in
   this CLI — a gap in the API. `docs/api.md` §6 has the enumeration and the
   citations. Do not add a `documents` command against a guessed endpoint.
4. **The refresh token dies after 7 days and cannot be renewed.** Every
   unattended-automation idea runs into this wall. It is Schwab policy.
   Live-verified: `auth refresh` does not move the deadline.

## Layout

| Path | |
|---|---|
| `src/main.rs` | clap tree, auth/config commands, exit-code mapping |
| `src/oauth.rs` | authorization URL, code exchange, refresh, error explanations |
| `src/credential.rs` | the two keychain entries; hand-written `Debug` impls |
| `src/client.rs` | GET-only HTTP client, token refresh, status→exit-code mapping |
| `src/config.rs` | non-secret settings (`base_url`, `redirect_uri`, `account`) |
| `src/parse.rs` | Schwab JSON → DTOs; every function total |
| `src/urlenc.rs` | percent encode/decode; the authorization-code trap lives here |
| `src/commands/` | one module per command group |
| `docs/api.md` | **the source of truth for API facts**, every claim cited |

## Conventions

- **`--json` on every command**, one `schema`-tagged DTO on stdout, exactly one
  JSON document per invocation. Human output → stdout, diagnostics → stderr.
- **Validate arguments before touching the keychain or the network.** `--help`
  and a typo must never prompt for keychain access or spend a token.
- **Exit codes:** 0 ok · 2 usage · 3 auth · 4 not found · 5 upstream · 6 refused.
- **ISO `YYYY-MM-DD` on every date flag.** Schwab's
  `yyyy-MM-dd'T'HH:mm:ss.SSSZ` is an internal detail.
- **Money as strings**, never floats, per the family output contract.
- Reach for `pk-cli-*` before writing a helper — dates, output, paging, range
  flags, secrets, and config are all already there.

## Traps, all of them paid for

**The authorization code must be URL-decoded exactly once.** Schwab documents
this. The code ends in `%40` → `@`; posting it still-encoded fails with an
error that looks like a malformed request. Decode with a real query-pair
parser, then let the HTTP layer re-encode when it builds the form body. Never
hand-glue the form body.

**`Approved - Pending` is not approved.** An app in that state fails the token
exchange with `invalid_client` even with correct credentials. `oauth.rs` says
so in the error text; keep it there.

**The redirect URI must match byte-for-byte** at registration, in the
authorization URL, and at the token exchange — a trailing slash breaks it.
`Config::redirect_uri()` deliberately does not normalize.

**Transactions truncate silently, two ways.** (1) A response holds at most
3,000 rows — the newest 3,000 of the window, the rest dropped without any
error or marker (live-verified 2026-08-12: a full-month window came back
missing its early days, with the oldest surviving day cut mid-day). (2) The
history ceiling: how far back data exists is undocumented; a too-old request
returns nothing rather than erroring. Live, 121-day-old data returned fine —
so the community's "60 days" is wrong — and 13-month-old requests were
accepted without error, but the true ceiling is unknown. This is why
`transactions.rs` chunks into calendar-month windows, **bisects any window
that comes back at the cap**, and warns past 12 months. **A silently
truncated transaction list is the worst bug this tool can have** — it looks
exactly like success and it understates realized gains. Treat changes near
the windowing logic with suspicion.

**A zero-width date window returns nothing.** `startDate == endDate` yields no
rows; windows span `T00:00:00.000Z` to `T23:59:59.999Z`. The `.999` matters:
Schwab's date grammar carries milliseconds, and ending a window at `.000`
would leave the following 999 ms owned by no window.

**`fields=positions` is mandatory** to get positions at all; without it the
array is absent, which looks like an empty portfolio.

**Schwab addresses accounts by `hashValue`, never account number.** The hash is
not an anonymization — treat it as an identifier.

**The published schema and live responses disagree.** Confirmed first-hand
2026-08-12: TRADE rows carry none of the promised `user`/`description`/
`settlementDate`/`activityType` fields (dividends carry two of them, plus an
undocumented `qualifiedDividend`); timestamps arrive as `…T14:25:19+0000`,
not the documented `.SSSZ`; ETFs report `assetType: COLLECTIVE_INVESTMENT`;
accounts arrive wrapped with an undocumented `aggregatedBalance`. That is why
`parse.rs` is total and omits what it cannot read rather than failing or
fabricating. Keep it that way; add an `Unknown`-style fallback rather than a
strict enum.

## Tests

Offline, always — no network, no keychain, no credential.

- `tests/cli_surface.rs` — black-box: every `--help` renders (catches a
  subcommand flag colliding with a global), exit codes, the `--json` error DTO,
  `info`, completions.
- `tests/fixture_shapes.rs` — contract tests over `tests/fixtures/`, plus a
  **positive** hygiene test asserting every identity-bearing value matches a
  dummy shape. Positive, not a denylist: a denylist of real account numbers
  would publish exactly what it exists to exclude.
- Unit tests inline in `#[cfg(test)] mod tests`.

Fixtures are **scrubbed live captures** (2026-08-12) —
`tests/fixtures/README.md` records what was captured, what remains
doc-derived, and the scrubbing policy, and a test enforces the dummy shapes.
If you refresh them from new captures, scrub them and update that file in the
same commit. Never commit a raw capture.

## Read-only, in three places that must agree

`client::Schwab` (no verb but `GET`), the `schw api` verb guard (exit 6), and
the claims in `README.md`/`AGENTS.md`. Schwab's write endpoints place real
orders with real money on the same host and token. Change one, change all three.

## Definition of done

`make verify` green, CI green, `docs/api.md` matching reality with every new
claim cited, and no secrets or personal data anywhere in the diff — fixtures
included. Dogfood through the installed binary; changes touching the network
surface should be exercised live when a valid session exists (`schw auth
status`), and labelled documented-only when one doesn't.
