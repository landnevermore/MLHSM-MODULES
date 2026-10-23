//! Provider HTTP client — sends requests to AI provider APIs.

use hsm_model::ai::*;

/// OpenRouter chat completions endpoint (free tier, no key required for listing,
/// but key IS required for chat completions).
pub const OPENROUTER_CHAT_URL: &str = "https://openrouter.ai/api";

/// Compress messages to reduce token consumption before sending to provider.
///
/// Strategy (RTK-inspired):
/// - Truncate individual messages exceeding 2000 chars to first 1500 + "...[truncated]"
/// - Remove duplicate consecutive messages with same role+content
/// - Keep last 20 messages max, compress older ones into a summary
pub fn compress_messages(messages: &[ChatMessage]) -> Vec<ChatMessage> {
    const KEEP_TAIL: usize = 20;
    const MAX_CONTENT_CHARS: usize = 8_000;

    if messages.len() <= KEEP_TAIL {
        return messages.to_vec();
    }

    let mut compressed: Vec<ChatMessage> = Vec::new();
    let mut last_role = String::new();
    let mut last_content = String::new();

    // Keep the leading system prompt verbatim, then the last KEEP_TAIL turns.
    if let Some(first) = messages.first() {
        if first.role == "system" {
            compressed.push(first.clone());
        }
    }

    let middle_count = messages.len().saturating_sub(KEEP_TAIL + 1);
    if middle_count > 0 {
        compressed.push(ChatMessage {
            role: "system".to_string(),
            content: Some(format!(
                "[{middle_count} earlier messages omitted for context compression]"
            )),
            tool_calls: None,
            tool_call_id: None,
        });
    }

    for msg in messages.iter().skip(1).take(KEEP_TAIL) {
        let content = msg.content.as_deref().unwrap_or("");
        if msg.role == last_role && content == last_content {
            continue; // Skip duplicate
        }

        let mut out = msg.clone();
        if content.chars().count() > MAX_CONTENT_CHARS {
            out.content = Some(truncate_content(content, MAX_CONTENT_CHARS));
        }
        // A tool-call turn without a tool_call_id is unusable for the provider,
        // so drop it rather than send a malformed message.
        if out.tool_calls.is_some() && out.tool_call_id.is_none() && out.content.is_none() {
            continue;
        }

        compressed.push(out);
        last_role = msg.role.clone();
        last_content = content.to_string();
    }

    compressed
}

/// Truncate a single long message to reduce token usage.
///
/// Truncation happens on a UTF-8 character boundary; slicing at a raw byte
/// index would panic on any multi-byte character.
fn truncate_content(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let total = content.chars().count();
    let head: String = content.chars().take(max_chars).collect();
    format!("{head}...[truncated, {total} chars total]")
}

/// A single outbound completion call. Grouped into a struct so the provider
/// surface stays explicit and does not grow an unbounded argument list.
pub struct CompletionCall<'a> {
    pub base_url: &'a str,
    pub api_key: &'a str,
    pub model_name: &'a str,
    pub messages: &'a [ChatMessage],
    pub temperature: Option<f64>,
    pub max_tokens: Option<u32>,
    pub tools: Option<&'a [Tool]>,
    pub tool_choice: Option<&'a serde_json::Value>,
}

