use std::io::{Read, Write};
use std::net::{IpAddr, TcpListener};
use std::thread;
use std::time::Duration;

use lorepia_providers::url_policy::ApprovedLocalNetworkOrigin;

use super::*;

fn assert_output_round_trip(output: &DeterministicDiscoveryOutput) {
    let encoded = serde_json::to_string(output).expect("serialize deterministic result");
    let hydrated: DeterministicDiscoveryResult =
        serde_json::from_str(&encoded).expect("hydrate deterministic result");
    assert_eq!(&hydrated, output);
}

fn approved_lan_policy(origin: &str, address: &str) -> UrlPolicy {
    let address = address.parse::<IpAddr>().expect("private IP address");
    let approval = ApprovedLocalNetworkOrigin::new(origin, &[address]).expect("exact LAN approval");
    UrlPolicy::approved_local_network(approval)
}

fn local_openapi_server(extra_response_padding: usize) -> (String, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind fixture");
    let address = listener.local_addr().expect("fixture address");
    let document = format!(
        r#"{{
            "openapi":"3.1.0",
            "servers":[{{"url":"http://{address}/v1"}}],
            "paths":{{
                "/models":{{"get":{{"operationId":"listModels"}}}},
                "/responses":{{
                    "post":{{
                        "operationId":"createResponse",
                        "requestBody":{{
                            "content":{{
                                "application/json":{{
                                    "schema":{{
                                        "type":"object",
                                        "properties":{{"model":{{"type":"string"}}}}
                                    }}
                                }}
                            }}
                        }}
                    }}
                }}
            }},
            "x-padding":"{}"
        }}"#,
        "x".repeat(extra_response_padding)
    );
    let start_url = format!("http://{address}/openapi.json");
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept fixture request");
        stream
            .set_read_timeout(Some(Duration::from_secs(3)))
            .expect("set fixture timeout");
        let mut request = [0_u8; 8 * 1024];
        let _ = stream.read(&mut request).expect("read fixture request");
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{document}",
            document.len()
        );
        stream
            .write_all(response.as_bytes())
            .expect("write fixture response");
    });
    (start_url, handle)
}

#[tokio::test]
async fn known_template_returns_selected_template_and_non_approval_hint() {
    let output = DeterministicDiscoveryExecutor::new()
        .execute(DeterministicDiscoverySource::known_provider(
            BuiltInTemplateId::OpenRouter,
        ))
        .await
        .expect("known provider");

    assert_eq!(
        output
            .selected_template
            .as_ref()
            .map(|template| template.id.as_str()),
        Some("openrouter-v1")
    );
    assert_eq!(output.connection_hints.len(), 1);
    assert_eq!(
        output.connection_hints[0].api_origin.as_str(),
        "https://openrouter.ai"
    );
    assert_eq!(
        output.connection_hints[0]
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        Some("/api/v1")
    );
    assert!(
        output.connection_hints[0].requires_credential_origin_approval,
        "the hint must not be mistaken for approval"
    );
}

#[tokio::test]
async fn active_signed_catalog_template_participates_in_known_provider_matching() {
    let mut template =
        AdapterRegistry::built_in_template(BuiltInTemplateId::OpenRouter).expect("template");
    template.id = ProviderTemplateId::from("signed-openrouter-compatible-v2");
    template.manifest_version = 2;
    template.source = TemplateSource::SignedCatalog;
    let source = DeterministicDiscoverySource::known_provider_id(template.id.clone());
    let output = DeterministicDiscoveryExecutor::new()
        .execute_with_templates(source, std::slice::from_ref(&template))
        .await
        .expect("signed catalog match");

    assert_eq!(output.selected_template, Some(template));
    assert_eq!(output.connection_hints.len(), 1);
    assert!(
        output.connection_hints[0].api_base_path.is_none(),
        "a signed manifest endpoint must not inherit a built-in base path by family"
    );
    assert_eq!(output.evidence[0].kind, "signed_catalog_template");
    assert_eq!(
        output.evidence[0].extracted_json["trust"],
        "verified_signed_catalog"
    );
}

