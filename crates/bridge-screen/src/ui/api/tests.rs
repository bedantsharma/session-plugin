//! The dashboard API through `tower::ServiceExt::oneshot`: no sockets.

use std::borrow::Cow;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use bridge_screen_core::config::Config;
use bridge_screen_core::paths::Home;
use bridge_screen_core::repo::RepoInfo;
use bridge_screen_core::session;
use bridge_screen_core::store::{EventDraft, Store, append_in_at, read_all};
use http_body_util::BodyExt as _;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Value, json};
use tower::ServiceExt as _;

use super::{AppState, AssetSource, router};

const TOKEN: &str = "0123abcd";
const PORT: u16 = 4321;
const HOST: &str = "127.0.0.1:4321";

/// Serves the build-instructions page, like a binary built without `web/dist`.
struct NoBuild;

impl AssetSource for NoBuild {
    fn get(&self, path: &str) -> Option<(Cow<'static, [u8]>, String)> {
        (path == "index.html").then(|| {
            (
                Cow::Borrowed(crate::ui::dist::PLACEHOLDER.as_bytes()),
                "text/html".to_string(),
            )
        })
    }
}

struct Fixture {
    home: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let f = Self {
            home: tempfile::tempdir().unwrap(),
        };
        let s = f.store();
        s.register_repo(&RepoInfo {
            id: "r1".into(),
            name: "shop".into(),
            root: "/code/shop".into(),
        })
        .unwrap();
        f
    }

    fn store(&self) -> Store {
        Store::open(&self.home.path().join("bridge-screen.db")).unwrap()
    }

    fn state(&self) -> AppState {
        AppState {
            home: Home::at(self.home.path()),
            cfg: Config::default(),
            token: TOKEN.into(),
            port: PORT,
            assets: Arc::new(NoBuild),
        }
    }

    fn events(&self) -> usize {
        read_all(self.store().conn(), None).unwrap().len()
    }

    /// Session `red1` 40 minutes into drift, red.
    fn red_session(&self) {
        let mut s = self.store();
        let now = Timestamp::now();
        let ago = |min: i64| now - SignedDuration::from_mins(min);
        let script = [
            (
                "session_started",
                json!({"cwd": "/w", "source": "startup"}),
                ago(50),
            ),
            ("prompt", json!({}), ago(45)),
            ("unit_stopped", json!({"score": 0.9}), ago(5)),
            (
                "unit_labeled",
                json!({"unit_no": 1, "label": "drift", "source": "agent"}),
                ago(5),
            ),
            (
                "nav_state_changed",
                json!({"from": "green", "to": "red", "away_secs": 2400}),
                ago(5),
            ),
        ];
        s.write(|tx| {
            for (kind, data, ts) in script {
                append_in_at(tx, EventDraft::new("r1", "red1", kind, data), ts)?;
            }
            Ok(())
        })
        .unwrap();
    }

    async fn send(&self, req: Request<Body>) -> (StatusCode, String) {
        let res = router(self.state()).oneshot(req).await.unwrap();
        let status = res.status();
        let body = res.into_body().collect().await.unwrap().to_bytes();
        (status, String::from_utf8(body.to_vec()).unwrap())
    }

    async fn get(&self, path: &str) -> (StatusCode, String) {
        self.send(
            Request::get(path)
                .header("host", HOST)
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
    }

    async fn act(&self, action: &Value) -> (StatusCode, Value) {
        let (status, body) = self
            .send(
                Request::post("/api/action")
                    .header("host", HOST)
                    .header("authorization", format!("Bearer {TOKEN}"))
                    .header("content-type", "application/json")
                    .body(Body::from(action.to_string()))
                    .unwrap(),
            )
            .await;
        (status, serde_json::from_str(&body).unwrap())
    }
}

fn continue_red1() -> Value {
    json!({"kind": "drift", "session": "red1", "answer": "continue"})
}

