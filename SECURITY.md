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
- **The `--http` bearer token**, only if you use `schw mcp --http`. Stored under the
  same service, account `mcp-http-token`, or set as `$SCHW_MCP_TOKEN`; **a copy lives
  in your Cloudflare Worker**, as its `HOME_SCHWAB_TOKEN` secret. It admits a caller to
  the read-only tools. See [The MCP server](#the-mcp-server).
- **Not a secret:** `~/.config/schw/token-refresh.lock` is an empty file. `schw`
  processes lock it while renewing the session, so two running copies do not
  renew at the same time. That holds once every copy sharing the keychain item
  is 0.1.3 or later, including a process not yet restarted.
- **The authorization code**, for the ~30 seconds it lives. It is a bearer
  credential, which is why `auth login` reads it from stdin or a prompt and
  never as a flag value — argv is visible in the process table and in shell
  history.
- **The account data itself.** Not a credential, but a complete record of
  someone's holdings and trading. The commands send it to stdout and nowhere else: no
  telemetry, no caching to disk, no third parties. `schw mcp` sends it to the MCP
  client that called it, which is the point; see [The MCP server](#the-mcp-server).

`AppCredential` and `TokenSet` both implement `Debug` **by hand** so a stray
`{:?}` cannot print key or token material, and a test asserts it. Account keys
are masked to their last four characters wherever displayed.

## What this tool does not do

- **It never writes.** No orders, no order cancellation, no transfers, no
  profile edits. This is enforced in code, not just documented: `client::Schwab`
  exposes no HTTP verb but `GET`, and the `api` passthrough refuses every other
  verb with exit code 6. The only POST in this binary is `oauth.rs`'s, to the token
  endpoint, at sign-in and renewal.

  This matters more here than for most providers. Schwab's Trader API places
  live orders at the same host, with the same bearer token, that the reads use.
  The only thing between this binary and someone's money is the absence of a
  method on that struct.
- It talks only to the configured API host (`api.schwabapi.com` by default). The
  `base_url` config key exists for proxies and test servers; if it is set to
  something else, that is where your requests go — and your tokens with them.
- It hardcodes no secrets. `gitleaks` runs in CI over the full history, and a
  test refuses credential-shaped strings in the fixtures.

## The MCP server

`schw mcp` serves two read-only tools to an MCP client (Claude Code, Claude Desktop)
over stdio; the server opens no port unless run with `--http` (below). **What the tools
return goes to Claude on every path**, as part of your conversation: account types and
the last four digits of their numbers, equity, cash, positions, and transactions with
their amounts and descriptions. No tool returns a full account number or an account
hash.

No tool can place, change or cancel an order, move money or change anything at Schwab.
A tool reaches Schwab only through a seam with `get` and nothing else, over a client
with no other verb. Every path is built from fixed constants and a hash from Schwab's
own account list; an `account` argument only selects from that list, so what a model
passes cannot name another endpoint, even for a read. `--allow-writes` adds nothing,
since every tool is read-only. `src/mcp/tests.rs` holds all of this to the requests the
tools actually make.

The server reads the config and both keychain items on each call and holds no token
between calls, so a sign-in or a renewal by another `schw` applies on the next call. A
renewal it does itself is under the same lock as every other `schw`. It sends `base_url`
the tokens and the app secret as the commands do, from your own config file in
`~/.config/schw`; nothing in a working directory is read. A tool never signs in, prompts
or creates a keychain item: a missing or dead session is an error asking the user to
run `schw auth login --overwrite` in a terminal, or `schw auth set-credential --stdin
--overwrite` when the app registration is what is missing or refused. The Claude Desktop
extension (`.mcpb`) takes no settings and so holds no credential of its own; on macOS its
bundled binary needs the user's approval, once per item, to read the `piekstra.schw`
keychain items.

`schw mcp --http --port <PORT>` serves the same tools over HTTP for the user's own
Cloudflare Worker, which reaches it through the user's own Cloudflare Tunnel
(cli-common's `connector/`). It binds 127.0.0.1 only, with no flag for another address,
and refuses to start without a bearer token of 32 or more characters, from
`SCHW_MCP_TOKEN` or the keychain item `mcp-http-token` (`piekstra.schw`), which `schw mcp
--new-token` makes from 256 random bits and replaces only with `--overwrite`. Every
request must carry the token (compared in constant time) and a `Host` of `127.0.0.1` or
`localhost`, and must not carry an `Origin`; there are no CORS headers. The Schwab
tokens and app secret stay on your computer: the Worker sends tool calls and receives
results, and never sees them.

**Tool results also cross Cloudflare on this path**, through your tunnel and your
Worker, on their way to Claude. Who holds what, and what a leaked token can do, is in
the connector's
[threat model](https://github.com/piekstra/cli-common/blob/main/connector/README.md#threat-model).

`schw auth logout` removes the Schwab session and does not touch `mcp-http-token` or
stop `schw mcp --http`. To revoke the connector's access, run `schw mcp --new-token
--overwrite` and restart the server (`launchctl kickstart -k gui/$(id -u)/com.piekstra.schw-mcp-http`);
the old token works until the restart. To stop serving, run `launchctl bootout
gui/$(id -u)/com.piekstra.schw-mcp-http` and stop the tunnel.

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