#[tokio::test]
async fn official_site_origin_matches_compiled_template() {
    let source = DeterministicDiscoverySource::known_provider_site(
        "https://ai.google.dev/gemini-api/docs",
        UrlPolicyMode::Public,
    )
    .expect("safe site");
    let output = DeterministicDiscoveryExecutor::new()
        .execute(source)
        .await
        .expect("known site");

    assert_eq!(
        output
            .selected_template
            .as_ref()
            .map(|template| template.api_family),
        Some(ApiFamily::GeminiGenerateContent)
    );
    assert_eq!(
        output.connection_hints[0].api_origin.as_str(),
        "https://generativelanguage.googleapis.com"
    );
}

#[tokio::test]
async fn known_provider_site_preserves_approved_lan_network_hint() {
    let origin = "https://192.168.42.9:11434";
    let mut template =
        AdapterRegistry::built_in_template(BuiltInTemplateId::OllamaNative).expect("template");
    template.default_manifest.default_api_origin =
        Some(CanonicalOrigin::parse(origin).expect("domain origin"));
    let source = DeterministicDiscoverySource::known_provider_site_with_policy(
        &format!("{origin}/docs"),
        approved_lan_policy(origin, "192.168.42.9"),
    )
    .expect("approved LAN site");

    let output = DeterministicDiscoveryExecutor::new()
        .execute_with_templates(source, std::slice::from_ref(&template))
        .await
        .expect("known LAN provider");

    assert_eq!(output.selected_template, Some(template));
    assert_eq!(output.connection_hints.len(), 1);
    assert_eq!(
        output.connection_hints[0].network_mode,
        ProviderNetworkMode::ApprovedLocalNetwork
    );
}

#[tokio::test]
async fn originless_custom_template_remains_selectable_without_fake_evidence() {
    let output = DeterministicDiscoveryExecutor::new()
        .execute(DeterministicDiscoverySource::known_provider(
            BuiltInTemplateId::OpenAiChatCompatible,
        ))
        .await
        .expect("custom template");

    assert_eq!(
        output
            .selected_template
            .as_ref()
            .map(|template| template.id.as_str()),
        Some("openai-chat-compatible-v1")
    );
    assert!(output.evidence.is_empty());
    assert!(output.connection_hints.is_empty());
    assert_eq!(output.family_candidates.len(), 1);
    assert!(output.family_candidates[0].evidence_indices.is_empty());
}

#[tokio::test]
async fn deterministic_result_round_trips_through_draft_json() {
    let output = DeterministicDiscoveryExecutor::new()
        .execute(DeterministicDiscoverySource::known_provider(
            BuiltInTemplateId::AnthropicMessages,
        ))
        .await
        .expect("known provider");
    assert_output_round_trip(&output);
}

#[tokio::test]
async fn site_result_round_trips_and_uses_the_bounded_credential_free_fetcher() {
    let (start_url, server) = local_openapi_server(0);
    let budget = DiscoveryFetchBudget {
        max_pages: 1,
        max_wall_clock: Duration::from_secs(3),
        max_request_duration: Duration::from_secs(3),
        ..DiscoveryFetchBudget::default()
    };
    let source =
        DeterministicDiscoverySource::site(&start_url, UrlPolicyMode::LocalLoopback, budget)
            .expect("policy-valid local fixture");
    let output = DeterministicDiscoveryExecutor::new()
        .execute(source)
        .await
        .expect("site discovery");
    server.join().expect("fixture server");

    assert_eq!(output.evidence.len(), 1);
    assert_eq!(output.manifest_candidates.len(), 1);
    assert_eq!(
        output
            .selected_template
            .as_ref()
            .map(|template| template.api_family),
        Some(ApiFamily::OpenAiResponses)
    );
    assert_eq!(
        output.connection_hints[0].network_mode,
        ProviderNetworkMode::LocalLoopback
    );
    assert!(
        output.connection_hints[0].api_base_path.is_none(),
        "the discovered template must own its API prefix"
    );
    let manifest = &output
        .selected_template
        .as_ref()
        .expect("selected discovered template")
        .default_manifest;
    assert_eq!(manifest.endpoints.generate.path.as_str(), "/v1/responses");
    assert_eq!(
        manifest
            .endpoints
            .models
            .as_ref()
            .expect("models endpoint")
            .path
            .as_str(),
        "/v1/models"
    );
    assert_output_round_trip(&output);
}

