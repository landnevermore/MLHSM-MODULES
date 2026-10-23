//! Auto-discovery: fetches free models from multiple providers.
//!
//! ONLY free-tier models are synced — no payment method required.
//! Sources: OpenRouter (free models), Ollama (local), Groq (free tier), Google AI Studio (free tier).

use hsm_model::ai::*;

// ── Provider Endpoints ──────────────────────────────────────────────────────

const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const GROQ_MODELS_URL: &str = "https://api.groq.com/openai/v1/models";
const GOOGLE_AI_MODELS_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const OLLAMA_DEFAULT_URL: &str = "http://localhost:11434";

/// Result of a discovery sync operation.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SyncResult {
    pub models_fetched: usize,
    pub models_synced: usize,
    pub models_skipped: usize,
    pub providers_discovered: Vec<String>,
    pub timestamp: String,
    pub errors: Vec<String>,
}

// ── OpenRouter Model Format ─────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct OpenRouterResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenRouterModel {
    id: String,
    name: Option<String>,
    context_length: Option<u64>,
    pricing: Option<OpenRouterPricing>,
    architecture: Option<OpenRouterArchitecture>,
    reasoning: Option<OpenRouterReasoning>,
    supported_parameters: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenRouterPricing {
    prompt: Option<String>,
    completion: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenRouterArchitecture {
    input_modalities: Option<Vec<String>>,
    /// Parsed to document the upstream shape; MLHSM derives capability flags
    /// from the catalog, not from this field.
    #[allow(dead_code)]
    output_modalities: Option<Vec<String>>,
}

#[derive(Debug, serde::Deserialize)]
struct OpenRouterReasoning {
    mandatory: Option<bool>,
    supported_efforts: Option<Vec<String>>,
}

// ── Groq Model Format ───────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct GroqModelsResponse {
    data: Vec<GroqModel>,
}

#[derive(Debug, serde::Deserialize)]
struct GroqModel {
    id: String,
    /// Upstream field, intentionally parsed but not consumed.
    #[allow(dead_code)]
    owned_by: Option<String>,
}

// ── Ollama Model Format ─────────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct OllamaModelsResponse {
    models: Vec<OllamaModel>,
}

#[derive(Debug, serde::Deserialize)]
struct OllamaModel {
    name: String,
    /// Upstream fields, intentionally parsed but not consumed. Disk size and
    /// timestamps are informational; MLHSM never uses them for routing.
    #[allow(dead_code)]
    size: Option<u64>,
    #[allow(dead_code)]
    modified_at: Option<String>,
}

// ── Google AI Studio Format ─────────────────────────────────────────────────

#[derive(Debug, serde::Deserialize)]
struct GoogleAIModelsResponse {
    models: Vec<GoogleAIModel>,
}

#[derive(Debug, serde::Deserialize)]
struct GoogleAIModel {
    name: String,
    display_name: Option<String>,
    supported_generation_methods: Option<Vec<String>>,
    input_token_limit: Option<u64>,
}

// ── Discovery Logic ────────────────────────────────────────────────────────

/// Fetch free models from all configured providers.
/// Returns (provider_name, models) for each source.
pub async fn discover_all_free_models(
    ollama_url: Option<&str>,
) -> Vec<(String, Vec<(String, Model)>)> {
    let mut results = Vec::new();

    // 1. OpenRouter Free Models
    match fetch_openrouter_free_models().await {
        Ok(models) => {
            if !models.is_empty() {
                results.push(("openrouter".to_string(), models));
            }
        }
        Err(e) => {
            tracing::warn!("OpenRouter discovery failed: {e}");
        }
    }

    // 2. Ollama (local, always free)
    let ollama_base = ollama_url.unwrap_or(OLLAMA_DEFAULT_URL);
    match fetch_ollama_models(ollama_base).await {
        Ok(models) => {
            if !models.is_empty() {
                results.push(("ollama".to_string(), models));
            }
        }
        Err(e) => {
            tracing::info!("Ollama not available at {}: {}", ollama_base, e);
        }
    }

    // 3. Groq Free Tier
    match fetch_groq_free_models().await {
        Ok(models) => {
            if !models.is_empty() {
                results.push(("groq".to_string(), models));
            }
        }
        Err(e) => {
            tracing::info!("Groq discovery skipped: {}", e);
        }
    }

    // 4. Google AI Studio (free tier)
    match fetch_google_ai_free_models().await {
        Ok(models) => {
            if !models.is_empty() {
                results.push(("google-ai".to_string(), models));
            }
        }
        Err(e) => {
            tracing::info!("Google AI Studio discovery skipped: {}", e);
        }
    }

    results
}

