//! Versioned, single-item native credential envelopes.
//!
//! The authority marker is stored in the same opaque OS credential item as
//! the secret. This lets the Rust host distinguish an owned credential from a
//! stale value left behind after a database rollback without exposing either
//! value to the webview.

use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::{
    BoundCredentialObservation, CredentialAuthority, LegacyCredentialObservation, NativeCredential,
    PlatformError, PlatformErrorCode, PlatformResult,
    validation::{validate_credential_write, validate_reference, validate_sensitive_capture},
};

const ENVELOPE_MAGIC: &str = "lorepia-provider-credential\n";
const ENVELOPE_PREFIX: &str = "lorepia-provider-credential\nv1\n";
const PHYSICAL_REFERENCE_DOMAIN: &[u8] = b"dev.lorepia.provider-credential.physical-slot.v2\0";
const PHYSICAL_REFERENCE_PREFIX: &str = "lpc2-";
const PHYSICAL_REFERENCE_LENGTH: usize = PHYSICAL_REFERENCE_PREFIX.len() + 64;
pub const MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES: usize =
    16 * 1024 - (ENVELOPE_PREFIX.len() + 256 + 1 + 64 + 1);

/// Derives the authority-scoped native slot used only by bound credentials.
///
/// The logical reference is validated before hashing so bound APIs retain the
/// same caller contract as raw/legacy APIs. Fixed-width big-endian lengths
/// make every field boundary unambiguous, while the versioned domain keeps
/// this namespace independent from every other project digest.
pub(crate) fn physical_reference(
    logical_reference: &str,
    authority: &CredentialAuthority,
) -> PlatformResult<String> {
    validate_reference(logical_reference)?;

    let mut digest = Sha256::new();
    digest.update(PHYSICAL_REFERENCE_DOMAIN);
    update_length_prefixed(&mut digest, logical_reference.as_bytes());
    update_length_prefixed(&mut digest, authority.authority_id().as_bytes());
    update_length_prefixed(&mut digest, authority.binding_sha256().as_bytes());
    let reference = format!("{PHYSICAL_REFERENCE_PREFIX}{:x}", digest.finalize());
    debug_assert_eq!(reference.len(), PHYSICAL_REFERENCE_LENGTH);
    Ok(reference)
}

fn update_length_prefixed(digest: &mut Sha256, value: &[u8]) {
    digest.update((value.len() as u64).to_be_bytes());
    digest.update(value);
}

pub(crate) fn encode(
    authority: &CredentialAuthority,
    secret: NativeCredential,
) -> PlatformResult<NativeCredential> {
    let secret = Zeroizing::new(secret.into_secret_string());
    validate_sensitive_capture(secret.as_str(), MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES)?;
    let encoded = Zeroizing::new(format!(
        "{ENVELOPE_PREFIX}{}\n{}\n{}",
        authority.authority_id(),
        authority.binding_sha256(),
        secret.as_str()
    ));
    validate_credential_write(&encoded)?;
    Ok(NativeCredential::from_zeroizing(encoded))
}

pub(crate) fn observe(
    stored: NativeCredential,
    expected: &CredentialAuthority,
) -> BoundCredentialObservation {
    let stored = Zeroizing::new(stored.into_secret_string());
    match parse(stored.as_str()) {
        ParsedEnvelope::Legacy => BoundCredentialObservation::Legacy,
        ParsedEnvelope::Unreadable => BoundCredentialObservation::Unreadable,
        ParsedEnvelope::Envelope {
            authority_id,
            binding_sha256,
            ..
        } if authority_id == expected.authority_id()
            && binding_sha256 == expected.binding_sha256() =>
        {
            BoundCredentialObservation::Match
        }
        ParsedEnvelope::Envelope { .. } => BoundCredentialObservation::Mismatch,
    }
}

pub(crate) fn read(
    stored: NativeCredential,
    expected: &CredentialAuthority,
) -> PlatformResult<NativeCredential> {
    let stored = Zeroizing::new(stored.into_secret_string());
    let ParsedEnvelope::Envelope {
        authority_id,
        binding_sha256,
        secret,
    } = parse(stored.as_str())
    else {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    };
    if authority_id != expected.authority_id() || binding_sha256 != expected.binding_sha256() {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    validate_sensitive_capture(secret, MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES)
        .map_err(|_| PlatformError::new(PlatformErrorCode::CredentialRecoveryRequired))?;
    Ok(NativeCredential::new(secret.to_owned()))
}

pub(crate) fn observe_legacy(stored: NativeCredential) -> LegacyCredentialObservation {
    let stored = Zeroizing::new(stored.into_secret_string());
    match parse(stored.as_str()) {
        ParsedEnvelope::Legacy => LegacyCredentialObservation::Raw,
        ParsedEnvelope::Unreadable => LegacyCredentialObservation::Unreadable,
        ParsedEnvelope::Envelope { .. } => LegacyCredentialObservation::Bound,
    }
}

pub(crate) fn read_legacy(stored: NativeCredential) -> PlatformResult<NativeCredential> {
    if stored.expose().starts_with(ENVELOPE_MAGIC) {
        return Err(PlatformError::new(
            PlatformErrorCode::CredentialRecoveryRequired,
        ));
    }
    Ok(stored)
}

enum ParsedEnvelope<'a> {
    Legacy,
    Unreadable,
    Envelope {
        authority_id: &'a str,
        binding_sha256: &'a str,
        secret: &'a str,
    },
}

