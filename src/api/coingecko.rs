
use std::time::Duration;

use serde_json::Value;

use crate::cache::{self, Cache};
use crate::error::{CoinError, Result};
use crate::model::{Quote, RawSeries};

const BASE: &str = "https://api.coingecko.com/api/v3";
const VS_CURRENCY: &str = "usd";
const HISTORY_DAYS: u32 = 7;

pub const MAX_COINS: usize = 8;

const REQUEST_SPACING_FREE: Duration = Duration::from_millis(250);
const REQUEST_SPACING_KEYED: Duration = Duration::from_millis(80);
const RATE_LIMIT_BACKOFF: Duration = Duration::from_secs(2);

pub struct MarketData {
    pub series: Vec<RawSeries>,
    pub quotes: Vec<Quote>,
    pub failed: Vec<(String, CoinError)>,
    pub notes: Vec<String>,
}

impl MarketData {
    pub fn quote(&self, id: &str) -> Option<&Quote> {
        self.quotes.iter().find(|q| q.id == id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub id: String,
    pub name: String,
    pub symbol: String,
    pub market_cap_rank: Option<u32>,
}

pub struct CoinGecko {
    client: reqwest::Client,
    api_key: Option<String>,
    cache: Cache,
    ttl_secs: u64,
}

impl CoinGecko {
    pub fn new(client: reqwest::Client, api_key: Option<&str>, ttl_secs: u64) -> Self {
        let api_key = api_key
            .map(str::trim)
            .filter(|key| !key.is_empty())
            .map(str::to_string);
        CoinGecko {
            client,
            api_key,
            cache: Cache::new(),
            ttl_secs,
        }
    }

    fn request_spacing(&self) -> Duration {
        if self.api_key.is_some() {
            REQUEST_SPACING_KEYED
        } else {
            REQUEST_SPACING_FREE
        }
    }

    async fn try_get(&self, url: &str) -> std::result::Result<Value, CoinError> {
        let mut request = self.client.get(url);
        if let Some(key) = &self.api_key {
            request = request.header("x-cg-demo-api-key", key);
        }

        let response = request.send().await.map_err(|e| {
            if e.is_timeout() {
                CoinError::Other("request timed out".to_string())
            } else {
                CoinError::Other("network unreachable".to_string())
            }
        })?;

        match response.status().as_u16() {
            200 => response
                .json::<Value>()
                .await
                .map_err(|e| CoinError::Other(format!("malformed response: {e}"))),
            404 => Err(CoinError::NotFound),
            429 => Err(CoinError::RateLimited),
            status => Err(CoinError::Other(format!("HTTP {status}"))),
        }
    }

    async fn get_json(
        &self,
        url: &str,
        cache_key: &str,
        used_network: &mut bool,
    ) -> std::result::Result<(Value, u64), CoinError> {
        let now = cache::now_secs();

        if let Some(hit) = self.cache.read_fresh(cache_key, self.ttl_secs, now) {
            return Ok((hit.payload, hit.age_secs));
        }

        if *used_network {
            tokio::time::sleep(self.request_spacing()).await;
        }
        *used_network = true;

        let mut rate_limited_once = false;
        loop {
            match self.try_get(url).await {
                Ok(value) => {
                    self.cache.write(cache_key, &value, now);
                    return Ok((value, 0));
                }

                Err(CoinError::NotFound) => return Err(CoinError::NotFound),
                Err(CoinError::RateLimited) if !rate_limited_once => {
                    rate_limited_once = true;
                    tokio::time::sleep(RATE_LIMIT_BACKOFF).await;
                }
                Err(err) => {
                    if let Some(hit) = self.cache.read_stale(cache_key, now) {
                        return Ok((hit.payload, hit.age_secs));
                    }
                    return Err(err);
                }
            }
        }
    }

    async fn fetch_quotes(
        &self,
        ids: &[String],
        used_network: &mut bool,
    ) -> std::result::Result<(Vec<Quote>, u64), CoinError> {
        let joined = ids.join(",");
        let url = format!("{BASE}/coins/markets?vs_currency={VS_CURRENCY}&ids={joined}");
        let key = format!("markets_{}", joined.replace(',', "_"));
        let (value, age) = self.get_json(&url, &key, used_network).await?;
        Ok((parse_markets(&value), age))
    }

    async fn fetch_history(
        &self,
        id: &str,
        used_network: &mut bool,
    ) -> std::result::Result<(RawSeries, u64), CoinError> {
        let url =
            format!("{BASE}/coins/{id}/market_chart?vs_currency={VS_CURRENCY}&days={HISTORY_DAYS}");
        let key = format!("chart_{id}_{HISTORY_DAYS}d");
        let (value, age) = self.get_json(&url, &key, used_network).await?;
        Ok((parse_market_chart(id, &value)?, age))
    }

    pub async fn search(&self, query: &str) -> std::result::Result<Vec<SearchHit>, CoinError> {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }

        let url = format!("{BASE}/search?query={}", encode_query(trimmed));
        let mut used_network = false;
        let (value, _age) = self
            .get_json(&url, &search_cache_key(trimmed), &mut used_network)
            .await?;
        Ok(parse_search(&value))
    }

    pub async fn quotes_only(&self, ids: &[String]) -> Vec<Quote> {
        let mut used_network = false;
        self.fetch_quotes(ids, &mut used_network)
            .await
            .map(|(quotes, _)| quotes)
            .unwrap_or_default()
    }

    pub async fn fetch_market_data(&self, ids: &[String]) -> MarketData {
        let mut notes = Vec::new();
        let mut failed = Vec::new();
        let mut series = Vec::new();
        let mut oldest_age = 0_u64;
        let mut used_network = false;

        let quotes = match self.fetch_quotes(ids, &mut used_network).await {
            Ok((quotes, age)) => {
                oldest_age = oldest_age.max(age);
                quotes
            }
            Err(err) => {
                notes.push(format!("current prices unavailable ({err})"));
                Vec::new()
            }
        };

        for id in ids {
            match self.fetch_history(id, &mut used_network).await {
                Ok((s, age)) => {
                    oldest_age = oldest_age.max(age);
                    series.push(s);
                }
                Err(err) => failed.push((id.clone(), err)),
            }
        }

        if oldest_age >= self.ttl_secs.max(1) {
            notes.push(format!("showing cached data from {oldest_age}s ago"));
        }

        MarketData {
            series,
            quotes,
            failed,
            notes,
        }
    }
}

fn encode_query(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(*byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn search_cache_key(query: &str) -> String {
    use std::hash::{DefaultHasher, Hash, Hasher};

    let normalized = query.to_ascii_lowercase();
    let mut hasher = DefaultHasher::new();
    normalized.hash(&mut hasher);
    format!("search_{normalized}_{:x}", hasher.finish())
}

pub fn parse_search(value: &Value) -> Vec<SearchHit> {
    let Some(coins) = value.get("coins").and_then(Value::as_array) else {
        return Vec::new();
    };

    coins
        .iter()
        .filter_map(|row| {
            let id = row.get("id")?.as_str()?.to_string();
            let name = row
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.is_empty())
                .unwrap_or(&id)
                .to_string();
            Some(SearchHit {
                symbol: row
                    .get("symbol")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_uppercase(),
                market_cap_rank: row
                    .get("market_cap_rank")
                    .and_then(Value::as_u64)
                    .and_then(|rank| u32::try_from(rank).ok()),
                id,
                name,
            })
        })
        .collect()
}

pub fn parse_market_chart(id: &str, value: &Value) -> std::result::Result<RawSeries, CoinError> {
    let prices = value
        .get("prices")
        .and_then(Value::as_array)
        .ok_or(CoinError::NoData)?;

    let mut points = Vec::with_capacity(prices.len());
    for row in prices {
        let Some(pair) = row.as_array() else { continue };
        if pair.len() < 2 {
            continue;
        }
        if let (Some(ts), Some(price)) = (pair[0].as_f64(), pair[1].as_f64()) {
            points.push((ts, price));
        }
    }

    if points.len() < 2 {
        return Err(CoinError::NoData);
    }

    Ok(RawSeries {
        id: id.to_string(),
        points,
    })
}

pub fn parse_markets(value: &Value) -> Vec<Quote> {
    let Some(rows) = value.as_array() else {
        return Vec::new();
    };

    rows.iter()
        .filter_map(|row| {
            Some(Quote {
                id: row.get("id")?.as_str()?.to_string(),
                symbol: row
                    .get("symbol")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_ascii_uppercase(),
                price: row.get("current_price")?.as_f64()?,
                change_24h: row
                    .get("price_change_percentage_24h")
                    .and_then(Value::as_f64),
            })
        })
        .collect()
}

pub fn client(api_key: Option<&str>, ttl_secs: u64) -> Result<CoinGecko> {
    Ok(CoinGecko::new(super::http_client()?, api_key, ttl_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHART: &str = include_str!("../../tests/fixtures/market_chart_bitcoin_7d.json");
    const MARKETS: &str = include_str!("../../tests/fixtures/markets_3coins.json");
    const NOT_FOUND: &str = include_str!("../../tests/fixtures/coin_not_found.json");
    const SEARCH: &str = include_str!("../../tests/fixtures/search_bit.json");

    fn gecko(api_key: Option<&str>) -> CoinGecko {
        CoinGecko::new(reqwest::Client::new(), api_key, 60)
    }

    #[test]
    fn a_key_relaxes_the_request_spacing() {
        assert_eq!(gecko(None).request_spacing(), REQUEST_SPACING_FREE);
        assert_eq!(
            gecko(Some("CG-abc123")).request_spacing(),
            REQUEST_SPACING_KEYED
        );
        assert!(REQUEST_SPACING_KEYED < REQUEST_SPACING_FREE);
    }

    #[test]
    fn a_blank_key_keeps_the_free_tier_behaviour() {

        let client = gecko(Some("   "));
        assert_eq!(client.api_key, None);
        assert_eq!(client.request_spacing(), REQUEST_SPACING_FREE);
    }

    #[test]
    fn parses_a_real_search_response() {
        let value: Value = serde_json::from_str(SEARCH).expect("fixture parses");
        let hits = parse_search(&value);

        assert_eq!(hits.len(), 8);
        assert_eq!(
            hits[0],
            SearchHit {
                id: "bitcoin".into(),
                name: "Bitcoin".into(),
                symbol: "BTC".into(),
                market_cap_rank: Some(1),
            }
        );

        let cash = hits.iter().find(|h| h.id == "bitcoin-cash").expect("bch");
        assert_eq!(cash.market_cap_rank, Some(22));
        assert_eq!(cash.symbol, "BCH");
    }

    #[test]
    fn parses_search_results_defensively() {
        let value = serde_json::json!({
            "coins": [
                { "id": "a", "name": "Ay", "symbol": "a", "market_cap_rank": 7 },
                { "id": "unranked", "name": "Unranked", "symbol": "unr", "market_cap_rank": null },
                { "id": "nameless", "symbol": "nl" },
                { "name": "no id at all", "symbol": "x" }
            ],
            "exchanges": [{ "id": "an-exchange", "name": "Not A Coin" }]
        });
        let hits = parse_search(&value);

        assert_eq!(hits.len(), 3, "the id-less row is the only unusable one");
        assert_eq!(hits[0].market_cap_rank, Some(7));
        assert_eq!(hits[1].market_cap_rank, None);

        assert_eq!(hits[2].name, "nameless");
        assert!(!hits.iter().any(|h| h.id == "an-exchange"));
    }

    #[test]
    fn parses_a_search_response_of_the_wrong_shape_to_an_empty_list() {
        assert!(parse_search(&serde_json::json!({ "error": "nope" })).is_empty());
        assert!(parse_search(&serde_json::json!([])).is_empty());
    }

    #[test]
    fn encodes_characters_that_would_otherwise_break_out_of_the_url() {
        assert_eq!(encode_query("bitcoin"), "bitcoin");
        assert_eq!(encode_query("shiba inu"), "shiba%20inu");
        assert_eq!(encode_query("a&b=c#d"), "a%26b%3Dc%23d");

        assert_eq!(encode_query("ü"), "%C3%BC");
    }

    #[test]
    fn queries_that_sanitize_alike_get_different_cache_keys() {

        assert_ne!(search_cache_key("a b"), search_cache_key("a+b"));

        assert_eq!(search_cache_key("BiT"), search_cache_key("bit"));
    }

    #[test]
    fn parses_a_real_seven_day_history() {
        let value: Value = serde_json::from_str(CHART).expect("fixture parses");
        let series = parse_market_chart("bitcoin", &value).expect("series");

        assert_eq!(series.id, "bitcoin");

        assert!(
            series.points.len() > 160,
            "expected ~169 points, got {}",
            series.points.len()
        );
        assert!(series.points.iter().all(|(t, p)| *t > 0.0 && *p > 0.0));

        let step = series.points[1].0 - series.points[0].0;
        assert!((step - 3_600_000.0).abs() < 1.0, "step was {step}");
    }

    #[test]
    fn parses_a_real_batch_of_current_prices() {
        let value: Value = serde_json::from_str(MARKETS).expect("fixture parses");
        let quotes = parse_markets(&value);

        assert_eq!(quotes.len(), 3);
        let btc = quotes.iter().find(|q| q.id == "bitcoin").expect("bitcoin");
        assert_eq!(btc.symbol, "BTC");
        assert!(btc.price > 0.0);
    }

    #[test]
    fn treats_the_not_found_body_as_missing_data() {

        let value: Value = serde_json::from_str(NOT_FOUND).expect("fixture parses");
        assert_eq!(
            parse_market_chart("notacoin123", &value),
            Err(CoinError::NoData)
        );
    }

    #[test]
    fn rejects_a_history_with_too_few_usable_points() {
        let value = serde_json::json!({ "prices": [[1_000, 5.0]] });
        assert_eq!(parse_market_chart("x", &value), Err(CoinError::NoData));
    }

    #[test]
    fn skips_malformed_rows_but_keeps_the_good_ones() {
        let value = serde_json::json!({
            "prices": [[1_000, 5.0], ["bad"], [2_000, 6.0], [3_000, null], [4_000, 7.0]]
        });
        let series = parse_market_chart("x", &value).expect("series");
        assert_eq!(
            series.points,
            vec![(1000.0, 5.0), (2000.0, 6.0), (4000.0, 7.0)]
        );
    }

    #[test]
    fn parses_markets_defensively() {
        let value = serde_json::json!([
            { "id": "a", "symbol": "a", "current_price": 1.0, "price_change_percentage_24h": -2.5 },
            { "id": "b", "symbol": "b" },
            { "symbol": "c", "current_price": 3.0 }
        ]);
        let quotes = parse_markets(&value);
        assert_eq!(quotes.len(), 1);
        assert_eq!(quotes[0].id, "a");
        assert_eq!(quotes[0].change_24h, Some(-2.5));
    }

    #[test]
    fn parses_markets_of_the_wrong_shape_to_an_empty_list() {
        assert!(parse_markets(&serde_json::json!({ "error": "nope" })).is_empty());
    }
}
