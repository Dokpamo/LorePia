    #[test]
    fn content_module_linked_documents_must_be_in_the_exact_approved_selection() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("unbound-content-module.zip");
        synthetic_unbound_content_module_package(&source);
        let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
        let inspection = core
            .inspect_content_package_import(&source)
            .expect("inspect unbound content module");
        assert!(inspection.inspection.is_allowed());
        let error = core
            .select_content_package_import(
                &inspection.import_id,
                &selection_request(&inspection, vec!["unbound-content-module".to_owned()]),
            )
            .expect_err("unbound module dependency must fail before selection is stored");
        assert_eq!(error.code, CoreErrorCode::InvalidInput);
        let unchanged = core
            .get_content_package_import(&inspection.import_id)
            .expect("unbound import remains reviewable");
        assert_eq!(unchanged.status, PackageImportStatus::Inspected);
        assert!(unchanged.selection.is_none());
        assert!(
            core.get_content_module(&ContentModuleId::from(
                "core.package.unbound-content-module"
            ))
            .is_err()
        );
    }

    fn assert_durable_package_selection_recovery(
        data_root: &Path,
        inspection: &ContentPackageImportInspection,
    ) -> ContentPackageSelectionReceipt {
        let selection_input = selection_request(inspection, vec!["transform".to_owned()]);
        let core = Core::open(CoreConfig::new(data_root)).expect("reopen before select");
        assert_eq!(
            core.list_pending_content_package_import_reviews(16)
                .expect("list inspected import")
                .iter()
                .map(|review| review.import_id.as_str())
                .collect::<Vec<_>>(),
            [inspection.import_id.as_str()]
        );
        let selected = core
            .select_content_package_import(&inspection.import_id, &selection_input)
            .expect("select");
        selected
            .target_review
            .verify()
            .expect("verify sealed create target review");
        assert_eq!(selected.target_review.documents.len(), 1);
        assert_eq!(
            selected.target_review.documents[0].disposition,
            PackageDocumentTargetDisposition::Create
        );
        let selected_review = core
            .get_content_package_import_review(&inspection.import_id)
            .expect("reopen safe selected review");
        assert_eq!(selected_review.status, PackageImportStatus::AwaitingReview);
        assert_eq!(
            selected_review
                .selection
                .as_ref()
                .expect("selected review")
                .normalization_evidence_sha256,
            selected.normalization_evidence_sha256
        );
        assert_eq!(
            selected_review
                .selection
                .as_ref()
                .expect("selected target review")
                .target_review,
            selected.target_review
        );
        assert!(selected_review.approval.is_none());
        assert_eq!(
            core.list_pending_content_package_import_reviews(16)
                .expect("list selected import"),
            [selected_review]
        );
        drop(core);
        let core = Core::open(CoreConfig::new(data_root)).expect("reopen select replay");
        let selected_replay = core
            .select_content_package_import(&inspection.import_id, &selection_input)
            .expect("select replay");
        assert_eq!(selected_replay, selected);
        assert!(selected.normalization_evidence.iter().any(|entry| {
            entry.component_id == "transform"
                && entry.object_id == "core-package-transform"
                && entry.field == "enabled"
                && entry.before
                && !entry.after
        }));
        selected
    }

    fn assert_durable_package_approval_recovery(
        data_root: &Path,
        inspection: &ContentPackageImportInspection,
        selected: &ContentPackageSelectionReceipt,
    ) -> (
        Core,
        ContentPackageApprovalRequest,
        ContentPackageApprovalReceipt,
    ) {
        let core = Core::open(CoreConfig::new(data_root)).expect("reopen before approval");
        let approval_input = approval_request(
            inspection,
            selected,
            "approval-transform-restart",
            vec!["transform".to_owned()],
            vec![PackageCapability::Transforms],
        );
        let mut stale_evidence_approval = approval_input.clone();
        stale_evidence_approval.expected_normalization_evidence_sha256 = "00".repeat(32);
        core.approve_content_package_import(&inspection.import_id, &stale_evidence_approval)
            .expect_err("stale normalization evidence hash");
        assert_eq!(
            core.get_content_package_import(&inspection.import_id)
                .expect("awaiting-review import")
                .status,
            PackageImportStatus::AwaitingReview
        );
        let mut stale_target_review_approval = approval_input.clone();
        stale_target_review_approval.expected_target_review_sha256 = "00".repeat(32);
        core.approve_content_package_import(&inspection.import_id, &stale_target_review_approval)
            .expect_err("stale target-review digest");
        assert_eq!(
            core.get_content_package_import(&inspection.import_id)
                .expect("target digest rejection preserves selection")
                .status,
            PackageImportStatus::AwaitingReview
        );
        let premature_commit = ContentPackageCommitRequest {
            expected_revision: selected.import.revision,
            expected_package_plan_hash: inspection.inspection.plan_hash.clone(),
            expected_content_selection_plan_hash: selected
                .content_selection
                .selection_plan_hash
                .clone(),
            expected_review_sha256: inspection.review.review_sha256.clone(),
            expected_import_plan_sha256: selected.import_plan.plan_sha256.clone(),
            expected_approval_sha256: Sha256Digest::parse("11".repeat(32)).expect("digest"),
            expected_capability_review_sha256: inspection.capability_review_sha256.clone(),
            expected_normalization_evidence_sha256: selected.normalization_evidence_sha256.clone(),
        };
        core.commit_content_package_import(&inspection.import_id, &premature_commit)
            .expect_err("commit without approval");
        let approved = core
            .approve_content_package_import(&inspection.import_id, &approval_input)
            .expect("approve");
        assert_eq!(approved.target_review, selected.target_review);
        assert!(approved.normalization_evidence.iter().any(|entry| {
            entry.component_id == "transform"
                && entry.object_id == "core-package-transform"
                && entry.field == "enabled"
                && entry.before
                && !entry.after
        }));
        drop(core);
        let core = Core::open(CoreConfig::new(data_root)).expect("reopen approval replay");
        let approved_replay = core
            .approve_content_package_import(&inspection.import_id, &approval_input)
            .expect("approval replay");
        assert_eq!(approved_replay, approved);
        let approved_review = core
            .get_content_package_import_review(&inspection.import_id)
            .expect("reopen safe approved review");
        assert_eq!(
            core.list_pending_content_package_import_reviews(16)
                .expect("list approved import"),
            std::slice::from_ref(&approved_review)
        );
        let approval_review = approved_review.approval.expect("approved review");
        assert_eq!(
            approval_review.approval_sha256,
            approved.approved_plan.approval_sha256
        );
        assert_eq!(approval_review.enabled_component_ids, ["transform"]);
        assert_eq!(
            approval_review.approved_capabilities,
            [PackageCapability::Transforms]
        );
        core.get_completed_content_package_authority(&approval_input.approval_id)
            .expect_err("approval without a completed commit has no module authority");
        (core, approval_input, approved)
    }

    #[test]
    fn durable_package_lifecycle_replays_exact_receipts_after_response_loss_and_restart() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("transform.zip");
        synthetic_transform_package(&source);

        let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
        let inspection = core
            .inspect_content_package_import(&source)
            .expect("durable inspection");
        fs::write(&source, b"caller source changed after one-shot inspection")
            .expect("mutate caller source");
        drop(core);
        let selected = assert_durable_package_selection_recovery(data_root.path(), &inspection);
        let (core, approval_input, approved) =
            assert_durable_package_approval_recovery(data_root.path(), &inspection, &selected);

        let commit_input = commit_request(&inspection, &selected, &approved);
        let committed = core
            .commit_content_package_import(&inspection.import_id, &commit_input)
            .expect("commit");
        assert_eq!(committed.import.status, PackageImportStatus::Completed);
        assert_eq!(committed.committed_document_ids, ["core-package-transform"]);
        let completed_authority = core
            .get_completed_content_package_authority(&approval_input.approval_id)
            .expect("completed package authority");
        assert_eq!(completed_authority.status, PackageImportStatus::Completed);
        assert_eq!(
            completed_authority.approval_sha256,
            approved.approved_plan.approval_sha256.as_str()
        );
        assert_eq!(completed_authority.enabled_components.len(), 1);
        assert_eq!(
            completed_authority.enabled_components[0].component_id,
            "transform"
        );
        assert_eq!(
            completed_authority.enabled_components[0]
                .committed_documents
                .iter()
                .map(|document| document.target_object_id.as_str())
                .collect::<Vec<_>>(),
            ["core-package-transform"]
        );
        drop(core);

        let core = Core::open(CoreConfig::new(data_root.path())).expect("reopen commit replay");
        let committed_replay = core
            .commit_content_package_import(&inspection.import_id, &commit_input)
            .expect("commit replay");
        assert_eq!(committed_replay, committed);
        assert_eq!(
            core.get_content_package_import_review(&inspection.import_id)
                .expect("completed safe review")
                .status,
            PackageImportStatus::Completed
        );
        assert!(
            core.list_pending_content_package_import_reviews(16)
                .expect("completed import excluded")
                .is_empty()
        );
        let discarded_source = source_root.path().join("discarded.zip");
        synthetic_transform_package(&discarded_source);
        let discarded_inspection = core
            .inspect_content_package_import(&discarded_source)
            .expect("inspect import to discard");
        core.discard_content_package_import(
            &discarded_inspection.import_id,
            &ContentPackageDiscardRequest {
                expected_revision: discarded_inspection.revision,
                expected_review_sha256: discarded_inspection.review.review_sha256.clone(),
                expected_import_plan_sha256: None,
                expected_capability_review_sha256: discarded_inspection
                    .capability_review_sha256
                    .clone(),
            },
        )
        .expect("discard inspected import");
        assert!(
            core.list_pending_content_package_import_reviews(16)
                .expect("discarded import excluded")
                .is_empty()
        );
        let stored = core
            .get_transform_set(&TransformSetId::from("core-package-transform"))
            .expect("stored transform");
        assert!(!stored.value.enabled);
        assert!(stored.value.imported_author_enabled);
        assert!(
            fs::read_dir(data_root.path().join("staging"))
                .expect("staging directory")
                .next()
                .is_none()
        );
    }

    #[test]
    fn multi_document_component_commits_contiguous_ordinals_and_reopens_both_objects() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("transform-array.zip");
        synthetic_transform_array_package(&source);
        let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
        let inspection = core
            .inspect_content_package_import(&source)
            .expect("inspect array");
        let selected = core
            .select_content_package_import(
                &inspection.import_id,
                &selection_request(&inspection, vec!["transform-array".to_owned()]),
            )
            .expect("select array");
        let approved = core
            .approve_content_package_import(
                &inspection.import_id,
                &approval_request(
                    &inspection,
                    &selected,
                    "approval-transform-array",
                    vec!["transform-array".to_owned()],
                    vec![PackageCapability::Transforms],
                ),
            )
            .expect("approve array");
        assert_eq!(
            approved
                .normalization_evidence
                .iter()
                .filter(|entry| entry.field == "enabled")
                .count(),
            2
        );
        let committed = core
            .commit_content_package_import(
                &inspection.import_id,
                &commit_request(&inspection, &selected, &approved),
            )
            .expect("commit array");
        assert_eq!(
            committed.committed_document_ids,
            ["array-transform-a", "array-transform-b"]
        );
        drop(core);

        let reopened = Core::open(CoreConfig::new(data_root.path())).expect("reopen array");
        for id in ["array-transform-a", "array-transform-b"] {
            let stored = reopened
                .get_transform_set(&TransformSetId::from(id))
                .expect("stored array transform");
            assert!(!stored.value.enabled);
            assert!(stored.value.imported_author_enabled);
        }
    }

    fn inspect_after_atomic_selection_failure(
        core: &Core,
        source: &Path,
        database_path: &Path,
    ) -> (
        ContentPackageImportInspection,
        ContentPackageSelectionRequest,
    ) {
        let inspection = core
            .inspect_content_package_import(source)
            .expect("inspect mixed array");
        let precompleted_export = core
            .prepare_content_source_export(&ContentSourceExportSelector::ContentPackage {
                import_id: inspection.import_id.clone(),
            })
            .expect_err("an inspected package source is not completed export authority");
        assert_eq!(precompleted_export.code, CoreErrorCode::InvalidInput);
        let selection_input = selection_request(&inspection, vec!["transform-array".to_owned()]);
        Connection::open(database_path)
            .expect("open selection failure injector")
            .execute_batch(
                "CREATE TRIGGER package_test_target_review_abort
                 BEFORE INSERT ON package_import_document_target_reviews
                 BEGIN
                     SELECT RAISE(ABORT, 'synthetic target-review failure');
                 END;",
            )
            .expect("install selection failure injector");
        core.select_content_package_import(&inspection.import_id, &selection_input)
            .expect_err("target-review insertion failure must abort selection");
        let unchanged_inspection = core
            .get_content_package_import(&inspection.import_id)
            .expect("unchanged inspected import");
        assert_eq!(unchanged_inspection.status, PackageImportStatus::Inspected);
        assert_eq!(unchanged_inspection.revision, inspection.revision);
        let connection = Connection::open(database_path).expect("inspect selection rollback");
        for table in [
            "package_import_components",
            "package_import_document_target_reviews",
        ] {
            let count = connection
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE import_id = ?1"),
                    [inspection.import_id.as_str()],
                    |row| row.get::<_, u32>(0),
                )
                .expect("count rolled-back selection rows");
            assert_eq!(count, 0, "{table} must roll back with selection");
        }
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM package_import_audit_events
                     WHERE import_id = ?1 AND event_kind = 'review_requested'",
                    [inspection.import_id.as_str()],
                    |row| row.get::<_, u32>(0),
                )
                .expect("count rolled-back selection audit"),
            0
        );
        connection
            .execute("DROP TRIGGER package_test_target_review_abort", [])
            .expect("remove selection failure injector");
        (inspection, selection_input)
    }

    fn active_test_database_path(data_root: &Path) -> PathBuf {
        fs::read_dir(data_root.join("db/schema-cutover"))
            .expect("read committed database generations")
            .filter_map(|entry| {
                let entry = entry.expect("read database generation");
                if !entry.path().join("generation-committed.json").is_file() {
                    return None;
                }
                let manifest = fs::read(entry.path().join("generation-manifest.json")).ok()?;
                let manifest = serde_json::from_slice::<serde_json::Value>(&manifest)
                    .expect("decode database generation manifest");
                Some((
                    manifest["activation_sequence"]
                        .as_u64()
                        .expect("database generation activation sequence"),
                    data_root.join(
                        manifest["active_database_relative_path"]
                            .as_str()
                            .expect("active database relative path"),
                    ),
                ))
            })
            .max_by_key(|(sequence, _)| *sequence)
            .map(|(_, path)| path)
            .expect("active committed database generation")
    }

    fn select_mixed_targets(
        core: &Core,
        inspection: &ContentPackageImportInspection,
        selection_input: &ContentPackageSelectionRequest,
    ) -> (
        ContentPackageSelectionReceipt,
        ContentPackageApprovalRequest,
    ) {
        let selected = core
            .select_content_package_import(&inspection.import_id, selection_input)
            .expect("select mixed array");
        selected
            .target_review
            .verify()
            .expect("verify mixed target review");
        assert_eq!(selected.target_review.documents.len(), 2);
        assert_eq!(
            selected
                .target_review
                .documents
                .iter()
                .map(|document| document.disposition)
                .collect::<Vec<_>>(),
            [
                PackageDocumentTargetDisposition::Update,
                PackageDocumentTargetDisposition::Create,
            ]
        );
        let approval_input = approval_request(
            inspection,
            &selected,
            "approval-mixed-transform-array",
            vec!["transform-array".to_owned()],
            vec![PackageCapability::Transforms],
        );
        assert_eq!(approval_input.confirmed_update_targets.len(), 1);
        assert_eq!(
            approval_input.confirmed_update_targets[0].target_object_id,
            "array-transform-a"
        );
        let mut missing_confirmation = approval_input.clone();
        missing_confirmation.confirmed_update_targets.clear();
        core.approve_content_package_import(&inspection.import_id, &missing_confirmation)
            .expect_err("every update target requires explicit confirmation");
        let selection_after_rejected_confirmation = core
            .get_content_package_import(&inspection.import_id)
            .expect("selection survives rejected confirmation");
        assert_eq!(
            selection_after_rejected_confirmation.status,
            PackageImportStatus::AwaitingReview
        );
        assert_eq!(
            selection_after_rejected_confirmation.revision,
            selected.import.revision
        );
        (selected, approval_input)
    }

    fn assert_atomic_approval_failure(
        core: &Core,
        inspection: &ContentPackageImportInspection,
        selected: &ContentPackageSelectionReceipt,
        approval_input: &ContentPackageApprovalRequest,
        database_path: &Path,
    ) {
        Connection::open(database_path)
            .expect("open approval failure injector")
            .execute_batch(
                "CREATE TRIGGER package_test_approval_audit_abort
                 BEFORE INSERT ON package_import_audit_events
                 WHEN NEW.event_kind = 'approved'
                 BEGIN
                     SELECT RAISE(ABORT, 'synthetic approval audit failure');
                 END;",
            )
            .expect("install approval failure injector");
        core.approve_content_package_import(&inspection.import_id, approval_input)
            .expect_err("approval audit failure must abort the transaction");
        let selection_after_approval_failure = core
            .get_content_package_import(&inspection.import_id)
            .expect("selection survives approval failure");
        assert_eq!(
            selection_after_approval_failure.status,
            PackageImportStatus::AwaitingReview
        );
        assert_eq!(
            selection_after_approval_failure.revision,
            selected.import.revision
        );
        let connection = Connection::open(database_path).expect("inspect approval rollback");
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM package_import_approvals WHERE import_id = ?1",
                    [inspection.import_id.as_str()],
                    |row| row.get::<_, u32>(0),
                )
                .expect("count rolled-back approval"),
            0
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM package_import_document_target_reviews
                     WHERE import_id = ?1",
                    [inspection.import_id.as_str()],
                    |row| row.get::<_, u32>(0),
                )
                .expect("count preserved target review"),
            2
        );
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM package_import_audit_events
                     WHERE import_id = ?1 AND event_kind = 'approved'",
                    [inspection.import_id.as_str()],
                    |row| row.get::<_, u32>(0),
                )
                .expect("count rolled-back approval audit"),
            0
        );
        connection
            .execute("DROP TRIGGER package_test_approval_audit_abort", [])
            .expect("remove approval failure injector");
    }

    fn assert_stale_mixed_target_approval_fails(core: &Core, source: &Path) {
        let stale_inspection = core
            .inspect_content_package_import(source)
            .expect("inspect all-update replay");
        let stale_selection_input =
            selection_request(&stale_inspection, vec!["transform-array".to_owned()]);
        let stale_selection = core
            .select_content_package_import(&stale_inspection.import_id, &stale_selection_input)
            .expect("select exact update targets");
        assert!(
            stale_selection
                .target_review
                .documents
                .iter()
                .all(|document| {
                    document.disposition == PackageDocumentTargetDisposition::Update
                })
        );
        let mut changed = core
            .get_transform_set(&TransformSetId::from("array-transform-a"))
            .expect("load target before stale mutation");
        changed.value.name.push_str(" locally changed");
        core.upsert_transform_set(&changed.value, Some(changed.revision))
            .expect("advance reviewed update target");
        assert_eq!(
            core.select_content_package_import(
                &stale_inspection.import_id,
                &stale_selection_input,
            )
            .expect("selection response-loss replay uses sealed targets"),
            stale_selection
        );
        assert_eq!(
            core.get_content_package_import_review(&stale_inspection.import_id)
                .expect("reopen stale selection safely")
                .selection
                .expect("sealed stale selection")
                .target_review,
            stale_selection.target_review
        );
        let stale_approval = approval_request(
            &stale_inspection,
            &stale_selection,
            "approval-stale-transform-array",
            vec!["transform-array".to_owned()],
            vec![PackageCapability::Transforms],
        );
        core.approve_content_package_import(&stale_inspection.import_id, &stale_approval)
            .expect_err("changed update target must fail approval CAS");
        let selection_after_stale_approval = core
            .get_content_package_import(&stale_inspection.import_id)
            .expect("stale approval leaves selection intact");
        assert_eq!(
            selection_after_stale_approval.status,
            PackageImportStatus::AwaitingReview
        );
        assert_eq!(
            selection_after_stale_approval.revision,
            stale_selection.import.revision
        );
    }

    #[test]
    fn mixed_document_targets_require_exact_confirmation_and_fail_atomically_when_stale() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("mixed-transform-array.zip");
        synthetic_transform_array_package(&source);
        let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
        core.upsert_transform_set(
            &local_transform_set("array-transform-a", "Existing array A"),
            None,
        )
        .expect("seed one reviewed update target");
        let database_path = active_test_database_path(data_root.path());
        let (inspection, selection_input) =
            inspect_after_atomic_selection_failure(&core, &source, &database_path);
        let (selected, approval_input) = select_mixed_targets(&core, &inspection, &selection_input);
        assert_atomic_approval_failure(
            &core,
            &inspection,
            &selected,
            &approval_input,
            &database_path,
        );

        let approved = core
            .approve_content_package_import(&inspection.import_id, &approval_input)
            .expect("approve exact mixed targets");
        assert_eq!(approved.target_review, selected.target_review);
        assert_eq!(
            approved.approved_plan.target_review_sha256.as_str(),
            selected.target_review.target_review_sha256.as_str()
        );
        assert_eq!(
            approved
                .approved_plan
                .update_target_confirmations_sha256
                .as_str(),
            package_update_target_confirmations_sha256(&approval_input.confirmed_update_targets,)
                .expect("hash exact mixed confirmations")
        );
        approved
            .approved_plan
            .verify()
            .expect("approval hash binds mixed target authority");
        drop(core);
        let core = Core::open(CoreConfig::new(data_root.path())).expect("reopen approval replay");
        assert_eq!(
            core.approve_content_package_import(&inspection.import_id, &approval_input)
                .expect("replay exact mixed approval"),
            approved
        );
        core.commit_content_package_import(
            &inspection.import_id,
            &commit_request(&inspection, &selected, &approved),
        )
        .expect("commit mixed targets");
        let prepared_export = core
            .prepare_content_source_export(&ContentSourceExportSelector::ContentPackage {
                import_id: inspection.import_id.clone(),
            })
            .expect("prepare completed package source export");
        assert_eq!(
            prepared_export.descriptor().kind,
            ContentSourceExportKind::LorepiaPackage
        );
        assert_eq!(prepared_export.descriptor().source_id, inspection.import_id);
        assert_eq!(
            prepared_export.descriptor().sha256,
            inspection.inspection.source_sha256
        );
        assert_eq!(
            prepared_export.descriptor().size_bytes,
            inspection.inspection.source_size
        );
        assert_eq!(
            fs::read(prepared_export.source_path()).expect("read private package CAS export"),
            fs::read(&source).expect("read original synthetic package source"),
            "completed package export must preserve the exact imported archive bytes"
        );
        assert_stale_mixed_target_approval_fails(&core, &source);
    }

    #[test]
    fn completed_package_export_catalog_survives_restart_and_rejects_cas_tamper() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("completed-export.zip");
        synthetic_transform_package(&source);
        let core = Core::open(CoreConfig::new(data_root.path())).expect("open Core");
        let inspection = core
            .inspect_content_package_import(&source)
            .expect("inspect completed export package");
        let component_ids = vec!["transform".to_owned()];
        let selected = core
            .select_content_package_import(
                &inspection.import_id,
                &selection_request(&inspection, component_ids.clone()),
            )
            .expect("select completed export package");
        let approved = core
            .approve_content_package_import(
                &inspection.import_id,
                &approval_request(
                    &inspection,
                    &selected,
                    "approval-completed-export-catalog",
                    component_ids,
                    vec![PackageCapability::Transforms],
                ),
            )
            .expect("approve completed export package");
        core.commit_content_package_import(
            &inspection.import_id,
            &commit_request(&inspection, &selected, &approved),
        )
        .expect("commit completed export package");
        let prepared = core
            .prepare_content_source_export(&ContentSourceExportSelector::ContentPackage {
                import_id: inspection.import_id.clone(),
            })
            .expect("prepare exact completed package export");
        let expected_descriptor = prepared.descriptor().clone();
        let package_cas_path = prepared.source_path().to_path_buf();
        drop(prepared);
        drop(core);

        let core = Core::open(CoreConfig::new(data_root.path()))
            .expect("reopen completed package export catalog");
        assert_eq!(
            core.list_completed_content_package_export_descriptors(
                lorepia_storage::MAX_COMPLETED_PACKAGE_EXPORTS,
            )
            .expect("discover completed package export after restart"),
            vec![expected_descriptor]
        );
        for invalid_limit in [
            0,
            lorepia_storage::MAX_COMPLETED_PACKAGE_EXPORTS
                .checked_add(1)
                .expect("small export catalog bound"),
        ] {
            assert_eq!(
                core.list_completed_content_package_export_descriptors(invalid_limit)
                    .expect_err("completed package export catalog bound must fail closed")
                    .code,
                CoreErrorCode::InvalidInput
            );
        }
        fs::write(
            package_cas_path,
            vec![
                b'x';
                usize::try_from(inspection.inspection.source_size)
                    .expect("synthetic package size fits memory")
            ],
        )
        .expect("tamper completed package CAS bytes");
        let catalog_error = core
            .list_completed_content_package_export_descriptors(
                lorepia_storage::MAX_COMPLETED_PACKAGE_EXPORTS,
            )
            .expect_err("one corrupt completed source must fail the whole catalog closed");
        assert_eq!(catalog_error.code, CoreErrorCode::StorageCorrupted);
    }

    #[test]
    fn png_audio_and_video_assets_complete_full_review_commit_and_restart() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("media.zip");
        let component_ids = synthetic_media_package(&source);
        let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
        let inspection = core
            .inspect_content_package_import(&source)
            .expect("inspect media package");
        assert!(inspection.review.local_import_allowed);
        assert_eq!(
            inspection.review.manifest.required_capabilities,
            [
                ContentCapability::ImageAssets,
                ContentCapability::AudioAssets,
                ContentCapability::VideoAssets,
            ]
        );
        let selected = core
            .select_content_package_import(
                &inspection.import_id,
                &selection_request(&inspection, component_ids.clone()),
            )
            .expect("select media");
        let approved = core
            .approve_content_package_import(
                &inspection.import_id,
                &approval_request(
                    &inspection,
                    &selected,
                    "approval-media-restart",
                    component_ids.clone(),
                    Vec::new(),
                ),
            )
            .expect("approve media");
        assert!(approved.normalization_evidence.is_empty());
        let committed = core
            .commit_content_package_import(
                &inspection.import_id,
                &commit_request(&inspection, &selected, &approved),
            )
            .expect("commit media");
        assert_eq!(committed.asset_ids.len(), 3);
        drop(core);

        let reopened = Core::open(CoreConfig::new(data_root.path())).expect("reopen media");
        for asset_id in &committed.asset_ids {
            reopened
                .storage()
                .resolve_approved_asset_by_id(asset_id)
                .expect("durable approved media");
        }
        let authority = reopened
            .get_completed_content_package_authority("approval-media-restart")
            .expect("reopen exact media authority");
        assert_eq!(authority.committed_assets.len(), committed.asset_ids.len());
        assert_eq!(
            authority
                .committed_assets
                .iter()
                .flat_map(|asset| asset.source_components.iter())
                .map(|source| source.component_id.as_str())
                .collect::<std::collections::BTreeSet<_>>(),
            component_ids
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>()
        );
        for asset in authority.committed_assets {
            assert_eq!(asset.asset_id, asset.descriptor.id);
            assert_eq!(asset.cas_sha256, asset.descriptor.sha256.as_str());
            assert_eq!(asset.descriptor_sha256.len(), 64);
            assert!(!asset.source_components.is_empty());
            assert!(
                asset
                    .source_components
                    .iter()
                    .all(|source| source.component_sha256.len() == 64)
            );
        }
    }

    #[test]
    fn durable_source_cas_tamper_fails_before_selection_mutates_state() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("transform.zip");
        synthetic_transform_package(&source);
        let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
        let inspection = core
            .inspect_content_package_import(&source)
            .expect("inspect transform package");
        let source_cas = content_cas_path(
            data_root.path(),
            "sources",
            &inspection.inspection.source_sha256,
        );
        fs::write(&source_cas, b"tampered durable source").expect("tamper source CAS");

        let error = core
            .select_content_package_import(
                &inspection.import_id,
                &selection_request(&inspection, vec!["transform".to_owned()]),
            )
            .expect_err("tampered source must fail closed");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        let stored = core
            .get_content_package_import(&inspection.import_id)
            .expect("unchanged import");
        assert_eq!(stored.status, PackageImportStatus::Inspected);
        assert!(stored.selection.is_none());
        assert!(
            core.get_transform_set(&TransformSetId::from("core-package-transform"))
                .is_err(),
            "no typed document may be committed after source tamper"
        );
    }

    #[test]
    fn approved_asset_cas_tamper_breaks_commit_replay_without_new_state() {
        let source_root = tempdir().expect("source root");
        let data_root = tempdir().expect("data root");
        let source = source_root.path().join("media.zip");
        let component_ids = synthetic_media_package(&source);
        let core = Core::open(CoreConfig::new(data_root.path())).expect("open core");
        let inspection = core
            .inspect_content_package_import(&source)
            .expect("inspect media");
        let selected = core
            .select_content_package_import(
                &inspection.import_id,
                &selection_request(&inspection, component_ids.clone()),
            )
            .expect("select media");
        let approved = core
            .approve_content_package_import(
                &inspection.import_id,
                &approval_request(
                    &inspection,
                    &selected,
                    "approval-media-tamper",
                    component_ids,
                    Vec::new(),
                ),
            )
            .expect("approve media");
        let commit_input = commit_request(&inspection, &selected, &approved);
        let committed = core
            .commit_content_package_import(&inspection.import_id, &commit_input)
            .expect("commit media");
        let tampered_asset_id = committed.asset_ids[0].clone();
        let descriptor = core
            .storage()
            .resolve_approved_asset_by_id(&tampered_asset_id)
            .expect("approved descriptor");
        let asset_cas = content_cas_path(data_root.path(), "assets", descriptor.sha256.as_str());
        drop(core);
        let mut tampered = fs::read(&asset_cas).expect("read asset CAS");
        let last = tampered.last_mut().expect("non-empty test asset");
        *last ^= 0x01;
        fs::write(&asset_cas, tampered).expect("tamper asset CAS");

        let reopened = Core::open(CoreConfig::new(data_root.path())).expect("reopen core");
        let error = reopened
            .commit_content_package_import(&inspection.import_id, &commit_input)
            .expect_err("tampered asset must break exact commit replay");
        assert_eq!(error.code, CoreErrorCode::StorageCorrupted);
        assert_eq!(
            reopened
                .get_content_package_import(&inspection.import_id)
                .expect("completed import remains")
                .status,
            PackageImportStatus::Completed
        );
        assert!(
            reopened
                .storage()
                .resolve_approved_asset_by_id(&tampered_asset_id)
                .is_err(),
            "tampered bytes must not resolve as approved media"
        );
        let authority_error = reopened
            .get_completed_content_package_authority("approval-media-tamper")
            .expect_err("tampered asset must invalidate package authority");
        assert_eq!(authority_error.code, CoreErrorCode::StorageCorrupted);
    }

