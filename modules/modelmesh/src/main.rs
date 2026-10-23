//! ModelMesh - external module for AI model routing and inference management.
//!
//! This binary exposes the full ModelMesh API as HTTP endpoints.
//! It manages its own SQLite database and integrates with the MLHSM
//! module platform through standard HTTP calls.

mod db;
mod discovery;
mod jev;
mod provider;
mod routing;
mod usage;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::routing::{get, post, put};
use axum::{Json, Router, extract::State, extract::Path, extract::Query};
use axum::http::StatusCode;
use db::ModelMeshDb;
use tracing_subscriber::EnvFilter;

#[derive(Debug, serde::Deserialize)]
struct QueryParams { limit: Option<u32> }

#[derive(Debug, serde::Deserialize)]
struct TestRoutingRequest {
    messages: Option<Vec<serde_json::Value>>,
    model: Option<String>,
    temperature: Option<f64>,
    max_tokens: Option<u32>,
}

#[derive(Debug, serde::Deserialize)]
struct JevConfigRequest { api_key: Option<String> }

mod ai_types {
    pub use hsm_model::ai::*;
}
use ai_types::*;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();

    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(18_766);

    let data_dir: PathBuf = std::env::var("MODELMECH_DATA_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap().join("data").join("modelmesh"));

    std::fs::create_dir_all(&data_dir).expect("Failed to create data directory");
    let db_path = data_dir.join("modelmesh.db");
    let db = ModelMeshDb::open(&db_path).expect("Failed to open database");

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    println!("MODELMECH_PORT={port}");
    std::io::Write::flush(&mut std::io::stdout()).ok();
    eprintln!("modelmesh on http://127.0.0.1:{port}/ (PID={})", std::process::id());

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/v1/providers", get(list_providers).post(create_provider))
        .route("/api/v1/providers/{id}", put(update_provider).delete(delete_provider))
        .route("/api/v1/providers/{id}/test", post(test_provider))
        .route("/api/v1/models", get(list_models).post(create_model))
        .route("/api/v1/models/{id}", put(update_model).delete(delete_model))
        .route("/api/v1/routing/decisions", get(list_decisions))
        .route("/api/v1/routing/test", post(test_routing))
        .route("/api/v1/route", post(route_request))
        .route("/api/v1/usage", get(list_usage))
        .route("/api/v1/usage/stats", get(get_usage_stats))
        .route("/api/v1/chat", post(chat_completion))
        .route("/api/v1/discovery", get(discovery_status))
        .route("/api/v1/sync", post(sync_models))
        .route("/api/v1/jev/config", get(get_jev_config).put(set_jev_config))
        .route("/api/v1/jev/test", post(test_jev))
        .with_state(db);

    let listener = tokio::net::TcpListener::bind(addr).await.expect("Failed to bind");
    axum::serve(listener, app).await.expect("ModelMesh server error");
}

// ── Health ──────────────────────────────────────────────────────────────────

#[axum::debug_handler]
async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "healthy", "module": "modelmesh" }))
}

// ── Providers ───────────────────────────────────────────────────────────────

#[axum::debug_handler]
async fn list_providers(State(db): State<ModelMeshDb>) -> Result<Json<Vec<Provider>>, (StatusCode, String)> {
    let providers = db.list_providers().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    Ok(Json(providers))
}

#[axum::debug_handler]
async fn create_provider(State(db): State<ModelMeshDb>, Json(req): Json<CreateProviderRequest>) -> Result<Json<Provider>, (StatusCode, String)> {
    let provider = db.create_provider(&req).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    Ok(Json(provider))
}

#[axum::debug_handler]
async fn update_provider(State(db): State<ModelMeshDb>, Path(id): Path<String>, Json(req): Json<UpdateProviderRequest>) -> Result<Json<Provider>, (StatusCode, String)> {
    let provider = db.update_provider(&id, &req).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Provider nicht gefunden".into()))?;
    Ok(Json(provider))
}

