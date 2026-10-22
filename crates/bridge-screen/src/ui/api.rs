//! The dashboard's HTTP surface (spec §4.3, §6.4): a JSON API under `/api/` and the
//! embedded single-page app everywhere else.
//!
//! Every request must name this server in `Host` (`127.0.0.1:<port>` or
//! `localhost:<port>`), which defeats DNS rebinding; every `/api/` request must carry the
//! start-up token as `Authorization: Bearer <token>`. No CORS headers are sent, so no
//! other origin can read a response. Each request opens its own connection: reads never
//! block the hooks' writes (spec §6.3).

use std::borrow::Cow;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::rejection::JsonRejection;
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use bridge_screen_core::config::Config;
use bridge_screen_core::error::Error;
use bridge_screen_core::owner::{self, OwnerAction};
use bridge_screen_core::paths::Home;
use bridge_screen_core::store::Store;
use bridge_screen_core::view;
use serde::Deserialize;
use serde_json::{Value, json};

/// Largest request body accepted.
pub const BODY_LIMIT: usize = 64 * 1024;

/// Static files by path: the embedded build, or a test double.
pub trait AssetSource: Send + Sync {
    /// The file's bytes and MIME type.
    fn get(&self, path: &str) -> Option<(Cow<'static, [u8]>, String)>;
}

#[derive(rust_embed::RustEmbed)]
#[folder = "$BRIDGE_WEB_DIST"]
struct WebDist;

/// The dashboard as built into the binary (or the build-instructions placeholder).
pub struct Embedded;

impl AssetSource for Embedded {
    fn get(&self, path: &str) -> Option<(Cow<'static, [u8]>, String)> {
        WebDist::get(path).map(|f| (f.data, f.metadata.mimetype().to_string()))
    }
}

/// What every request handler shares.
#[derive(Clone)]
pub struct AppState {
    pub home: Home,
    pub cfg: Config,
    pub token: String,
    pub port: u16,
    pub assets: Arc<dyn AssetSource>,
}

/// The router: `/api/*` plus static assets.
pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/ping", get(ping))
        .route("/version", get(version))
        .route("/glance", get(glance))
        .route("/project/{repo}", get(project))
        .route("/session/{id}", get(session))
        .route("/task/{repo}/{id}", get(task))
        .route("/history/{repo}", get(history))
        .route("/tuning/{repo}", get(tuning))
        .route("/calibration/{repo}", get(calibration))
        .route("/pages/{repo}", get(pages))
        .route("/action", post(action))
        .fallback(|| async { error(StatusCode::NOT_FOUND, "no such API route") })
        .layer(middleware::from_fn_with_state(state.clone(), require_token));
    Router::new()
        .nest("/api", api)
        .fallback(get(asset))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
        .layer(middleware::from_fn_with_state(state.clone(), require_host))
        .layer(middleware::from_fn(security_headers))
        .with_state(state)
}

fn error(status: StatusCode, msg: &str) -> Response {
    (status, axum::Json(json!({ "error": msg }))).into_response()
}

/// Same bytes, compared in time that depends only on the length.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0_u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// 403 unless `Host` names this server (DNS-rebinding guard).
async fn require_host(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");
    let ok = [
        format!("127.0.0.1:{}", st.port),
        format!("localhost:{}", st.port),
    ]
    .iter()
    .any(|h| h == host);
    if !ok {
        return error(StatusCode::FORBIDDEN, "wrong Host");
    }
    next.run(req).await
}

/// 401 unless the request carries the start-up token.
async fn require_token(State(st): State<AppState>, req: Request, next: Next) -> Response {
    let given = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .unwrap_or("");
    if !same(given.as_bytes(), st.token.as_bytes()) {
        return error(
            StatusCode::UNAUTHORIZED,
            "token missing or wrong — open the URL that `bridge-screen ui` printed",
        );
    }
    next.run(req).await
}

async fn security_headers(req: Request, next: Next) -> Response {
    let mut res = next.run(req).await;
    let h = res.headers_mut();
    for (name, value) in [
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "no-referrer"),
        ("x-frame-options", "DENY"),
        ("cache-control", "no-store"),
        (
            "content-security-policy",
            "default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:; frame-ancestors 'none'",
        ),
    ] {
        h.insert(name, HeaderValue::from_static(value));
    }
    res
}

/// Run `f` on its own connection, off the async threads.
async fn with_store<T: serde::Serialize + Send + 'static>(
    st: &AppState,
    f: impl FnOnce(&mut Store, &AppState) -> Result<T, Error> + Send + 'static,
) -> Response {
    let st = st.clone();
    let out = tokio::task::spawn_blocking(move || {
        st.home.ensure()?;
        let mut store = Store::open(&st.home.db_path())?;
        f(&mut store, &st)
    })
    .await;
    match out {
        Ok(Ok(v)) => axum::Json(v).into_response(),
        Ok(Err(e)) => error_response(&e),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "the request failed"),
    }
}

