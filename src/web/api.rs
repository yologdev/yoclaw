use super::AppState;
use axum::extract::{Path, Query, State};
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/sessions", get(list_sessions))
        .route("/sessions/{id}/messages", get(get_session_messages))
        .route("/queue", get(queue_status))
        .route("/budget", get(budget_status))
        .route("/audit", get(audit_log))
}

#[derive(Serialize)]
struct SessionInfo {
    session_id: String,
    message_count: u64,
    created_at: u64,
    updated_at: u64,
}

async fn list_sessions(State(state): State<AppState>) -> Result<Json<Vec<SessionInfo>>, AppError> {
    let sessions = state.db.tape_list_sessions().await?;
    let result: Vec<SessionInfo> = sessions
        .into_iter()
        .map(|s| SessionInfo {
            session_id: s.session_id,
            message_count: s.message_count as u64,
            created_at: s.created_at,
            updated_at: s.updated_at,
        })
        .collect();
    Ok(Json(result))
}

async fn get_session_messages(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, AppError> {
    let messages = state.db.tape_load_messages(&id).await?;
    let json = serde_json::to_value(&messages)?;
    Ok(Json(json))
}

#[derive(Serialize)]
struct QueueStatus {
    pending: usize,
}

async fn queue_status(State(state): State<AppState>) -> Result<Json<QueueStatus>, AppError> {
    let pending = state.db.queue_pending_count().await?;
    Ok(Json(QueueStatus { pending }))
}

#[derive(Serialize)]
struct BudgetStatus {
    tokens_used_today: u64,
    daily_limit: Option<u64>,
    remaining: Option<u64>,
}

async fn budget_status(State(state): State<AppState>) -> Result<Json<BudgetStatus>, AppError> {
    let used = state.db.audit_token_usage_today().await?;
    let limit = state.config.agent.budget.max_tokens_per_day;
    let remaining = limit.map(|l| l.saturating_sub(used));
    Ok(Json(BudgetStatus {
        tokens_used_today: used,
        daily_limit: limit,
        remaining,
    }))
}

#[derive(Deserialize)]
struct AuditQuery {
    session: Option<String>,
    limit: Option<usize>,
}

#[derive(Serialize)]
struct AuditEntryResponse {
    id: i64,
    session_id: String,
    event_type: String,
    tool_name: Option<String>,
    detail: Option<String>,
    tokens_used: u64,
    timestamp: u64,
}

async fn audit_log(
    State(state): State<AppState>,
    Query(q): Query<AuditQuery>,
) -> Result<Json<Vec<AuditEntryResponse>>, AppError> {
    let limit = q.limit.unwrap_or(50);
    let entries = state.db.audit_query(q.session.as_deref(), limit).await?;
    let result: Vec<AuditEntryResponse> = entries
        .into_iter()
        .map(|e| AuditEntryResponse {
            id: e.id.unwrap_or(0),
            session_id: e.session_id.unwrap_or_default(),
            event_type: e.event_type,
            tool_name: e.tool_name,
            detail: e.detail,
            tokens_used: e.tokens_used,
            timestamp: e.timestamp,
        })
        .collect();
    Ok(Json(result))
}

/// Unified error type for API handlers.
struct AppError(anyhow::Error);

impl axum::response::IntoResponse for AppError {
    fn into_response(self) -> axum::response::Response {
        (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            self.0.to_string(),
        )
            .into_response()
    }
}

impl<E: Into<anyhow::Error>> From<E> for AppError {
    fn from(err: E) -> Self {
        Self(err.into())
    }
}
