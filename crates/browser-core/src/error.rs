use thiserror::Error;

/// Top-level application error. Avoid unwrap/expect on production paths.
#[derive(Debug, Error)]
pub enum BrowserError {
    #[error("engine error: {0}")]
    Engine(String),

    #[error("network error: {0}")]
    Network(String),

    #[error("storage error: {0}")]
    Storage(String),

    #[error("profile error: {0}")]
    Profile(String),

    #[error("database error: {0}")]
    Database(String),

    #[error("ui error: {0}")]
    Ui(String),

    #[error("invalid url: {0}")]
    InvalidUrl(String),

    #[error("tab not found: {0}")]
    TabNotFound(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl BrowserError {
    pub fn engine(msg: impl Into<String>) -> Self {
        Self::Engine(msg.into())
    }

    pub fn database(msg: impl AsRef<str>) -> Self {
        Self::Database(msg.as_ref().to_owned())
    }

    pub fn profile(msg: impl AsRef<str>) -> Self {
        Self::Profile(msg.as_ref().to_owned())
    }

    pub fn ui(msg: impl AsRef<str>) -> Self {
        Self::Ui(msg.as_ref().to_owned())
    }
}

pub type BrowserResult<T> = Result<T, BrowserError>;
