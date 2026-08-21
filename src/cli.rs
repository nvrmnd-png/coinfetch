use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "coinfetch",
    version,
    about = "Crypto prices and wallet balances in your terminal",
    long_about = None,
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,

    #[arg(value_name = "COINS")]
    pub coins: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {

    Config,

    Wallet {

        address: String,
    },

    ForgetKey,
}
