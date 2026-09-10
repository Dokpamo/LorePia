use super::*;
use crate::asset_protocol::admission::retain_permit;

fn request(range: Option<&str>) -> Request<Vec<u8>> {
    let mut request = Request::builder()
        .method(Method::GET)
        .uri(format!("lorepia-asset://sha256/{}", "ab".repeat(32)));
    if let Some(range) = range {
        request = request.header("range", range);
    }
    request.body(Vec::new()).expect("asset request")
}

#[test]
fn four_concurrent_small_assets_use_only_their_approved_body_sizes() {
    let admission = AssetProtocolAdmission::default();
    let mut responses = Vec::new();
    for _ in 0..4 {
        let backend = CountingBackend::new();
        let mut permit = admission.try_acquire().expect("work slot");
        let mut response = handle_with_backend(&backend, request(None), &mut permit);
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.body(), &backend.bytes);
        retain_permit(&mut response, permit);
        responses.push(response);
    }
    assert_eq!(admission.reserved_bytes(), 32);
    drop(responses);
    assert_eq!(admission.reserved_bytes(), 0);
}

#[test]
fn byte_reservation_matches_the_range_and_invalid_ranges_read_nothing() {
    let admission = AssetProtocolAdmission::default();
    let backend = CountingBackend::new();
    let mut permit = admission.try_acquire().expect("work slot");
    let response = handle_with_backend(&backend, request(Some("bytes=2-4")), &mut permit);
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(admission.reserved_bytes(), 3);
    assert_eq!(backend.last_read_length.get(), Some(3));
    drop(permit);
    let backend = CountingBackend::new();
    let mut permit = admission.try_acquire().expect("work slot");
    let response = handle_with_backend(&backend, request(Some("bytes=99-100")), &mut permit);
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(admission.reserved_bytes(), 0);
    assert_eq!(backend.read_calls.get(), 0);
}

#[test]
fn head_and_rejected_request_bodies_never_reserve_or_read_media_bytes() {
    let admission = AssetProtocolAdmission::default();
    let backend = CountingBackend::new();
    let mut permit = admission.try_acquire().expect("work slot");
    let mut head = request(None);
    *head.method_mut() = Method::HEAD;
    let response = handle_with_backend(&backend, head, &mut permit);
    assert_eq!(response.status(), StatusCode::OK);
    assert!(response.body().is_empty());
    assert_eq!(admission.reserved_bytes(), 0);
    assert_eq!(backend.read_calls.get(), 0);
    let mut get_with_body = request(None);
    *get_with_body.body_mut() = vec![1];
    assert_eq!(
        preflight_response(&get_with_body)
            .expect("unsupported body")
            .status(),
        StatusCode::BAD_REQUEST
    );
}
