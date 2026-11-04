# Changelog

All notable changes to `schwab-cli` are documented here. Newest first. The project
follows [SemVer](https://semver.org/); until 1.0, minors carry the breaking-change
signal.

## 0.2.2 — 2026-11-03

- `schwab_transactions` pages by position, so a transaction posted between pages repeats
  nothing and skips nothing. Rows of the same day now run in a fixed order of their own.
- `auth logout --forget` says what a removed connector token does not end: a running
  `schw mcp --http` keeps the token it started with.
- A code or a refresh token stays on Schwab's host. reqwest re-sends a body on a 307 or 308, so
  every request here now refuses a redirect to another host.
- On Windows, a session too long for one credential no longer fails at the end of a sign-in.
- cli-common v0.27.0, up from v0.22.0.

## 0.2.1 — 2026-11-03

- `auth logout --forget` also removes `mcp-http-token`, the `mcp --http` bearer token, so
  forgetting the CLI leaves no credential of its in the keychain.
- A release is published only once every asset is on it, so `self-update` never sends
  anyone to a release that lacks their platform. A build that failed leaves a draft.
- A release runs the binaries its runner can execute before packing them, so one that
  builds and then fails at startup is not published.
- cli-common v0.22.0, up from v0.21.0.

## 0.2.0 — 2026-11-03

- `schw mcp`: read-only tools for Claude, `schwab_portfolio` and
  `schwab_transactions`. They run over stdio for Claude Code and Claude Desktop, and
  with `--http` on 127.0.0.1 behind a bearer token for Claude on the web and mobile
  through your own Cloudflare Worker and tunnel. No tool can place, change or cancel an
  order or move money, and a test holds every request a tool makes to a list of four
  reads.
- Releases attach `schw-<version>.mcpb`, a Claude Desktop extension, and a Windows
  x86_64 tarball.
- A server reads the config and keychain on each call and renews under the same
  `RefreshLock` as every other `schw`, so two running copies do not both renew.
- `self-update` says to restart a running `schw mcp --http` and to install the new
  `.mcpb`.
- A 403 no longer spends a token renewal before it is reported: only a 401 is retried.
- cli-common v0.21.0, up from v0.20.0.
