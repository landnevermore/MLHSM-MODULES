//! OpenJEV client — calls the OpenJEV API for intelligent routing decisions.
//!
//! Endpoint: POST https://api.openjev.sh/v1/systemone
//! Auth: Bearer <API_KEY>
//! State: request context + available models
//! Questions: task_category (choice), complexity (score), priority (choice), selected_model (choice)

use hsm_model::ai::*;

const OPENJEV_URL: &str = "https://api.openjev.sh/v1/systemone";

/// Build the OpenJEV request body for routing decisions.
fn build_jev_request(
    req: &ChatCompletionRequest,
    available_models: &[(Model, String)],
) -> serde_json::Value {
    // Build state: the user's messages + available model list
    let messages_text = req
        .messages
        .iter()
        .filter_map(|m| m.content.as_ref().map(|c| format!("{}: {}", m.role, c)))
        .collect::<Vec<_>>()
        .join("\n");

    let model_list = available_models
        .iter()
        .filter(|(m, _)| m.enabled)
        .map(|(m, pname)| {
            format!(
                "{} (provider: {}, context: {}k, capabilities: {})",
                m.name,
                pname,
                m.context_window / 1000,
                capability_summary(&m.capabilities)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    let state = serde_json::json!({
        "request": messages_text,
        "available_models": model_list,
        "request_details": {
            "temperature": req.temperature,
            "max_tokens": req.max_tokens,
        }
    });

    // Build questions for JEV
    let model_criteria: serde_json::Map<String, serde_json::Value> = available_models
        .iter()
        .filter(|(m, _)| m.enabled)
        .map(|(m, pname)| {
            let desc = format!(
                "{} via {} (context: {}k, coding: {}, reasoning: {}, vision: {})",
                m.display_name,
                pname,
                m.context_window / 1000,
                m.capabilities.code,
                m.capabilities.reasoning,
                m.capabilities.vision
            );
            (m.name.clone(), serde_json::Value::String(desc))
        })
        .collect();

    let priority_criteria = serde_json::json!({
        "quality": "Highest quality output, acceptable latency and cost",
        "speed": "Lowest latency, acceptable quality",
        "cost": "Lowest cost, acceptable quality",
        "balanced": "Best balance of quality, speed, and cost"
    });

    let task_criteria = serde_json::json!({
        "chat": "General conversation, Q&A, simple text tasks",
        "code": "Code generation, debugging, refactoring, technical implementation",
        "reasoning": "Analysis, logic, planning, complex reasoning tasks",
        "vision": "Image understanding, visual analysis, OCR",
        "creative": "Writing, brainstorming, content creation, summarization"
    });

    serde_json::json!({
        "model": "openjev",
        "state": state,
        "questions": {
            "task_category": {
                "type": "choice",
                "instructions": "What type of task is this request? Classify the primary task category based on the content and context.",
                "criteria": task_criteria
            },
            "complexity": {
                "type": "score",
                "instructions": "How complex is this task? Consider the number of steps, domain expertise required, and context size.",
                "criteria": [
                    "Trivial - single word, simple greeting, trivial question",
                    "Simple - straightforward question, short response expected",
                    "Moderate - multi-step task, some domain knowledge needed",
                    "Complex - requires deep reasoning, long context, or specialized knowledge",
                    "Very Complex - multi-domain expertise, extensive reasoning, large codebase changes"
                ]
            },
            "priority": {
                "type": "choice",
                "instructions": "What priority should the routing optimize for?",
                "criteria": priority_criteria
            },
            "selected_model": {
                "type": "choice",
                "instructions": "Based on the task requirements and available models, which model should handle this request? Consider task type, complexity, required capabilities, and the requested priority.",
                "criteria": model_criteria
            }
        }
    })
}

/// Parse the OpenJEV response into routing decisions.
fn parse_jev_response(
    response: &serde_json::Value,
    available_models: &[(Model, String)],
) -> Option<JeVDecision> {
    let answers = response.get("answers")?;

    let task_category = answers
        .get("task_category")
        .and_then(|a| a.get("choice"))
        .and_then(|c| c.as_str())
        .and_then(|s| match s {
            "chat" => Some(TaskCategory::Chat),
            "code" => Some(TaskCategory::Code),
            "reasoning" => Some(TaskCategory::Reasoning),
            "vision" => Some(TaskCategory::Vision),
            "creative" => Some(TaskCategory::Creative),
            _ => None,
        })
        .unwrap_or(TaskCategory::Chat);

    let complexity_score = answers
        .get("complexity")
        .and_then(|a| a.get("score"))
        .and_then(|s| s.as_f64())
        .map(|s| (s / 4.0).clamp(0.0, 1.0)) // normalize 0-4 to 0.0-1.0
        .unwrap_or(0.5);

    let priority = answers
        .get("priority")
        .and_then(|a| a.get("choice"))
        .and_then(|c| c.as_str())
        .and_then(|s| match s {
            "quality" => Some(Priority::Quality),
            "speed" => Some(Priority::Speed),
            "cost" => Some(Priority::Cost),
            "balanced" => Some(Priority::Balanced),
            _ => None,
        })
        .unwrap_or(Priority::Balanced);

    // The decision engine is advisory only. Its model choice is accepted only
    // when it names a model that actually exists in our registry — an external
    // service must never be able to steer routing to an arbitrary model id.
    // Anything else is dropped so the deterministic selector in `routing` picks.
    let raw_choice = answers
        .get("selected_model")
        .and_then(|a| a.get("choice"))
        .and_then(|c| c.as_str())
        .map(String::from);

    let (selected_name, rejected_selection) = match raw_choice {
        None => (None, false),
        Some(name)
            if available_models
                .iter()
                .any(|(m, _)| m.id == name || m.name == name) =>
        {
            (Some(name), false)
        }
        Some(_) => (None, true),
    };

    let mut reason_codes = vec![
        format!("task={}", task_category.label()),
        format!("complexity={:.2}", complexity_score),
        format!("priority={}", priority_label(&priority)),
    ];
    if rejected_selection {
        reason_codes.push("jev_model_rejected=not_in_registry".to_string());
    }

    Some(JeVDecision {
        task_category,
        complexity_score,
        priority,
        selected_model_name: selected_name,
        reason_codes,
    })
}

fn capability_summary(cap: &ModelCapabilities) -> String {
    let mut caps = Vec::new();
    if cap.chat {
        caps.push("chat");
    }
    if cap.code {
        caps.push("code");
    }
    if cap.reasoning {
        caps.push("reasoning");
    }
    if cap.vision {
        caps.push("vision");
    }
    if cap.function_calling {
        caps.push("tools");
    }
    if caps.is_empty() {
        "none".into()
    } else {
        caps.join("+")
    }
}

fn priority_label(p: &Priority) -> &'static str {
    match p {
        Priority::Quality => "quality",
        Priority::Speed => "speed",
        Priority::Cost => "cost",
        Priority::Balanced => "balanced",
    }
}

/// Structured JEV decision result.
#[derive(Debug)]
pub struct JeVDecision {
    pub task_category: TaskCategory,
    pub complexity_score: f64,
    pub priority: Priority,
    pub selected_model_name: Option<String>,
    pub reason_codes: Vec<String>,
}

/// Call OpenJEV API for a routing decision.
pub async fn call_jev(
    api_key: &str,
    req: &ChatCompletionRequest,
    available_models: &[(Model, String)],
) -> Result<JeVDecision, String> {
    let body = build_jev_request(req, available_models);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .post(OPENJEV_URL)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = response.status();
    if !status.is_success() {
        let error_text = response.text().await.unwrap_or_default();
        return Err(format!(
            "OpenJEV HTTP {}: {}",
            status,
            error_text.chars().take(200).collect::<String>()
        ));
    }

    let response_json: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    parse_jev_response(&response_json, available_models)
        .ok_or_else(|| "Konnte OpenJEV-Antwort nicht parsen".into())
}

/// Test the OpenJEV connection by sending a minimal request.
pub async fn test_jev_connection(api_key: &str) -> Result<String, String> {
    let body = serde_json::json!({
        "model": "openjev",
        "state": "Test connection",
        "questions": {
            "test": {
                "type": "noul",
                "instructions": "Is this a test?"
            }
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| e.to_string())?;

    let response = client
        .post(OPENJEV_URL)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;

    let status = response.status();
    if status.is_success() {
        Ok("OpenJEV Verbindung erfolgreich".into())
    } else {
        let text = response.text().await.unwrap_or_default();
        Err(format!(
            "OpenJEV HTTP {}: {}",
            status,
            text.chars().take(200).collect::<String>()
        ))
    }
}
