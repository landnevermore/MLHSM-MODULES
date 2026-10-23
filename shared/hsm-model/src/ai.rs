//! ModelMesh: AI provider, model, routing and usage data models.
//!
//! These types live in `hsm-model` so both `hsm-core` (engine) and `hsm-service`
//! (API) can share them without circular dependencies.

use serde::{Deserialize, Serialize};

// ── Provider ────────────────────────────────────────────────────────────────

/// Supported AI provider backends.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAI,
    Anthropic,
    GoogleGemini,
    Ollama,
    Custom,
}

impl ProviderKind {
    pub fn label(&self) -> &'static str {
        match self {
            Self::OpenAI => "OpenAI",
            Self::Anthropic => "Anthropic",
            Self::GoogleGemini => "Google Gemini",
            Self::Ollama => "Ollama",
            Self::Custom => "Custom",
        }
    }
}

/// A configured AI provider (e.g. "OpenAI prod", "Anthropic free").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provider {
    pub id: String,
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// Request to create a new provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateProviderRequest {
    pub name: String,
    pub kind: ProviderKind,
    pub base_url: String,
    pub api_key: Option<String>,
    /// Optional fixed ID (e.g. "openrouter"). If None, a UUID is generated.
    pub id: Option<String>,
}

/// Request to update a provider (all fields optional).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateProviderRequest {
    pub name: Option<String>,
    pub kind: Option<ProviderKind>,
    pub base_url: Option<String>,
    pub api_key: Option<String>,
    pub enabled: Option<bool>,
}

/// Live health status of a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderStatus {
    pub provider_id: String,
    pub reachable: bool,
    pub models_available: Option<usize>,
    pub rate_limit_remaining: Option<u64>,
    pub last_checked: Option<String>,
    pub message: Option<String>,
}

// ── Model ───────────────────────────────────────────────────────────────────

/// Capability flags for a model (used by JEV for matching).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub chat: bool,
    pub code: bool,
    pub reasoning: bool,
    pub vision: bool,
    pub function_calling: bool,
}

impl ModelCapabilities {
    pub fn none() -> Self {
        Self {
            chat: false,
            code: false,
            reasoning: false,
            vision: false,
            function_calling: false,
        }
    }
}

/// A model registered under a provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub provider_id: String,
    pub name: String,
    pub display_name: String,
    pub enabled: bool,
    pub context_window: u32,
    pub cost_per_1k_input: Option<f64>,
    pub cost_per_1k_output: Option<f64>,
    pub capabilities: ModelCapabilities,
    pub created_at: String,
    pub updated_at: String,
}

/// Request to create a new model entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreateModelRequest {
    pub provider_id: String,
    pub name: String,
    pub display_name: String,
    pub context_window: u32,
    pub cost_per_1k_input: Option<f64>,
    pub cost_per_1k_output: Option<f64>,
    pub capabilities: ModelCapabilities,
}

/// Request to update a model (all fields optional).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateModelRequest {
    pub display_name: Option<String>,
    pub enabled: Option<bool>,
    pub context_window: Option<u32>,
    pub cost_per_1k_input: Option<f64>,
    pub cost_per_1k_output: Option<f64>,
    pub capabilities: Option<ModelCapabilities>,
}

/// Aggregated model health (from recent usage probes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelHealth {
    pub model_id: String,
    pub status: ModelHealthStatus,
    pub avg_latency_ms: Option<f64>,
    pub error_rate: Option<f64>,
    pub last_used: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelHealthStatus {
    Healthy,
    Degraded,
    Unhealthy,
    Unknown,
}

// ── JEV Decision ────────────────────────────────────────────────────────────

/// Task categories that JEV uses for routing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskCategory {
    Chat,
    Code,
    Reasoning,
    Vision,
    Creative,
}

impl TaskCategory {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Chat => "Chat",
            Self::Code => "Code",
            Self::Reasoning => "Reasoning",
            Self::Vision => "Vision",
            Self::Creative => "Creative",
        }
    }
}

/// User-specified priority for JEV decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Quality,
    Speed,
    Cost,
    Balanced,
}

/// A structured JEV routing decision.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    pub id: String,
    pub task_category: TaskCategory,
    pub priority: Priority,
    pub context_size_hint: Option<u32>,
    pub selected_model_id: String,
    pub selected_model_name: String,
    pub selected_provider_id: String,
    pub selected_provider_name: String,
    pub fallback_model_id: Option<String>,
    pub fallback_provider_id: Option<String>,
    pub complexity_score: f64,
    pub decision_reason: String,
    pub alternatives_considered: Vec<String>,
    pub alternatives_rejected: Vec<String>,
    pub created_at: String,
}

// ── Usage ───────────────────────────────────────────────────────────────────

/// A single API request routed through ModelMesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRecord {
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    pub task_category: Option<TaskCategory>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub latency_ms: Option<u64>,
    pub cost_usd: Option<f64>,
    pub success: bool,
    pub error_message: Option<String>,
    pub created_at: String,
}

/// Aggregated usage stats for display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageStats {
    pub total_requests: u64,
    pub successful_requests: u64,
    pub failed_requests: u64,
    pub total_input_tokens: u64,
    pub total_output_tokens: u64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub requests_today: u64,
    pub cost_today_usd: f64,
    /// Requests with no recorded price. `total_cost_usd` sums only the priced
    /// ones, so this is how a caller knows the total is partial rather than
    /// complete-and-zero.
    #[serde(default)]
    pub requests_without_price: u64,
    /// Same, restricted to today.
    #[serde(default)]
    pub requests_today_without_price: u64,
}

/// Per-model usage breakdown.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelUsageBreakdown {
    pub model_id: String,
    pub model_name: String,
    pub provider_name: String,
    pub requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: f64,
}

// ── API Request/Response for chat completions ───────────────────────────────

/// Tool definition for function calling.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tool {
    #[serde(rename = "type")]
    pub tool_type: String, // always "function"
    pub function: FunctionDef,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionDef {
    pub name: String,
    pub description: Option<String>,
    pub parameters: Option<serde_json::Value>,
}

/// A tool call returned by the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub tool_type: Option<String>,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

/// An incoming chat completion request routed through ModelMesh.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatCompletionRequest {
    pub model: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub stream: Option<bool>,
    pub task_category: Option<TaskCategory>,
    pub priority: Option<Priority>,
    /// Tool definitions for function calling.
    #[serde(default)]
    pub tools: Option<Vec<Tool>>,
    /// Tool choice strategy: "auto", "none", or a specific tool.
    #[serde(default)]
    pub tool_choice: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

/// Tool call result message for multi-turn conversations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResultMessage {
    pub role: String,
    pub content: String,
    pub tool_call_id: String,
}

/// JEV's analysis of a request before routing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestAnalysis {
    pub task_category: TaskCategory,
    pub complexity_score: f64,
    pub estimated_tokens: u32,
    pub priority: Priority,
}

impl Default for RequestAnalysis {
    fn default() -> Self {
        Self {
            task_category: TaskCategory::Chat,
            complexity_score: 0.5,
            estimated_tokens: 512,
            priority: Priority::Balanced,
        }
    }
}
