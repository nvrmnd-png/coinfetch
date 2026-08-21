
use serde_json::Value;

use crate::error::{Error, Result};

const BASE: &str = "https://mempool.space/api";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletBalance {
    pub address: String,
    pub confirmed_sats: i64,
    pub pending_sats: i64,
    pub tx_count: u64,
}

impl WalletBalance {

    pub fn total_sats(&self) -> i64 {
        self.confirmed_sats + self.pending_sats
    }
}

pub fn looks_like_btc_address(input: &str) -> bool {
    let address = input.trim();
    if address.len() < 26 || address.len() > 90 {
        return false;
    }
    if !address.chars().all(|c| c.is_ascii_alphanumeric()) {
        return false;
    }
    let lower = address.to_ascii_lowercase();
    lower.starts_with("bc1") || lower.starts_with("tb1") || {
        let first = address.as_bytes()[0];
        first == b'1' || first == b'3'
    }
}

pub fn parse_address(address: &str, value: &Value) -> Result<WalletBalance> {
    let field = |section: Option<&Value>, key: &str| -> i64 {
        section
            .and_then(|s| s.get(key))
            .and_then(Value::as_i64)
            .unwrap_or(0)
    };

    let chain = value
        .get("chain_stats")
        .ok_or_else(|| Error::msg("unexpected response from mempool.space"))?;
    let mempool = value.get("mempool_stats");

    let confirmed = field(Some(chain), "funded_txo_sum") - field(Some(chain), "spent_txo_sum");
    let pending = field(mempool, "funded_txo_sum") - field(mempool, "spent_txo_sum");

    let tx_count = chain
        .get("tx_count")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .saturating_add(
            mempool
                .and_then(|m| m.get("tx_count"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );

    Ok(WalletBalance {
        address: address.to_string(),
        confirmed_sats: confirmed,
        pending_sats: pending,
        tx_count,
    })
}

pub async fn fetch_balance(client: &reqwest::Client, address: &str) -> Result<WalletBalance> {
    let address = address.trim();
    if !looks_like_btc_address(address) {
        return Err(Error::msg(format!(
            "`{address}` does not look like a Bitcoin address (expected 1…, 3…, or bc1…)"
        )));
    }

    let response = client
        .get(format!("{BASE}/address/{address}"))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                Error::msg("mempool.space timed out")
            } else {
                Error::msg("could not reach mempool.space")
            }
        })?;

    let status = response.status();
    if !status.is_success() {

        let body = response.text().await.unwrap_or_default();
        let detail = body.trim();
        return Err(Error::msg(if detail.is_empty() {
            format!("mempool.space returned HTTP {}", status.as_u16())
        } else {
            format!("mempool.space: {detail}")
        }));
    }

    let value: Value = response
        .json()
        .await
        .map_err(|e| Error::msg(format!("malformed response from mempool.space: {e}")))?;

    parse_address(address, &value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADDRESS: &str = include_str!("../../tests/fixtures/mempool_address.json");

    #[test]
    fn computes_the_balance_from_a_real_response() {
        let value: Value = serde_json::from_str(ADDRESS).expect("fixture parses");
        let balance = parse_address("bc1qexample", &value).expect("balance");

        assert_eq!(
            balance.confirmed_sats,
            530_775_983_880_776 - 517_774_975_978_040
        );
        assert_eq!(balance.pending_sats, 0);
        assert_eq!(balance.tx_count, 338);
        assert_eq!(balance.total_sats(), balance.confirmed_sats);
    }

    #[test]
    fn counts_unconfirmed_funds_separately() {
        let value = serde_json::json!({
            "chain_stats": { "funded_txo_sum": 1_000, "spent_txo_sum": 400, "tx_count": 3 },
            "mempool_stats": { "funded_txo_sum": 250, "spent_txo_sum": 50, "tx_count": 1 }
        });
        let balance = parse_address("bc1qexample", &value).expect("balance");
        assert_eq!(balance.confirmed_sats, 600);
        assert_eq!(balance.pending_sats, 200);
        assert_eq!(balance.total_sats(), 800);
        assert_eq!(balance.tx_count, 4);
    }

    #[test]
    fn handles_an_address_that_has_never_been_used() {
        let value = serde_json::json!({
            "chain_stats": { "funded_txo_sum": 0, "spent_txo_sum": 0, "tx_count": 0 },
            "mempool_stats": { "funded_txo_sum": 0, "spent_txo_sum": 0, "tx_count": 0 }
        });
        let balance = parse_address("bc1qexample", &value).expect("balance");
        assert_eq!(balance.confirmed_sats, 0);
        assert_eq!(balance.tx_count, 0);
    }

    #[test]
    fn rejects_a_response_without_chain_stats() {
        let value = serde_json::json!({ "oops": true });
        assert!(parse_address("bc1qexample", &value).is_err());
    }

    #[test]
    fn accepts_the_common_address_formats() {
        assert!(looks_like_btc_address(
            "bc1qgdjqv0av3q56jvd82tkdjpy7gdp9ut8tlqmgrpmv24sq90ecnvqqjwvw97"
        ));
        assert!(looks_like_btc_address("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"));
        assert!(looks_like_btc_address("3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy"));
        assert!(looks_like_btc_address(
            "  1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa  "
        ));
    }

    #[test]
    fn rejects_obvious_non_addresses_without_a_request() {
        assert!(!looks_like_btc_address("notanaddress"));
        assert!(!looks_like_btc_address(""));
        assert!(!looks_like_btc_address(
            "0x71C7656EC7ab88b098defB751B7401B5f6d8976F"
        ));
        assert!(!looks_like_btc_address(
            "bc1qgdjqv0av3q56jvd82tkdjpy7gdp9ut8tlqmgrpmv24sq90ecnvqq!!!!!"
        ));
    }
}