#[axum::debug_handler]
async fn delete_provider(State(db): State<ModelMeshDb>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let removed = db.delete_provider(&id).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    if !removed { return Err((StatusCode::NOT_FOUND, "Provider nicht gefunden".into())); }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[axum::debug_handler]
async fn test_provider(State(db): State<ModelMeshDb>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let provider = db.get_provider(&id).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Provider nicht gefunden".into()))?;
    let key = std::env::var("MODELMECH_API_KEY").ok().unwrap_or_default();
    match provider::test_provider(&provider.base_url, &key).await {
        Ok(models) => Ok(Json(serde_json::json!({ "ok": true, "models": models }))),
        Err(e) => Ok(Json(serde_json::json!({ "ok": false, "error": e }))),
    }
}

// ── Models ──────────────────────────────────────────────────────────────────

#[axum::debug_handler]
async fn list_models(State(db): State<ModelMeshDb>) -> Result<Json<Vec<Model>>, (StatusCode, String)> {
    let models = db.list_models().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    Ok(Json(models))
}

#[axum::debug_handler]
async fn create_model(State(db): State<ModelMeshDb>, Json(req): Json<CreateModelRequest>) -> Result<Json<Model>, (StatusCode, String)> {
    let model = db.create_model(&req).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    Ok(Json(model))
}

#[axum::debug_handler]
async fn update_model(State(db): State<ModelMeshDb>, Path(id): Path<String>, Json(req): Json<UpdateModelRequest>) -> Result<Json<Model>, (StatusCode, String)> {
    let model = db.update_model(&id, &req).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Model nicht gefunden".into()))?;
    Ok(Json(model))
}

#[axum::debug_handler]
async fn delete_model(State(db): State<ModelMeshDb>, Path(id): Path<String>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let removed = db.delete_model(&id).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    if !removed { return Err((StatusCode::NOT_FOUND, "Model nicht gefunden".into())); }
    Ok(Json(serde_json::json!({ "ok": true })))
}

// ── Routing ─────────────────────────────────────────────────────────────────

#[axum::debug_handler]
async fn list_decisions(State(db): State<ModelMeshDb>, Query(query): Query<QueryParams>) -> Result<Json<Vec<RoutingDecision>>, (StatusCode, String)> {
    let limit = query.limit.unwrap_or(50).min(500);
    let decisions = db.list_routing_decisions(limit as usize).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    Ok(Json(decisions))
}

#[axum::debug_handler]
async fn test_routing(State(db): State<ModelMeshDb>, Json(req): Json<TestRoutingRequest>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let models = db.list_models().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    let providers = db.list_providers().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    let mut routable: Vec<routing::RoutableModel> = Vec::new();
    for model in &models {
        if let Some(provider) = providers.iter().find(|p| p.id == model.provider_id) {
            let api_key = std::env::var(format!("MODELMECH_API_KEY_{}", model.provider_id)).unwrap_or_default();
            if !model.enabled || provider.base_url.is_empty() { continue; }
            routable.push(routing::RoutableModel { model: model.clone(), provider_id: provider.id.clone(), provider_name: provider.name.clone(), base_url: provider.base_url.clone(), api_key });
        }
    }
    let jev_key = std::env::var("MODELMECH_JEV_KEY").ok().unwrap_or_default();
    let chat_messages: Vec<ChatMessage> = req.messages.unwrap_or_default().into_iter().filter_map(|m| {
        let role = m.get("role")?.as_str()?.to_string();
        let content = m.get("content").and_then(|c| c.as_str()).map(String::from);
        Some(ChatMessage { role, content, tool_calls: None, tool_call_id: None })
    }).collect();
    let request = ChatCompletionRequest { messages: chat_messages, model: req.model, temperature: req.temperature, max_tokens: req.max_tokens, tools: None, tool_choice: None, task_category: None, priority: None, stream: None };
    let result = routing::route_request(request.clone(), &routable, &jev_key).await;
    Ok(Json(serde_json::json!({ "success": result.decision.is_some(), "decision": result.decision, "error": result.error })))
}

#[axum::debug_handler]
async fn route_request(State(db): State<ModelMeshDb>, Json(req): Json<ChatCompletionRequest>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let models = db.list_models().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    let providers = db.list_providers().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    let mut routable: Vec<routing::RoutableModel> = Vec::new();
    for model in &models {
        if let Some(provider) = providers.iter().find(|p| p.id == model.provider_id) {
            let api_key = std::env::var(format!("MODELMECH_API_KEY_{}", model.provider_id)).unwrap_or_default();
            if !model.enabled || provider.base_url.is_empty() { continue; }
            routable.push(routing::RoutableModel { model: model.clone(), provider_id: provider.id.clone(), provider_name: provider.name.clone(), base_url: provider.base_url.clone(), api_key });
        }
    }
    let jev_key = std::env::var("MODELMECH_JEV_KEY").ok().unwrap_or_default();
    let result = routing::route_request(req.clone(), &routable, &jev_key).await;
    if let Some(decision) = &result.decision { let _ = db.record_routing_decision(decision); }
    Ok(Json(serde_json::json!({ "success": result.decision.is_some(), "decision": result.decision, "error": result.error })))
}

// ── Usage ───────────────────────────────────────────────────────────────────

#[axum::debug_handler]
async fn list_usage(State(db): State<ModelMeshDb>, Query(query): Query<QueryParams>) -> Result<Json<Vec<UsageRecord>>, (StatusCode, String)> {
    let limit = query.limit.unwrap_or(50).min(500);
    let records = db.list_usage(limit as usize).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    Ok(Json(records))
}

#[axum::debug_handler]
async fn get_usage_stats(State(db): State<ModelMeshDb>) -> Result<Json<UsageStats>, (StatusCode, String)> {
    let stats = db.get_usage_stats().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    Ok(Json(stats))
}

// ── Chat ────────────────────────────────────────────────────────────────────

#[axum::debug_handler]
async fn chat_completion(State(db): State<ModelMeshDb>, Json(req): Json<ChatCompletionRequest>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let models = db.list_models().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    let providers = db.list_providers().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    let mut routable: Vec<routing::RoutableModel> = Vec::new();
    for model in &models {
        if let Some(provider) = providers.iter().find(|p| p.id == model.provider_id) {
            let api_key = std::env::var(format!("MODELMECH_API_KEY_{}", model.provider_id)).unwrap_or_default();
            if !model.enabled || provider.base_url.is_empty() { continue; }
            routable.push(routing::RoutableModel { model: model.clone(), provider_id: provider.id.clone(), provider_name: provider.name.clone(), base_url: provider.base_url.clone(), api_key });
        }
    }
    let jev_key = std::env::var("MODELMECH_JEV_KEY").ok().unwrap_or_default();
    let result = routing::route_request(req.clone(), &routable, &jev_key).await;
    Ok(Json(serde_json::json!({ "success": result.decision.is_some(), "decision": result.decision, "error": result.error })))
}

// ── Discovery ───────────────────────────────────────────────────────────────

#[axum::debug_handler]
async fn discovery_status(State(db): State<ModelMeshDb>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let models = db.list_models().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    let providers = db.list_providers().map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("DB error: {e}")))?;
    let free_models = models.iter().filter(|m| m.cost_per_1k_input == Some(0.0) && m.cost_per_1k_output == Some(0.0)).count();
    let openrouter_provider = providers.iter().find(|p| p.id == "openrouter");
    Ok(Json(serde_json::json!({
        "total_models": models.len(), "free_models": free_models, "total_providers": providers.len(),
        "openrouter_configured": openrouter_provider.is_some(),
        "openrouter_provider": openrouter_provider.map(|p| serde_json::json!({ "name": p.name, "base_url": p.base_url, "enabled": p.enabled })),
        "sync_available": openrouter_provider.is_some(),
    })))
}

