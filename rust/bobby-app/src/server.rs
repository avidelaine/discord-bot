// The HTTP + SSE contract of app/server.js on 127.0.0.1: the Tauri window loads it, and any browser can too.
use crate::journal;
use crate::metrics::{self, Db};
use crate::supervisor::{now_ms, Bus, Supervisor};
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::sse::{Event as SseEvent, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{stream, Stream, StreamExt};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;

// ponytail: the UI is compiled in from app/ui (single source while the Node app still exists); move it here when app/ goes
const INDEX: &str = include_str!("../../../app/ui/index.html");

#[derive(Clone)]
pub struct AppState {
    pub sup: Arc<Supervisor>,
    pub db: Db,
    pub bus: Bus,
    pub logs: PathBuf,
}

type Q = Query<HashMap<String, String>>;

fn no_store(code: StatusCode, body: Value) -> Response {
    (code, [(header::CACHE_CONTROL, "no-store")], Json(body)).into_response()
}

fn num(q: &HashMap<String, String>, k: &str) -> Option<f64> {
    q.get(k).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| *v != 0.0 && v.is_finite())
}

/// A web page open in the browser could POST here (no preflight for a plain POST): only the app's own pages may.
pub fn origin_ok(origin: Option<&str>) -> bool {
    let Some(o) = origin else { return true };
    let Some(rest) = o.strip_prefix("http://127.0.0.1:").or_else(|| o.strip_prefix("http://localhost:")) else { return false };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit())
}

async fn action(State(s): State<AppState>, UrlPath(what): UrlPath<String>, headers: HeaderMap) -> Response {
    if !origin_ok(headers.get(header::ORIGIN).and_then(|v| v.to_str().ok())) {
        return no_store(StatusCode::FORBIDDEN, json!({ "error": "forbidden origin" }));
    }
    if !matches!(what.as_str(), "start" | "stop" | "restart") {
        return no_store(StatusCode::NOT_FOUND, json!({ "error": "not found" }));
    }
    let sup = s.sup.clone();
    // stop waits for taskkill: keep it off the async workers
    let res = tokio::task::spawn_blocking(move || sup.act(&what)).await.unwrap_or_else(|e| Err(e.to_string()));
    match res {
        Ok(st) => no_store(StatusCode::OK, st),
        Err(e) => no_store(StatusCode::CONFLICT, json!({ "error": e, "status": s.sup.status() })),
    }
}

async fn status(State(s): State<AppState>) -> Response {
    no_store(StatusCode::OK, s.sup.status())
}

async fn live(State(s): State<AppState>) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let first = SseEvent::default().event("status").data(s.sup.status().to_string());
    let rest = BroadcastStream::new(s.bus.subscribe())
        .filter_map(|e| async move { e.ok().map(|e| Ok(SseEvent::default().event(e.name).data(e.data.to_string()))) });
    Sse::new(stream::once(async { Ok(first) }).chain(rest)).keep_alive(KeepAlive::default())
}

async fn metrics_h(State(s): State<AppState>, Query(q): Q) -> Response {
    let to = num(&q, "to").map_or(now_ms(), |v| v as i64);
    let from = num(&q, "from").map_or(to - 3_600_000, |v| v as i64);
    let step = num(&q, "step").unwrap_or(10.0);
    let db = s.db.clone();
    match tokio::task::spawn_blocking(move || metrics::history(&db.lock().unwrap(), from, to, step)).await {
        Ok(Ok(rows)) => no_store(StatusCode::OK, json!(rows)),
        Ok(Err(e)) => no_store(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e.to_string() })),
        Err(e) => no_store(StatusCode::INTERNAL_SERVER_ERROR, json!({ "error": e.to_string() })),
    }
}

async fn journal_h(State(s): State<AppState>, Query(q): Q) -> Response {
    let limit = num(&q, "limit").unwrap_or(500.0).clamp(1.0, 5000.0) as usize;
    let before = journal::iso_or_null(q.get("before").map(String::as_str));
    let from = journal::iso_or_null(q.get("from").map(String::as_str));
    let path = s.logs.join("conversations.jsonl");
    let items = tokio::task::spawn_blocking(move || journal::read(&path, limit, before.as_deref(), from.as_deref())).await.unwrap_or_default();
    no_store(StatusCode::OK, json!({ "stats": journal::stats(&items), "items": items }))
}

async fn logs_h(State(s): State<AppState>, Query(q): Q) -> Response {
    let file = match q.get("part").map(String::as_str) {
        Some("bot") => "bot.log",
        Some("voice") => "voice.log",
        Some("llm") => "llm.log",
        Some("app") => "bobby.log",
        _ => return no_store(StatusCode::BAD_REQUEST, json!({ "error": "part must be bot, voice, llm or app" })),
    };
    let n = num(&q, "lines").unwrap_or(300.0).clamp(1.0, 5000.0) as usize;
    let path = s.logs.join(file);
    let lines = tokio::task::spawn_blocking(move || journal::tail_lines(&path, n)).await.unwrap_or_default();
    no_store(StatusCode::OK, json!({ "lines": lines }))
}

pub fn router(state: AppState, bridge: Router) -> Router {
    Router::new()
        .route("/", get(|| async { ([(header::CACHE_CONTROL, "no-store")], Html(INDEX)) }))
        .route("/index.html", get(|| async { ([(header::CACHE_CONTROL, "no-store")], Html(INDEX)) }))
        .route("/api/{action}", post(action))
        .route("/api/status", get(status))
        .route("/api/live", get(live))
        .route("/api/metrics", get(metrics_h))
        .route("/api/journal", get(journal_h))
        .route("/api/logs", get(logs_h))
        .with_state(state)
        .merge(bridge)
        .fallback(|| async { no_store(StatusCode::NOT_FOUND, json!({ "error": "not found" })) })
}

pub async fn bind(port: u16) -> std::io::Result<tokio::net::TcpListener> {
    tokio::net::TcpListener::bind(("127.0.0.1", port)).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_check() {
        assert!(origin_ok(None));
        assert!(origin_ok(Some("http://127.0.0.1:4100")));
        assert!(origin_ok(Some("http://localhost:1430")));
        assert!(!origin_ok(Some("http://127.0.0.1:")));
        assert!(!origin_ok(Some("https://evil.example")));
        assert!(!origin_ok(Some("http://127.0.0.1:4100.evil.example")));
        assert!(!origin_ok(Some("null")));
    }
}
