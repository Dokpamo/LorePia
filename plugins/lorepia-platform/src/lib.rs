//! Native platform services for the Tauri shell.
//!
//! No command from this plugin is registered with the webview. The high-level
//! app backend calls these services and returns only safe projections.

mod credential_envelope;
mod error;
mod model;
mod staging;
mod validation;

pub use credential_envelope::MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES;
/// Maximum size of a legacy raw credential accepted by the native vault.
///
/// Bound credentials use a smaller limit so their authority envelope still
/// fits inside the same native item. Legacy raw slots retain the historical
/// full 16 KiB contract.
pub const MAXIMUM_LEGACY_CREDENTIAL_BYTES: usize = validation::MAXIMUM_CREDENTIAL_WRITE_BYTES;

#[cfg(desktop)]
mod desktop;
#[cfg(mobile)]
mod mobile;

use std::path::Path;
use uuid::Uuid;
use zeroize::{Zeroize, Zeroizing};

pub use error::{PlatformError, PlatformErrorCode, PlatformResult};
pub use model::{
    BoundCredentialObservation, ClipboardCleanupStatus, CredentialAuthority, CredentialStatus,
    LegacyCredentialObservation, NativeCaptureStatus, NativeCredential, NativeCredentialEffect,
    NativeCredentialEffectConfirmation, NativeCredentialEffectContext, NativeSavedContentSource,
    NativeSensitiveText, PreparedBoundCredentialStore, StagedImport,
};
use tauri::{Manager, Runtime, plugin::TauriPlugin};

pub struct LorepiaPlatform<R: Runtime> {
    #[cfg(desktop)]
    inner: desktop::DesktopPlatform<R>,
    #[cfg(mobile)]
    inner: mobile::MobilePlatform<R>,
    legacy_confirmation_process_key: Zeroizing<[u8; 32]>,
}

fn new_legacy_confirmation_process_key() -> Zeroizing<[u8; 32]> {
    let first = Uuid::new_v4();
    let second = Uuid::new_v4();
    let mut key = Zeroizing::new([0_u8; 32]);
    key[..16].copy_from_slice(first.as_bytes());
    key[16..].copy_from_slice(second.as_bytes());
    key
}

fn prepare_bound_credential_store_with(
    reference: &str,
    value: NativeCredential,
    authority: &CredentialAuthority,
    validate_native_store: impl FnOnce(&PreparedBoundCredentialStore) -> PlatformResult<()>,
) -> PlatformResult<PreparedBoundCredentialStore> {
    let physical_reference = credential_envelope::physical_reference(reference, authority)?;
    let encoded_credential = credential_envelope::encode(authority, value)?;
    let prepared = PreparedBoundCredentialStore::new(physical_reference, encoded_credential);
    validate_native_store(&prepared)?;
    Ok(prepared)
}

impl<R: Runtime> LorepiaPlatform<R> {
    pub fn data_root(&self) -> &Path {
        self.inner.data_root()
    }

    pub async fn pick_import(&self) -> PlatformResult<Option<StagedImport>> {
        self.inner.pick_import().await
    }

    /// Save one Rust-owned, verified CAS content source through a scoped native
    /// picker without exposing its path or bytes to the webview.
    ///
    /// The source must be exactly `data_root/sources/sha256/<2>/<62>` for the
    /// supplied lowercase digest. Picker cancellation is `Ok(None)` and is not
    /// reported as a completed export.
    pub async fn save_content_source(
        &self,
        source_path: &Path,
        suggested_name: &str,
        expected_size_bytes: u64,
        expected_sha256: &str,
    ) -> PlatformResult<Option<NativeSavedContentSource>> {
        self.inner
            .save_content_source(
                source_path,
                suggested_name,
                expected_size_bytes,
                expected_sha256,
            )
            .await
    }