#[tokio::test]
async fn site_fetch_enforces_the_caller_budget() {
    let (start_url, server) = local_openapi_server(4 * 1024);
    let budget = DiscoveryFetchBudget {
        max_pages: 1,
        max_response_bytes_per_document: 256,
        max_decompressed_bytes_per_document: 256,
        max_total_response_bytes: 256,
        max_wall_clock: Duration::from_secs(3),
        max_request_duration: Duration::from_secs(3),
        ..DiscoveryFetchBudget::default()
    };
    let source =
        DeterministicDiscoverySource::site(&start_url, UrlPolicyMode::LocalLoopback, budget)
            .expect("valid bounded plan");
    let output = DeterministicDiscoveryExecutor::new()
        .execute(source)
        .await
        .expect("bounded failure is a discovery result");
    server.join().expect("fixture server");

    assert!(output.evidence.is_empty());
    assert_eq!(output.fetch_issues.len(), 1);
    assert_eq!(output.fetch_issues[0].kind, "document_too_large");
    assert_output_round_trip(&output);
}

#[tokio::test]
async fn c_url_is_consumed_once_and_no_scalar_or_credential_is_returned() {
    let secret = "sk-test-secret-never-return";
    let signed_path_secret = "signed-path-secret-never-return";
    let model_scalar = "credentiallookingmodelscalar";
    let raw = format!(
        "curl 'https://example.com/{signed_path_secret}/v1/chat/completions?api_key={secret}' \
         -H 'Authorization: Bearer {secret}' \
         -H 'Content-Type: application/json' \
         --data '{{\"model\":\"{model_scalar}\",\"messages\":[],\"stream\":true}}'"
    );
    let source = sanitize_curl_source(SecretCurlInput::new(raw), UrlPolicyMode::Public)
        .expect("sanitized source");
    let debug = format!("{source:?}");
    assert!(!debug.contains(secret));
    assert!(!debug.contains(model_scalar));
    assert!(!debug.contains(signed_path_secret));

    let output = DeterministicDiscoveryExecutor::new()
        .execute(source)
        .await
        .expect("curl discovery");
    let serialized = serde_json::to_string(&output).expect("serialize output");
    assert!(!serialized.contains(secret));
    assert!(!serialized.contains(model_scalar));
    assert!(!serialized.contains(signed_path_secret));
    assert!(!serialized.contains("redacted_curl"));
    assert!(!serialized.contains("model_hint"));
    assert!(serialized.contains("sanitized_curl_request"));
    assert!(
        output.evidence[0].extracted_json.get("path").is_none(),
        "cURL source paths must be represented only by their digest"
    );
    assert_eq!(output.family_candidates.len(), 1);
    assert_eq!(
        output.family_candidates[0].api_family,
        ApiFamily::OpenAiChatCompletions
    );
    assert_eq!(
        output.connection_hints[0]
            .api_base_path
            .as_ref()
            .map(EndpointPath::as_str),
        None
    );
    let manifest = &output
        .selected_template
        .as_ref()
        .expect("selected discovered template")
        .default_manifest;
    assert_eq!(
        manifest.endpoints.generate.path.as_str(),
        "/v1/chat/completions"
    );
    assert_eq!(
        manifest
            .endpoints
            .models
            .as_ref()
            .expect("models endpoint")
            .path
            .as_str(),
        "/v1/models"
    );
    assert_output_round_trip(&output);
}