fn parse(value: &str) -> ParsedEnvelope<'_> {
    if !value.starts_with(ENVELOPE_MAGIC) {
        return ParsedEnvelope::Legacy;
    }
    let Some(rest) = value.strip_prefix(ENVELOPE_PREFIX) else {
        return ParsedEnvelope::Unreadable;
    };
    let mut parts = rest.splitn(3, '\n');
    let (Some(authority_id), Some(binding_sha256), Some(secret)) =
        (parts.next(), parts.next(), parts.next())
    else {
        return ParsedEnvelope::Unreadable;
    };
    if CredentialAuthority::new(authority_id.to_owned(), binding_sha256.to_owned()).is_err()
        || validate_sensitive_capture(secret, MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES).is_err()
    {
        return ParsedEnvelope::Unreadable;
    }
    ParsedEnvelope::Envelope {
        authority_id,
        binding_sha256,
        secret,
    }
}

#[cfg(test)]
mod tests {
    use sha2::{Digest, Sha256};

    use super::{
        MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES, PHYSICAL_REFERENCE_LENGTH,
        PHYSICAL_REFERENCE_PREFIX, encode, observe, observe_legacy, physical_reference, read,
        read_legacy, update_length_prefixed,
    };
    use crate::{
        BoundCredentialObservation, CredentialAuthority, LegacyCredentialObservation,
        NativeCredential,
    };

    const RECOVERY_COMPATIBILITY_VECTORS: &str =
        include_str!("../../../testdata/tauri-upgrade/recovery-compatibility-v1-vectors.json");

    fn authority(id: &str, byte: u8) -> CredentialAuthority {
        CredentialAuthority::new(id.to_owned(), format!("{byte:02x}").repeat(32))
            .expect("authority")
    }

    #[test]
    fn recovery_compatibility_v1_known_vector() {
        let vectors: serde_json::Value = serde_json::from_str(RECOVERY_COMPATIBILITY_VECTORS)
            .expect("recovery compatibility vectors must be JSON");
        let vector = &vectors["bound_credential"];
        let authority = CredentialAuthority::new(
            vector["authority_id"]
                .as_str()
                .expect("authority ID vector")
                .to_owned(),
            vector["binding_sha256"]
                .as_str()
                .expect("binding digest vector")
                .to_owned(),
        )
        .expect("known-vector authority");
        let reference = physical_reference(
            vector["logical_reference"]
                .as_str()
                .expect("logical reference vector"),
            &authority,
        )
        .expect("derive known-vector physical reference");
        assert_eq!(
            reference,
            vector["physical_reference"]
                .as_str()
                .expect("physical reference vector")
        );

        let encoded = encode(
            &authority,
            NativeCredential::new(
                vector["synthetic_secret"]
                    .as_str()
                    .expect("synthetic secret vector")
                    .to_owned(),
            ),
        )
        .expect("encode known-vector credential");
        assert_eq!(
            encoded.expose(),
            vector["encoded_envelope"]
                .as_str()
                .expect("encoded envelope vector")
        );
    }

