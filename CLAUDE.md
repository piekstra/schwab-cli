# CLAUDE.md

The canonical agent guide is [AGENTS.md](AGENTS.md) — read it first. It has the
layout, the OAuth model, the conventions, and Schwab's traps.

Claude-specific notes:

- **`make verify` is the gate.** Run it before calling a change done; it's
  exactly what CI runs (`fmt-check` + `clippy -D warnings` + tests + smoke).
- **Live verification happened once, on 2026-08-12** — auth, refresh, and the
  read surface all ran against a real account, and the fixtures are scrubbed
  from those captures. Anything not exercised that day is still documented-only;
  `docs/api.md` says which is which. Don't write "verified" about anything you
  didn't actually run.
- **Tests are offline.** No test may touch the network, the keychain, or a
  credential. If you need new Schwab data, add a fixture and cite the doc URL
  in `tests/fixtures/README.md`.
- **This CLI is read-only, enforced in three places that must agree:**
  `client::Schwab` (no verb but GET), the `api` verb guard, and the claims in
  `README.md`/`AGENTS.md`. Change one, change all three. Schwab's Trader API
  places real orders with real money — this is not a theoretical boundary.
- **Never log a token, a code, or an account number.** `--verbose` prints
  method, path, and query parameters only. Account hashes are identifiers, not
  anonymizations; mask them wherever they are displayed.
- **Don't invent API facts.** Every claim in `docs/api.md` carries a source URL,
  and the unverified ones are labelled. Keep it that way — if you learn
  something new, cite it; if you can't, mark it unconfirmed.
- **Tax documents are not in this API.** The consolidated 1099 is portal-only
  (docs/api.md). Do not add a `documents` command against a guessed endpoint;
  that has been looked for and is not there.
- **A silently truncated transaction list is the worst bug this tool can have.**
  It looks like success and it understates realized gains. The date-window
  chunking logic and its tests exist for that reason; treat changes there with
  suspicion.
