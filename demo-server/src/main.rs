use axum::{
    extract::{Json, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect},
    routing::{delete, get, patch, post},
    Router,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

// --- State ---

#[derive(Clone)]
struct AppState {
    users: Arc<Mutex<HashMap<String, User>>>,
    tokens: Arc<Mutex<HashMap<String, String>>>, // token -> email
}

impl AppState {
    fn new() -> Self {
        Self {
            users: Arc::new(Mutex::new(HashMap::new())),
            tokens: Arc::new(Mutex::new(HashMap::new())),
        }
    }
}

// --- Models ---

#[derive(Debug, Clone, Serialize, Deserialize)]
struct User {
    id: String,
    name: String,
    email: String,
    role: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(rename = "createdAt")]
    created_at: String,
    #[serde(rename = "deletedAt")]
    deleted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateUserRequest {
    name: Option<String>,
    email: Option<String>,
    role: Option<String>,
    #[serde(default)]
    tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct UpdateUserRequest {
    name: Option<String>,
    email: Option<String>,
    role: Option<String>,
    tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    email: Option<String>,
    password: Option<String>,
}

#[derive(Debug, Deserialize)]
struct PaginationParams {
    page: Option<u32>,
    limit: Option<u32>,
    sort: Option<String>,
}

#[derive(Debug, Serialize)]
struct PaginationMeta {
    page: u32,
    limit: u32,
    #[serde(rename = "totalCount")]
    total_count: usize,
}

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    details: Option<Vec<ValidationDetail>>,
}

#[derive(Debug, Serialize)]
struct ValidationDetail {
    field: String,
    message: String,
}

// --- Handlers ---

async fn health() -> impl IntoResponse {
    let request_id = Uuid::new_v4().to_string();
    let mut headers = HeaderMap::new();
    headers.insert("x-request-id", request_id.parse().unwrap());
    (
        StatusCode::OK,
        headers,
        Json(serde_json::json!({"status": "ok"})),
    )
}

async fn plain_text() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("content-type", "text/plain; charset=utf-8")],
        "plain text response",
    )
}

async fn empty_response() -> impl IntoResponse {
    StatusCode::NO_CONTENT
}

async fn unicode_json() -> impl IntoResponse {
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "message": "Привіт, Tarn 👋",
            "emoji": "🌍"
        })),
    )
}

async fn redirect_to_health() -> impl IntoResponse {
    Redirect::temporary("/health")
}

async fn large_response() -> impl IntoResponse {
    let blob = "x".repeat(1024 * 1024);
    (
        StatusCode::OK,
        Json(serde_json::json!({
            "blob": blob,
            "size": 1024 * 1024
        })),
    )
}

async fn html_error_page() -> impl IntoResponse {
    (
        StatusCode::BAD_GATEWAY,
        [("content-type", "text/html; charset=utf-8")],
        Html("<html><body><h1>Upstream failure</h1></body></html>"),
    )
}

async fn login(State(state): State<AppState>, Json(body): Json<LoginRequest>) -> impl IntoResponse {
    let email = match body.email {
        Some(e) => e,
        None => {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!(ErrorResponse {
                    error: "validation_error".into(),
                    message: Some("Missing required fields".into()),
                    details: Some(vec![ValidationDetail {
                        field: "email".into(),
                        message: "email is required".into(),
                    }]),
                })),
            )
                .into_response();
        }
    };

    if body.password.is_none() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!(ErrorResponse {
                error: "validation_error".into(),
                message: Some("Missing required fields".into()),
                details: Some(vec![ValidationDetail {
                    field: "password".into(),
                    message: "password is required".into(),
                }]),
            })),
        )
            .into_response();
    }

    let token = Uuid::new_v4().to_string();
    state.tokens.lock().unwrap().insert(token.clone(), email);

    (StatusCode::OK, Json(serde_json::json!({"token": token}))).into_response()
}

fn extract_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|s| s.to_string())
}

fn check_auth(
    headers: &HeaderMap,
    state: &AppState,
) -> Result<String, (StatusCode, Json<serde_json::Value>)> {
    let token = extract_token(headers).ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized", "message": "Missing authorization token"})),
        )
    })?;

    let tokens = state.tokens.lock().unwrap();
    let email = tokens.get(&token).cloned().ok_or_else(|| {
        (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "unauthorized", "message": "Invalid token"})),
        )
    })?;

    Ok(email)
}