#[axum::debug_handler]
async fn sync_models(State(db): State<ModelMeshDb>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let all_models = discovery::discover_all_free_models(None).await;
    let mut synced = 0usize;
    let mut skipped = 0usize;
    let mut providers_seen = std::collections::HashSet::new();
    for (source, models) in &all_models {
        let existing = db.list_providers().unwrap_or_default();
        if !existing.iter().any(|p| p.id == source.as_str()) {
            if let Some(req) = discovery::provider_for_source(source) { let _ = db.create_provider(&req); }
        }
        for (_id, model) in models {
            let existing = db.list_models().unwrap_or_default();
            let already_exists = existing.iter().any(|m| m.name == model.name && m.provider_id == model.provider_id);
            if already_exists { skipped += 1; }
            else {
                let req = CreateModelRequest { provider_id: model.provider_id.clone(), name: model.name.clone(), display_name: model.display_name.clone(), context_window: model.context_window, cost_per_1k_input: model.cost_per_1k_input, cost_per_1k_output: model.cost_per_1k_output, capabilities: model.capabilities.clone() };
                let _ = db.create_model(&req);
                synced += 1;
                providers_seen.insert(source.clone());
            }
        }
    }
    Ok(Json(serde_json::json!({ "ok": true, "models_fetched": all_models.iter().map(|(_, m)| m.len()).sum::<usize>(), "models_synced": synced, "models_skipped": skipped, "providers_discovered": providers_seen.into_iter().collect::<Vec<_>>() })))
}

// ── JEV ─────────────────────────────────────────────────────────────────────

#[axum::debug_handler]
async fn get_jev_config() -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let key = std::env::var("MODELMECH_JEV_KEY").ok();
    Ok(Json(serde_json::json!({ "configured": key.is_some(), "has_key": key.is_some() })))
}

#[axum::debug_handler]
async fn set_jev_config(Json(req): Json<JevConfigRequest>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    if let Some(key) = req.api_key { std::env::set_var("MODELMECH_JEV_KEY", key); }
    Ok(Json(serde_json::json!({ "ok": true })))
}

#[axum::debug_handler]
async fn test_jev() -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let key = std::env::var("MODELMECH_JEV_KEY").map_err(|_| (StatusCode::BAD_REQUEST, "Kein OpenJEV-Key konfiguriert".to_string()))?;
    match jev::test_jev_connection(&key).await {
        Ok(msg) => Ok(Json(serde_json::json!({ "ok": true, "message": msg }))),
        Err(e) => Ok(Json(serde_json::json!({ "ok": false, "message": e }))),
    }
}