    #[test]
    fn physical_reference_is_fixed_length_lowercase_and_domain_separated() {
        let authority = authority("install-a", 0xab);
        let reference = physical_reference("connection-a", &authority).expect("reference");
        assert_eq!(reference.len(), PHYSICAL_REFERENCE_LENGTH);
        assert!(reference.starts_with(PHYSICAL_REFERENCE_PREFIX));
        assert!(
            reference[PHYSICAL_REFERENCE_PREFIX.len()..]
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_eq!(
            reference,
            "lpc2-5c3607fcc99a0026c030c0ed2507c5535f509f5e16fa8db97cf02b08aca5447b"
        );

        let mut without_domain = Sha256::new();
        update_length_prefixed(&mut without_domain, b"connection-a");
        update_length_prefixed(&mut without_domain, b"install-a");
        update_length_prefixed(
            &mut without_domain,
            format!("{:02x}", 0xab).repeat(32).as_bytes(),
        );
        assert_ne!(
            reference,
            format!("{PHYSICAL_REFERENCE_PREFIX}{:x}", without_domain.finalize())
        );
    }

    #[test]
    fn physical_reference_preserves_every_logical_and_authority_boundary() {
        let binding = format!("{:02x}", 0xcd).repeat(32);
        let left = CredentialAuthority::new("bc".to_owned(), binding.clone()).expect("authority");
        let right = CredentialAuthority::new("c".to_owned(), binding).expect("authority");
        let first = physical_reference("a", &left).expect("first");
        let shifted = physical_reference("ab", &right).expect("shifted");
        let changed_authority =
            physical_reference("a", &authority("bd", 0xcd)).expect("changed authority");
        let changed_binding =
            physical_reference("a", &authority("bc", 0xce)).expect("changed binding");

        assert_ne!(first, shifted, "length prefixes must prevent ambiguity");
        assert_ne!(first, changed_authority);
        assert_ne!(first, changed_binding);
        assert_ne!(first, "a", "native slot must not be the logical slot");

        let logical_canary = "logical-provider/reference-canary";
        let authority_canary = CredentialAuthority::new(
            "authority-id-canary".to_owned(),
            format!("{:02x}", 0xcd).repeat(32),
        )
        .expect("canary authority");
        let opaque =
            physical_reference(logical_canary, &authority_canary).expect("opaque reference");
        let debug = format!("{opaque:?}");
        assert!(!opaque.contains(logical_canary));
        assert!(!opaque.contains(authority_canary.authority_id()));
        assert!(!opaque.contains(authority_canary.binding_sha256()));
        assert!(!debug.contains(logical_canary));
        assert!(!debug.contains(authority_canary.authority_id()));
        assert!(!debug.contains(authority_canary.binding_sha256()));
    }

    #[test]
    fn physical_reference_rejects_invalid_logical_references() {
        let authority = authority("install-a", 0xab);
        assert!(physical_reference("", &authority).is_err());
        assert!(physical_reference("   ", &authority).is_err());
        assert!(physical_reference(&"r".repeat(257), &authority).is_err());
    }

    #[test]
    fn envelope_round_trips_without_debugging_secret_or_digest() {
        let secret = "sk-bound-envelope-canary";
        let secret_sha256 = format!("{:x}", Sha256::digest(secret.as_bytes()));
        let authority = authority("install-a", 0xab);
        let encoded = encode(&authority, NativeCredential::new(secret.to_owned())).expect("encode");
        let debug = format!("{encoded:?}");
        assert!(!debug.contains(secret));
        assert!(!debug.contains(&secret_sha256));
        assert_eq!(
            observe(encoded, &authority),
            BoundCredentialObservation::Match
        );

        let encoded = encode(&authority, NativeCredential::new(secret.to_owned())).expect("encode");
        assert_eq!(
            read(encoded, &authority)
                .expect("read")
                .into_secret_string(),
            secret
        );
    }

    #[test]
    fn stale_marker_and_legacy_raw_slot_never_match() {
        let expected = authority("install-a", 0xaa);
        let newer = authority("install-b", 0xbb);
        let newer_value = encode(
            &newer,
            NativeCredential::new("synthetic-secret-b".to_owned()),
        )
        .expect("encode");
        assert_eq!(
            observe(newer_value, &expected),
            BoundCredentialObservation::Mismatch
        );
        assert_eq!(
            observe(
                NativeCredential::new("legacy-raw-secret".to_owned()),
                &expected
            ),
            BoundCredentialObservation::Legacy
        );
        assert_eq!(
            observe(
                NativeCredential::new("lorepia-provider-credential\nv2\ninstall-a\n".to_owned()),
                &expected
            ),
            BoundCredentialObservation::Unreadable
        );
    }

    #[test]
    fn raw_remainder_preserves_newlines_nul_unicode_and_enforces_bound() {
        let authority = authority("install-lines", 0xcd);
        let secret = "첫 줄\nsecond\0tail";
        let encoded = encode(&authority, NativeCredential::new(secret.to_owned())).expect("encode");
        assert_eq!(
            read(encoded, &authority)
                .expect("read")
                .into_secret_string(),
            secret
        );
        assert!(
            encode(
                &authority,
                NativeCredential::new("s".repeat(MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES))
            )
            .is_ok()
        );
        assert!(
            encode(
                &authority,
                NativeCredential::new("s".repeat(MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES + 1))
            )
            .is_err()
        );
    }

    #[test]
    fn legacy_reader_never_releases_a_bound_or_malformed_envelope() {
        let authority = authority("owned-install", 0xef);
        let bound = encode(
            &authority,
            NativeCredential::new("must-not-escape".to_owned()),
        )
        .expect("encode bound item");
        assert_eq!(observe_legacy(bound), LegacyCredentialObservation::Bound);
        let bound = encode(
            &authority,
            NativeCredential::new("must-not-escape".to_owned()),
        )
        .expect("encode bound item");
        assert!(read_legacy(bound).is_err());
        assert_eq!(
            observe_legacy(NativeCredential::new(
                "lorepia-provider-credential\nv2\nfuture".to_owned()
            )),
            LegacyCredentialObservation::Unreadable
        );
        assert!(
            read_legacy(NativeCredential::new(
                "lorepia-provider-credential\nv2\nfuture".to_owned()
            ))
            .is_err()
        );
        let raw = NativeCredential::new("legacy-raw-secret".to_owned());
        assert_eq!(observe_legacy(raw), LegacyCredentialObservation::Raw);
        assert_eq!(
            read_legacy(NativeCredential::new("legacy-raw-secret".to_owned()))
                .expect("read legacy raw")
                .into_secret_string(),
            "legacy-raw-secret"
        );
    }
}
