//! ModelMesh database operations - CRUD for providers, models, routing
//! decisions, and usage records.

use rusqlite::{params, Connection, Result};
use std::sync::Arc;
use ai_types::*;

mod ai_types {
    pub use hsm_model::ai::*;
}

#[derive(Clone)]
pub struct ModelMeshDb { conn: Arc<std::sync::Mutex<Connection>> }

impl ModelMeshDb {

    pub fn open(path: &std::path::Path) -> Result<Self> {
        let conn = Arc::new(std::sync::Mutex::new(Connection::open(path)?));
        conn.lock().unwrap().execute_batch("
            CREATE TABLE IF NOT EXISTS mm_providers (
                id TEXT PRIMARY KEY, name TEXT NOT NULL, kind TEXT NOT NULL DEFAULT 'custom',
                base_url TEXT NOT NULL DEFAULT '', enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS mm_models (
                id TEXT PRIMARY KEY, provider_id TEXT NOT NULL, name TEXT NOT NULL,
                display_name TEXT NOT NULL, enabled INTEGER NOT NULL DEFAULT 1,
                context_window INTEGER NOT NULL DEFAULT 4096,
                cost_per_1k_input REAL, cost_per_1k_output REAL,
                capabilities TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS mm_routing_decisions (
                id TEXT PRIMARY KEY, selected_model_id TEXT NOT NULL,
                selected_provider_id TEXT NOT NULL, provider_name TEXT NOT NULL,
                decision_reason TEXT NOT NULL, task_category TEXT,
                complexity_score REAL NOT NULL DEFAULT 0,
                priority TEXT NOT NULL DEFAULT 'balanced', created_at TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS mm_usage (
                id TEXT PRIMARY KEY, provider_id TEXT NOT NULL, model_id TEXT NOT NULL,
                task_category TEXT, input_tokens INTEGER, output_tokens INTEGER,
                latency_ms INTEGER, cost_usd REAL, success INTEGER NOT NULL DEFAULT 1,
                error_message TEXT, created_at TEXT NOT NULL
            );
        ")?;
        Ok(Self { conn })
    }

    pub fn create_provider(&self, req: &CreateProviderRequest) -> Result<Provider> {
        use chrono::Utc;
        let now = Utc::now().to_rfc3339();
        let id = req.id.clone().unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        self.conn.lock().unwrap().execute(
            "INSERT INTO mm_providers (id, name, kind, base_url, enabled, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
            params![id, &req.name, serde_json::to_string(&req.kind).unwrap(), &req.base_url, now],
        )?;
        Ok(Provider { id, name: req.name.clone(), kind: req.kind.clone(), base_url: req.base_url.clone(), enabled: true, created_at: now.clone(), updated_at: now })
    }

    pub fn list_providers(&self) -> Result<Vec<Provider>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare("SELECT id, name, kind, base_url, enabled, created_at, updated_at FROM mm_providers ORDER BY created_at")?;
        let rows = stmt.query_map([], |row| {
            let kind_str: String = row.get(2)?;
            let kind: ProviderKind = serde_json::from_str(&kind_str).unwrap_or(ProviderKind::Custom);
            Ok(Provider { id: row.get(0)?, name: row.get(1)?, kind, base_url: row.get(3)?, enabled: row.get::<_, i64>(4)? != 0, created_at: row.get(5)?, updated_at: row.get(6)? })
        })?;
        rows.collect()
    }

    pub fn get_provider(&self, id: &str) -> Result<Option<Provider>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare("SELECT id, name, kind, base_url, enabled, created_at, updated_at FROM mm_providers WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], |row| {
            let kind_str: String = row.get(2)?;
            let kind: ProviderKind = serde_json::from_str(&kind_str).unwrap_or(ProviderKind::Custom);
            Ok(Provider { id: row.get(0)?, name: row.get(1)?, kind, base_url: row.get(3)?, enabled: row.get::<_, i64>(4)? != 0, created_at: row.get(5)?, updated_at: row.get(6)? })
        })?;
        rows.next().map_or(Ok(None), |r| r.map(Some))
    }

    pub fn update_provider(&self, id: &str, req: &UpdateProviderRequest) -> Result<Option<Provider>> {
        use chrono::Utc;
        let existing = match self.get_provider(id)? { Some(p) => p, None => return Ok(None) };
        let now = Utc::now().to_rfc3339();
        let name = req.name.clone().unwrap_or(existing.name.clone());
        let base_url = req.base_url.clone().unwrap_or(existing.base_url.clone());
        let kind = serde_json::to_string(&req.kind.clone().unwrap_or(existing.kind.clone())).unwrap();
        let enabled = req.enabled.unwrap_or(existing.enabled) as i64;
        let guard = self.conn.lock().unwrap();
        guard.execute("UPDATE mm_providers SET name=?1, kind=?2, base_url=?3, enabled=?4, updated_at=?5 WHERE id=?6", params![name, kind, base_url, enabled, now, id])?;
        Ok(Some(Provider { id: id.to_string(), name, kind: req.kind.clone().unwrap_or(existing.kind), base_url, enabled: enabled != 0, created_at: existing.created_at, updated_at: now }))
    }

    pub fn delete_provider(&self, id: &str) -> Result<bool> {
        let guard = self.conn.lock().unwrap();
        let count = guard.execute("DELETE FROM mm_providers WHERE id = ?1", params![id])?;
        Ok(count > 0)
    }

    pub fn create_model(&self, req: &CreateModelRequest) -> Result<Model> {
        use chrono::Utc;
        let now = Utc::now().to_rfc3339();
        let id = uuid::Uuid::new_v4().to_string();
        let caps = serde_json::to_string(&req.capabilities).unwrap_or_else(|_| "{}".to_string());
        self.conn.lock().unwrap().execute(
            "INSERT INTO mm_models (id, provider_id, name, display_name, enabled, context_window, cost_per_1k_input, cost_per_1k_output, capabilities, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?6, ?7, ?8, ?9, ?9)",
            params![&id, &req.provider_id, &req.name, &req.display_name, req.context_window, req.cost_per_1k_input, req.cost_per_1k_output, caps, now],
        )?;
        Ok(Model { id, provider_id: req.provider_id.clone(), name: req.name.clone(), display_name: req.display_name.clone(), enabled: true, context_window: req.context_window, cost_per_1k_input: req.cost_per_1k_input, cost_per_1k_output: req.cost_per_1k_output, capabilities: req.capabilities.clone(), created_at: now.clone(), updated_at: now })
    }

    pub fn list_models(&self) -> Result<Vec<Model>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare("SELECT id, provider_id, name, display_name, enabled, context_window, cost_per_1k_input, cost_per_1k_output, capabilities, created_at, updated_at FROM mm_models ORDER BY created_at")?;
        let rows = stmt.query_map([], |row| {
            let caps_str: String = row.get(8)?;
            let caps: ModelCapabilities = serde_json::from_str(&caps_str).unwrap_or(ModelCapabilities::none());
            Ok(Model { id: row.get(0)?, provider_id: row.get(1)?, name: row.get(2)?, display_name: row.get(3)?, enabled: row.get::<_, i64>(4)? != 0, context_window: row.get(5)?, cost_per_1k_input: row.get(6)?, cost_per_1k_output: row.get(7)?, capabilities: caps, created_at: row.get(9)?, updated_at: row.get(10)? })
        })?;
        rows.collect()
    }

