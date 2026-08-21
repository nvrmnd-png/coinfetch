mod api;
mod cache;
mod chain;
mod cli;
mod coins;
mod config;
mod error;
mod model;
mod secret;
mod ui;

use std::io::{self, IsTerminal};
use std::process::ExitCode;

use clap::Parser;

use crate::api::coingecko::{self, MAX_COINS};
use crate::cli::{Cli, Command};
use crate::config::{ChartRender, Config};
use crate::error::{Error, Result};

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("coinfetch: {err}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let (config, warnings) = Config::load();

    match cli.command {
        Some(Command::Config) => {

            let gecko = coingecko::client(config.api_key(), config.cache_ttl_secs)?;

            let graphics = ui::graphics::supported();

            ui::config_tui::run(config, gecko, warnings, graphics).await?;
            Ok(ExitCode::SUCCESS)
        }
        Some(Command::Wallet { ref address }) => {
            for warning in &warnings {
                eprintln!("coinfetch: {warning}");
            }
            show_wallet(&config, address).await
        }
        Some(Command::ForgetKey) => forget_key(),
        None => show_prices(&cli, &config, warnings).await,
    }
}

fn forget_key() -> Result<ExitCode> {
    use crate::secret::{KeyStore, Keyring, SERVICE};
    let store = Keyring;
    match store.load() {
        Ok(None) => {
            println!("no CoinGecko api key stored in the {SERVICE} keyring");
            Ok(ExitCode::SUCCESS)
        }
        Ok(Some(_)) => match store.clear() {
            Ok(()) => {
                println!("removed CoinGecko api key from the {SERVICE} keyring");
                Ok(ExitCode::SUCCESS)
            }
            Err(err) => Err(Error::msg(format!("could not clear the keyring: {err}"))),
        },
        Err(err) => Err(Error::msg(format!("could not reach the keyring: {err}"))),
    }
}

async fn show_prices(cli: &Cli, config: &Config, warnings: Vec<String>) -> Result<ExitCode> {
    let ids = match cli.coins.as_deref() {
        Some(text) => coins::parse_list(text),
        None => config
            .default_coins
            .iter()
            .map(|c| coins::resolve(c))
            .collect(),
    };

    if ids.is_empty() {
        return Err(Error::msg(
            "no coins given and the configured default list is empty — try `coinfetch bitcoin` or `coinfetch config`",
        ));
    }
    if ids.len() > MAX_COINS {
        return Err(Error::msg(format!(
            "{} coins requested, but each one costs a separate history request; \
             the CoinGecko free tier realistically supports {MAX_COINS} per run",
            ids.len()
        )));
    }

    let (colors, color_warnings) = config.colors();
    let gecko = coingecko::client(config.api_key(), config.cache_ttl_secs)?;

    let refused = config.chart_render.needs_graphics() && !ui::graphics::supported();

    let style = ui::chart::ChartStyle {

        render: if refused {
            ChartRender::Steps
        } else {
            config.chart_render
        },
        minimal: config.chart_minimal,
    };

    let data = gecko.fetch_market_data(&ids).await;
    let mut view = ui::chart::build(&data, &colors, style);

    let mut notes = warnings;
    notes.extend(color_warnings);

    if refused && io::stdout().is_terminal() {
        notes.push(
            "chart_render = lines is drawn as an image, which needs a terminal with kitty or \
             sixel graphics — falling back to the box-drawing stroke of chart_render = steps"
                .to_string(),
        );
    }

    notes.append(&mut view.notes);
    view.notes = notes;

    if style.minimal {
        for note in &view.notes {
            eprintln!("coinfetch: {note}");
        }
    }

    let charted_nothing = !view.has_chart();
    ui::oneshot::show(&mut view)?;

    Ok(if charted_nothing {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    })
}

async fn show_wallet(config: &Config, address: &str) -> Result<ExitCode> {
    match chain::detect(address) {
        Some(chain::Chain::Bitcoin) => show_bitcoin_wallet(config, address).await,
        Some(chain::Chain::Ethereum) => show_ethereum_wallet(config, address).await,
        None => Err(Error::msg(
            "Adressformat nicht erkannt, unterstützt: Bitcoin, Ethereum",
        )),
    }
}

async fn show_bitcoin_wallet(config: &Config, address: &str) -> Result<ExitCode> {
    let client = api::http_client()?;
    let balance = api::mempool::fetch_balance(&client, address).await?;

    let gecko = coingecko::client(config.api_key(), config.cache_ttl_secs)?;
    let btc_price = gecko
        .quotes_only(&["bitcoin".to_string()])
        .await
        .first()
        .map(|q| q.price);

    let total = balance.total_sats();
    let fiat = btc_price
        .map(|price| format!("  ({})", model::format_price(total as f64 / 1e8 * price)))
        .unwrap_or_default();

    println!("Bitcoin wallet");
    println!("  address   {}", balance.address);
    println!(
        "  balance   {} BTC{fiat}",
        model::format_btc(balance.confirmed_sats)
    );
    if balance.pending_sats != 0 {
        println!(
            "  pending   {} BTC (unconfirmed)",
            model::format_btc(balance.pending_sats)
        );
    }
    println!("  txs       {}", balance.tx_count);

    Ok(ExitCode::SUCCESS)
}

async fn show_ethereum_wallet(config: &Config, address: &str) -> Result<ExitCode> {
    let client = api::http_client()?;
    let balance = api::ethereum::fetch_balance(&client, address).await?;

    let gecko = coingecko::client(config.api_key(), config.cache_ttl_secs)?;
    let eth_price = gecko
        .quotes_only(&["ethereum".to_string()])
        .await
        .first()
        .map(|q| q.price);

    let eth = balance.wei as f64 / 1e18;
    let fiat = eth_price
        .map(|price| format!("  ({})", model::format_price(eth * price)))
        .unwrap_or_default();

    println!("Ethereum wallet");
    println!("  address   {}", balance.address);
    println!("  balance   {} ETH{fiat}", model::format_eth(balance.wei));

    Ok(ExitCode::SUCCESS)
}