    /// Pick a file and consume a bounded app-owned copy entirely in Rust.
    ///
    /// This is intended for sensitive signed inputs which must never expose a
    /// host path or raw bytes to the webview.
    pub async fn pick_bounded_file(&self, maximum_bytes: u64) -> PlatformResult<Option<Vec<u8>>> {
        let Some(staged) = self.pick_import().await? else {
            return Ok(None);
        };
        let mut bytes = staging::read_staged_file(&staged, maximum_bytes)?;
        if self.discard_staged_import(&staged).await.is_err() {
            bytes.zeroize();
            return Err(PlatformError::new(PlatformErrorCode::StorageUnavailable));
        }
        Ok(Some(bytes))
    }

    pub fn discard_staged_import<'a>(
        &'a self,
        staged: &'a StagedImport,
    ) -> impl std::future::Future<Output = PlatformResult<()>> + 'a {
        #[cfg(desktop)]
        {
            std::future::ready(self.inner.discard_staged_import(staged))
        }
        #[cfg(mobile)]
        {
            self.inner.discard_staged_import(staged)
        }
    }

    pub fn credential_status<'a>(
        &'a self,
        reference: &'a str,
    ) -> impl std::future::Future<Output = PlatformResult<CredentialStatus>> + 'a {
        #[cfg(desktop)]
        {
            std::future::ready(self.inner.credential_status(reference))
        }
        #[cfg(mobile)]
        {
            self.inner.credential_status(reference)
        }
    }

    /// Presents one OS-owned modal and returns a non-reusable Rust-only proof
    /// only when the foreground user accepts the exact effect context.
    pub async fn confirm_credential_effect(
        &self,
        context: NativeCredentialEffectContext,
    ) -> PlatformResult<NativeCredentialEffectConfirmation> {
        self.inner.confirm_credential_effect(&context).await?;
        Ok(NativeCredentialEffectConfirmation::new(context))
    }

    pub fn read_credential<'a>(
        &'a self,
        reference: &'a str,
    ) -> impl std::future::Future<Output = PlatformResult<Option<NativeCredential>>> + 'a {
        #[cfg(desktop)]
        {
            std::future::ready(self.inner.read_credential(reference))
        }
        #[cfg(mobile)]
        {
            self.inner.read_credential(reference)
        }
    }

    pub fn store_credential<'a>(
        &'a self,
        reference: &'a str,
        value: NativeCredential,
    ) -> impl std::future::Future<Output = PlatformResult<()>> + 'a {
        #[cfg(desktop)]
        {
            std::future::ready(self.inner.store_credential(reference, value))
        }
        #[cfg(mobile)]
        {
            self.inner.store_credential(reference, value)
        }
    }

    pub fn delete_credential<'a>(
        &'a self,
        reference: &'a str,
    ) -> impl std::future::Future<Output = PlatformResult<()>> + 'a {
        #[cfg(desktop)]
        {
            std::future::ready(self.inner.delete_credential(reference))
        }
        #[cfg(mobile)]
        {
            self.inner.delete_credential(reference)
        }
    }

    /// Fully prepares one authority-bound native write without mutating the
    /// native vault.
    ///
    /// Logical-reference validation, exact physical-reference derivation,
    /// envelope encoding, and platform write validation all finish before the
    /// opaque value is returned. Durable workflows must call this while their
    /// operation is still prepared, persist the started cutpoint, and then
    /// pass the result directly to [`Self::store_prepared_bound_credential`].
    pub fn prepare_bound_credential_store(
        &self,
        reference: &str,
        value: NativeCredential,
        authority: &CredentialAuthority,
    ) -> PlatformResult<PreparedBoundCredentialStore> {
        prepare_bound_credential_store_with(reference, value, authority, |prepared| {
            self.inner.validate_credential_store(
                prepared.physical_reference(),
                prepared.encoded_credential(),
            )
        })
    }

    /// Performs exactly one already-prepared native credential store.
    ///
    /// This consumes the opaque prepared value. All deterministic validation,
    /// physical-key derivation, and envelope encoding have already completed;
    /// the remaining fallible work is native platform publication and the
    /// platform credential-store operation itself.
    pub fn store_prepared_bound_credential(
        &self,
        prepared: PreparedBoundCredentialStore,
    ) -> impl std::future::Future<Output = PlatformResult<()>> + '_ {
        #[cfg(desktop)]
        {
            let (physical_reference, encoded_credential) = prepared.into_parts();
            std::future::ready(
                self.inner
                    .store_prevalidated_bound_credential(&physical_reference, encoded_credential),
            )
        }
        #[cfg(mobile)]
        {
            async move {
                let (physical_reference, encoded_credential) = prepared.into_parts();
                self.inner
                    .store_prevalidated_bound_credential(&physical_reference, encoded_credential)
                    .await
            }
        }
    }

    /// Returns the raw native status of the authority-scoped bound slot.
    pub async fn bound_credential_status(
        &self,
        reference: &str,
        authority: &CredentialAuthority,
    ) -> PlatformResult<CredentialStatus> {
        let physical_reference = credential_envelope::physical_reference(reference, authority)?;
        #[cfg(desktop)]
        return std::future::ready(self.inner.bound_credential_status(&physical_reference)).await;
        #[cfg(mobile)]
        self.inner
            .bound_credential_status(&physical_reference)
            .await
    }

    /// Observes whether an OS item carries the exact expected authority.
    pub async fn observe_bound_credential(
        &self,
        reference: &str,
        authority: &CredentialAuthority,
    ) -> PlatformResult<BoundCredentialObservation> {
        let physical_reference = credential_envelope::physical_reference(reference, authority)?;
        #[cfg(desktop)]
        let credential =
            std::future::ready(self.inner.read_bound_credential(&physical_reference)).await?;
        #[cfg(mobile)]
        let credential = self
            .inner
            .read_bound_credential(&physical_reference)
            .await?;
        match credential {
            None => Ok(BoundCredentialObservation::Missing),
            Some(value) => Ok(credential_envelope::observe(value, authority)),
        }
    }

    /// Reads a credential only when its atomic marker exactly matches Core.
    pub async fn read_bound_credential(
        &self,
        reference: &str,
        authority: &CredentialAuthority,
    ) -> PlatformResult<Option<NativeCredential>> {
        let physical_reference = credential_envelope::physical_reference(reference, authority)?;
        #[cfg(desktop)]
        let credential =
            std::future::ready(self.inner.read_bound_credential(&physical_reference)).await?;
        #[cfg(mobile)]
        let credential = self
            .inner
            .read_bound_credential(&physical_reference)
            .await?;
        credential
            .map(|value| credential_envelope::read(value, authority))
            .transpose()
    }

    /// Deletes only the authority-scoped bound slot, never the logical raw or
    /// legacy slot with the same reference.
    pub async fn delete_bound_credential(
        &self,
        reference: &str,
        authority: &CredentialAuthority,
    ) -> PlatformResult<()> {
        let physical_reference = credential_envelope::physical_reference(reference, authority)?;
        #[cfg(desktop)]
        return std::future::ready(self.inner.delete_bound_credential(&physical_reference)).await;
        #[cfg(mobile)]
        self.inner
            .delete_bound_credential(&physical_reference)
            .await
    }

    /// Classifies a legacy slot without returning its contents. Atomic bound
    /// envelopes and malformed/future envelope versions never look raw.
    pub async fn observe_legacy_credential(
        &self,
        reference: &str,
    ) -> PlatformResult<LegacyCredentialObservation> {
        match self.read_credential(reference).await? {
            None => Ok(LegacyCredentialObservation::Missing),
            Some(value) => Ok(credential_envelope::observe_legacy(value)),
        }
    }

    /// Binds one native confirmation to the exact current legacy slot without
    /// returning credential contents or a renderer-reusable secret hash.
    pub async fn legacy_credential_confirmation_revision(
        &self,
        reference: &str,
    ) -> PlatformResult<String> {
        credential_envelope::legacy_confirmation_revision(
            reference,
            self.read_credential(reference).await?,
            &self.legacy_confirmation_process_key,
        )
    }

    /// Reads one legacy raw credential while rejecting every envelope-shaped
    /// item before any bytes can reach a provider request.
    pub async fn read_legacy_credential(
        &self,
        reference: &str,
    ) -> PlatformResult<Option<NativeCredential>> {
        self.read_credential(reference)
            .await?
            .map(credential_envelope::read_legacy)
            .transpose()
    }

    /// Capture the foreground clipboard once and store it in the native
    /// credential vault. No credential value is returned to the caller.
    pub async fn capture_credential_from_clipboard(
        &self,
        reference: &str,
    ) -> PlatformResult<NativeCaptureStatus> {
        self.inner
            .capture_credential_from_clipboard(reference)
            .await
    }

    /// Capture one credential-sized clipboard value for a Rust-owned durable
    /// installation workflow. The value is not written to a vault yet and
    /// cannot be serialized or debug-formatted.
    pub async fn capture_credential_text_from_clipboard(
        &self,
    ) -> PlatformResult<NativeSensitiveText> {
        self.inner
            .capture_sensitive_text_from_clipboard(MAXIMUM_BOUND_CREDENTIAL_SECRET_BYTES)
            .await
    }

    /// Capture one legacy raw credential without applying bound-envelope
    /// overhead. This API is intentionally separate from bound installation.
    pub async fn capture_legacy_credential_text_from_clipboard(
        &self,
    ) -> PlatformResult<NativeSensitiveText> {
        self.inner
            .capture_sensitive_text_from_clipboard(MAXIMUM_LEGACY_CREDENTIAL_BYTES)
            .await
    }

    /// Capture foreground clipboard text for immediate Rust-only consumption.
    ///
    /// The returned value is non-serializable, zeroized on drop, and bounded by
    /// both the caller limit and the plugin-wide hard maximum.
    pub async fn capture_sensitive_text_from_clipboard(
        &self,
        maximum_bytes: usize,
    ) -> PlatformResult<NativeSensitiveText> {
        self.inner
            .capture_sensitive_text_from_clipboard(maximum_bytes)
            .await
    }
}

