//! Error types for SDP Maze

use thiserror::Error;

#[derive(Error, Debug)]
pub enum MazeError {
    #[error("Crypto error: {0}")]
    CryptoError(String),

    #[error("Invalid meta-address: {0}")]
    InvalidMetaAddress(String),

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("RPC error: {0}")]
    RpcError(String),

    #[error("Maze generation error: {0}")]
    MazeGenerationError(String),

    #[error("Transaction error: {0}")]
    TransactionError(String),

    #[error("Insufficient funds: required {required}, available {available}")]
    InsufficientFunds { required: u64, available: u64 },

    #[error("Request expired")]
    RequestExpired,

    #[error("Request not found: {0}")]
    RequestNotFound(String),

    #[error("Invalid parameters: {0}")]
    InvalidParameters(String),

    #[error("Encryption error: {0}")]
    EncryptionError(String),

    #[error("Decryption error: {0}")]
    DecryptionError(String),
}

pub type Result<T> = std::result::Result<T, MazeError>;

impl From<rusqlite::Error> for MazeError {
    fn from(err: rusqlite::Error) -> Self {
        MazeError::DatabaseError(err.to_string())
    }
}

impl From<solana_client::client_error::ClientError> for MazeError {
    fn from(err: solana_client::client_error::ClientError) -> Self {
        MazeError::RpcError(err.to_string())
    }
}