    pub fn get_model(&self, id: &str) -> Result<Option<Model>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare("SELECT id, provider_id, name, display_name, enabled, context_window, cost_per_1k_input, cost_per_1k_output, capabilities, created_at, updated_at FROM mm_models WHERE id = ?1")?;
        let mut rows = stmt.query_map([id], |row| {
            let caps_str: String = row.get(8)?;
            let caps: ModelCapabilities = serde_json::from_str(&caps_str).unwrap_or(ModelCapabilities::none());
            Ok(Model { id: row.get(0)?, provider_id: row.get(1)?, name: row.get(2)?, display_name: row.get(3)?, enabled: row.get::<_, i64>(4)? != 0, context_window: row.get(5)?, cost_per_1k_input: row.get(6)?, cost_per_1k_output: row.get(7)?, capabilities: caps, created_at: row.get(9)?, updated_at: row.get(10)? })
        })?;
        rows.next().map_or(Ok(None), |r| r.map(Some))
    }

    pub fn update_model(&self, id: &str, req: &UpdateModelRequest) -> Result<Option<Model>> {
        use chrono::Utc;
        let existing = match self.get_model(id)? { Some(m) => m, None => return Ok(None) };
        let now = Utc::now().to_rfc3339();
        let name = existing.name.clone();
        let display_name = req.display_name.clone().unwrap_or(existing.display_name.clone());
        let ctx_window = req.context_window.unwrap_or(existing.context_window);
        let cost_input = req.cost_per_1k_input.or(existing.cost_per_1k_input);
        let cost_output = req.cost_per_1k_output.or(existing.cost_per_1k_output);
        let caps = serde_json::to_string(&req.capabilities.clone().unwrap_or(existing.capabilities.clone())).unwrap_or_else(|_| "{}".to_string());
        let enabled = req.enabled.unwrap_or(existing.enabled) as i64;
        let guard = self.conn.lock().unwrap();
        guard.execute("UPDATE mm_models SET name=?1, display_name=?2, enabled=?3, context_window=?4, cost_per_1k_input=?5, cost_per_1k_output=?6, capabilities=?7, updated_at=?8 WHERE id=?9", params![name, display_name, enabled, ctx_window, cost_input, cost_output, caps, now, id])?;
        Ok(Some(Model { id: id.to_string(), provider_id: existing.provider_id, name, display_name, enabled: enabled != 0, context_window: ctx_window, cost_per_1k_input: cost_input, cost_per_1k_output: cost_output, capabilities: req.capabilities.clone().unwrap_or(existing.capabilities.clone()), created_at: existing.created_at, updated_at: now }))
    }

    pub fn delete_model(&self, id: &str) -> Result<bool> {
        let guard = self.conn.lock().unwrap();
        let count = guard.execute("DELETE FROM mm_models WHERE id = ?1", params![id])?;
        Ok(count > 0)
    }

    pub fn record_routing_decision(&self, decision: &RoutingDecision) -> Result<()> {
        use chrono::Utc;
        let now = Utc::now().to_rfc3339();
        let task_cat = serde_json::to_string(&decision.task_category).ok();
        self.conn.lock().unwrap().execute(
            "INSERT INTO mm_routing_decisions (id, selected_model_id, selected_provider_id, provider_name, decision_reason, task_category: task_category.unwrap_or(TaskCategory::Chat), complexity_score, priority, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![&decision.id, &decision.selected_model_id, &decision.selected_provider_id, &decision.selected_provider_name, &decision.decision_reason, task_cat, decision.complexity_score, "balanced", now],
        )?;
        Ok(())
    }

    pub fn list_routing_decisions(&self, limit: usize) -> Result<Vec<RoutingDecision>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare("SELECT id, selected_model_id, selected_provider_id, provider_name, decision_reason, task_category: task_category.unwrap_or(TaskCategory::Chat), complexity_score, priority, created_at FROM mm_routing_decisions ORDER BY created_at DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit as i64], |row| {
            let tc: Option<String> = row.get(5)?;
            let task_category = tc.and_then(|s| serde_json::from_str(&s).ok());
            Ok(RoutingDecision { id: row.get(0)?, selected_model_id: row.get(1)?, selected_provider_id: row.get(2)?, selected_provider_name: row.get(3)?, decision_reason: row.get(4)?, task_category: task_category.unwrap_or(TaskCategory::Chat), complexity_score: row.get(6)?, priority: match row.get::<_, String>(7).ok().as_deref() { Some("quality") => Priority::Quality, Some("speed") => Priority::Speed, Some("cost") => Priority::Cost, _ => Priority::Balanced }, context_size_hint: None, selected_model_name: String::new(), fallback_model_id: None, fallback_provider_id: None, alternatives_considered: vec![String::new()], alternatives_rejected: vec![], created_at: row.get(8)? })})?;
        rows.collect()
    }

    pub fn record_usage(&self, record: &UsageRecord) -> Result<()> {
        use chrono::Utc;
        let now = Utc::now().to_rfc3339();
        let task_cat = serde_json::to_string(&record.task_category).ok();
        let success = if record.success { 1i64 } else { 0i64 };
        self.conn.lock().unwrap().execute(
            "INSERT INTO mm_usage (id, provider_id, model_id, task_category: task_category, input_tokens, output_tokens, latency_ms, cost_usd, success, error_message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![&record.id, &record.provider_id, &record.model_id, task_cat, record.input_tokens, record.output_tokens, record.latency_ms, record.cost_usd, success, &record.error_message, now],
        )?;
        Ok(())
    }

    pub fn list_usage(&self, limit: usize) -> Result<Vec<UsageRecord>> {
        let guard = self.conn.lock().unwrap();
        let mut stmt = guard.prepare("SELECT id, provider_id, model_id, task_category: task_category, input_tokens, output_tokens, latency_ms, cost_usd, success, error_message, created_at FROM mm_usage ORDER BY created_at DESC LIMIT ?1")?;
        let rows = stmt.query_map([limit as i64], |row| {
            let tc: Option<String> = row.get(3)?;
            let task_category = tc.and_then(|s| serde_json::from_str(&s).ok());
            Ok(UsageRecord { id: row.get(0)?, provider_id: row.get(1)?, model_id: row.get(2)?, task_category: task_category, input_tokens: row.get(4)?, output_tokens: row.get(5)?, latency_ms: row.get(6)?, cost_usd: row.get(7)?, success: row.get::<_, i64>(8)? != 0, error_message: row.get(9)?, created_at: row.get(10)? })
        })?;
        rows.collect()
    }

    pub fn get_usage_stats(&self) -> Result<UsageStats> {
        let row = self.conn.lock().unwrap().query_row(
            "SELECT COUNT(*), COALESCE(SUM(CASE WHEN success=1 THEN 1 ELSE 0 END), 0), COALESCE(SUM(CASE WHEN success=0 THEN 1 ELSE 0 END), 0), COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COALESCE(SUM(cost_usd), 0.0), COALESCE(AVG(CAST(latency_ms AS REAL)), 0.0), COALESCE(SUM(CASE WHEN created_at >= date('now') THEN 1 ELSE 0 END), 0), COALESCE(SUM(CASE WHEN created_at >= date('now') THEN cost_usd ELSE 0 END), 0.0)
             FROM mm_usage",
            [],
            |r| Ok(UsageStats {
                total_requests: r.get(0)?, successful_requests: r.get(1)?, failed_requests: r.get(2)?,
                total_input_tokens: r.get(3)?, total_output_tokens: r.get(4)?, total_cost_usd: r.get(5)?,
                avg_latency_ms: r.get(6)?, requests_today: r.get(7)?, cost_today_usd: r.get(8)?, requests_today_without_price: 0, requests_without_price: 0,
            }),
        )?;
        Ok(row)
    }
}