/// Stale actions are 409, actions the owner can fix 400, the rest 500.
fn error_response(e: &Error) -> Response {
    let status = match e {
        Error::Stale(_) => StatusCode::CONFLICT,
        Error::NotFound(_) => StatusCode::NOT_FOUND,
        Error::Invalid(_)
        | Error::Claim(_)
        | Error::Tangent(_)
        | Error::Verify(_)
        | Error::Plan(_)
        | Error::NotARepo(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
    };
    error(status, &e.to_string())
}

async fn ping() -> Response {
    axum::Json(json!({
        "ok": true,
        "version": bridge_screen_core::VERSION,
        "pid": std::process::id(),
    }))
    .into_response()
}

async fn version(State(st): State<AppState>) -> Response {
    with_store(&st, |s, _| {
        Ok(json!({ "last_event_pos": view::last_event_pos(s.conn())? }))
    })
    .await
}

async fn glance(State(st): State<AppState>) -> Response {
    with_store(&st, |s, st| {
        view::glance(s.conn(), &st.cfg, jiff::Timestamp::now())
    })
    .await
}

async fn project(State(st): State<AppState>, Path(repo): Path<String>) -> Response {
    with_store(&st, move |s, _| {
        view::drill::project(s.conn(), &repo, jiff::Timestamp::now())
    })
    .await
}

async fn session(State(st): State<AppState>, Path(id): Path<String>) -> Response {
    with_store(&st, move |s, st| {
        view::drill::session(s.conn(), &id, &st.cfg, jiff::Timestamp::now())?
            .ok_or_else(|| Error::NotFound(format!("no session {id}")))
    })
    .await
}

async fn task(State(st): State<AppState>, Path((repo, id)): Path<(String, String)>) -> Response {
    with_store(&st, move |s, st| {
        view::drill::task(s.conn(), &repo, &id, &st.cfg)
    })
    .await
}

/// `/api/history/{repo}` filters.
#[derive(Debug, Default, Deserialize)]
pub struct HistoryQuery {
    pub kind: Option<String>,
    pub session: Option<String>,
    pub task: Option<String>,
    /// Events before this position (the next page).
    pub before: Option<i64>,
    pub limit: Option<usize>,
}

async fn history(
    State(st): State<AppState>,
    Path(repo): Path<String>,
    Query(q): Query<HistoryQuery>,
) -> Response {
    with_store(&st, move |s, _| {
        let filter = view::drill::HistoryFilter {
            kind: q.kind.filter(|k| !k.is_empty()),
            session: q.session.filter(|k| !k.is_empty()),
            task: q.task.filter(|k| !k.is_empty()),
            before: q.before,
            limit: q.limit.unwrap_or(view::drill::HISTORY_MAX),
        };
        view::drill::history(s.conn(), &repo, &filter)
    })
    .await
}

async fn tuning(State(st): State<AppState>, Path(repo): Path<String>) -> Response {
    with_store(&st, move |s, st| {
        view::drill::tuning(s.conn(), &repo, &st.cfg)
    })
    .await
}

async fn calibration(State(st): State<AppState>, Path(repo): Path<String>) -> Response {
    with_store(&st, move |s, st| {
        view::drill::calibration(s.conn(), &repo, &st.cfg, jiff::Timestamp::now())
    })
    .await
}

/// The project pages of a repo (read-only; the pages live in bridge-screen's folder).
async fn pages(State(st): State<AppState>, Path(repo): Path<String>) -> Response {
    with_store(&st, move |_, st| {
        bridge_screen_core::pages::view(&st.home, &repo)
    })
    .await
}

async fn action(
    State(st): State<AppState>,
    body: Result<axum::Json<OwnerAction>, JsonRejection>,
) -> Response {
    let action = match body {
        Ok(axum::Json(a)) => a,
        Err(e) => {
            // Too large stays 413; anything else that isn't an action is a bad request.
            let status = match e.status() {
                StatusCode::PAYLOAD_TOO_LARGE => StatusCode::PAYLOAD_TOO_LARGE,
                _ => StatusCode::BAD_REQUEST,
            };
            return error(status, &e.body_text());
        }
    };
    with_store(&st, move |s, st| {
        let ctx = owner::ActCtx {
            home: &st.home,
            cfg: &st.cfg,
            now: jiff::Timestamp::now(),
            dir: None,
        };
        let reply = owner::act(s, &ctx, &action)?;
        Ok::<Value, Error>(json!({ "reply": reply }))
    })
    .await
}

/// A static file; any other non-API path is the app itself (client-side routes).
async fn asset(State(st): State<AppState>, req: Request<Body>) -> Response {
    let path = req.uri().path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    let found = st.assets.get(path).or_else(|| st.assets.get("index.html"));
    match found {
        Some((bytes, mime)) => (
            [(header::CONTENT_TYPE, mime)],
            Body::from(bytes.into_owned()),
        )
            .into_response(),
        None => error(StatusCode::NOT_FOUND, "not found"),
    }
}

#[cfg(test)]
mod tests;
