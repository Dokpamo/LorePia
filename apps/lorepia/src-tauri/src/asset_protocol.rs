//! Strict custom-protocol adapter for approved content-addressed media.
//!
//! Only the canonical `lorepia-asset://sha256/<digest>` form and Tauri/Wry's
//! strict localhost transport forms are accepted. The native handler never
//! accepts a path, package member name, URL, MIME override, or caller-provided
//! bytes.

mod admission;
mod png;

pub(crate) use admission::AssetProtocolAdmission;
use admission::AssetProtocolPermit;

use lorepia_shell_api::{
    AssetDeliveryDto, AssetDeliveryKindDto, AssetProtocolRange, ShellApi, ShellErrorCode,
};
use tauri::{
    State,
    http::{
        Method, Request, Response, StatusCode, Uri,
        header::{
            ACCEPT_RANGES, ALLOW, CACHE_CONTROL, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE,
            RETRY_AFTER,
        },
        response::Builder,
    },
};

use crate::state::AppState;

const MAX_RENDERABLE_ASSET_BYTES: u64 = 64 * 1_024 * 1_024;
const MAX_RENDERABLE_IMAGE_BYTES: u64 = 16 * 1_024 * 1_024;
const MAX_RANGE_BYTES: u64 = 1_024 * 1_024;

fn handle(
    state: State<'_, AppState>,
    request: Request<Vec<u8>>,
    permit: &mut AssetProtocolPermit,
) -> Response<Vec<u8>> {
    if let Some(response) = preflight_response(&request) {
        return response;
    }
    let Ok(shell) = state.shell() else {
        return empty(StatusCode::SERVICE_UNAVAILABLE);
    };
    handle_with_backend(&shell, request, permit)
}

trait AssetProtocolBackend {
    fn resolve_descriptor(&self, sha256: &str) -> Result<AssetDeliveryDto, ShellErrorCode>;

    fn read_verified_range(
        &self,
        sha256: &str,
        start: u64,
        requested_bytes: u64,
    ) -> Result<AssetProtocolRange, ShellErrorCode>;
}

impl AssetProtocolBackend for ShellApi {
    fn resolve_descriptor(&self, sha256: &str) -> Result<AssetDeliveryDto, ShellErrorCode> {
        self.resolve_asset_protocol_sha256(sha256)
            .map_err(|error| error.code)
    }

    fn read_verified_range(
        &self,
        sha256: &str,
        start: u64,
        requested_bytes: u64,
    ) -> Result<AssetProtocolRange, ShellErrorCode> {
        self.read_asset_protocol_range(sha256, start, requested_bytes)
            .map_err(|error| error.code)
    }
}