pub trait LorepiaPlatformExt<R: Runtime> {
    fn lorepia_platform(&self) -> &LorepiaPlatform<R>;
}

impl<R: Runtime, T: Manager<R>> LorepiaPlatformExt<R> for T {
    fn lorepia_platform(&self) -> &LorepiaPlatform<R> {
        self.state::<LorepiaPlatform<R>>().inner()
    }
}

pub fn init<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::<R>::new("lorepia-platform")
        .setup(|app, api| {
            #[cfg(not(mobile))]
            let _ = &api;
            #[cfg(target_os = "android")]
            let inner = mobile::MobilePlatform::new(
                api.register_android_plugin("dev.lorepia.tauri.platform", "LorepiaPlatformPlugin")?,
            )
            .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;

            #[cfg(target_os = "ios")]
            let inner =
                mobile::MobilePlatform::new(api.register_ios_plugin(init_plugin_lorepia_platform)?)
                    .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;

            #[cfg(desktop)]
            let inner = desktop::DesktopPlatform::new(app.app_handle().clone())
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error>)?;

            app.manage(LorepiaPlatform {
                inner,
                legacy_confirmation_process_key: new_legacy_confirmation_process_key(),
            });
            Ok(())
        })
        .build()
}

#[cfg(target_os = "ios")]
tauri::ios_plugin_binding!(init_plugin_lorepia_platform);

