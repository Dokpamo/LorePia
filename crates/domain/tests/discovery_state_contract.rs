//! Public-surface contract test for the durable provider discovery state
//! machine. Exhaustive transition/property coverage lives beside the reducer.

use lorepia_domain::{
    DiscoverySessionId,
    discovery::{
        DiscoveryActionEnvelope, DiscoveryActionId, DiscoveryEffect, DiscoveryState,
        ProviderDiscoveryAction, ProviderDiscoverySession, SanitizedDiscoveryInput,
    },
};

const HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

#[test]
fn exported_discovery_contract_starts_with_a_revision_checked_durable_effect() {
    let input = serde_json::from_str::<SanitizedDiscoveryInput>(
        r#"{"connection_id":"connection-public","display_name":"Public Provider","site_url":"https://provider.example/","docs_url":null,"credential_ref":null,"preferred_assistant":null,"local_network_mode":false,"supplied_evidence_ids":[]}"#,
    )
    .unwrap();
    let session =
        ProviderDiscoverySession::new(DiscoverySessionId::from("session-public"), input).unwrap();
    let transition = session
        .apply(&DiscoveryActionEnvelope {
            id: DiscoveryActionId::parse("action-public").expect("valid action id"),
            expected_revision: 0,
            request_sha256: HASH.to_owned(),
            action: ProviderDiscoveryAction::Begin,
        })
        .unwrap();

    assert_eq!(
        transition.session.state,
        DiscoveryState::ResolvingKnownProvider
    );
    assert_eq!(transition.session.revision, 1);
    assert_eq!(transition.event.sequence, 1);
    assert_eq!(transition.effect, DiscoveryEffect::ResolveKnownProvider);
}
