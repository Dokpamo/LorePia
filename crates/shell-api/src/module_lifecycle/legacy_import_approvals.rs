use lorepia_core::{ContentModuleImportApprovalCandidate, CoreError, CoreErrorCode};

use super::ContentModuleImportApprovalCandidateDto;
use crate::{ShellError, ShellResult};

pub(super) fn project_revision_import_approvals(
    result: Result<Vec<ContentModuleImportApprovalCandidate>, CoreError>,
) -> ShellResult<Vec<ContentModuleImportApprovalCandidateDto>> {
    match result {
        Ok(approvals) => approvals
            .into_iter()
            .map(ContentModuleImportApprovalCandidateDto::try_from)
            .collect(),
        // A historical approval can predate the current complete capability
        // policy matrix. It is not valid rollback authority, but it must not
        // hide the durable binding or other valid revisions from the reader.
        Err(error) if error.code == CoreErrorCode::InvalidInput => Ok(Vec::new()),
        Err(error) => Err(ShellError::from(error)),
    }
}

#[cfg(test)]
mod tests {
    use lorepia_core::{CoreError, CoreErrorCode};

    use super::project_revision_import_approvals;

    #[test]
    fn historical_invalid_import_approval_is_omitted_without_hiding_the_binding() {
        let approvals = project_revision_import_approvals(Err(CoreError::invalid(
            "legacy capability policy matrix is incomplete",
        )))
        .expect("invalid historical authority is omitted");
        assert!(approvals.is_empty());

        let corrupted = project_revision_import_approvals(Err(CoreError::new(
            CoreErrorCode::StorageCorrupted,
            "synthetic corrupt historical authority",
            false,
        )))
        .expect_err("storage corruption must still fail closed");
        assert_eq!(corrupted.code, crate::ShellErrorCode::StorageCorrupted);
    }
}
