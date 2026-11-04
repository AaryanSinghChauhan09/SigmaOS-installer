use thiserror::Error;

#[derive(Error, Debug)]
pub enum ComponentError {
    #[error("Configuration error: {0}")]
    ConfigError(String),
    #[error("Initialization error: {0}")]
    InitError(String),
    #[error("Runtime error: {0}")]
    RuntimeError(String),
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ComponentError>;
