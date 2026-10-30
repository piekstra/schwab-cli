# Security Policy

## Reporting a vulnerability

Please report security issues privately via GitHub's
[security advisories](https://github.com/piekstra/schwab-cli/security/advisories/new)
rather than a public issue.

## Threat model

`schw` authenticates to Charles Schwab on behalf of one account holder and reads
their brokerage accounts, positions, and full transaction history. What's worth
protecting:

- **The app credential** (app key + app secret from developer.schwab.com).
  Stored in the OS keychain under service `piekstra.schw`, account
  `app-credentials`, as one JSON blob. It is the client half of the OAuth
  registration: with it and a valid code, an attacker can mint tokens.
- **The OAuth tokens.** Stored under the same service, account `oauth-tokens`.
  The access token lasts ~30 minutes; **the refresh token is a 7-day key to the
  account** and is the most valuable secret this tool holds.
- **The authorization code**, for the ~30 seconds it lives. It is a bearer
  credential, which is why `auth login` reads it from stdin or a prompt and
  never as a flag value — argv is visible in the process table and in shell
  history.
- **The account data itself.** Not a credential, but a complete record of
  someone's holdings and trading. It goes to stdout and nowhere else: no
  telemetry, no caching to disk, no third parties.

`AppCredential` and `TokenSet` both implement `Debug` **by hand** so a stray
`{:?}` cannot print key or token material, and a test asserts it. Account keys
are masked to their last four characters wherever displayed.

## What this tool does not do

- **It never writes.** No orders, no order cancellation, no transfers, no
  profile edits. This is enforced in code, not just documented: `client::Schwab`
  exposes no HTTP verb but `GET`, and the `api` passthrough refuses every other
  verb with exit code 6. There is no code path that can POST.

  This matters more here than for most providers. Schwab's Trader API places
  live orders at the same host, with the same bearer token, that the reads use.
  The only thing between this binary and someone's money is the absence of a
  method on that struct.
- It talks only to the configured API host (`api.schwabapi.com` by default). The
  `base_url` config key exists for proxies and test servers; if it is set to
  something else, that is where your requests go — and your tokens with them.
- It hardcodes no secrets. `gitleaks` runs in CI over the full history, and a
  test refuses credential-shaped strings in the fixtures.

## A note on scope

Schwab's OAuth scheme advertises `scope=readonly`, but the token it issues comes
back with `scope=api` regardless — so a read-only scope cannot be relied on as a
security boundary. That is precisely why the guarantee above is enforced
client-side instead. See `docs/api.md` §4.

There is no way to provision a read-only Schwab app. If you want a smaller blast
radius, the lever available is which accounts you tick during the consent step
of `schw auth login` — accounts not selected there are invisible to the API.

## Handling the credential safely

Pipe it; never paste it as an argument, where it would land in shell history and
in the process table:

```console
$ schw auth set-credential --stdin      # then paste APPKEY:APPSECRET, Ctrl-D
```

From a password manager:

```console
$ op read 'op://Private/schwab-app/credential' \
    | schw auth set-credential --stdin --overwrite
```

Or from the environment, for headless setup:

```console
$ SCHW_APP="$(cat app.json)" schw auth set-credential --from-env SCHW_APP
```

`schw auth logout` clears the session but **keeps** the app credential — losing
it means re-registering an app and waiting days for Schwab's approval queue.
`schw auth logout --forget` removes both and clears the config.

Rotating the app secret is `auth set-credential --stdin --overwrite`; also
rotate it in the Schwab developer portal, since replacing the local copy does
not.

## A note on macOS keychain grants

`cargo install` ad-hoc signs the binary, minting a new code identity each time,
and macOS scopes "Always Allow" grants to that identity — so a plain reinstall
silently revokes the grant. `make install` re-signs with the family's stable
identity to avoid this. That is a usability fix, not a security boundary: the
grant still gates keychain access.
