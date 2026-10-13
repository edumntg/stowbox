use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("not signed in: add a provider API key first")]
    NotConfigured,
    #[error("authentication failed: {0}")]
    Auth(String),
    #[error("{0}")]
    Provider(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("rate limited by the provider, retry in a moment")]
    RateLimited,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("network: {0}")]
    Network(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<tonic::Status> for Error {
    fn from(s: tonic::Status) -> Self {
        use tonic::Code;
        match s.code() {
            Code::Unauthenticated => Error::Auth(s.message().to_string()),
            Code::PermissionDenied => Error::Auth(format!("permission denied: {}", s.message())),
            Code::NotFound => Error::NotFound(s.message().to_string()),
            Code::ResourceExhausted => Error::RateLimited,
            Code::Unavailable => Error::Network(s.message().to_string()),
            _ => Error::Provider(s.message().to_string()),
        }
    }
}

impl From<tonic::transport::Error> for Error {
    fn from(e: tonic::transport::Error) -> Self {
        Error::Network(e.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        Error::Network(e.to_string())
    }
}