/// Send a chat completion request to a specific provider.
///
/// Returns the provider response; transport failures are reported inside
/// `ProviderResponse` rather than as a `Result`.
pub async fn send_chat_completion(call: CompletionCall<'_>) -> ProviderResponse {
    let CompletionCall {
        base_url,
        api_key,
        model_name,
        messages,
        temperature,
        max_tokens,
        tools,
        tool_choice,
    } = call;
    let start = std::time::Instant::now();

    // Apply prompt compression (RTK-inspired token optimization)
    let compressed_messages = compress_messages(messages);

    let msg_values: Vec<serde_json::Value> = compressed_messages
        .iter()
        .map(|m| {
            let mut msg = serde_json::json!({ "role": &m.role });
            if let Some(ref content) = m.content {
                msg["content"] = serde_json::Value::String(content.clone());
            }
            if let Some(ref tc) = m.tool_calls {
                msg["tool_calls"] = serde_json::to_value(tc).unwrap_or_default();
            }
            if let Some(ref tcid) = m.tool_call_id {
                msg["tool_call_id"] = serde_json::Value::String(tcid.clone());
            }
            msg
        })
        .collect();

    let mut body = serde_json::json!({
        "model": model_name,
        "messages": msg_values,
        "temperature": temperature.unwrap_or(0.7),
        "max_tokens": max_tokens.unwrap_or(4096),
    });

    if let Some(t) = tools {
        if !t.is_empty() {
            body["tools"] = serde_json::to_value(t).unwrap_or_default();
        }
    }
    if let Some(tc) = tool_choice {
        body["tool_choice"] = tc.clone();
    }

    let url = format!("{}/v1/chat/completions", base_url.trim_end_matches('/'));

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build();

    let client = match client {
        Ok(c) => c,
        Err(e) => {
            return ProviderResponse::error(
                start.elapsed().as_millis() as u64,
                &format!("Client-Build-Fehler: {e}"),
            )
        }
    };

    let request = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .json(&body)
        .send()
        .await;

    match request {
        Ok(resp) => {
            let status = resp.status();
            let latency = start.elapsed().as_millis() as u64;
            match resp.text().await {
                Ok(text) => {
                    if !status.is_success() {
                        return ProviderResponse::error(
                            latency,
                            &format!("HTTP {}: {}", status, truncate(&text, 200)),
                        );
                    }
                    match serde_json::from_str::<serde_json::Value>(&text) {
                        Ok(val) => {
                            let content = val["choices"][0]["message"]["content"]
                                .as_str()
                                .unwrap_or("")
                                .to_string();
                            let tool_calls: Vec<hsm_model::ai::ToolCall> = val["choices"][0]
                                ["message"]["tool_calls"]
                                .as_array()
                                .map(|arr| {
                                    arr.iter()
                                        .map(|tc| hsm_model::ai::ToolCall {
                                            id: tc["id"].as_str().map(String::from),
                                            tool_type: tc["type"].as_str().map(String::from),
                                            function: hsm_model::ai::FunctionCall {
                                                name: tc["function"]["name"]
                                                    .as_str()
                                                    .unwrap_or("")
                                                    .to_string(),
                                                arguments: tc["function"]["arguments"]
                                                    .as_str()
                                                    .unwrap_or("{}")
                                                    .to_string(),
                                            },
                                        })
                                        .collect()
                                })
                                .unwrap_or_default();
                            let input_tokens = val["usage"]["prompt_tokens"].as_u64();
                            let output_tokens = val["usage"]["completion_tokens"].as_u64();
                            ProviderResponse::success(
                                content,
                                tool_calls,
                                input_tokens,
                                output_tokens,
                                latency,
                            )
                        }
                        Err(e) => ProviderResponse::error(latency, &format!("Parse error: {e}")),
                    }
                }
                Err(e) => ProviderResponse::error(latency, &format!("Read error: {e}")),
            }
        }
        Err(e) => ProviderResponse::error(start.elapsed().as_millis() as u64, &e.to_string()),
    }
}

/// Test a provider connection by listing available models.
pub async fn test_provider(base_url: &str, api_key: &str) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let url = format!("{}/v1/models", base_url.trim_end_matches('/'));
    let resp = client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    let val: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let models = val["data"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|m| m["id"].as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    Ok(models)
}

/// Result of a provider API call.
#[derive(Debug)]
pub struct ProviderResponse {
    pub content: String,
    pub tool_calls: Vec<hsm_model::ai::ToolCall>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub latency_ms: u64,
    pub success: bool,
    pub error: Option<String>,
}

impl ProviderResponse {
    fn success(
        content: String,
        tool_calls: Vec<hsm_model::ai::ToolCall>,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        latency_ms: u64,
    ) -> Self {
        Self {
            content,
            tool_calls,
            input_tokens,
            output_tokens,
            latency_ms,
            success: true,
            error: None,
        }
    }

    fn error(latency_ms: u64, msg: &str) -> Self {
        Self {
            content: String::new(),
            tool_calls: Vec::new(),
            input_tokens: None,
            output_tokens: None,
            latency_ms,
            success: false,
            error: Some(msg.to_string()),
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}…", &s[..max])
    }
}