#[test]
fn embedding_a_safe_custom_base_is_idempotent_and_changes_the_manifest_hash() {
    let mut manifest = AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
        .expect("seed template")
        .default_manifest;
    let original_hash = lorepia_providers::validate_manifest(&manifest)
        .expect("validate seed manifest")
        .sha256()
        .to_owned();
    let base_path = EndpointPath::parse("/api/v2").expect("safe custom base");

    embed_discovered_api_base_path(&mut manifest, Some(&base_path)).expect("embed custom base");
    embed_discovered_api_base_path(&mut manifest, Some(&base_path))
        .expect("embedding is idempotent");

    assert_eq!(
        manifest.endpoints.generate.path.as_str(),
        "/api/v2/chat/completions"
    );
    assert_eq!(
        manifest
            .endpoints
            .models
            .as_ref()
            .expect("models endpoint")
            .path
            .as_str(),
        "/api/v2/models"
    );
    assert_ne!(
        lorepia_providers::validate_manifest(&manifest)
            .expect("validate embedded manifest")
            .sha256(),
        original_hash
    );

    let conflicting_base = EndpointPath::parse("/v3").expect("conflicting safe base");
    let error = embed_discovered_api_base_path(&mut manifest, Some(&conflicting_base))
        .expect_err("an already-evidenced base must not be nested under another base");
    assert_eq!(
        error.kind(),
        DeterministicDiscoveryErrorKind::UnsafeEvidence
    );
    assert_eq!(
        manifest.endpoints.generate.path.as_str(),
        "/api/v2/chat/completions"
    );

    let mut inconsistent =
        AdapterRegistry::built_in_template(BuiltInTemplateId::OpenAiChatCompatible)
            .expect("seed inconsistent template")
            .default_manifest;
    inconsistent.endpoints.generate.path =
        EndpointPath::parse("/chat/completions").expect("root generation endpoint");
    inconsistent
        .endpoints
        .models
        .as_mut()
        .expect("models endpoint")
        .path = EndpointPath::parse("/v3/models").expect("conflicting models endpoint");
    let unchanged = inconsistent.clone();
    let error = embed_discovered_api_base_path(&mut inconsistent, Some(&base_path))
        .expect_err("a secondary endpoint with another safe base must fail closed");
    assert_eq!(
        error.kind(),
        DeterministicDiscoveryErrorKind::UnsafeEvidence
    );
    assert_eq!(
        inconsistent, unchanged,
        "failed embedding must be atomic across every endpoint"
    );
}

#[test]
fn site_source_strips_queries_and_fragments_and_rejects_invalid_budget() {
    let source = DeterministicDiscoverySource::site(
        "https://example.com/docs?token=secret&view=reference#credential",
        UrlPolicyMode::Public,
        DiscoveryFetchBudget::default(),
    )
    .expect("query and fragment are discarded");
    let debug = format!("{source:?}");
    assert!(!debug.contains("secret"));
    assert!(!debug.contains("reference"));
    assert!(!debug.contains("credential"));
    let DeterministicDiscoverySourceKind::Site { plan } = source.kind else {
        panic!("site source");
    };
    assert!(plan.start_url().url().query().is_none());
    assert!(plan.start_url().url().fragment().is_none());

    let budget = DiscoveryFetchBudget {
        max_pages: 0,
        ..DiscoveryFetchBudget::default()
    };
    let error = DeterministicDiscoverySource::site(
        "https://example.com/docs",
        UrlPolicyMode::Public,
        budget,
    )
    .expect_err("invalid budget");
    assert_eq!(
        error.kind(),
        DeterministicDiscoveryErrorKind::InvalidFetchBudget
    );
}

#[test]
fn document_allowlist_does_not_create_credential_approval() {
    let mut source = DeterministicDiscoverySource::site(
        "https://docs.example.com",
        UrlPolicyMode::Public,
        DiscoveryFetchBudget {
            max_wall_clock: Duration::from_secs(1),
            max_request_duration: Duration::from_secs(1),
            ..DiscoveryFetchBudget::default()
        },
    )
    .expect("site");
    source
        .allow_document_url("https://api.example.com/openapi.json")
        .expect("allow document host");
    let debug = format!("{source:?}");
    assert!(!debug.contains("approved"));
    assert!(!debug.contains("credential"));
}

#[test]
fn approved_lan_site_allowlist_retains_the_exact_policy() {
    let origin = "http://192.168.42.9:11434";
    let mut source = DeterministicDiscoverySource::site_with_policy(
        &format!("{origin}/docs"),
        approved_lan_policy(origin, "192.168.42.9"),
        DiscoveryFetchBudget::default(),
    )
    .expect("approved LAN site");

    source
        .allow_document_url(&format!("{origin}/openapi.json"))
        .expect("same exact approved origin");
    let error = source
        .allow_document_url("http://192.168.42.10:11434/openapi.json")
        .expect_err("another private origin must not inherit approval");
    assert_eq!(
        error.kind(),
        DeterministicDiscoveryErrorKind::InvalidDocumentUrl
    );
}

