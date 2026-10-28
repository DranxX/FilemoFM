use thiserror::Error;

#[derive(Error, Debug)]
pub enum StorageError {
    #[error("Database is locked")]
    DatabaseLocked,
    #[error("Data conflicts or is invalid")]
    ConstraintViolation,
    #[error("Storage operation failed")]
    QueryFailed,
    #[error("Metadata changed. Reload before saving")]
    RevisionConflict,
}

#[derive(Error, Debug)]
pub enum ScanError {
    #[error("Location cannot be accessed")]
    PermissionDenied,
    #[error("Location not found")]
    PathNotFound,
    #[error("Location is outside the allowed scope")]
    OutOfScope,
    #[error("Path is invalid")]
    InvalidPath,
}

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("{0}")]
    Field(&'static str),
    #[error("Settings could not be read")]
    Read,
    #[error("Settings could not be saved")]
    Write,
    #[error("Settings are invalid")]
    Parse,
}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        tracing::error!(error = %error, "storage operation failed");
        match error {
            rusqlite::Error::SqliteFailure(ref code, _)
                if code.code == rusqlite::ErrorCode::DatabaseBusy
                    || code.code == rusqlite::ErrorCode::DatabaseLocked =>
            {
                Self::DatabaseLocked
            }
            rusqlite::Error::SqliteFailure(ref code, _)
                if code.code == rusqlite::ErrorCode::ConstraintViolation =>
            {
                Self::ConstraintViolation
            }
            _ => Self::QueryFailed,
        }
    }
}
