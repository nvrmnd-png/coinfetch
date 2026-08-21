pub mod coingecko;
pub mod ethereum;
pub mod mempool;

use std::time::Duration;

use crate::error::Result;

pub fn http_client() -> Result<reqwest::Client> {
    let client = reqwest::Client::builder()
        .user_agent(concat!("coinfetch/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(15))
        .build()?;
    Ok(client)
}