#[tokio::test]
async fn api_rejects_untrusted_requests() {
    let f = Fixture::new();
    f.red_session();
    let before = f.events();
    let action = continue_red1().to_string();
    let post = |host: &str, auth: Option<&str>, ctype: &str| {
        let mut b = Request::post("/api/action")
            .header("host", host)
            .header("content-type", ctype)
            .header("origin", "https://evil.example");
        if let Some(a) = auth {
            b = b.header("authorization", a);
        }
        b.body(Body::from(action.clone())).unwrap()
    };
    // No token, a wrong token: 401.
    let (s, _) = f.send(post(HOST, None, "application/json")).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = f
        .send(post(HOST, Some("Bearer nope"), "application/json"))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = f
        .send(post(
            HOST,
            Some(&format!("Bearer {TOKEN}x")),
            "application/json",
        ))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // A foreign Host (DNS rebinding), even with the token: 403.
    let (s, _) = f
        .send(post(
            "evil.example:4321",
            Some(&format!("Bearer {TOKEN}")),
            "application/json",
        ))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = f
        .send(post(
            "127.0.0.1:9999",
            Some(&format!("Bearer {TOKEN}")),
            "application/json",
        ))
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // A cross-site form POST carries no token.
    let (s, _) = f
        .send(post(HOST, None, "application/x-www-form-urlencoded"))
        .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    // GETs are guarded the same way; no CORS headers are ever sent.
    let res = router(f.state())
        .oneshot(
            Request::get("/api/glance")
                .header("host", HOST)
                .header("origin", "https://evil.example")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    assert!(res.headers().get("access-control-allow-origin").is_none());
    let (s, _) = f
        .send(
            Request::get("/")
                .header("host", "attacker.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    assert_eq!(f.events(), before, "nothing recorded");
    // localhost:<port> is fine.
    let (s, _) = f
        .send(
            Request::get("/api/ping")
                .header("host", "localhost:4321")
                .header("authorization", format!("Bearer {TOKEN}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn glance_and_version_return_the_models() {
    let f = Fixture::new();
    f.red_session();
    let (s, body) = f.get("/api/glance").await;
    assert_eq!(s, StatusCode::OK);
    let g: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(g["needs_you_count"], 1);
    assert_eq!(g["needs_you"][0]["kind"], "drift_red");
    assert_eq!(g["needs_you"][0]["actions"][0]["action"], continue_red1());
    assert_eq!(g["projects"][0]["name"], "shop");
    let (s, body) = f.get("/api/version").await;
    assert_eq!(s, StatusCode::OK);
    let v: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(v["last_event_pos"], g["last_event_pos"]);
    let (s, body) = f.get("/api/ping").await;
    assert_eq!((s, body.contains("\"ok\":true")), (StatusCode::OK, true));
    let (s, _) = f.get("/api/nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn an_action_is_recorded_and_a_stale_one_is_409() {
    let f = Fixture::new();
    f.red_session();
    let before = f.events();
    let (s, body) = f.act(&continue_red1()).await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(body["reply"], "Told red1: continue.");
    let events = read_all(f.store().conn(), None).unwrap();
    assert_eq!(events[before].kind, "owner_action");
    assert_eq!(events[before].session_id, "owner");
    let s1 = session::get(f.store().conn(), "red1").unwrap().unwrap();
    assert!(s1.pending_reply.unwrap().starts_with("continue:"));
    // A second click (another tab): already resolved, nothing recorded.
    let after = f.events();
    let (s, body) = f.act(&continue_red1()).await;
    assert_eq!(s, StatusCode::CONFLICT);
    assert!(
        body["error"]
            .as_str()
            .unwrap()
            .starts_with("already resolved")
    );
    assert_eq!(f.events(), after);
    // An invalid reason: 400, nothing recorded.
    let (s, body) = f
        .act(&json!({"kind": "drift", "session": "red1", "answer": "continue", "reason": "ok"}))
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{body}");
    // Not an action at all: 400.
    let (s, _) = f.act(&json!({"kind": "launch_rockets"})).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert_eq!(f.events(), after);
}

#[tokio::test]
async fn bodies_over_64_kb_are_refused() {
    let f = Fixture::new();
    let big = json!({"kind": "reset_drift", "repo": "x".repeat(70 * 1024)});
    let (s, _) = f
        .send(
            Request::post("/api/action")
                .header("host", HOST)
                .header("authorization", format!("Bearer {TOKEN}"))
                .header("content-type", "application/json")
                .body(Body::from(big.to_string()))
                .unwrap(),
        )
        .await;
    assert_eq!(s, StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn ui_without_built_assets() {
    let f = Fixture::new();
    let page = |path: &'static str| {
        Request::get(path)
            .header("host", HOST)
            .body(Body::empty())
            .unwrap()
    };
    for path in ["/", "/some/client/route"] {
        let (s, body) = f.send(page(path)).await;
        assert_eq!(s, StatusCode::OK);
        assert!(
            body.contains("npm --prefix web ci &amp;&amp; npm --prefix web run build"),
            "{body}"
        );
    }
    // The API keeps working.
    let (s, body) = f.get("/api/glance").await;
    assert_eq!(s, StatusCode::OK);
    assert!(body.contains("\"needs_you_count\":0"), "{body}");
}

#[tokio::test]
async fn drill_endpoints_return_the_models() {
    let f = Fixture::new();
    f.red_session();
    let (s, body) = f.get("/api/project/r1").await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let p: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(p["name"], "shop");
    let (s, body) = f.get("/api/session/red1").await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let d: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(d["units"][0]["label"], "drift");
    let (s, _) = f.get("/api/session/nope").await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, body) = f.get("/api/task/r1/1.1").await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let (s, body) = f.get("/api/tuning/r1").await;
    assert_eq!(s, StatusCode::OK, "{body}");
    assert_eq!(
        serde_json::from_str::<Value>(&body).unwrap()["tuned"],
        false
    );
}

#[tokio::test]
async fn history_pages_and_filters() {
    let f = Fixture::new();
    f.red_session();
    let (s, body) = f.get("/api/history/r1?limit=2").await;
    assert_eq!(s, StatusCode::OK, "{body}");
    let p1: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(p1["events"].as_array().unwrap().len(), 2);
    assert_eq!(p1["events"][0]["kind"], "nav_state_changed");
    let before = p1["next_before"].as_i64().unwrap();
    let (_, body) = f
        .get(&format!("/api/history/r1?limit=2&before={before}"))
        .await;
    let p2: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(p2["events"][0]["kind"], "unit_stopped");
    let (_, body) = f.get("/api/history/r1?kind=prompt&session=red1").await;
    let only: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(only["events"].as_array().unwrap().len(), 1);
    assert_eq!(only["next_before"], Value::Null);
    // At most 200 per page, whatever is asked.
    let (s, _) = f.get("/api/history/r1?limit=100000").await;
    assert_eq!(s, StatusCode::OK);
}
