use thiserror::Error;

#[derive(Debug, Error)]
pub enum OmronError {
    #[error("BLE error: {0}")]
    Ble(#[from] btleplug::Error),

    #[error("device not found")]
    NotFound,

    #[error("characteristic not found: {0}")]
    CharNotFound(String),

    #[error("BLE disconnected ({0}); retry when in range")]
    Disconnected(String),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("unlock failed: {0}")]
    Unlock(String),

    #[error("pairing failed: {0}")]
    Pairing(String),

    #[error("invalid record: {0}")]
    InvalidRecord(String),

    #[error("timed out waiting for {0}")]
    Timeout(String),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("other: {0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, OmronError>;
