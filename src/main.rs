//! `schw` — piekstra-family CLI for Charles Schwab (`api.schwabapi.com`).
//!
//! Conforms to piekstra-cli/1. Read-only: every command observes, none mutate.
//! The read-only guarantee is enforced in code — [`schw::client::Schwab`]
//! exposes no verb but GET, and `schw api` refuses anything else — not merely
//! asserted in the docs. That matters here more than for most providers: the
//! same host and token that read a position will also place a live order.
//!
//! **Nothing in this binary has been run against a real Schwab account.** It
//! requires an application approved by Schwab, which takes days; see
//! `docs/api.md`.

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use pk_cli_auth::{AuthStatus, LogoutArgs, SecretSourceArgs, SetCredentialArgs};
use pk_cli_config::ConfigStore;
use pk_cli_core::info::{AuthInfo, CliInfo};
use pk_cli_core::{dates, output, CliError, CommonArgs};
use pk_cli_http::ApiArgs;
use pk_cli_secrets::CredentialStore;
use pk_cli_selfupdate::{SelfUpdateArgs, Updater};

use schw::client::Schwab;
use schw::commands::{accounts, api, positions, transactions, Ctx, ACCOUNT_NUMBERS};
use schw::config::{self, Config, KEYCHAIN_APP, KEYCHAIN_TOKENS};
use schw::credential::{self, AppCredential};
use schw::oauth;

const BIN: &str = "schw";
const REPO: &str = "piekstra/schwab-cli";

/// Charles Schwab brokerage accounts, positions, and transaction history from
/// the command line — the raw material for realized gain/loss. Read-only.
/// Unofficial.
#[derive(Parser, Debug)]
#[command(name = BIN, version, about, long_about = None)]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// OAuth app credential, login, and session status.
    #[command(subcommand)]
    Auth(AuthCmd),
    /// Non-secret settings.
    #[command(subcommand)]
    Config(ConfigCmd),
    /// Linked brokerage accounts and the hashes they are addressed by.
    #[command(subcommand)]
    Accounts(accounts::Cmd),
    /// Open positions and their unrealized P/L.
    #[command(subcommand)]
    Positions(positions::Cmd),
    /// Transaction history — the realized-gains estimation feed.
    #[command(subcommand, visible_alias = "txns")]
    Transactions(transactions::Cmd),
    /// Raw Schwab API passthrough (GET only — this CLI is read-only).
    Api(ApiArgs),
    /// Update to the latest release from GitHub.
    SelfUpdate(SelfUpdateArgs),
    /// Print a shell completion script.
    Completions { shell: Shell },
    /// Machine-readable capability discovery (cli-info/v1).
    Info,
}

#[derive(Subcommand, Debug)]
enum AuthCmd {
    /// Authorize this app in a browser and store the resulting tokens.
    Login(LoginArgs),
    /// Report credential and session state (auth-status/v1).
    Status,
    /// Trade the refresh token for a new access token, now.
    Refresh,
    /// Clear the session; --forget also removes the app credential.
    Logout(LogoutArgs),
    /// Store the app key/secret from developer.schwab.com (alias: set-app).
    #[command(visible_alias = "set-app")]
    SetCredential(SetCredentialArgs),
}

#[derive(clap::Args, Debug)]
struct LoginArgs {
    /// Print the authorization URL and exit, without waiting for the redirect.
    /// Complete the login afterwards with `schw auth login --stdin`.
    #[arg(long)]
    print_url: bool,

    /// Open the authorization URL in the default browser.
    #[arg(long)]
    open: bool,

    /// Where to read the redirected URL from. The authorization code is a
    /// bearer credential, so it is never accepted as a flag value.
    #[command(flatten)]
    source: SecretSourceArgs,

    /// Skip the verification request after storing the tokens.
    #[arg(long)]
    no_verify: bool,

    /// Replace an existing session without asking.
    #[arg(long)]
    overwrite: bool,

    /// Never prompt; requires --stdin or --from-env.
    #[arg(long)]
    non_interactive: bool,
}

#[derive(Subcommand, Debug)]
enum ConfigCmd {
    /// Print the resolved config file path.
    Path,
    /// Show the effective configuration.
    Show,
    /// Set a config key (base_url, redirect_uri, account).
    Set { key: String, value: String },
    /// Remove a config key.
    Unset { key: String },
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(&cli) {
        std::process::exit(output::fail(&e, cli.common.json));
    }
}

