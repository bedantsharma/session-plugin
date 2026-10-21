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
        "bridge-screen was updated while this session was open. Start a new Claude session to keep using it (data format {found}; this session's copy understands up to {supported})."
    )]
    SchemaTooNew { found: i64, supported: i64 },
    #[error("could not find a home directory; set BRIDGE_SCREEN_HOME")]
    NoHome,
    #[error("{0}")]
    Claim(String),
    /// A detour, resume or label request the agent or owner can fix.
    #[error("{0}")]
    Tangent(String),
    /// A verification or decision the agent or owner can fix (spec §8.5–8.6).
    #[error("{0}")]
    Verify(String),
    /// An owner action on something that is no longer open (already answered, resolved
    /// or verified): nothing was recorded.
    #[error("{0}")]
    Stale(String),
    /// An owner action the owner can fix and resend (e.g. a reason that says nothing).
    #[error("{0}")]
    Invalid(String),
    /// A screen asked for something that doesn't exist.
    #[error("{0}")]
    NotFound(String),
    #[error("invalid event: {0}")]
    BadEvent(String),
    #[error(transparent)]
    Plan(#[from] crate::plan::PlanError),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
