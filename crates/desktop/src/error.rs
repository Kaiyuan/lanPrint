use std::fmt;

#[derive(Debug)]
pub enum AppError {
    Config(String),
    Platform(String),
    Print(String),
    Scan(String),
    Network(String),
    Auth(String),
    Cloud(String),
    Io(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(m) => write!(f, "{m}"),
            Self::Platform(m) => write!(f, "{m}"),
            Self::Print(m) => write!(f, "print error: {m}"),
            Self::Scan(m) => write!(f, "scan error: {m}"),
            Self::Network(m) => write!(f, "network error: {m}"),
            Self::Auth(m) => write!(f, "authentication error: {m}"),
            Self::Cloud(m) => write!(f, "cloud error: {m}"),
            Self::Io(m) => write!(f, "I/O error: {m}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value.to_string())
    }
}

impl From<reqwest::Error> for AppError {
    fn from(value: reqwest::Error) -> Self {
        Self::Network(value.to_string())
    }
}

impl From<serde_json::Error> for AppError {
    fn from(value: serde_json::Error) -> Self {
        Self::Config(value.to_string())
    }
}

impl From<lanprint_core::auth::AuthError> for AppError {
    fn from(value: lanprint_core::auth::AuthError) -> Self {
        Self::Auth(value.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;