// ── OpenRouter Fetcher ──────────────────────────────────────────────────────

async fn fetch_openrouter_free_models() -> Result<Vec<(String, Model)>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(OPENROUTER_MODELS_URL)
        .header(
            "HTTP-Referer",
            "https://github.com/landnevermore/HomeServerManager",
        )
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("OpenRouter HTTP {}", resp.status()));
    }

    let body: OpenRouterResponse = resp.json().await.map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().to_rfc3339();

    let free_models: Vec<(String, Model)> = body
        .data
        .into_iter()
        .filter(is_free)
        .map(|m| {
            let id = uuid::Uuid::new_v4().to_string();
            let provider_name = extract_provider_name(&m.id);
            let display_name = m.name.clone().unwrap_or_else(|| provider_name.clone());
            let capabilities = extract_openrouter_capabilities(&m);

            let model = Model {
                id: id.clone(),
                provider_id: "openrouter".to_string(),
                name: m.id.clone(),
                display_name,
                enabled: true,
                context_window: m.context_length.unwrap_or(4096) as u32,
                cost_per_1k_input: Some(0.0),
                cost_per_1k_output: Some(0.0),
                capabilities,
                created_at: now.clone(),
                updated_at: now.clone(),
            };

            (id, model)
        })
        .collect();

    Ok(free_models)
}

fn is_free(model: &OpenRouterModel) -> bool {
    match &model.pricing {
        Some(p) => p.prompt.as_deref() == Some("0") && p.completion.as_deref() == Some("0"),
        None => false,
    }
}

fn extract_provider_name(model_id: &str) -> String {
    model_id.split('/').next().unwrap_or(model_id).to_string()
}

fn extract_openrouter_capabilities(model: &OpenRouterModel) -> ModelCapabilities {
    let input_mods = model
        .architecture
        .as_ref()
        .and_then(|a| a.input_modalities.as_ref())
        .map(|v| v.as_slice())
        .unwrap_or(&[]);

    let params = model.supported_parameters.as_deref().unwrap_or(&[]);

    let has_reasoning = model
        .reasoning
        .as_ref()
        .map(|r| r.mandatory.unwrap_or(false) || r.supported_efforts.is_some())
        .unwrap_or(false);

    ModelCapabilities {
        chat: input_mods.iter().any(|s| s == "text"),
        code: input_mods.iter().any(|s| s == "text")
            && (params.iter().any(|s| s == "tools")
                || params.iter().any(|s| s == "structured_outputs")),
        reasoning: has_reasoning,
        vision: input_mods.iter().any(|s| s == "image" || s == "video"),
        function_calling: params.iter().any(|s| s == "tools"),
    }
}

// ── Ollama Fetcher (Local, Free) ────────────────────────────────────────────

async fn fetch_ollama_models(base_url: &str) -> Result<Vec<(String, Model)>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| e.to_string())?;

    let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
    let resp = client.get(&url).send().await.map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("Ollama HTTP {}", resp.status()));
    }

    let body: OllamaModelsResponse = resp.json().await.map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().to_rfc3339();

    let models: Vec<(String, Model)> = body
        .models
        .into_iter()
        .map(|m| {
            let id = uuid::Uuid::new_v4().to_string();
            let display_name = m.name.clone();
            let name = m.name.clone();

            let model = Model {
                id: id.clone(),
                provider_id: "ollama".to_string(),
                name,
                display_name,
                enabled: true,
                context_window: 32768, // Default for local models
                cost_per_1k_input: Some(0.0),
                cost_per_1k_output: Some(0.0),
                capabilities: ModelCapabilities {
                    chat: true,
                    code: true, // Assume local models can code
                    reasoning: false,
                    vision: false,
                    function_calling: false,
                },
                created_at: now.clone(),
                updated_at: now.clone(),
            };

            (id, model)
        })
        .collect();

    Ok(models)
}

/// Check if Ollama is running locally.
pub async fn check_ollama(base_url: &str) -> bool {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(3))
        .build();
    match client {
        Ok(c) => c
            .get(format!("{}/api/tags", base_url.trim_end_matches('/')))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false),
        Err(_) => false,
    }
}

// ── Groq Fetcher (Free Tier) ────────────────────────────────────────────────

