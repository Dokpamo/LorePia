use super::*;

fn backend() -> CountingBackend {
    let mut backend = CountingBackend::new();
    backend.bytes = png::valid_test_image();
    backend.descriptor.media_type = "image/png".to_owned();
    backend.descriptor.size_bytes = backend.bytes.len() as u64;
    backend
}

fn request(range: Option<&str>, method: Method) -> Request<Vec<u8>> {
    let mut builder = Request::builder()
        .method(method)
        .uri(format!("lorepia-asset://sha256/{}", "ab".repeat(32)));
    if let Some(range) = range {
        builder = builder.header("range", range);
    }
    builder.body(Vec::new()).expect("asset request")
}

#[test]
fn valid_png_ranges_reserve_only_the_requested_verified_bytes() {
    for range in [None, Some("bytes=2-4")] {
        let backend = backend();
        let admission = AssetProtocolAdmission::default();
        let mut permit = admission.try_acquire().expect("work slot");
        let response = handle_with_backend(&backend, request(range, Method::GET), &mut permit);
        let (start, length) = if range.is_some() { (2, 3) } else { (0, 70) };
        assert_eq!(backend.last_read_start.get(), Some(start));
        assert_eq!(backend.last_read_length.get(), Some(length));
        assert_eq!(admission.reserved_bytes(), length);
        assert_eq!(response.headers()[CONTENT_TYPE], "image/png");
        if range.is_some() {
            assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
            assert_eq!(response.headers()[CONTENT_RANGE], "bytes 2-4/70");
            assert_eq!(response.headers()[CONTENT_LENGTH], "3");
            assert_eq!(response.body(), &backend.bytes[2..5]);
        } else {
            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(response.body(), &backend.bytes);
        }
        drop(permit);
        assert_eq!(admission.reserved_bytes(), 0);
    }
}

#[test]
fn corrupt_png_is_never_delivered_even_when_bad_crc_is_outside_requested_range() {
    let mut backend = backend();
    backend.bytes[54..58].copy_from_slice(&[0x89, 0x99, 0x3d, 0x1d]);
    for range in [None, Some("bytes=0-7"), Some("bytes=-4")] {
        let response = respond(&backend, request(range, Method::GET));
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(response.body().is_empty());
    }
}

#[test]
fn png_head_and_invalid_or_oversized_requests_still_read_no_body() {
    let backend = backend();
    for range in [None, Some("bytes=2-4")] {
        let response = respond(&backend, request(range, Method::HEAD));
        assert!(response.status().is_success());
        assert!(response.body().is_empty());
    }
    assert_eq!(backend.read_calls.get(), 0);
    let response = respond(&backend, request(Some("bytes=70-71"), Method::GET));
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(backend.read_calls.get(), 0);
    let mut oversized = backend;
    oversized.descriptor.size_bytes = MAX_RENDERABLE_IMAGE_BYTES + 1;
    let response = respond(&oversized, request(Some("bytes=0-1"), Method::GET));
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(oversized.read_calls.get(), 0);
}

#[test]
fn stale_or_unknown_image_policy_cannot_authorize_a_native_response() {
    for policy in [0, AssetProtocolRange::IMAGE_VALIDATION_POLICY + 1, u32::MAX] {
        let mut backend = backend();
        backend.policy = policy;
        let response = respond(&backend, request(Some("bytes=0-7"), Method::GET));
        assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        assert!(response.body().is_empty());
        assert_eq!(response.headers()["cache-control"], "no-store");
    }
}
