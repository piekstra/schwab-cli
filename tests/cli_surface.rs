//! Black-box tests over the built binary.
//!
//! Fully offline: no network, no keychain, no credential. Every test here runs
//! a command that either fails before touching either, or renders help. That
//! constraint is what makes this suite safe to run in CI on a machine with no
//! Schwab app at all.
//!
//! The `--help` sweep exists for a specific failure: a subcommand flag that
//! collides with a global (`-q`, `-v`, `--json`) makes clap panic at *runtime*,
//! not at compile time, so only actually rendering every help page catches it.

use assert_cmd::Command;
use predicates::prelude::*;

fn schw() -> Command {
    let mut cmd = Command::cargo_bin("schw").expect("binary builds");
    // Point the config at a scratch directory so a developer's real config
    // cannot influence a test result, and a test cannot influence their config.
    cmd.env("XDG_CONFIG_HOME", env!("CARGO_TARGET_TMPDIR"));
    cmd
}

// ------------------------------------------------------------------ help ----

#[test]
fn every_help_page_renders() {
    for args in [
        vec!["--help"],
        vec!["auth", "--help"],
        vec!["auth", "login", "--help"],
        vec!["auth", "status", "--help"],
        vec!["auth", "refresh", "--help"],
        vec!["auth", "logout", "--help"],
        vec!["auth", "set-credential", "--help"],
        vec!["config", "--help"],
        vec!["config", "set", "--help"],
        vec!["accounts", "--help"],
        vec!["accounts", "list", "--help"],
        vec!["positions", "--help"],
        vec!["positions", "list", "--help"],
        vec!["transactions", "--help"],
        vec!["transactions", "list", "--help"],
        vec!["api", "--help"],
        vec!["self-update", "--help"],
        vec!["completions", "--help"],
        vec!["info", "--help"],
    ] {
        schw()
            .args(&args)
            .assert()
            .success()
            .stdout(predicate::str::is_empty().not());
    }
}

#[test]
fn version_prints() {
    schw()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

/// The read-only claim must be visible in the top-level help, not only in the
/// docs — it is the first thing anyone pointing this at a brokerage account
/// wants to know.
#[test]
fn the_top_level_help_says_the_tool_is_read_only() {
    schw()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("Read-only"));
}

// ------------------------------------------------------------------ info ----

#[test]
fn info_emits_the_discovery_dto() {
    let out = schw().arg("info").assert().success();
    let v: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("info emits JSON");
    assert_eq!(v["schema"], "cli-info/v1");
    assert_eq!(v["name"], "schw");
    assert_eq!(v["spec"], "piekstra-cli/1");
    assert_eq!(v["auth"]["required"], true);
    // A driver reads this string to know the setup is an OAuth app, not a
    // password.
    assert_eq!(v["auth"]["method"], "oauth2");
    let caps = v["capabilities"].as_array().expect("capabilities");
    for expected in ["accounts", "positions", "transactions", "api"] {
        assert!(
            caps.iter().any(|c| c == expected),
            "missing capability {expected}"
        );
    }
}

// ------------------------------------------------------------------ auth ----

/// SPEC v1 §1.2: `auth status` answers logged out. It is the command a driver
/// runs to decide whether to log in, so it must never require being logged in.
#[test]
fn auth_status_works_logged_out_and_emits_the_canonical_dto() {
    let out = schw().args(["auth", "status", "--json"]).assert().success();
    let v: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("auth status emits JSON");
    assert_eq!(v["schema"], "auth-status/v1");
    assert_eq!(v["required"], true);
    assert!(v["authenticated"].is_boolean());
}

/// Exactly one JSON document per invocation (SPEC v1 §1.4).
#[test]
fn auth_status_json_is_a_single_document() {
    let out = schw().args(["auth", "status", "--json"]).assert().success();
    let stdout = String::from_utf8_lossy(&out.get_output().stdout);
    let mut stream = serde_json::Deserializer::from_str(&stdout).into_iter::<serde_json::Value>();
    assert!(stream.next().is_some(), "expected one document");
    assert!(stream.next().is_none(), "expected exactly one document");
}

// ----------------------------------------------------------- exit codes ----

/// An unknown subcommand is a usage error: exit 2, clap's convention.
#[test]
fn an_unknown_command_exits_2() {
    schw().arg("definitely-not-a-command").assert().code(2);
}

/// The read-only guarantee, checked through the CLI rather than the library.
/// Exit 6 distinguishes "this tool will not do that" from "you typed it wrong".
#[test]
fn a_write_verb_through_the_api_passthrough_is_refused_with_exit_6() {
    for verb in ["POST", "PUT", "DELETE", "patch"] {
        schw()
            .args(["api", verb, "/trader/v1/accounts"])
            .assert()
            .code(6)
            .stderr(predicate::str::contains("read-only"));
    }
}

/// The refusal must come from argument validation, before any keychain or
/// network access — so it holds even with no credential stored at all.
#[test]
fn the_write_refusal_is_reported_as_a_json_error_dto() {
    let out = schw()
        .args(["--json", "api", "POST", "/trader/v1/accounts"])
        .assert()
        .code(6);
    let v: serde_json::Value =
        serde_json::from_slice(&out.get_output().stdout).expect("error DTO is JSON");
    assert_eq!(v["error"]["code"], "confirmation_required");
    assert!(v["error"]["message"]
        .as_str()
        .unwrap()
        .contains("read-only"));
}

/// A malformed path is caught before the keychain, so this exits 2 rather than
/// 3 even though nothing is logged in.
#[test]
fn a_bad_api_path_is_a_usage_error_not_an_auth_error() {
    schw()
        .args(["api", "GET", "trader/v1/accounts"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("must start with"));
}

/// Argument validation must beat credential lookup everywhere, or `--help` and
/// typos start prompting for keychain access.
#[test]
fn an_inverted_date_range_is_a_usage_error_before_any_credential_is_read() {
    schw()
        .args([
            "transactions",
            "list",
            "--account",
            "DUMMYHASH",
            "--since",
            "2026-06-01",
            "--until",
            "2026-01-01",
        ])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("--since"));
}

#[test]
fn a_malformed_date_is_a_usage_error() {
    schw()
        .args([
            "transactions",
            "list",
            "--account",
            "DUMMYHASH",
            "--since",
            "june",
        ])
        .assert()
        .code(2);
}

/// With no account flag and no configured default, the missing-account message
/// must arrive as a usage error naming the command that finds one.
#[test]
fn a_missing_account_is_a_usage_error_that_names_the_fix() {
    schw()
        .args(["positions", "list"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("schw accounts list"));
}

// ---------------------------------------------------------------- config ----

#[test]
fn an_unknown_config_key_lists_the_known_ones() {
    schw()
        .args(["config", "set", "nonsense", "value"])
        .assert()
        .code(2)
        .stderr(predicate::str::contains("redirect_uri"));
}

#[test]
fn config_path_prints_a_path() {
    schw()
        .args(["config", "path"])
        .assert()
        .success()
        .stdout(predicate::str::contains("schw"));
}

// ----------------------------------------------------------- completions ----

#[test]
fn completions_render_for_every_supported_shell() {
    for shell in ["bash", "zsh", "fish"] {
        schw()
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(predicate::str::contains("schw"));
    }
}
