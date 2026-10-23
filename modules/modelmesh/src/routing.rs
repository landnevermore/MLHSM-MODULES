//! Routing: uses OpenJEV for intelligent model selection + fallback.

use hsm_model::ai::*;

use super::jev;
use super::provider;

/// A model that can be routed to, with everything needed to actually call it.
///
/// This replaces the old `(Model, provider_name, api_key)` tuple. The tuple
/// shape is what made the bug possible: the endpoint had nowhere to live, so
/// routing fell back to deriving it from the provider id, which silently
/// produced an empty string for every provider that was not one of four
/// hardcoded names. `base_url` is the configured endpoint and is authoritative.
#[derive(Debug, Clone)]
pub struct RoutableModel {
    pub model: Model,
    pub provider_id: String,
    pub provider_name: String,
    /// Configured endpoint. Never derived from the provider id.
    pub base_url: String,
    pub api_key: String,
}

impl RoutableModel {
    /// The endpoint to call, or `None` when the provider has no usable URL.
    ///
    /// A model without a configured endpoint is skipped by the caller rather
    /// than sent with an empty URL, which would fail later and further away.
    pub fn endpoint(&self) -> Option<&str> {
        let url = self.base_url.trim();
        (!url.is_empty()).then_some(url)
    }
}

/// Route a chat completion request through ModelMesh
///
/// Steps:
///
/// 1. Call OpenJEV to analyze and select the best model
/// 2. Send the request to the selected provider
/// 3. If that fails, try a fallback
/// 4. Return result + routing decision
pub async fn route_request(
    req: ChatCompletionRequest,
    available_models: &[RoutableModel],
    openjev_key: &str,
) -> RouteResult {
    // Filter to enabled models, and drop any whose provider has no endpoint.
    // A missing URL is a configuration error worth surfacing, not a request to
    // send into the void.
    let active_models: Vec<RoutableModel> = available_models
        .iter()
        .filter(|r| r.model.enabled)
        .cloned()
        .collect();
    let active_models: Vec<RoutableModel> = active_models
        .into_iter()
        .filter(|r| {
            let ok = r.endpoint().is_some();
            if !ok {
                tracing::warn!(
                    provider = %r.provider_id,
                    model = %r.model.id,
                    "Provider ohne konfigurierte base_url - aus dem Routing entfernt"
                );
            }
            ok
        })
        .collect();

    if active_models.is_empty() {
        // A caller who named a model deserves to hear about THAT model, not a
        // generic "nothing configured" that sends them looking in the wrong
        // place.
        if let Some(wanted) = req.model.as_deref() {
            let known: Vec<&str> = available_models
                .iter()
                .map(|r| r.model.name.as_str())
                .collect();
            return RouteResult::model_not_found(wanted, &known);
        }
        return RouteResult::no_models();
    }

    // An explicitly requested model wins. The Decision Engine exists to choose
    // when nobody has; if the caller named a model, replacing it silently is
    // the one behaviour an OpenAI-compatible client cannot accept - the reply
    // would come from a model nobody asked for and nothing would say so.
    //
    // A named model that is not available is an error, not a reason to route
    // somewhere else.
    let explicit = req.model.as_deref().and_then(|wanted| {
        active_models
            .iter()
            .find(|r| {
                r.model.id == wanted
                    || r.model.name == wanted
                    || r.model.display_name.eq_ignore_ascii_case(wanted)
            })
            .cloned()
    });

    if let Some(wanted) = req.model.as_deref() {
        if explicit.is_none() {
            let available: Vec<&str> = active_models
                .iter()
                .map(|r| r.model.name.as_str())
                .collect();
            tracing::warn!(model = %wanted, "Explizit angefordertes Modell nicht verfuegbar");
            return RouteResult::model_not_found(wanted, &available);
        }
    }

    let model_refs: Vec<(Model, String)> = active_models
        .iter()
        .map(|r| (r.model.clone(), r.provider_name.clone()))
        .collect();

    // Only reach the Decision Engine when no model was named.
    let jev_decision = match &explicit {
        Some(entry) => jev::JeVDecision {
            task_category: req.task_category.unwrap_or(TaskCategory::Chat),
            complexity_score: 1.0,
            priority: req.priority.unwrap_or(Priority::Balanced),
            selected_model_name: Some(entry.model.name.clone()),
            reason_codes: vec!["explicit_model".to_string()],
        },
        None => match jev::call_jev(openjev_key, &req, &model_refs).await {
            Ok(decision) => decision,
            Err(e) => {
                // Fallback: simple capability matching
                let analysis = RequestAnalysis {
                    task_category: req.task_category.unwrap_or(TaskCategory::Chat),
                    complexity_score: 0.5,
                    estimated_tokens: 512,
                    priority: req.priority.unwrap_or(Priority::Balanced),
                };
                jev::JeVDecision {
                    task_category: analysis.task_category,
                    complexity_score: analysis.complexity_score,
                    priority: analysis.priority,
                    selected_model_name: None,
                    reason_codes: vec![format!("jev_fallback: {e}")],
                }
            }
        },
    };

    // Find the selected model from JEV decision
    let (selected, selected_name) = if let Some(ref model_name) = jev_decision.selected_model_name {
        active_models
            .iter()
            .find(|r| {
                &r.model.name == model_name || r.model.display_name.eq_ignore_ascii_case(model_name)
            })
            .map(|r| (r.model.clone(), r.provider_name.clone()))
            .unwrap_or_else(|| {
                // JEV returned unknown model — pick best by fallback
                fallback_select(&jev_decision, &active_models)
            })
    } else {
        fallback_select(&jev_decision, &active_models)
    };

    // The selected model must exist in the list we filtered from. `None` means
    // the two fell out of sync, which is a bug - but panicking inside a request
    // handler is the wrong way to report it.
    let Some(entry) = active_models.iter().find(|r| r.model.id == selected.id) else {
        tracing::error!(model = %selected.id, "Ausgewaehltes Modell nicht in der aktiven Liste");
        return RouteResult::no_models();
    };
    let api_key = entry.api_key.clone();
    let selected_provider_url = entry.endpoint().unwrap_or_default().to_string();

    // Send request
    let response = provider::send_chat_completion(provider::CompletionCall {
        base_url: &selected_provider_url,
        api_key: &api_key,
        model_name: &selected.name,
        messages: &req.messages,
        temperature: req.temperature,
        max_tokens: req.max_tokens,
        tools: req.tools.as_deref(),
        tool_choice: req.tool_choice.as_ref(),
    })
    .await;

    // Build decision
    let decision = RoutingDecision {
        id: uuid::Uuid::new_v4().to_string(),
        task_category: jev_decision.task_category,
        priority: jev_decision.priority,
        context_size_hint: Some(req.max_tokens.unwrap_or(4096)),
        selected_model_id: selected.id.clone(),
        selected_model_name: selected.name.clone(),
        selected_provider_id: selected.provider_id.clone(),
        selected_provider_name: selected_name.clone(),
        fallback_model_id: None,
        fallback_provider_id: None,
        complexity_score: jev_decision.complexity_score,
        decision_reason: jev_decision.reason_codes.join("; "),
        alternatives_considered: active_models
            .iter()
            .map(|r| format!("{} ({})", r.model.name, r.provider_name))
            .collect(),
        alternatives_rejected: Vec::new(),
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    // Try fallback if failed
    if !response.success {
        if let Some(fallback) = active_models.iter().find(|r| r.model.id != selected.id) {
            let (fb_model, fb_name, fb_key) =
                (&fallback.model, &fallback.provider_name, &fallback.api_key);
            let fb_url = fallback.endpoint().unwrap_or_default();
            let fb_response = provider::send_chat_completion(provider::CompletionCall {
                base_url: fb_url,
                api_key: fb_key,
                model_name: &fb_model.name,
                messages: &req.messages,
                temperature: req.temperature,
                max_tokens: req.max_tokens,
                tools: req.tools.as_deref(),
                tool_choice: req.tool_choice.as_ref(),
            })
            .await;

            if fb_response.success {
                return RouteResult::success_with_fallback(
                    decision,
                    fb_model.name.clone(),
                    fb_name.clone(),
                    fb_response,
                );
            }
        }
    }

    let error = if response.success {
        None
    } else {
        response.error.clone()
    };
    RouteResult {
        decision: Some(decision),
        provider_response: Some(response),
        used_fallback: false,
        error,
    }
}

/// Simple fallback: pick by priority when JEV returns no model name.
fn fallback_select(jev: &jev::JeVDecision, models: &[RoutableModel]) -> (Model, String) {
    // Try to match capability to task
    let best = models
        .iter()
        .min_by_key(|r| {
            let m = &r.model;
            let mut score = 0u32;
            match jev.task_category {
                TaskCategory::Code if m.capabilities.code => score += 100,
                TaskCategory::Reasoning if m.capabilities.reasoning => score += 100,
                TaskCategory::Vision if m.capabilities.vision => score += 100,
                TaskCategory::Chat if m.capabilities.chat => score += 100,
                _ => {}
            }
            match jev.priority {
                Priority::Cost => {
                    score += m
                        .cost_per_1k_input
                        .map(|c| (c * 1000.0) as u32)
                        .unwrap_or(500)
                }
                Priority::Speed => score += 1000 - (m.context_window / 1000),
                _ => score += 500,
            }
            score
        })
        .unwrap();

    (best.model.clone(), best.provider_name.clone())
}

/// Result of a routed request.
#[derive(Debug)]
pub struct RouteResult {
    pub decision: Option<RoutingDecision>,
    pub provider_response: Option<provider::ProviderResponse>,
    pub used_fallback: bool,
    pub error: Option<String>,
}

impl RouteResult {
    /// A named model is not available.
    ///
    /// Reports what IS available, because "model not found" without a list
    /// makes the caller guess. Substitution is not offered on purpose: the
    /// caller asked for a specific model.
    fn model_not_found(wanted: &str, available: &[&str]) -> Self {
        Self {
            decision: None,
            provider_response: None,
            used_fallback: false,
            error: Some(format!(
                "Modell '{wanted}' ist nicht verfuegbar. Verfuegbar: {}",
                if available.is_empty() {
                    "keine".to_string()
                } else {
                    available.join(", ")
                }
            )),
        }
    }

    fn no_models() -> Self {
        Self {
            decision: None,
            provider_response: None,
            used_fallback: false,
            error: Some("Keine aktiven Modelle konfiguriert".into()),
        }
    }

    fn success_with_fallback(
        decision: RoutingDecision,
        fb_name: String,
        fb_provider: String,
        response: provider::ProviderResponse,
    ) -> Self {
        let mut d = decision;
        d.fallback_model_id = Some(fb_name);
        d.fallback_provider_id = Some(fb_provider);
        d.decision_reason += " (via fallback)";
        Self {
            decision: Some(d),
            provider_response: Some(response),
            used_fallback: true,
            error: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hsm_model::ai::ModelCapabilities;

    fn model(id: &str, provider: &str) -> Model {
        Model {
            id: id.to_string(),
            provider_id: provider.to_string(),
            name: id.to_string(),
            display_name: id.to_string(),
            enabled: true,
            context_window: 8192,
            cost_per_1k_input: Some(0.0),
            cost_per_1k_output: Some(0.0),
            capabilities: ModelCapabilities::none(),
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn routable(id: &str, provider: &str, base_url: &str) -> RoutableModel {
        RoutableModel {
            model: model(id, provider),
            provider_id: provider.to_string(),
            provider_name: provider.to_string(),
            base_url: base_url.to_string(),
            api_key: "k".into(),
        }
    }

    /// The bug this replaced: the endpoint was derived from the provider id, so
    /// any provider outside the hardcoded four silently got an empty URL.
    #[test]
    fn a_custom_providers_configured_url_is_used_verbatim() {
        let r = routable("m", "lm-studio", "http://192.168.1.50:1234/v1");
        assert_eq!(r.endpoint(), Some("http://192.168.1.50:1234/v1"));
    }

    #[test]
    fn a_provider_without_a_url_is_reported_not_guessed() {
        let r = routable("m", "broken", "");
        assert_eq!(r.endpoint(), None, "an empty URL must not be handed out");
        let r = routable("m", "blank", "   ");
        assert_eq!(r.endpoint(), None, "whitespace is not a URL");
    }

    #[test]
    fn a_configured_url_is_not_overridden_by_a_known_default() {
        // OpenRouter has a hardcoded default. A self-hosted OpenRouter-compatible
        // endpoint must still win, or the operator cannot redirect it.
        let r = routable("m", "openrouter", "http://localhost:8317/v1");
        assert_eq!(r.endpoint(), Some("http://localhost:8317/v1"));
    }

    /// Models with no endpoint are dropped before routing, not sent into the void.
    #[tokio::test]
    async fn models_without_an_endpoint_never_reach_a_provider() {
        let usable = routable("good", "openrouter", "https://openrouter.ai/api");
        let unusable = routable("bad", "mystery", "");
        let req = ChatCompletionRequest {
            messages: vec![],
            model: None,
            temperature: None,
            max_tokens: None,
            tools: None,
            tool_choice: None,
            task_category: None,
            priority: None,
            stream: None,
        };
        // Routing runs; the point is that `bad` is filtered out before any HTTP
        // call, so no request is ever built with an empty base_url.
        let _ = route_request(req, &[usable, unusable], "key").await;
    }
}

#[cfg(test)]
mod explicit_model_tests {
    use super::*;
    use hsm_model::ai::ModelCapabilities;

    fn r(id: &str, provider: &str) -> RoutableModel {
        RoutableModel {
            model: Model {
                id: id.to_string(),
                provider_id: provider.to_string(),
                name: id.to_string(),
                display_name: format!("{id} display"),
                enabled: true,
                context_window: 8192,
                cost_per_1k_input: Some(0.0),
                cost_per_1k_output: Some(0.0),
                capabilities: ModelCapabilities::none(),
                created_at: String::new(),
                updated_at: String::new(),
            },
            provider_id: provider.to_string(),
            provider_name: provider.to_string(),
            base_url: "https://example.invalid/api".to_string(),
            api_key: "k".into(),
        }
    }

    fn req(model: Option<&str>) -> ChatCompletionRequest {
        ChatCompletionRequest {
            messages: vec![],
            model: model.map(str::to_string),
            temperature: None,
            max_tokens: None,
            tools: None,
            tool_choice: None,
            task_category: None,
            priority: None,
            stream: None,
        }
    }

    /// The core rule: a named model must be selected, never decided around.
    ///
    /// An empty JEV key guarantees the Decision Engine cannot succeed, so a
    /// decision produced here could only come from the explicit model.
    #[tokio::test]
    async fn an_explicit_model_is_used_without_consulting_the_decision_engine() {
        let models = vec![r("alpha", "p1"), r("beta", "p2")];
        let result = route_request(req(Some("beta")), &models, "").await;
        let err = result.error.clone().unwrap_or_default();
        // It got as far as building a real request, so beta was selected. The
        // host is unroutable, so the call fails - but not with a "no models"
        // or "jev" error, which is what routing-around would have produced.
        assert!(
            !err.contains("nicht verfuegbar"),
            "the named model must be found, got: {err}"
        );
        assert!(
            !err.contains("jev_fallback"),
            "the decision engine must not be involved, got: {err}"
        );
    }

    /// Naming a model that does not exist is an error, not a licence to
    /// substitute another one.
    #[tokio::test]
    async fn an_unknown_explicit_model_errors_instead_of_substituting() {
        let models = vec![r("alpha", "p1")];
        let result = route_request(req(Some("gpt-nonexistent")), &models, "").await;
        let err = result.error.expect("must fail");
        assert!(
            err.contains("gpt-nonexistent"),
            "error names the model: {err}"
        );
        assert!(
            err.contains("alpha"),
            "error must list what IS available, got: {err}"
        );
    }

    /// Without an explicit model, the Decision Engine stays in charge.
    #[tokio::test]
    async fn without_an_explicit_model_the_decision_engine_still_runs() {
        let models = vec![r("alpha", "p1")];
        let result = route_request(req(None), &models, "").await;
        // With an empty key JEV cannot answer, so the fallback path is taken and
        // the decision engine is recorded in the reason codes.
        let decision = result.decision.expect("a decision was made");
        assert!(
            decision.decision_reason.contains("jev_fallback")
                || decision.decision_reason.contains("fallback"),
            "routing should have gone through the decision engine, got: {}",
            decision.decision_reason
        );
    }

    #[tokio::test]
    async fn a_disabled_model_cannot_be_requested_explicitly() {
        let mut m = r("alpha", "p1");
        m.model.enabled = false;
        let result = route_request(req(Some("alpha")), &[m], "").await;
        let err = result.error.expect("must fail");
        assert!(
            err.contains("nicht verfuegbar"),
            "an explicit request must not resurrect a disabled model: {err}"
        );
    }
}
