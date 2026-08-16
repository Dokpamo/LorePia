use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::provider::{EvidenceId, ModelRouteId, ObservationId};

/// A normalized provider or model capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityKey {
    Streaming,
    Reasoning,
    PromptCaching,
    ToolCalling,
    ParallelToolCalling,
    StructuredOutput,
    JsonMode,
    ImageInput,
    AudioInput,
    AudioOutput,
    Logprobs,
    Seed,
    Batch,
    Background,
    ContextWindow,
    MaxOutputTokens,
}

/// A typed capability value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum CapabilityValue {
    Boolean(bool),
    Integer(u64),
    EnumValues(Vec<String>),
    Structured(Value),
}

/// How directly the capability claim was established.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SupportStatus {
    Verified,
    Documented,
    Inferred,
    Unsupported,
    Unknown,
    Conditional,
}

/// Provenance category for a capability observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    ProviderApi,
    OfficialDocumentation,
    SignedLorepiaCatalog,
    CapabilityProbe,
    UserOverride,
    LlmInference,
}

/// Confidence in an observation, independent from its provenance category.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Confidence {
    Low,
    Medium,
    High,
}

/// A source-attributed, freshness-bounded capability claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilityObservation {
    pub id: ObservationId,
    pub model_route_id: ModelRouteId,
    pub key: CapabilityKey,
    pub value: CapabilityValue,
    pub status: SupportStatus,
    pub source: ObservationSource,
    pub confidence: Confidence,
    pub observed_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub evidence_ref: Option<EvidenceId>,
}

impl CapabilityObservation {
    pub fn is_fresh_at(&self, now: DateTime<Utc>) -> bool {
        self.expires_at.is_none_or(|expires_at| expires_at > now)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::{
        CapabilityKey, CapabilityObservation, CapabilityValue, Confidence, ObservationSource,
        SupportStatus,
    };
    use crate::provider::{EvidenceId, ModelRouteId, ObservationId};

    #[test]
    fn capability_wire_values_are_stable_and_typed() {
        assert_eq!(
            serde_json::to_string(&CapabilityKey::PromptCaching).expect("serialize key"),
            "\"prompt_caching\""
        );
        assert_eq!(
            serde_json::to_string(&CapabilityValue::Integer(128_000))
                .expect("serialize capability value"),
            r#"{"type":"integer","value":128000}"#
        );
        assert_eq!(
            serde_json::to_string(&ObservationSource::SignedLorepiaCatalog)
                .expect("serialize source"),
            "\"signed_lorepia_catalog\""
        );
    }

    #[test]
    fn observation_round_trip_preserves_source_and_freshness() {
        let observed_at = Utc
            .with_ymd_and_hms(2026, 7, 31, 12, 0, 0)
            .single()
            .expect("valid timestamp");
        let expires_at = observed_at + chrono::Duration::days(7);
        let observation = CapabilityObservation {
            id: ObservationId::from("observation-1"),
            model_route_id: ModelRouteId::from("route-1"),
            key: CapabilityKey::StructuredOutput,
            value: CapabilityValue::Boolean(true),
            status: SupportStatus::Verified,
            source: ObservationSource::CapabilityProbe,
            confidence: Confidence::High,
            observed_at,
            expires_at: Some(expires_at),
            evidence_ref: Some(EvidenceId::from("evidence-1")),
        };

        let encoded = serde_json::to_string(&observation).expect("serialize observation");
        let decoded: CapabilityObservation =
            serde_json::from_str(&encoded).expect("deserialize observation");
        assert_eq!(decoded, observation);
        assert!(decoded.is_fresh_at(observed_at));
        assert!(!decoded.is_fresh_at(expires_at));
    }
}