#[cfg(test)]
mod tests {
    use super::{
        BoundCredentialObservation, ClipboardCleanupStatus, CredentialAuthority,
        NativeCaptureStatus, NativeCredential, NativeCredentialEffect,
        NativeCredentialEffectConfirmation, NativeCredentialEffectContext,
        NativeSavedContentSource, NativeSensitiveText, PlatformError, PlatformErrorCode,
        PreparedBoundCredentialStore, StagedImport, credential_envelope,
        prepare_bound_credential_store_with,
    };
    use std::cell::Cell;
    use std::marker::PhantomData;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    trait AmbiguousIfSerialize<Marker> {
        fn marker() {}
    }

    trait AmbiguousIfDebug<Marker> {
        fn marker() {}
    }

    trait AmbiguousIfClone<Marker> {
        fn marker() {}
    }

    struct SerializeCheck<T: ?Sized>(PhantomData<T>);
    struct DebugCheck<T: ?Sized>(PhantomData<T>);
    struct CloneCheck<T: ?Sized>(PhantomData<T>);

    impl<T: ?Sized> AmbiguousIfSerialize<()> for SerializeCheck<T> {}
    impl<T: ?Sized + serde::Serialize> AmbiguousIfSerialize<u8> for SerializeCheck<T> {}
    impl<T: ?Sized> AmbiguousIfDebug<()> for DebugCheck<T> {}
    impl<T: ?Sized + std::fmt::Debug> AmbiguousIfDebug<u8> for DebugCheck<T> {}
    impl<T: ?Sized> AmbiguousIfClone<()> for CloneCheck<T> {}
    impl<T: Clone> AmbiguousIfClone<u8> for CloneCheck<T> {}

