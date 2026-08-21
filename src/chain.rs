
use crate::api::{ethereum, mempool};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chain {
    Bitcoin,
    Ethereum,
}

pub fn detect(address: &str) -> Option<Chain> {
    let address = address.trim();
    if mempool::looks_like_btc_address(address) {
        Some(Chain::Bitcoin)
    } else if ethereum::looks_like_eth_address(address) {
        Some(Chain::Ethereum)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_bitcoin_addresses() {
        assert_eq!(
            detect("bc1qgdjqv0av3q56jvd82tkdjpy7gdp9ut8tlqmgrpmv24sq90ecnvqqjwvw97"),
            Some(Chain::Bitcoin)
        );
        assert_eq!(
            detect("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"),
            Some(Chain::Bitcoin)
        );
        assert_eq!(
            detect("3J98t1WpEZ73CNmQviecrnyiWrnqRhWNLy"),
            Some(Chain::Bitcoin)
        );
    }

    #[test]
    fn recognizes_ethereum_addresses() {
        assert_eq!(
            detect("0x71C7656EC7ab88b098defB751B7401B5f6d8976F"),
            Some(Chain::Ethereum)
        );
    }

    #[test]
    fn rejects_an_unrecognized_format_without_guessing() {
        assert_eq!(detect("not-an-address"), None);
        assert_eq!(detect(""), None);
    }
}
