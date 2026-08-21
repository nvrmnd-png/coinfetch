
const ALIASES: &[(&str, &str)] = &[
    ("btc", "bitcoin"),
    ("xbt", "bitcoin"),
    ("eth", "ethereum"),
    ("usdt", "tether"),
    ("usdc", "usd-coin"),
    ("bnb", "binancecoin"),
    ("sol", "solana"),
    ("xrp", "ripple"),
    ("ada", "cardano"),
    ("doge", "dogecoin"),
    ("trx", "tron"),
    ("avax", "avalanche-2"),
    ("dot", "polkadot"),
    ("matic", "matic-network"),
    ("pol", "polygon-ecosystem-token"),
    ("link", "chainlink"),
    ("ltc", "litecoin"),
    ("bch", "bitcoin-cash"),
    ("xlm", "stellar"),
    ("atom", "cosmos"),
    ("uni", "uniswap"),
    ("etc", "ethereum-classic"),
    ("xmr", "monero"),
    ("near", "near"),
    ("apt", "aptos"),
    ("arb", "arbitrum"),
    ("op", "optimism"),
    ("fil", "filecoin"),
    ("icp", "internet-computer"),
    ("hbar", "hedera-hashgraph"),
    ("vet", "vechain"),
    ("algo", "algorand"),
    ("aave", "aave"),
    ("shib", "shiba-inu"),
    ("pepe", "pepe"),
    ("sui", "sui"),
    ("ton", "the-open-network"),
    ("dai", "dai"),
];

pub fn resolve(input: &str) -> String {
    let key = input.trim().to_ascii_lowercase();
    for (alias, id) in ALIASES {
        if *alias == key {
            return (*id).to_string();
        }
    }
    key
}

pub fn parse_list(input: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for part in input.split(',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let id = resolve(trimmed);
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_known_symbols_case_and_space_insensitively() {
        assert_eq!(resolve("btc"), "bitcoin");
        assert_eq!(resolve("BTC"), "bitcoin");
        assert_eq!(resolve("  btc  "), "bitcoin");
        assert_eq!(resolve("Eth"), "ethereum");
    }

    #[test]
    fn passes_unknown_input_through_as_an_id() {
        assert_eq!(resolve("bitcoin"), "bitcoin");
        assert_eq!(resolve("some-new-coin"), "some-new-coin");
    }

    #[test]
    fn parses_a_comma_list_dropping_blanks_and_duplicates() {
        assert_eq!(
            parse_list("btc, ethereum ,,eth,sol,"),
            vec!["bitcoin", "ethereum", "solana"]
        );
    }

    #[test]
    fn parses_an_empty_argument_to_an_empty_list() {
        assert!(parse_list("").is_empty());
        assert!(parse_list("  , ,").is_empty());
    }
}
