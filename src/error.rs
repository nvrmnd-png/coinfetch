use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Message(String),
}

impl Error {
    pub fn msg(text: impl Into<String>) -> Self {
        Error::Message(text.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CoinError {
    #[error("unknown coin")]
    NotFound,

    #[error("rate limited by CoinGecko")]
    RateLimited,

    #[error("no price history returned")]
    NoData,

    #[error("price history starts at zero, cannot normalize")]
    ZeroBaseline,

    #[error("no overlap with the shared time window")]
    OutsideWindow,

    #[error("{0}")]
    Other(String),
}