fn run(cli: &Cli) -> Result<(), CliError> {
    let store = ConfigStore::new(BIN);
    let creds = CredentialStore::for_binary(BIN);
    let cfg: Config = store.load()?;
    let ctx = Ctx {
        common: &cli.common,
        cfg: &cfg,
        creds: &creds,
    };

    match &cli.command {
        Command::Auth(cmd) => auth(cli, cmd, &store, &creds, &cfg),
        Command::Config(cmd) => config_cmd(cli, cmd, &store),
        Command::Accounts(cmd) => accounts::run(&ctx, cmd),
        Command::Positions(cmd) => positions::run(&ctx, cmd),
        Command::Transactions(cmd) => transactions::run(&ctx, cmd),
        Command::Api(args) => api::run(&ctx, args),
        Command::SelfUpdate(args) => Updater {
            repo: REPO.into(),
            binary: BIN.into(),
            target: env!("BUILD_TARGET").into(),
            current: env!("CARGO_PKG_VERSION").into(),
        }
        .run(args, cli.common.json, cli.common.quiet),
        Command::Completions { shell } => {
            clap_complete::generate(*shell, &mut Cli::command(), BIN, &mut std::io::stdout());
            Ok(())
        }
        Command::Info => {
            let info = CliInfo::new(
                BIN,
                env!("CARGO_PKG_VERSION"),
                &format!("https://github.com/{REPO}"),
                AuthInfo {
                    required: true,
                    // `auth status`'s AuthMethod enum has no OAuth variant, so
                    // the DTO reports the nearest one. `info.auth.method` is a
                    // free string and is the value a driver should trust: the
                    // credential is an OAuth2 app registration plus a browser
                    // login, and the setup differs from a password entirely.
                    method: "oauth2".into(),
                    login_hint: Some(format!("{BIN} auth login")),
                },
                &["accounts", "positions", "transactions", "api"],
            );
            output::json(&serde_json::to_value(&info).unwrap());
            Ok(())
        }
    }
}

fn auth(
    cli: &Cli,
    cmd: &AuthCmd,
    store: &ConfigStore,
    creds: &CredentialStore,
    cfg: &Config,
) -> Result<(), CliError> {
    match cmd {
        AuthCmd::Login(args) => login(cli, args, creds, cfg),
        AuthCmd::Status => status(cli, creds, cfg),
        AuthCmd::Refresh => refresh(cli, creds, cfg),
        AuthCmd::Logout(args) => {
            creds.delete(KEYCHAIN_TOKENS)?;
            if args.forget {
                // The app credential is deliberately *not* dropped by a plain
                // logout. Re-registering an app costs days in Schwab's approval
                // queue, so throwing one away has to be asked for explicitly.
                creds.delete(KEYCHAIN_APP)?;
                store.clear()?;
                if !cli.common.quiet {
                    eprintln!("session cleared, app credential removed, config cleared");
                }
            } else if !cli.common.quiet {
                eprintln!(
                    "session cleared. The app key/secret are still stored — \
                     use `schw auth logout --forget` to remove them too."
                );
            }
            Ok(())
        }
        AuthCmd::SetCredential(args) => {
            if creds.get(KEYCHAIN_APP)?.is_some() && !args.overwrite {
                return Err(CliError::Usage(
                    "an app credential is already stored; pass --overwrite to replace it".into(),
                ));
            }
            let secret = args.source.read(Some(
                "Schwab app key and secret (APPKEY:APPSECRET, or the JSON form)",
            ))?;
            // Parse before storing: a malformed credential that only fails on
            // first use is far worse to debug than one rejected at write time,
            // especially when the provider's answer is a bare `invalid_client`.
            let app = AppCredential::parse(secret.expose())?;
            credential::write_app(creds, &app)?;
            if !cli.common.quiet {
                eprintln!(
                    "app credential {} stored in the OS keychain ({})",
                    app.masked_key(),
                    creds.service()
                );
                eprintln!("next: run `schw auth login` to authorize it in a browser");
            }
            Ok(())
        }
    }
}

