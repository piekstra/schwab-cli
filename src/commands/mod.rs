//! Domain command modules. Each read emits a `schema`-tagged DTO in `--json`
//! mode and a shaped table/kv view in text mode.

pub mod accounts;
pub mod api;
pub mod positions;
pub mod transactions;

use pk_cli_core::{CliError, CommonArgs};
use pk_cli_secrets::CredentialStore;

use crate::client::Schwab;
use crate::config::Config;

// The output contract lives in the shared crate; re-exported so the command
// modules keep their short `use super::{items, table_view}`.
pub use pk_cli_core::output::{rows_of as items, table_view};

/// Schwab Trader API paths the reads share.
pub const ACCOUNT_NUMBERS: &str = "/trader/v1/accounts/accountNumbers";
pub const ACCOUNTS: &str = "/trader/v1/accounts";

/// Per-account paths.
///
/// `hash` is Schwab's `hashValue`, never the plain account number — every
/// account-scoped endpoint rejects the latter. See docs/api.md.
pub fn account(hash: &str) -> String {
    format!("{ACCOUNTS}/{hash}")
}

pub fn account_transactions(hash: &str) -> String {
    format!("{ACCOUNTS}/{hash}/transactions")
}

pub struct Ctx<'a> {
    pub common: &'a CommonArgs,
    pub cfg: &'a Config,
    pub creds: &'a CredentialStore,
}

impl<'a> Ctx<'a> {
    /// A client with a live access token, refreshing it first if needed.
    pub fn client(&self) -> Result<Schwab<'a>, CliError> {
        Schwab::new(self.cfg, self.creds, self.common.verbose)
    }

    /// The account hash to act on: the flag, else the configured default.
    ///
    /// Deliberately does **not** silently pick the only account when there is
    /// one — that would work fine until the day a second account is opened and
    /// a command that used to mean "my brokerage account" starts meaning
    /// something else.
    pub fn account(&self, flag: Option<&str>) -> Result<String, CliError> {
        flag.map(String::from)
            .or_else(|| self.cfg.account())
            .ok_or_else(|| {
                CliError::Usage(
                    "no account given — run `schw accounts list` to see your account hashes, \
                     then pass `--account <HASH>` or set a default with \
                     `schw config set account <HASH>`"
                        .into(),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_paths_are_scoped_to_the_hash() {
        assert_eq!(account("ABC123"), "/trader/v1/accounts/ABC123");
        assert_eq!(
            account_transactions("ABC123"),
            "/trader/v1/accounts/ABC123/transactions"
        );
    }
}
