//! `schw positions list` — what the account currently holds.
//!
//! Positions are a *point-in-time* view and carry only **unrealized** P/L.
//! Nothing here contributes to a realized gain/loss figure, and the fields that
//! sound like they might — `longOpenProfitLoss`, `currentDayProfitLoss` — are
//! open-position marks that change with the market. The tax-relevant command is
//! `schw transactions list`.

use clap::Subcommand;
use pk_cli_core::{CliError, Paged};
use serde_json::Value;

use super::{account, table_view, Ctx};
use crate::parse;

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// List open positions (position-list/v1).
    #[command(visible_alias = "ls")]
    List(ListArgs),
}

#[derive(clap::Args, Debug)]
pub struct ListArgs {
    /// Account hash (from `schw accounts list`). Defaults to the account set
    /// with `schw config set account <HASH>`.
    #[arg(long, value_name = "HASH")]
    pub account: Option<String>,
}

pub fn run(ctx: &Ctx, cmd: &Cmd) -> Result<(), CliError> {
    let Cmd::List(args) = cmd;
    let hash = ctx.account(args.account.as_deref())?;

    // `fields=positions` is what makes the account payload include holdings at
    // all; without it Schwab returns balances only and the list comes back
    // empty, which looks exactly like "you hold nothing".
    let payload = ctx
        .client()?
        .get(&account(&hash), &[("fields", "positions".into())])?;

    let positions = parse::positions(&payload);
    let page = Paged::new("position", positions);
    if ctx.common.json {
        page.emit(true);
    } else {
        let v = serde_json::to_value(&page).unwrap_or(Value::Null);
        let rows = super::items(&v, "items");
        pk_cli_core::output::table(&table_view(
            &rows,
            &[
                "symbol",
                "asset_type",
                "quantity",
                "average_price",
                "market_value",
                "open_pl",
            ],
        ));
    }
    Ok(())
}
