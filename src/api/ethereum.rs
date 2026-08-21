
use serde_json::{Value, json};

use crate::error::{Error, Result};

const RPC_URL: &str = "https://ethereum-rpc.publicnode.com";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletBalance {
    pub address: String,
    pub wei: u128,
}

pub fn looks_like_eth_address(input: &str) -> bool {
    let address = input.trim();
    let Some(hex) = address
        .strip_prefix("0x")
        .or_else(|| address.strip_prefix("0X"))
    else {
        return false;
    };
    hex.len() == 40 && hex.chars().all(|c| c.is_ascii_hexdigit())
}

fn parse_balance(address: &str, value: &Value) -> Result<WalletBalance> {
    if let Some(err) = value.get("error") {
        let message = err
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("RPC error");
        return Err(Error::msg(format!("ethereum RPC: {message}")));
    }

    let hex = value
        .get("result")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::msg("unexpected response from the Ethereum RPC endpoint"))?;
    let hex = hex.strip_prefix("0x").unwrap_or(hex);
    let wei = u128::from_str_radix(hex, 16)
        .map_err(|_| Error::msg("malformed balance in the Ethereum RPC response"))?;

    Ok(WalletBalance {
        address: address.to_string(),
        wei,
    })
}

pub async fn fetch_balance(client: &reqwest::Client, address: &str) -> Result<WalletBalance> {
    let address = address.trim();
    if !looks_like_eth_address(address) {
        return Err(Error::msg(format!(
            "`{address}` does not look like an Ethereum address (expected 0x followed by 40 hex characters)"
        )));
    }

    let body = json!({
        "jsonrpc": "2.0",
        "method": "eth_getBalance",
        "params": [address, "latest"],
        "id": 1,
    });

    let response = client.post(RPC_URL).json(&body).send().await.map_err(|e| {
        if e.is_timeout() {
            Error::msg("the Ethereum RPC endpoint timed out")
        } else {
            Error::msg("could not reach the Ethereum RPC endpoint")
        }
    })?;

    let status = response.status();
    if !status.is_success() {
        return Err(Error::msg(format!(
            "the Ethereum RPC endpoint returned HTTP {}",
            status.as_u16()
        )));
    }

    let value: Value = response.json().await.map_err(|e| {
        Error::msg(format!(
            "malformed response from the Ethereum RPC endpoint: {e}"
        ))
    })?;

    parse_balance(address, &value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_well_formed_address() {
        assert!(looks_like_eth_address(
            "0x71C7656EC7ab88b098defB751B7401B5f6d8976F"
        ));
        assert!(looks_like_eth_address(
            "  0x71C7656EC7ab88b098defB751B7401B5f6d8976F  "
        ));
    }

    #[test]
    fn rejects_obvious_non_addresses_without_a_request() {
        assert!(!looks_like_eth_address("notanaddress"));
        assert!(!looks_like_eth_address(""));
        assert!(!looks_like_eth_address(
            "0x71C7656EC7ab88b098defB751B7401B5f6d897"
        ));
        assert!(!looks_like_eth_address(
            "0x71C7656EC7ab88b098defB751B7401B5f6d8976FFF"
        ));
        assert!(!looks_like_eth_address(
            "bc1qgdjqv0av3q56jvd82tkdjpy7gdp9ut8tlqmgrpmv24sq90ecnvqqjwvw97"
        ));
        assert!(!looks_like_eth_address(
            "0xZZC7656EC7ab88b098defB751B7401B5f6d8976"
        ));
    }

    #[test]
    fn parses_a_hex_balance_from_a_real_response() {
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "result": "0x1a055690d9db80000"
        });
        let balance = parse_balance("0xabc", &value).expect("balance");
        assert_eq!(balance.wei, 0x1a055690d9db80000);
        assert_eq!(balance.address, "0xabc");
    }

    #[test]
    fn handles_a_zero_balance() {
        let value = serde_json::json!({ "jsonrpc": "2.0", "id": 1, "result": "0x0" });
        let balance = parse_balance("0xabc", &value).expect("balance");
        assert_eq!(balance.wei, 0);
    }

    #[test]
    fn surfaces_an_rpc_error_instead_of_a_parse_failure() {
        let value = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "error": { "code": -32602, "message": "invalid argument 0" }
        });
        let err = parse_balance("0xabc", &value).unwrap_err();
        assert!(err.to_string().contains("invalid argument 0"), "{err}");
    }

    #[test]
    fn rejects_a_response_without_a_result_or_error() {
        let value = serde_json::json!({ "oops": true });
        assert!(parse_balance("0xabc", &value).is_err());
    }
}