async fn create_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<CreateUserRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&headers, &state) {
        return e.into_response();
    }

    let mut errors = Vec::new();
    if body.name.is_none() || body.name.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
        errors.push(ValidationDetail {
            field: "name".into(),
            message: "name is required".into(),
        });
    }
    if body.email.is_none() || body.email.as_ref().map(|s| s.is_empty()).unwrap_or(false) {
        errors.push(ValidationDetail {
            field: "email".into(),
            message: "email is required".into(),
        });
    }

    // Validate email format
    if let Some(ref email) = body.email {
        if !email.is_empty() && !email.contains('@') {
            errors.push(ValidationDetail {
                field: "email".into(),
                message: "email format is invalid".into(),
            });
        }
    }

    if !errors.is_empty() {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(serde_json::json!(ErrorResponse {
                error: "validation_error".into(),
                message: Some("Validation failed".into()),
                details: Some(errors),
            })),
        )
            .into_response();
    }

    let id = format!("usr_{}", Uuid::new_v4().simple());
    let now = chrono::Utc::now().to_rfc3339();
    let user = User {
        id: id.clone(),
        name: body.name.unwrap(),
        email: body.email.unwrap(),
        role: body.role.unwrap_or_else(|| "viewer".into()),
        tags: body.tags,
        created_at: now,
        deleted_at: None,
    };

    state.users.lock().unwrap().insert(id, user.clone());

    let request_id = Uuid::new_v4().to_string();
    let mut resp_headers = HeaderMap::new();
    resp_headers.insert("x-request-id", request_id.parse().unwrap());

    (
        StatusCode::CREATED,
        resp_headers,
        Json(serde_json::to_value(&user).unwrap()),
    )
        .into_response()
}

async fn get_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&headers, &state) {
        return e.into_response();
    }

    let users = state.users.lock().unwrap();
    match users.get(&user_id) {
        Some(user) => (StatusCode::OK, Json(serde_json::to_value(user).unwrap())).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not_found", "message": "User not found"})),
        )
            .into_response(),
    }
}

async fn update_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
    Json(body): Json<UpdateUserRequest>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&headers, &state) {
        return e.into_response();
    }

    let mut users = state.users.lock().unwrap();
    match users.get_mut(&user_id) {
        Some(user) => {
            if let Some(name) = body.name {
                user.name = name;
            }
            if let Some(email) = body.email {
                user.email = email;
            }
            if let Some(role) = body.role {
                user.role = role;
            }
            if let Some(tags) = body.tags {
                user.tags = tags;
            }
            (StatusCode::OK, Json(serde_json::to_value(user).unwrap())).into_response()
        }
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not_found", "message": "User not found"})),
        )
            .into_response(),
    }
}

async fn delete_user(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&headers, &state) {
        return e.into_response();
    }

    let mut users = state.users.lock().unwrap();
    match users.remove(&user_id) {
        Some(_) => StatusCode::NO_CONTENT.into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "not_found", "message": "User not found"})),
        )
            .into_response(),
    }
}

async fn list_users(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(params): Query<PaginationParams>,
) -> impl IntoResponse {
    if let Err(e) = check_auth(&headers, &state) {
        return e.into_response();
    }

    let users = state.users.lock().unwrap();
    let page = params.page.unwrap_or(1);
    let limit = params.limit.unwrap_or(10);

    let mut all_users: Vec<&User> = users.values().collect();

    // Sort
    if let Some(ref sort) = params.sort {
        let parts: Vec<&str> = sort.split(':').collect();
        let field = parts.first().copied().unwrap_or("name");
        let order = parts.get(1).copied().unwrap_or("asc");

        all_users.sort_by(|a, b| {
            let cmp = match field {
                "name" => a.name.cmp(&b.name),
                "email" => a.email.cmp(&b.email),
                "role" => a.role.cmp(&b.role),
                _ => a.name.cmp(&b.name),
            };
            if order == "desc" {
                cmp.reverse()
            } else {
                cmp
            }
        });
    }

    let total = all_users.len();
    let start = ((page - 1) * limit) as usize;
    let page_users: Vec<&User> = all_users
        .into_iter()
        .skip(start)
        .take(limit as usize)
        .collect();

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "data": page_users,
            "meta": PaginationMeta {
                page,
                limit,
                total_count: total,
            }
        })),
    )
        .into_response()
}

async fn cleanup(State(state): State<AppState>) -> impl IntoResponse {
    state.users.lock().unwrap().clear();
    state.tokens.lock().unwrap().clear();
    (StatusCode::OK, Json(serde_json::json!({"cleaned": true})))
}

// --- App ---

pub fn create_app() -> Router {
    let state = AppState::new();

    Router::new()
        .route("/health", get(health))
        .route("/plain-text", get(plain_text))
        .route("/empty", get(empty_response))
        .route("/unicode", get(unicode_json))
        .route("/redirect-health", get(redirect_to_health))
        .route("/large", get(large_response))
        .route("/html-error", get(html_error_page))
        .route("/auth/login", post(login))
        .route("/users", post(create_user))
        .route("/users", get(list_users))
        .route("/users/{id}", get(get_user))
        .route("/users/{id}", patch(update_user))
        .route("/users/{id}", delete(delete_user))
        .route("/test/cleanup", post(cleanup))
        .with_state(state)
}

#[tokio::main]
async fn main() {
    let port = std::env::var("PORT").unwrap_or_else(|_| "3000".into());
    let addr = format!("0.0.0.0:{}", port);
    println!("Demo server listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    axum::serve(listener, create_app()).await.unwrap();
}
