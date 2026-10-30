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
- **Fixtures are hand-authored from published docs, with dummy values only.**
  See [`tests/fixtures/README.md`](tests/fixtures/README.md); a test enforces
  the rule positively. If you ever replace them with real captures, scrub them
  and say so there.
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
$ make verify     # fmt-check + clippy -D warnings + tests + smoke — the CI gate
$ make test       # unit + integration (fully offline; no network, no creds)
$ make dev        # debug build, re-signed so keychain grants survive rebuilds
$ cargo run -- <command>
```

`make verify` is exactly what CI runs; a green local run predicts a green CI run.

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

**This CLI has never made a live Schwab API call, and cannot yet.** Schwab's
Trader API requires a registered application that Schwab approves by hand; at
the time of writing this repo has no approved app, so every line here was
written from published documentation and prior art rather than from a real
response. See [`docs/api.md`](docs/api.md) for what that means field by field.

If you are the first person to run it against an approved app: please work
through the verification checklist in [`docs/api.md`](docs/api.md), and turn
whatever you find into fixtures and doc corrections. Expect to find something —
a renamed field, a date format that wants milliseconds, a page cap the docs
don't mention. That first run is the most valuable contribution this repo can
receive right now.

Three things to be careful with when you do:

- **Never paste a raw capture into an issue or a commit.** It carries account
  numbers, account hashes, positions, and balances.
- **Never paste an authorization code, an access token, or a refresh token**
  anywhere. The refresh token is a seven-day key to the account.
- **A silently truncated transaction list is the worst bug here** — it looks
  exactly like success, and it would understate realized gains on a tax return.
  Schwab caps a transactions query at a bounded window; if you change anything
  near date-window chunking, check it against an account with a long history.