fn handle_with_backend(
    backend: &impl AssetProtocolBackend,
    request: Request<Vec<u8>>,
    permit: &mut AssetProtocolPermit,
) -> Response<Vec<u8>> {
    if let Some(response) = preflight_response(&request) {
        return response;
    }
    let Some(sha256) = digest_from_uri(request.uri()) else {
        return empty(StatusCode::BAD_REQUEST);
    };

    let descriptor = match backend.resolve_descriptor(sha256) {
        Ok(descriptor) => descriptor,
        Err(error) => return empty(status_for_shell_error(error)),
    };
    if descriptor.sha256 != sha256 {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    }
    if !delivery_size_is_allowed(&descriptor) {
        return empty(StatusCode::PAYLOAD_TOO_LARGE);
    }

    let range = match request.headers().get("range") {
        Some(header) => {
            let Ok(header) = header.to_str() else {
                return range_not_satisfiable(descriptor.size_bytes);
            };
            match parse_single_range(header, descriptor.size_bytes) {
                Some(range) => Some(range),
                None => return range_not_satisfiable(descriptor.size_bytes),
            }
        }
        None => None,
    };
    let response_range = range.unwrap_or(ByteRange {
        start: 0,
        length: descriptor.size_bytes,
    });

    let mut builder = media_response(&descriptor);
    if range.is_some() {
        let end = response_range.start + response_range.length - 1;
        builder = builder
            .status(StatusCode::PARTIAL_CONTENT)
            .header(
                CONTENT_RANGE,
                format!(
                    "bytes {}-{end}/{}",
                    response_range.start, descriptor.size_bytes
                ),
            )
            .header(CONTENT_LENGTH, response_range.length.to_string());
    } else {
        builder = builder
            .status(StatusCode::OK)
            .header(CONTENT_LENGTH, descriptor.size_bytes.to_string());
    }

    if request.method() == Method::HEAD {
        return finish(builder, Vec::new());
    }
    // A partial PNG response must not bypass validation of the complete image.
    let is_png = descriptor.media_type == "image/png";
    let read_range = if is_png {
        ByteRange {
            start: 0,
            length: descriptor.size_bytes,
        }
    } else {
        response_range
    };
    if !permit.reserve_bytes(read_range.length) {
        return overloaded_response();
    }
    let verified = match backend.read_verified_range(sha256, read_range.start, read_range.length) {
        Ok(verified) => verified,
        Err(error) => return empty(status_for_shell_error(error)),
    };
    if verified.descriptor != descriptor || verified.start != read_range.start {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    }
    let mut body = verified.bytes;
    let Ok(actual_length) = u64::try_from(body.len()) else {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    };
    if actual_length != read_range.length {
        return empty(StatusCode::INTERNAL_SERVER_ERROR);
    }
    if is_png {
        if !png::has_valid_chunk_checksums(&body) {
            return empty(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
        // Keep the full buffer reservation; avoid allocating another range buffer.
        let start = usize::try_from(response_range.start).expect("bounded image offset");
        let length = usize::try_from(response_range.length).expect("bounded image length");
        body.copy_within(start..start + length, 0);
        body.truncate(length);
    }
    finish(builder, body)
}

pub(crate) fn preflight_response(request: &Request<Vec<u8>>) -> Option<Response<Vec<u8>>> {
    if request.method() != Method::GET && request.method() != Method::HEAD {
        return Some(finish(
            base_response(StatusCode::METHOD_NOT_ALLOWED).header(ALLOW, "GET, HEAD"),
            Vec::new(),
        ));
    }
    if !request.body().is_empty() || digest_from_uri(request.uri()).is_none() {
        return Some(empty(StatusCode::BAD_REQUEST));
    }
    None
}

pub(crate) fn overloaded_response() -> Response<Vec<u8>> {
    finish(
        base_response(StatusCode::SERVICE_UNAVAILABLE)
            .header(RETRY_AFTER, "1")
            .header(CONTENT_LENGTH, "0"),
        Vec::new(),
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    length: u64,
}

fn digest_from_uri(uri: &Uri) -> Option<&str> {
    if uri.query().is_some() {
        return None;
    }
    let authority = uri.authority()?.as_str();
    let path = uri.path();
    let is_tauri_localhost_transport = (uri.scheme_str() == Some("lorepia-asset")
        && authority == "localhost")
        || (matches!(uri.scheme_str(), Some("http" | "https"))
            && authority == "lorepia-asset.localhost");
    let digest = if uri.scheme_str() == Some("lorepia-asset")
        && authority == "sha256"
        && path.starts_with('/')
        && !path[1..].contains('/')
    {
        &path[1..]
    } else if is_tauri_localhost_transport
        && path.starts_with("/sha256/")
        && !path["/sha256/".len()..].contains('/')
    {
        &path["/sha256/".len()..]
    } else {
        return None;
    };
    if digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Some(digest)
    } else {
        None
    }
}

fn parse_single_range(value: &str, total: u64) -> Option<ByteRange> {
    if total == 0 || value.contains(',') {
        return None;
    }
    let value = value.strip_prefix("bytes=")?;
    let (start, end) = value.split_once('-')?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().ok()?;
        if suffix == 0 {
            return None;
        }
        let length = suffix.min(total).min(MAX_RANGE_BYTES);
        return Some(ByteRange {
            start: total - length,
            length,
        });
    }
    let start = start.parse::<u64>().ok()?;
    if start >= total {
        return None;
    }
    let requested_end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<u64>().ok()?.min(total - 1)
    };
    if requested_end < start {
        return None;
    }
    let requested_length = requested_end.checked_sub(start)?.checked_add(1)?;
    Some(ByteRange {
        start,
        length: requested_length.min(MAX_RANGE_BYTES),
    })
}

fn delivery_size_is_allowed(descriptor: &AssetDeliveryDto) -> bool {
    descriptor.size_bytes > 0
        && descriptor.size_bytes <= MAX_RENDERABLE_ASSET_BYTES
        && (descriptor.kind != AssetDeliveryKindDto::Image
            || descriptor.size_bytes <= MAX_RENDERABLE_IMAGE_BYTES)
}

fn media_response(descriptor: &AssetDeliveryDto) -> Builder {
    base_response(StatusCode::OK)
        .header(CONTENT_TYPE, descriptor.media_type.as_str())
        .header(ACCEPT_RANGES, "bytes")
}

