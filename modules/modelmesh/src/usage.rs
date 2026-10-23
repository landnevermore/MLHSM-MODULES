//! Usage tracking: record and aggregate API usage.
//!
//! Cost comes from the model's recorded price. It is never estimated.
//!
//! The previous implementation applied a hardcoded "mid-range" price
//! ($0.002/1K input, $0.006/1K output) to every request regardless of the
//! model, so a free-only gateway reported real money on models priced at zero.
//! That is the one number the whole product is sold on, and it was fabricated.
//!
//! Three distinct outcomes, deliberately not collapsed into one:
//!
//! - `Some(0.0)` — the model's price is known and is zero. It IS free.
//! - `Some(x)`  — the model's price is known and is non-zero.
//! - `None`     — the price is unknown. NOT "free", and NOT a guess. An unknown
//!   price that renders as `$0.00` is a lie that hides missing configuration.

use hsm_model::ai::*;

/// The price of one model, as recorded in the local registry.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModelPricing {
    /// USD per 1K input tokens. `None` = unknown.
    pub input_per_1k: Option<f64>,
    /// USD per 1K output tokens. `None` = unknown.
    pub output_per_1k: Option<f64>,
}

impl ModelPricing {
    /// A model that is known to cost nothing.
    pub const FREE: Self = Self {
        input_per_1k: Some(0.0),
        output_per_1k: Some(0.0),
    };

    /// Pricing is only fully known when both halves are known.
    pub fn is_fully_known(&self) -> bool {
        self.input_per_1k.is_some() && self.output_per_1k.is_some()
    }

    /// True only when both prices are known *and* zero.
    ///
    /// Deliberately not `input == Some(0.0) || output == Some(0.0)`: a model
    /// that is free to prompt but charges for completion is not free.
    pub fn is_free(&self) -> bool {
        self.is_fully_known() && self.input_per_1k == Some(0.0) && self.output_per_1k == Some(0.0)
    }
}

/// Cost of one request, in USD.
///
/// `None` means "not determinable", which is different from zero.
pub fn estimate_cost(input_tokens: u64, output_tokens: u64, pricing: &ModelPricing) -> Option<f64> {
    if input_tokens == 0 && output_tokens == 0 {
        return None;
    }
    if !pricing.is_fully_known() {
        // Unknown price. Guessing produced the exact number this whole change
        // exists to remove.
        return None;
    }
    let input = input_tokens as f64 / 1000.0 * pricing.input_per_1k?;
    let output = output_tokens as f64 / 1000.0 * pricing.output_per_1k?;
    Some(((input + output) * 10000.0).round() / 10000.0)
}

/// Create a usage record from a provider response + routing decision.
pub fn record_from_response(
    decision: &RoutingDecision,
    response: &crate::provider::ProviderResponse,
    task_category: Option<TaskCategory>,
    pricing: &ModelPricing,
) -> UsageRecord {
    let cost = estimate_cost(
        response.input_tokens.unwrap_or(0),
        response.output_tokens.unwrap_or(0),
        pricing,
    );

    UsageRecord {
        id: uuid::Uuid::new_v4().to_string(),
        provider_id: decision.selected_provider_id.clone(),
        model_id: decision.selected_model_id.clone(),
        task_category,
        input_tokens: response.input_tokens,
        output_tokens: response.output_tokens,
        latency_ms: Some(response.latency_ms),
        cost_usd: cost,
        success: response.success,
        error_message: response.error.clone(),
        created_at: chrono::Utc::now().to_rfc3339(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn priced(input: Option<f64>, output: Option<f64>) -> ModelPricing {
        ModelPricing {
            input_per_1k: input,
            output_per_1k: output,
        }
    }

    /// The bug: a free model was billed at a made-up mid-range price.
    #[test]
    fn a_free_model_costs_exactly_zero() {
        let c = estimate_cost(1000, 1000, &ModelPricing::FREE).expect("known price");
        assert_eq!(c, 0.0, "a free model must never accrue cost");
        // And at a volume large enough that rounding used to surface something.
        let c = estimate_cost(250_000, 180_000, &ModelPricing::FREE).expect("known price");
        assert_eq!(c, 0.0);
    }

    #[test]
    fn a_paid_model_is_costed_from_its_own_price() {
        let c = estimate_cost(1000, 1000, &priced(Some(0.003), Some(0.015))).expect("cost");
        assert!((c - 0.018).abs() < 1e-9, "got {c}");
    }

    /// The important distinction: unknown is not free.
    #[test]
    fn an_unknown_price_yields_no_cost_rather_than_a_guess() {
        assert_eq!(estimate_cost(1000, 1000, &priced(None, Some(0.0))), None);
        assert_eq!(estimate_cost(1000, 1000, &priced(Some(0.0), None)), None);
        assert_eq!(estimate_cost(1000, 1000, &priced(None, None)), None);
    }

    /// Free to prompt but charged for completion is NOT free.
    #[test]
    fn a_half_free_model_is_not_free() {
        let p = priced(Some(0.0), Some(0.01));
        assert!(!p.is_free(), "one-sided zero is not free");
        assert!(p.is_fully_known(), "both halves are known here");
        let c = estimate_cost(1000, 1000, &p).expect("cost");
        assert!(c > 0.0, "completion still costs, got {c}");
    }

    #[test]
    fn no_tokens_means_no_cost_even_for_a_paid_model() {
        assert_eq!(estimate_cost(0, 0, &priced(Some(0.1), Some(0.2))), None);
    }

    #[test]
    fn is_free_requires_both_halves_known() {
        assert!(ModelPricing::FREE.is_free());
        assert!(!priced(None, None).is_free());
        assert!(!priced(Some(0.0), None).is_free());
        assert!(!priced(Some(0.0), Some(0.001)).is_free());
    }
}
