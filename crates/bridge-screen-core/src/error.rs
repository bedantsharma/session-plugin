use thiserror::Error;

/// Every error the core library can return.
#[derive(Debug, Error)]
pub enum Error {
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config error: {0}")]
    Config(String),
    #[error("not inside a git repository: {0}")]
    NotARepo(String),
    #[error("git failed: {0}")]
    Git(String),
    #[error(
        "database schema version {found} is newer than this bridge-screen supports ({supported}); upgrade bridge-screen"
    )]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("could not find a home directory; set BRIDGE_SCREEN_HOME")]
    NoHome,
    #[error("invalid event: {0}")]
    BadEvent(String),
    #[error(transparent)]
    Plan(#[from] crate::plan::PlanError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
