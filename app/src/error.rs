use thiserror::Error;

#[derive(Error, Debug)]
pub(crate) enum LingError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Plugin error: {0}")]
    Plugin(String),

    #[error("Other error: {0}")]
    Other(String),
}

pub(crate) type Result<T> = std::result::Result<T, LingError>;

impl From<LingError> for String {
    fn from(err: LingError) -> Self {
        err.to_string()
    }
}

impl From<String> for LingError {
    fn from(s: String) -> Self {
        LingError::Other(s)
    }
}