    #[test]
    fn sensitive_values_are_not_projected_through_safe_views() {
        let secret = "sk-platform-canary";
        let path = "/Users/synthetic/private/card.json";
        let display_name = "private-card.json";
        assert!(!format!("{:?}", NativeCredential::new(secret.to_owned())).contains(secret));
        let staged = format!(
            "{:?}",
            StagedImport::new(PathBuf::from(path), display_name.to_owned(), 4)
        );
        assert!(!staged.contains(path));
        assert!(!staged.contains(display_name));
        assert!(staged.contains("[REDACTED]"));

        let captured = NativeSensitiveText::new(
            secret.to_owned(),
            NativeCaptureStatus {
                clipboard_cleanup: ClipboardCleanupStatus::Cleared,
            },
        );
        assert_eq!(
            captured.status().clipboard_cleanup,
            ClipboardCleanupStatus::Cleared
        );
        assert_eq!(captured.into_secret_string(), secret);
    }

    #[test]
    fn native_sensitive_text_has_no_serialize_or_debug_surface() {
        let _ = <SerializeCheck<NativeSensitiveText> as AmbiguousIfSerialize<_>>::marker;
        let _ = <DebugCheck<NativeSensitiveText> as AmbiguousIfDebug<_>>::marker;
        let _ = <SerializeCheck<PreparedBoundCredentialStore> as AmbiguousIfSerialize<_>>::marker;
        let _ = <DebugCheck<PreparedBoundCredentialStore> as AmbiguousIfDebug<_>>::marker;
        let _ = <CloneCheck<PreparedBoundCredentialStore> as AmbiguousIfClone<_>>::marker;
        let _ =
            <SerializeCheck<NativeCredentialEffectConfirmation> as AmbiguousIfSerialize<_>>::marker;
        let _ = <DebugCheck<NativeCredentialEffectConfirmation> as AmbiguousIfDebug<_>>::marker;
        let _ = <CloneCheck<NativeCredentialEffectConfirmation> as AmbiguousIfClone<_>>::marker;
    }