fn base_response(status: StatusCode) -> Builder {
    Response::builder()
        .status(status)
        .header(CACHE_CONTROL, "no-store")
        .header("X-Content-Type-Options", "nosniff")
        .header("Referrer-Policy", "no-referrer")
}

fn range_not_satisfiable(total: u64) -> Response<Vec<u8>> {
    finish(
        base_response(StatusCode::RANGE_NOT_SATISFIABLE)
            .header(CONTENT_RANGE, format!("bytes */{total}"))
            .header(CONTENT_LENGTH, "0"),
        Vec::new(),
    )
}

fn empty(status: StatusCode) -> Response<Vec<u8>> {
    finish(
        base_response(status).header(CONTENT_LENGTH, "0"),
        Vec::new(),
    )
}

fn finish(builder: Builder, body: Vec<u8>) -> Response<Vec<u8>> {
    builder.body(body).unwrap_or_else(|_| {
        let mut response = Response::new(Vec::new());
        *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
        response
    })
}

const fn status_for_shell_error(code: ShellErrorCode) -> StatusCode {
    match code {
        ShellErrorCode::InvalidInput => StatusCode::BAD_REQUEST,
        ShellErrorCode::UnsupportedContent | ShellErrorCode::UnsafeArchive => {
            StatusCode::UNSUPPORTED_MEDIA_TYPE
        }
        ShellErrorCode::ResourceLimitExceeded => StatusCode::PAYLOAD_TOO_LARGE,
        ShellErrorCode::NotFound => StatusCode::NOT_FOUND,
        ShellErrorCode::PermissionDenied => StatusCode::FORBIDDEN,
        ShellErrorCode::StorageUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        ShellErrorCode::StorageCorrupted
        | ShellErrorCode::ProviderAuthFailed
        | ShellErrorCode::ProviderRateLimited
        | ShellErrorCode::ProviderUnavailable
        | ShellErrorCode::NetworkUnavailable
        | ShellErrorCode::Cancelled
        | ShellErrorCode::Internal => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::*;

    fn respond(
        backend: &impl AssetProtocolBackend,
        request: Request<Vec<u8>>,
    ) -> Response<Vec<u8>> {
        let admission = AssetProtocolAdmission::default();
        let mut permit = admission.try_acquire().expect("request slot");
        handle_with_backend(backend, request, &mut permit)
    }

    mod admission_integration;
    mod png_rendering;

    struct CountingBackend {
        descriptor: AssetDeliveryDto,
        bytes: Vec<u8>,
        resolve_calls: Cell<usize>,
        read_calls: Cell<usize>,
        last_read_start: Cell<Option<u64>>,
        last_read_length: Cell<Option<u64>>,
    }

    impl CountingBackend {
        fn new() -> Self {
            let sha256 = "ab".repeat(32);
            Self {
                descriptor: AssetDeliveryDto {
                    asset_id: "asset".to_owned(),
                    sha256: sha256.clone(),
                    media_type: "image/webp".to_owned(),
                    kind: AssetDeliveryKindDto::Image,
                    size_bytes: 8,
                    width: Some(1),
                    height: Some(1),
                    duration_ms: None,
                    url: format!("lorepia-asset://sha256/{sha256}"),
                },
                bytes: (0_u8..8).collect(),
                resolve_calls: Cell::new(0),
                read_calls: Cell::new(0),
                last_read_start: Cell::new(None),
                last_read_length: Cell::new(None),
            }
        }
    }

    impl AssetProtocolBackend for CountingBackend {
        fn resolve_descriptor(&self, _sha256: &str) -> Result<AssetDeliveryDto, ShellErrorCode> {
            self.resolve_calls.set(self.resolve_calls.get() + 1);
            Ok(self.descriptor.clone())
        }

        fn read_verified_range(
            &self,
            _sha256: &str,
            start: u64,
            requested_bytes: u64,
        ) -> Result<lorepia_shell_api::AssetProtocolRange, ShellErrorCode> {
            self.read_calls.set(self.read_calls.get() + 1);
            self.last_read_start.set(Some(start));
            self.last_read_length.set(Some(requested_bytes));
            let start = usize::try_from(start).map_err(|_| ShellErrorCode::Internal)?;
            let length = usize::try_from(requested_bytes).map_err(|_| ShellErrorCode::Internal)?;
            let end = start.checked_add(length).ok_or(ShellErrorCode::Internal)?;
            let bytes = self.bytes.get(start..end).ok_or(ShellErrorCode::Internal)?;
            Ok(lorepia_shell_api::AssetProtocolRange {
                descriptor: self.descriptor.clone(),
                start: u64::try_from(start).map_err(|_| ShellErrorCode::Internal)?,
                bytes: bytes.to_vec(),
            })
        }
    }

    #[test]
    fn protocol_uri_accepts_only_opaque_canonical_digest_forms() {
        let digest = "ab".repeat(32);
        for accepted in [
            format!("lorepia-asset://sha256/{digest}"),
            format!("lorepia-asset://localhost/sha256/{digest}"),
            format!("http://lorepia-asset.localhost/sha256/{digest}"),
            format!("https://lorepia-asset.localhost/sha256/{digest}"),
        ] {
            let uri = accepted.parse::<Uri>().expect("URI");
            assert_eq!(digest_from_uri(&uri), Some(digest.as_str()), "{accepted}");
        }
        for rejected in [
            "lorepia-asset://sha256/../../private",
            "lorepia-asset://sha256/not-a-digest",
            "lorepia-asset://path/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "lorepia-asset://sha256/AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "lorepia-asset://sha256/aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa?download=1",
            "file:///Users/synthetic/private.png",
        ] {
            if let Ok(uri) = rejected.parse::<Uri>() {
                assert_eq!(digest_from_uri(&uri), None, "{rejected}");
            }
        }
    }

    #[test]
    fn range_parser_is_single_part_bounded_and_overflow_safe() {
        assert_eq!(
            parse_single_range("bytes=0-499", 1_000),
            Some(ByteRange {
                start: 0,
                length: 500
            })
        );
        assert_eq!(
            parse_single_range("bytes=500-", 1_000),
            Some(ByteRange {
                start: 500,
                length: 500
            })
        );
        assert_eq!(
            parse_single_range("bytes=-100", 1_000),
            Some(ByteRange {
                start: 900,
                length: 100
            })
        );
        assert_eq!(
            parse_single_range("bytes=0-", MAX_RANGE_BYTES + 10),
            Some(ByteRange {
                start: 0,
                length: MAX_RANGE_BYTES
            })
        );
        for invalid in [
            "bytes=1000-1001",
            "bytes=3-2",
            "bytes=-0",
            "bytes=0-1,4-5",
            "items=0-1",
            "bytes=18446744073709551615-",
        ] {
            assert_eq!(parse_single_range(invalid, 1_000), None, "{invalid}");
        }
    }

    #[test]
    fn base_response_is_nosniff_and_never_cacheable() {
        let response = empty(StatusCode::NOT_FOUND);
        assert_eq!(response.headers()["x-content-type-options"], "nosniff");
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        assert!(response.body().is_empty());
    }

    #[test]
    fn get_reads_only_the_requested_verified_range_while_head_reads_no_body() {
        let digest = "ab".repeat(32);
        let get_backend = CountingBackend::new();
        let get = Request::builder()
            .method(Method::GET)
            .uri(format!("lorepia-asset://sha256/{digest}"))
            .header("range", "bytes=2-4")
            .body(Vec::new())
            .expect("GET request");
        let response = respond(&get_backend, get);

        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[CONTENT_RANGE], "bytes 2-4/8");
        assert_eq!(response.headers()[CONTENT_TYPE], "image/webp");
        assert_eq!(response.body(), &[2, 3, 4]);
        assert_eq!(get_backend.resolve_calls.get(), 1);
        assert_eq!(get_backend.read_calls.get(), 1);
        assert_eq!(get_backend.last_read_start.get(), Some(2));
        assert_eq!(get_backend.last_read_length.get(), Some(3));

        let head_backend = CountingBackend::new();
        let head = Request::builder()
            .method(Method::HEAD)
            .uri(format!("lorepia-asset://sha256/{digest}"))
            .body(Vec::new())
            .expect("HEAD request");
        let response = respond(&head_backend, head);

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.body().is_empty());
        assert_eq!(head_backend.resolve_calls.get(), 1);
        assert_eq!(head_backend.read_calls.get(), 0);

        let ranged_head_backend = CountingBackend::new();
        let ranged_head = Request::builder()
            .method(Method::HEAD)
            .uri(format!("lorepia-asset://sha256/{digest}"))
            .header("range", "bytes=2-4")
            .body(Vec::new())
            .expect("ranged HEAD request");
        let response = respond(&ranged_head_backend, ranged_head);

        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[CONTENT_RANGE], "bytes 2-4/8");
        assert_eq!(response.headers()[CONTENT_LENGTH], "3");
        assert!(response.body().is_empty());
        assert_eq!(ranged_head_backend.resolve_calls.get(), 1);
        assert_eq!(ranged_head_backend.read_calls.get(), 0);
    }
}
