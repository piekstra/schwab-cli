# Contributing to schwab-cli

Thanks for your interest! Contributions — bug reports, fixes, new commands,
docs — are welcome under the project's dual [MIT](LICENSE-MIT) /
[Apache-2.0](LICENSE-APACHE) license: by opening a pull request you agree your
contribution is licensed under those same terms.

## Ground rules (this reads a real person's brokerage account)

- **Never commit secrets or personal data.** The Schwab app key/secret and the
  OAuth tokens live only in the OS keychain. Nothing credential-like or
  personal belongs in the repo, tests, fixtures, or commit messages.
- **Account numbers and account hashes are both identifiers.** The `hashValue`
  Schwab returns is not an anonymization — it is a stable handle to a specific
  brokerage account. Treat it exactly like an account number.
- **Fixtures are scrubbed live captures, with dummy values only.** They were
  captured on 2026-08-12. [`tests/fixtures/README.md`](tests/fixtures/README.md)
  has the scrubbing policy and the few shapes that remain doc-derived, and a
  test enforces the dummy shapes positively. If you refresh them from new
  captures, scrub them and update that file in the same commit.
- **Read-only by default.** Every command observes. Schwab's Trader API can
  place and cancel live orders; the guarantee that this tool cannot lives in
  `client::Schwab` (no verb but GET), the `api` verb guard, and the docs — all
  three change together or not at all.
- **Secrets never on argv or in logs.** Read them from the keychain or stdin.
  That includes the authorization `code`: it is a bearer credential for the
  ~30 seconds it lives, so `auth login` takes it on stdin, not as a flag value.
- **Cite your API facts.** Every claim in `docs/api.md` carries a source URL,
  and unverified ones are labelled as such. Don't add an uncited one.

## Dev loop

```console
$ make verify     # fmt-check + clippy -D warnings + tests + smoke — the local gate
$ make test       # unit + integration (fully offline; no network, no creds, never the real keychain)
$ make dev        # debug build, re-signed so keychain grants survive rebuilds
$ cargo run -- <command>
```

CI (`.github/workflows/ci.yml`) runs `cargo fmt --check`, clippy with
`-D warnings`, the tests and a release build, plus a cargo-audit / cargo-deny /
gitleaks job. It does not run `smoke`. A green `make verify` predicts a green
build job. `smoke` runs `auth status` against an empty keychain service
(`piekstra.schw.hermetic-tests`), so it never reads your real entries.

## Pull requests

1. Branch from `main`; keep the change focused.
2. Add or update tests. Contract tests load fixtures from disk — extend
   `tests/fixtures/` rather than inlining JSON in test code.
3. Keep the human table and the `--json` DTO in sync. A breaking DTO change
   bumps its `schema` version suffix.
4. Update [`docs/api.md`](docs/api.md) when you learn something new about the
   API, and the README when you change the command surface.
5. Run `make verify` and include what you tested.

## A note on the current state

**This CLI was verified live once, on 2026-08-12.** OAuth login, refresh, and
every read command ran against a real individual Trader API app and two linked
accounts, one CASH and one MARGIN. The fixtures are scrubbed from that run's
captures. [`docs/api.md`](docs/api.md) marks each claim **[live]** or gives its
documentary source, and §7 keeps the first-live-run checklist with its answers.

Anything `docs/api.md` does not mark **[live]** is documented-only. That
includes a 401 response body, a short position, how far back transaction
history reaches, and renewal under the token-refresh lock added in 0.1.3,
after that run. If you run `schw` against an approved app, settle what you can
of that and turn what you find into fixtures and doc corrections.

Three things to be careful with when you do:

- **Never paste a raw capture into an issue or a commit.** It carries account
  numbers, account hashes, positions, and balances.
- **Never paste an authorization code, an access token, or a refresh token**
  anywhere. The refresh token is a seven-day key to the account.
- **A silently truncated transaction list is the worst bug here** — it looks
  exactly like success, and it would understate realized gains on a tax return.
  Schwab silently caps a transactions response at 3,000 rows, keeping the
  newest; if you change anything near date-window chunking or the bisection of
  a capped window, check it against an account with more than 3,000 rows in a
  month.