    #[test]
    fn macos_and_windows_confirmation_policy_rejects_prompt_spoofing_controls() {
        let valid = NativeCredentialEffectContext::new(
            NativeCredentialEffect::Delete,
            "connection-a".to_owned(),
            "https://api.example.test".to_owned(),
            "revision-7".to_owned(),
        )
        .expect("bounded exact context");
        assert_eq!(valid.target_id(), "connection-a");

        for invalid in [
            "",
            "   ",
            "connection\nApprove",
            "connection\u{0000}Approve",
            "connection\u{2028}Approve",
            "connection\u{2029}Approve",
            "connection\u{202e}Approve",
            "connection\u{2066}Approve",
            "connection\u{2069}Approve",
            "connection\u{200b}Approve",
            "connection\u{200d}Approve",
            "connection\u{2060}Approve",
            "connection\u{feff}Approve",
            "connection\u{00ad}Approve",
            "connection\u{034f}Approve",
            "connection\u{e0001}Approve",
        ] {
            assert!(
                NativeCredentialEffectContext::new(
                    NativeCredentialEffect::Delete,
                    invalid.to_owned(),
                    "https://api.example.test".to_owned(),
                    "revision-7".to_owned(),
                )
                .is_err()
            );
        }

        NativeCredentialEffectContext::new(
            NativeCredentialEffect::Delete,
            "연결 a".to_owned(),
            "https://예시.test".to_owned(),
            "revision-8".to_owned(),
        )
        .expect("visible Unicode and ASCII spaces remain valid");
    }

    #[test]
    fn stale_credential_confirmation_cannot_reach_native_mutation() {
        let confirmation = NativeCredentialEffectConfirmation::new(
            NativeCredentialEffectContext::new(
                NativeCredentialEffect::Delete,
                "connection-a".to_owned(),
                "https://api.example.test".to_owned(),
                "revision-7".to_owned(),
            )
            .expect("prompt context"),
        );
        let native_mutations = Cell::new(0_u32);

        let result = confirmation.consume_exact(
            NativeCredentialEffect::Delete,
            "connection-a",
            "https://changed.example.test",
            "revision-8",
        );
        if result.is_ok() {
            native_mutations.set(native_mutations.get() + 1);
        }

        assert!(result.is_err());
        assert_eq!(native_mutations.get(), 0);
    }

    #[test]
    fn stale_credential_authority_confirmation_cannot_reach_native_mutation() {
        let confirmation = NativeCredentialEffectConfirmation::new(
            NativeCredentialEffectContext::new(
                NativeCredentialEffect::Delete,
                "connection-a".to_owned(),
                "https://api.example.test".to_owned(),
                "row=2026-08-16T00:00:00Z;authority=authority-a:aaaaaaaa".to_owned(),
            )
            .expect("authority-a prompt context"),
        );
        let native_mutations = Cell::new(0_u32);

        let result = confirmation.consume_exact(
            NativeCredentialEffect::Delete,
            "connection-a",
            "https://api.example.test",
            "row=2026-08-16T00:00:00Z;authority=authority-b:bbbbbbbb",
        );
        if result.is_ok() {
            native_mutations.set(native_mutations.get() + 1);
        }

        assert!(result.is_err());
        assert_eq!(native_mutations.get(), 0);
    }

    #[test]
    fn expired_credential_confirmation_cannot_reach_native_mutation() {
        let confirmation = NativeCredentialEffectConfirmation::new_with_deadline_for_test(
            NativeCredentialEffectContext::new(
                NativeCredentialEffect::Delete,
                "connection-a".to_owned(),
                "https://api.example.test".to_owned(),
                "revision-7".to_owned(),
            )
            .expect("prompt context"),
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .expect("past monotonic instant"),
        );
        let native_mutations = Cell::new(0_u32);

        let result = confirmation.consume_exact(
            NativeCredentialEffect::Delete,
            "connection-a",
            "https://api.example.test",
            "revision-7",
        );
        if result.is_ok() {
            native_mutations.set(native_mutations.get() + 1);
        }

        assert_eq!(
            result.expect_err("expired confirmation must fail").code(),
            PlatformErrorCode::PermissionDenied
        );
        assert_eq!(native_mutations.get(), 0);
    }

