use std::error::Error as _;

/// Every failure the engine can report.
///
/// Errors are `Clone` so they can be stored in download records and shown in
/// any front-end.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Error {
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("connection timed out")]
    Timeout,
    #[error("server responded with HTTP {0}")]
    Http(u16),
    #[error("server is busy (HTTP {code}); it asked to wait {retry_after} s")]
    Throttled { code: u16, retry_after: u64 },
    #[error("server ignored the byte-range request")]
    RangeIgnored,
    #[error("the remote file changed since the download started")]
    RemoteChanged,
    #[error("the server now reports a different file size ({old} before, {new} now)")]
    SizeChanged { old: String, new: String },
    #[error("the server no longer supports resuming this download")]
    ResumeUnsupported,
    #[error("not enough disk space: {needed} needed, {available} free")]
    DiskFull { needed: String, available: String },
    #[error("downloaded size {actual} does not match expected size {expected}")]
    SizeMismatch { expected: u64, actual: u64 },
    #[error("checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("invalid checksum: {0}")]
    InvalidChecksum(String),
    #[error("disk error: {0}")]
    Io(String),
    #[error("another StewardGet instance is already using {0}")]
    Locked(String),
    #[error("download not found")]
    NotFound,
    #[error("cancelled")]
    Cancelled,
    /// Something StewardGet can't download (a live or protected stream…); retrying won't help.
    #[error("{0}")]
    Unsupported(String),
}

impl Error {
    /// The partial data is intact but cannot be continued without the user
    /// deciding how (resume anyway, refresh the link or start over).
    pub fn needs_decision(&self) -> bool {
        matches!(self, Error::RemoteChanged | Error::SizeChanged { .. } | Error::ResumeUnsupported)
    }

    /// Whether retrying the same request may succeed.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Network(_) | Error::Timeout | Error::Throttled { .. } => true,
            Error::Http(code) => matches!(code, 408 | 425 | 429 | 500..=599),
            _ => false,
        }
    }
}

impl Error {
    /// How long the server asked us to wait before the next request, in seconds.
    pub fn retry_after(&self) -> Option<u64> {
        match self {
            Error::Throttled { retry_after, .. } => Some(*retry_after),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::StorageFull {
            return Error::DiskFull { needed: "more space".into(), available: "almost nothing".into() };
        }
        Error::Io(e.to_string())
    }
}

impl From<reqwest::Error> for Error {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            return Error::Timeout;
        }
        if let Some(status) = e.status() {
            return Error::Http(status.as_u16());
        }
        if e.is_builder() {
            return Error::InvalidUrl(e.to_string());
        }
        // reqwest's top-level message is generic; the root cause is what users need.
        let mut msg = e.to_string();
        let mut source = e.source();
        while let Some(s) = source {
            msg = s.to_string();
            source = s.source();
        }
        Error::Network(msg)
    }
}