/// The authorization-code flow, in one or two invocations.
///
/// Schwab requires an **https** callback, so nothing can usefully listen on it
/// without a self-signed certificate and a browser warning. The redirect
/// carries the code in its URL either way, so this asks for that URL instead.
/// See `schw::oauth` for the full reasoning.
fn login(
    cli: &Cli,
    args: &LoginArgs,
    creds: &CredentialStore,
    cfg: &Config,
) -> Result<(), CliError> {
    let app = credential::require_app(creds)?;
    let redirect_uri = cfg.redirect_uri();
    let url = oauth::authorize_url(&cfg.base_url(), &app.app_key, &redirect_uri);

    // `--print-url` is the non-interactive half of the flow: print, exit, and
    // let a later `--stdin` invocation finish the job. The authorization URL
    // carries no per-attempt state, so splitting it across two processes is
    // safe in a way a PKCE flow would not be.
    if args.print_url {
        println!("{url}");
        return Ok(());
    }

    if credential::read_tokens(creds)?.is_some() && !args.overwrite {
        return Err(CliError::Usage(
            "a session is already stored; pass --overwrite to replace it (or run \
             `schw auth refresh` to renew it without a browser)"
                .into(),
        ));
    }

    if !cli.common.quiet {
        eprintln!("1. Open this URL and sign in to Schwab, then select the accounts to link:\n");
        eprintln!("   {url}\n");
        eprintln!(
            "2. Your browser will be redirected to {redirect_uri} and will most likely show a \
             connection error — that is expected and harmless; nothing is listening there.\n"
        );
        eprintln!("3. Copy the full URL out of the address bar and paste it below.");
        eprintln!(
            "   It contains a single-use code that Schwab expires in about 30 seconds, so \
             paste promptly.\n"
        );
    }
    if args.open {
        open_browser(&url, cli.common.quiet);
    }

    let prompt = if args.non_interactive {
        None
    } else {
        Some("Redirected URL")
    };
    let pasted = args.source.read(prompt)?;
    let code = oauth::code_from_input(pasted.expose())?;

    let http = pk_cli_http::client(BIN, env!("CARGO_PKG_VERSION"))?;
    let tokens = oauth::exchange_code(
        &http,
        &cfg.base_url(),
        &app,
        &code,
        &redirect_uri,
        oauth::now_unix(),
    )?;
    credential::write_tokens(creds, &tokens)?;

    if !cli.common.quiet {
        eprintln!(
            "logged in. Access token valid ~30 minutes (refreshed automatically); \
             browser re-authorization required by {}.",
            dates::fmt_iso(dates::civil_from_unix(tokens.refresh_expires_at as i64))
        );
    }

    if !args.no_verify {
        // A stored token Schwab rejects is worse than no token at all. This is
        // the one cheap read that proves the whole chain: app approved, token
        // accepted, account linked.
        let client = Schwab::new(cfg, creds, cli.common.verbose)?;
        client.get(ACCOUNT_NUMBERS, &[])?;
        if !cli.common.quiet {
            eprintln!("verified: Schwab accepted the token and returned your linked accounts");
        }
    }
    Ok(())
}

/// Best-effort browser launch. A failure here is a convenience lost, not an
/// error — the URL was already printed.
fn open_browser(url: &str, quiet: bool) {
    #[cfg(target_os = "macos")]
    let launcher = "open";
    #[cfg(target_os = "linux")]
    let launcher = "xdg-open";
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let launcher = "";

    if launcher.is_empty() {
        return;
    }
    if std::process::Command::new(launcher)
        .arg(url)
        .status()
        .is_err()
        && !quiet
    {
        eprintln!("could not open a browser automatically — open the URL above by hand");
    }
}

fn refresh(cli: &Cli, creds: &CredentialStore, cfg: &Config) -> Result<(), CliError> {
    let app = credential::require_app(creds)?;
    let tokens = credential::read_tokens(creds)?
        .ok_or_else(|| CliError::Auth("not logged in — run `schw auth login`".into()))?;
    let now = oauth::now_unix();
    if tokens.refresh_expired(now) {
        return Err(CliError::Auth(
            "the refresh token has expired (they last 7 days and cannot be renewed) — \
             run `schw auth login`"
                .into(),
        ));
    }
    let http = pk_cli_http::client(BIN, env!("CARGO_PKG_VERSION"))?;
    let fresh = oauth::refresh_tokens(
        &http,
        &cfg.base_url(),
        &app,
        &tokens.refresh_token,
        now,
        tokens.refresh_expires_at,
    )?;
    credential::write_tokens(creds, &fresh)?;
    if !cli.common.quiet {
        eprintln!(
            "access token refreshed; browser re-authorization still required by {}",
            dates::fmt_iso(dates::civil_from_unix(fresh.refresh_expires_at as i64))
        );
    }
    Ok(())
}