async fn fetch_groq_free_models() -> Result<Vec<(String, Model)>, String> {
    // Groq models list doesn't require auth
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(GROQ_MODELS_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("Groq HTTP {}", resp.status()));
    }

    let body: GroqModelsResponse = resp.json().await.map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().to_rfc3339();

    // Groq free models have specific IDs containing "free"
    let free_models: Vec<(String, Model)> = body
        .data
        .into_iter()
        .filter(|m| m.id.contains(":free") || m.id.ends_with("-free"))
        .map(|m| {
            let id = uuid::Uuid::new_v4().to_string();
            let model = Model {
                id: id.clone(),
                provider_id: "groq".to_string(),
                name: m.id.clone(),
                display_name: format!("Groq: {}", m.id),
                enabled: true,
                context_window: 8192,
                cost_per_1k_input: Some(0.0),
                cost_per_1k_output: Some(0.0),
                capabilities: ModelCapabilities {
                    chat: true,
                    code: true,
                    reasoning: false,
                    vision: false,
                    function_calling: true,
                },
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            (id, model)
        })
        .collect();

    Ok(free_models)
}

// ── Google AI Studio Fetcher (Free Tier) ────────────────────────────────────

async fn fetch_google_ai_free_models() -> Result<Vec<(String, Model)>, String> {
    // Google AI models list is public (no key needed for listing)
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let resp = client
        .get(GOOGLE_AI_MODELS_URL)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("Google AI HTTP {}", resp.status()));
    }

    let body: GoogleAIModelsResponse = resp.json().await.map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().to_rfc3339();

    // Only include text generation models (not embedding, image generation, etc.)
    let free_models: Vec<(String, Model)> = body
        .models
        .into_iter()
        .filter(|m| {
            m.supported_generation_methods
                .as_ref()
                .map(|methods| methods.iter().any(|s| s == "generateContent"))
                .unwrap_or(false)
        })
        .map(|m| {
            let id = uuid::Uuid::new_v4().to_string();
            let display_name = m.display_name.clone().unwrap_or_else(|| m.name.clone());
            let model = Model {
                id: id.clone(),
                provider_id: "google-ai".to_string(),
                name: m.name.clone(),
                display_name,
                enabled: true,
                context_window: m.input_token_limit.unwrap_or(8192) as u32,
                cost_per_1k_input: Some(0.0),
                cost_per_1k_output: Some(0.0),
                capabilities: ModelCapabilities {
                    chat: true,
                    code: true,
                    reasoning: false,
                    vision: m.name.contains("vision") || m.name.contains("multimodal"),
                    function_calling: false,
                },
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            (id, model)
        })
        .collect();

    Ok(free_models)
}

/// Build provider entries for auto-discovered providers.
pub fn provider_for_source(source: &str) -> Option<CreateProviderRequest> {
    match source {
        "openrouter" => Some(CreateProviderRequest {
            name: "OpenRouter (Free)".to_string(),
            kind: ProviderKind::Custom,
            base_url: "https://openrouter.ai/api".to_string(),
            api_key: None,
            id: Some("openrouter".to_string()),
        }),
        "ollama" => Some(CreateProviderRequest {
            name: "Ollama (Local)".to_string(),
            kind: ProviderKind::Ollama,
            base_url: OLLAMA_DEFAULT_URL.to_string(),
            api_key: None,
            id: Some("ollama".to_string()),
        }),
        "groq" => Some(CreateProviderRequest {
            name: "Groq (Free Tier)".to_string(),
            kind: ProviderKind::Custom,
            base_url: "https://api.groq.com/openai".to_string(),
            api_key: None,
            id: Some("groq".to_string()),
        }),
        "google-ai" => Some(CreateProviderRequest {
            name: "Google AI Studio (Free)".to_string(),
            kind: ProviderKind::GoogleGemini,
            base_url: "https://generativelanguage.googleapis.com/v1beta".to_string(),
            api_key: None,
            id: Some("google-ai".to_string()),
        }),
        _ => None,
    }
}

/// Get the chat completions base URL for a provider.
/// A default endpoint for a provider that has none configured.
///
/// This is a LAST RESORT, not the source of truth. Routing uses the endpoint
/// stored on the provider record; this only fills a gap, and the result is
/// logged because a provider silently pointed at a default is exactly how
/// "my custom endpoint is ignored" stays invisible.
///
/// Returning an empty string here used to be the normal path for every provider
/// outside four hardcoded names, which broke LM Studio, OpenAI-compatible
/// servers and anything custom without a single error message.
pub fn chat_base_url(provider_id: &str) -> String {
    let url = match provider_id {
        "openrouter" => "https://openrouter.ai/api".to_string(),
        "ollama" => OLLAMA_DEFAULT_URL.to_string(),
        "groq" => "https://api.groq.com/openai".to_string(),
        "google-ai" => "https://generativelanguage.googleapis.com/v1beta".to_string(),
        _ => String::new(),
    };
    if url.is_empty() {
        tracing::warn!(
            provider = %provider_id,
            "Kein Default-Endpunkt bekannt - es muss eine base_url konfiguriert sein"
        );
    }
    url
}
