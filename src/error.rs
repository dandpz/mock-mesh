use std::path::PathBuf;

/// Errors raised while loading or validating the OpenAPI spec / mock config.
///
/// At startup these are fatal; during hot-reload they are logged and the
/// previous rule table is kept.
#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is too large ({size} bytes, max {max})")]
    TooLarge { path: PathBuf, size: u64, max: u64 },
    #[error("invalid {kind} in {path}: {message}")]
    Parse {
        kind: &'static str,
        path: PathBuf,
        message: String,
    },
    #[error("external $ref is not supported: {0}")]
    ExternalRef(String),
    #[error("unresolvable $ref: {0}")]
    UnknownRef(String),
    #[error("path {0} collides with the reserved admin prefix /_mockmesh")]
    ReservedPath(String),
    #[error("invalid config: {0}")]
    Validation(String),
}