/// `auth status`, which must work logged out — and must work with an
/// unreachable keychain, since that is exactly when a driver most needs to know
/// what is wrong.
///
/// Spends no request: everything reported is derived from what is stored. The
/// access token's expiry is knowable offline, and the refresh token's deadline
/// is the number that actually matters, because reaching it means a human has
/// to open a browser.
fn status(cli: &Cli, creds: &CredentialStore, cfg: &Config) -> Result<(), CliError> {
    let (app_raw, app_note) = credential::peek(creds, KEYCHAIN_APP);
    let (token_raw, token_note) = credential::peek(creds, KEYCHAIN_TOKENS);
    let app = app_raw
        .as_ref()
        .and_then(|s| AppCredential::parse(s.expose()).ok());
    let tokens = token_raw
        .as_ref()
        .and_then(|s| schw::credential::TokenSet::parse(s.expose()).ok());
    let now = oauth::now_unix();

    // "Authenticated" means a browser round trip is *not* needed. An expired
    // access token still counts, because the CLI renews it silently; an expired
    // refresh token does not, because only a human can fix that.
    let authenticated = tokens.as_ref().is_some_and(|t| !t.refresh_expired(now));

    let mut st = AuthStatus::new(true, authenticated, pk_cli_auth::AuthMethod::Password);
    st.username = app.as_ref().map(|a| a.masked_key());
    st.account = cfg.account();
    st.credential_in_keychain = Some(app.is_some());
    st.session_valid = Some(tokens.as_ref().is_some_and(|t| !t.access_expired(now)));
    // The deadline worth surfacing is the refresh token's: it is the one that
    // costs a human a browser.
    st.expires_at = tokens
        .as_ref()
        .map(|t| dates::fmt_rfc3339(t.refresh_expires_at as i64));
    st.emit(cli.common.json);

    if !cli.common.quiet {
        for note in [app_note, token_note].into_iter().flatten() {
            eprintln!("{note}");
        }
        match (&app, &tokens) {
            (None, _) => eprintln!(
                "no app credential stored — register an app at https://developer.schwab.com, \
                 then run `schw auth set-credential --stdin` (see docs/api.md)"
            ),
            (Some(_), None) => eprintln!("app credential stored; run `schw auth login`"),
            (Some(_), Some(t)) if t.refresh_expired(now) => eprintln!(
                "the 7-day refresh token has expired — run `schw auth login` to authorize \
                 again in a browser"
            ),
            (Some(_), Some(t)) => {
                let days = (t.refresh_expires_at.saturating_sub(now)) / 86_400;
                eprintln!("browser re-authorization needed in ~{days} day(s)");
            }
        }
    }
    Ok(())
}

fn config_cmd(cli: &Cli, cmd: &ConfigCmd, store: &ConfigStore) -> Result<(), CliError> {
    match cmd {
        ConfigCmd::Path => {
            println!("{}", store.path()?.display());
            Ok(())
        }
        ConfigCmd::Show => {
            let cfg: Config = store.load()?;
            let v = serde_json::to_value(&cfg).unwrap_or_default();
            if cli.common.json {
                output::json(&v);
            } else {
                output::render(&v);
            }
            Ok(())
        }
        ConfigCmd::Set { key, value } => {
            let mut cfg: Config = store.load()?;
            match key.as_str() {
                "base_url" => cfg.base_url = Some(value.clone()),
                "redirect_uri" => cfg.redirect_uri = Some(value.clone()),
                "account" => cfg.account = Some(value.clone()),
                other => return Err(unknown_key(other)),
            }
            store.save(&cfg)
        }
        ConfigCmd::Unset { key } => {
            let mut cfg: Config = store.load()?;
            match key.as_str() {
                "base_url" => cfg.base_url = None,
                "redirect_uri" => cfg.redirect_uri = None,
                "account" => cfg.account = None,
                other => return Err(unknown_key(other)),
            }
            store.save(&cfg)
        }
    }
}

fn unknown_key(key: &str) -> CliError {
    CliError::Usage(format!(
        "unknown config key `{key}` (known: {})",
        config::KNOWN_KEYS.join(", ")
    ))
}