#[test]
fn public_http_curl_origin_fails_closed_with_safe_error() {
    let error = sanitize_curl_source(
        SecretCurlInput::from(
            "curl -X POST http://api.example.com/v1/responses \
         -H 'Authorization: Bearer should-not-leak' \
         --data '{\"model\":\"safe-model\"}'",
        ),
        UrlPolicyMode::Public,
    )
    .expect_err("network policy rejects public HTTP");
    assert_eq!(
        error.kind(),
        DeterministicDiscoveryErrorKind::InvalidDocumentUrl
    );
    assert!(!error.to_string().contains("should-not-leak"));
    assert!(!error.to_string().contains("api.example.com"));
}

#[tokio::test]
async fn loopback_curl_requires_explicit_local_mode() {
    let command = "curl -X POST http://127.0.0.1:11434/api/chat \
                   --data '{\"model\":\"llama3\",\"messages\":[]}'";
    let public_error = sanitize_curl_source(SecretCurlInput::from(command), UrlPolicyMode::Public)
        .expect_err("public mode must not authorize loopback");
    assert_eq!(
        public_error.kind(),
        DeterministicDiscoveryErrorKind::InvalidDocumentUrl
    );

    let local = sanitize_curl_source(SecretCurlInput::from(command), UrlPolicyMode::LocalLoopback)
        .expect("explicit local mode");
    let output = DeterministicDiscoveryExecutor::new()
        .execute(local)
        .await
        .expect("local cURL discovery");
    assert_eq!(
        output.connection_hints[0].network_mode,
        ProviderNetworkMode::LocalLoopback
    );
    assert_output_round_trip(&output);
}

#[tokio::test]
async fn approved_lan_curl_retains_exact_policy_through_execution() {
    let origin = "http://192.168.42.9:11434";
    let parsed = parse_curl(SecretCurlInput::from(format!(
        "curl -X POST {origin}/api/chat --data '{{\"model\":\"llama3\",\"messages\":[]}}'"
    )))
    .expect("parse LAN cURL");
    let source = DeterministicDiscoverySource::sanitized_curl_with_policy(
        parsed,
        approved_lan_policy(origin, "192.168.42.9"),
    )
    .expect("approved LAN source");

    let output = DeterministicDiscoveryExecutor::new()
        .execute(source)
        .await
        .expect("approved LAN cURL discovery");

    assert_eq!(output.connection_hints.len(), 1);
    assert_eq!(
        output.connection_hints[0].network_mode,
        ProviderNetworkMode::ApprovedLocalNetwork
    );
    assert!(
        output
            .selected_template
            .as_ref()
            .is_some_and(|template| template.default_manifest.default_api_origin.is_none()),
        "an exact LAN grant must remain connection-specific"
    );
    assert_output_round_trip(&output);
}

#[test]
fn sanitized_curl_json_omits_model_hint_and_redacted_command() {
    let parsed = parse_curl(SecretCurlInput::from(
        "curl -X POST https://example.com/v1/responses \
         -H 'Authorization: Bearer top-secret' \
         --data '{\"model\":\"secret-shaped-model\",\"input\":\"private\"}'",
    ))
    .expect("parse");
    let sanitized = SanitizedCurlDiscoveryEvidence::from(parsed);
    let value = sanitized_curl_json(&sanitized).expect("sanitized JSON");
    let encoded = serde_json::to_string(&value).expect("encode");
    assert!(!encoded.contains("secret-shaped-model"));
    assert!(!encoded.contains("top-secret"));
    assert!(!encoded.contains("private"));
    assert!(!encoded.contains("model_hint"));
    assert!(!encoded.contains("redacted_curl"));
}

#[test]
fn extracted_curl_body_is_only_a_scalar_free_shape() {
    let parsed = parse_curl(SecretCurlInput::from(
        "curl -X POST https://example.com/v1/responses \
         --data '{\"input\":\"do-not-store\",\"nested\":{\"token\":\"also-secret\"}}'",
    ))
    .expect("parse");
    let shape = parsed.body_json_shape.as_ref().expect("shape");
    assert!(matches!(shape, lorepia_providers::JsonShape::Object { .. }));
    let sanitized = SanitizedCurlDiscoveryEvidence::from(parsed);
    let encoded =
        serde_json::to_string(&sanitized_curl_json(&sanitized).expect("safe")).expect("encode");
    assert!(!encoded.contains("do-not-store"));
    assert!(!encoded.contains("also-secret"));
}
