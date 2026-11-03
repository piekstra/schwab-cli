# Changelog

All notable changes to `schwab-cli` are documented here. Newest first. The project
follows [SemVer](https://semver.org/); until 1.0, minors carry the breaking-change
signal.

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