    #[test]
    fn legacy_raw_replacement_invalidates_confirmation_before_native_mutation() {
        let process_key = [0x5a; 32];
        let revision_a = credential_envelope::legacy_confirmation_revision(
            "legacy-profile-a",
            Some(NativeCredential::new("legacy-secret-a".to_owned())),
            &process_key,
        )
        .expect("raw A revision");
        let revision_a_reobserved = credential_envelope::legacy_confirmation_revision(
            "legacy-profile-a",
            Some(NativeCredential::new("legacy-secret-a".to_owned())),
            &process_key,
        )
        .expect("stable raw A revision");
        let revision_b = credential_envelope::legacy_confirmation_revision(
            "legacy-profile-a",
            Some(NativeCredential::new("legacy-secret-b".to_owned())),
            &process_key,
        )
        .expect("raw B revision");

        assert_eq!(revision_a, revision_a_reobserved);
        assert_ne!(revision_a, revision_b);
        assert!(!revision_a.contains("legacy-secret-a"));
        assert!(!revision_b.contains("legacy-secret-b"));

        let confirmation = NativeCredentialEffectConfirmation::new(
            NativeCredentialEffectContext::new(
                NativeCredentialEffect::Delete,
                "legacy-profile-a".to_owned(),
                "https://api.example.test".to_owned(),
                revision_a,
            )
            .expect("raw A prompt context"),
        );
        let native_mutations = Cell::new(0_u32);
        let result = confirmation.consume_exact(
            NativeCredentialEffect::Delete,
            "legacy-profile-a",
            "https://api.example.test",
            &revision_b,
        );
        if result.is_ok() {
            native_mutations.set(native_mutations.get() + 1);
        }

        assert!(result.is_err());
        assert_eq!(native_mutations.get(), 0);
    }

    #[test]
    fn prepared_bound_store_preflight_failure_precedes_every_native_mutation() {
        let authority = CredentialAuthority::new("discovery-native-b".to_owned(), "b".repeat(64))
            .expect("authority");
        let native_mutations = Cell::new(0_u32);

        let result = (|| {
            let prepared = prepare_bound_credential_store_with(
                "connection-a",
                NativeCredential::new("must-zeroize-on-error".to_owned()),
                &authority,
                |_| Err(PlatformError::new(PlatformErrorCode::StorageUnavailable)),
            )?;
            native_mutations.set(native_mutations.get() + 1);
            drop(prepared);
            Ok::<(), PlatformError>(())
        })();

        assert_eq!(
            result.expect_err("platform preflight must fail").code(),
            PlatformErrorCode::StorageUnavailable
        );
        assert_eq!(native_mutations.get(), 0);
    }

    #[test]
    fn prepared_bound_store_uses_the_exact_physical_execution_authority() {
        let prior = CredentialAuthority::new("discovery-native-a".to_owned(), "a".repeat(64))
            .expect("prior authority");
        let current = CredentialAuthority::new("discovery-native-b".to_owned(), "b".repeat(64))
            .expect("current authority");
        let prepared = prepare_bound_credential_store_with(
            "connection-a",
            NativeCredential::new("current-execution-secret".to_owned()),
            &current,
            |_| Ok(()),
        )
        .expect("prepare current execution");
        let (physical_reference, encoded) = prepared.into_parts();

        assert_eq!(
            physical_reference,
            credential_envelope::physical_reference("connection-a", &current)
                .expect("current physical reference")
        );
        assert_ne!(
            physical_reference,
            credential_envelope::physical_reference("connection-a", &prior)
                .expect("prior physical reference")
        );
        assert_eq!(
            credential_envelope::observe(encoded, &current),
            BoundCredentialObservation::Match
        );
    }

    #[test]
    fn saved_content_source_receipt_has_no_path_or_bytes() {
        let receipt =
            NativeSavedContentSource::new("character.json".to_owned(), 42, "a".repeat(64));
        let projection = serde_json::to_value(&receipt).expect("serialize safe receipt");
        assert_eq!(projection["display_name"], "character.json");
        assert_eq!(projection["size_bytes"], 42);
        assert_eq!(projection["sha256"], "a".repeat(64));
        assert!(projection.get("path").is_none());
        assert!(projection.get("bytes").is_none());
    }
}
