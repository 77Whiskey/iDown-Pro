use serde::ser::SerializeStruct;
use serde::Serialize;
use thiserror::Error;

// ---------------------------------------------------------------------------
// Sub-errors
// ---------------------------------------------------------------------------

#[derive(Error, Debug, Serialize, Clone)]
pub enum ExtractorError {
    #[error("network error: {message}")]
    NetworkError { message: String },

    #[error("parse error: {message}")]
    ParseError { message: String },

    #[error("rate limited by Instagram")]
    RateLimited,

    #[error("session expired — re-login required")]
    SessionExpired,

    #[error("unsupported URL: {url}")]
    Unsupported { url: String },
}

#[derive(Error, Debug, Serialize, Clone)]
pub enum DownloadError {
    #[error("network error: {message}")]
    NetworkError { message: String },

    #[error("I/O error: {message}")]
    IoError { message: String },

    #[error("mux error: {message}")]
    MuxError { message: String },

    #[error("download cancelled")]
    Cancelled,

    #[error("ffmpeg is not available")]
    FfmpegNotAvailable,
}

#[derive(Error, Debug, Serialize, Clone)]
pub enum FfmpegError {
    #[error("ffmpeg is not installed")]
    NotInstalled,

    #[error("ffmpeg download failed: {message}")]
    DownloadFailed { message: String },

    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },

    #[error("ffmpeg execution failed (exit code {exit_code:?}): {stderr}")]
    ExecutionFailed {
        exit_code: Option<i32>,
        stderr: String,
    },

    #[error("ffmpeg execution timed out")]
    Timeout,
}

#[derive(Error, Debug, Serialize, Clone)]
pub enum AuthError {
    #[error("login failed: {message}")]
    LoginFailed { message: String },

    #[error("session expired")]
    SessionExpired,

    #[error("keyring error: {message}")]
    KeyringError { message: String },
}

#[derive(Error, Debug)]
pub enum DbError {
    #[error("database error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("migration error: {0}")]
    Migration(String),
}

// DbError: manual Serialize because rusqlite::Error does not implement it.
impl Serialize for DbError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("DbError", 2)?;
        match self {
            DbError::Sqlite(e) => {
                state.serialize_field("type", "Sqlite")?;
                state.serialize_field("message", &e.to_string())?;
            }
            DbError::Migration(msg) => {
                state.serialize_field("type", "Migration")?;
                state.serialize_field("message", msg)?;
            }
        }
        state.end()
    }
}

// ---------------------------------------------------------------------------
// Top-level application error
// ---------------------------------------------------------------------------

#[derive(Error, Debug)]
pub enum AppError {
    #[error(transparent)]
    Extractor(#[from] ExtractorError),

    #[error(transparent)]
    Download(#[from] DownloadError),

    #[error(transparent)]
    Ffmpeg(#[from] FfmpegError),

    #[error(transparent)]
    Auth(#[from] AuthError),

    #[error(transparent)]
    Db(#[from] DbError),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
}

// Manual Serialize for AppError because std::io::Error does not implement Serialize.
impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut state = serializer.serialize_struct("AppError", 2)?;
        match self {
            AppError::Extractor(e) => {
                state.serialize_field("type", "Extractor")?;
                state.serialize_field("message", &e.to_string())?;
            }
            AppError::Download(e) => {
                state.serialize_field("type", "Download")?;
                state.serialize_field("message", &e.to_string())?;
            }
            AppError::Ffmpeg(e) => {
                state.serialize_field("type", "Ffmpeg")?;
                state.serialize_field("message", &e.to_string())?;
            }
            AppError::Auth(e) => {
                state.serialize_field("type", "Auth")?;
                state.serialize_field("message", &e.to_string())?;
            }
            AppError::Db(e) => {
                state.serialize_field("type", "Db")?;
                state.serialize_field("message", &e.to_string())?;
            }
            AppError::Io(e) => {
                state.serialize_field("type", "Io")?;
                state.serialize_field("message", &e.to_string())?;
            }
        }
        state.end()
    }
}

// ---------------------------------------------------------------------------
// From conversions for reqwest::Error
// ---------------------------------------------------------------------------

impl From<reqwest::Error> for ExtractorError {
    fn from(err: reqwest::Error) -> Self {
        ExtractorError::NetworkError {
            message: err.to_string(),
        }
    }
}

impl From<reqwest::Error> for DownloadError {
    fn from(err: reqwest::Error) -> Self {
        DownloadError::NetworkError {
            message: err.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// From<std::io::Error> for DownloadError
// ---------------------------------------------------------------------------

impl From<std::io::Error> for DownloadError {
    fn from(err: std::io::Error) -> Self {
        DownloadError::IoError {
            message: err.to_string(),
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience type alias
// ---------------------------------------------------------------------------

pub type AppResult<T> = Result<T, AppError>;
