pub mod api;
pub mod sse;

use crate::config::Config;
use crate::db::Db;
use axum::Router;
use std::sync::Arc;
use tokio::sync::broadcast;

/// Server-sent event payload for real-time UI updates.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "type")]
pub enum SseEvent {
    #[serde(rename = "message_processed")]
    MessageProcessed { session_id: String, channel: String },
    #[serde(rename = "queue_update")]
    QueueUpdate { pending: u64 },
    #[serde(rename = "stream_chunk")]
    StreamChunk {
        session_id: String,
        channel: String,
        text: String,
    },
    #[serde(rename = "stream_end")]
    StreamEnd { session_id: String, channel: String },
}

/// Shared application state for all web handlers.
#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub config: Arc<Config>,
    pub event_tx: broadcast::Sender<SseEvent>,
}

/// Build the axum router with all API routes and static file serving.
pub fn build_router(state: AppState) -> Router {
    Router::new()
        .nest("/api", api::routes())
        .route("/api/events", axum::routing::get(sse::events_handler))
        .fallback(static_handler)
        .with_state(state)
}

/// Serve embedded static files (SPA fallback).
async fn static_handler(uri: axum::http::Uri) -> impl axum::response::IntoResponse {
    // Try to serve the requested path from embedded assets
    let path = uri.path().trim_start_matches('/');

    // For SPA routing, return index.html for non-API, non-asset paths
    let (content, mime_path) = if path.is_empty() || !path.contains('.') {
        (StaticAssets::get("index.html"), "index.html")
    } else {
        (
            StaticAssets::get(path).or_else(|| StaticAssets::get("index.html")),
            path,
        )
    };

    match content {
        Some(file) => {
            let mime = mime_guess::from_path(mime_path).first_or_octet_stream();
            (
                [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                file.data.to_vec(),
            )
                .into_response()
        }
        None => (axum::http::StatusCode::NOT_FOUND, "Not found").into_response(),
    }
}

use axum::response::IntoResponse;

#[derive(rust_embed::Embed)]
#[folder = "web/dist/"]
struct StaticAssets;

/// Start the web server if enabled in config.
pub async fn start_server(
    db: Db,
    config: Arc<Config>,
    event_tx: broadcast::Sender<SseEvent>,
) -> Result<(), anyhow::Error> {
    let bind = &config.web.bind;
    let port = config.web.port;
    let addr = format!("{}:{}", bind, port);

    let state = AppState {
        db,
        config: config.clone(),
        event_tx,
    };

    let app = build_router(state).layer(
        tower_http::cors::CorsLayer::new()
            .allow_origin(tower_http::cors::Any)
            .allow_methods(tower_http::cors::Any)
            .allow_headers(tower_http::cors::Any),
    );

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("Web UI available at http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    fn test_state() -> AppState {
        let db = Db::open_memory().unwrap();
        let config = crate::config::parse_config(
            r#"
[agent]
model = "test"
api_key = "test"
"#,
        )
        .unwrap();
        let (event_tx, _) = broadcast::channel(16);
        AppState {
            db,
            config: Arc::new(config),
            event_tx,
        }
    }

    #[tokio::test]
    async fn test_api_sessions() {
        let state = test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/sessions")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_queue() {
        let state = test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/queue")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_budget() {
        let state = test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/budget")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_api_audit() {
        let state = test_state();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/audit")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }
}
