//! Revisioned transform-set persistence at the Core boundary.

use lorepia_domain::{CoreResult, TransformSet, TransformSetId};

use crate::{
    Core, Revisioned,
    revision::{project_revision, project_revisions},
};

impl Core {
    pub fn upsert_transform_set(
        &self,
        transform_set: &TransformSet,
        expected_revision: Option<u64>,
    ) -> CoreResult<Revisioned<TransformSet>> {
        self.storage()
            .save_transform_set(transform_set, expected_revision)
            .map(project_revision)
    }

    pub fn get_transform_set(&self, id: &TransformSetId) -> CoreResult<Revisioned<TransformSet>> {
        self.storage().get_transform_set(id).map(project_revision)
    }

    pub fn list_transform_sets(&self) -> CoreResult<Vec<Revisioned<TransformSet>>> {
        self.storage().list_transform_sets().map(project_revisions)
    }

    pub fn delete_transform_set(
        &self,
        id: &TransformSetId,
        expected_revision: u64,
    ) -> CoreResult<Revisioned<TransformSet>> {
        self.storage()
            .soft_delete_transform_set(id, expected_revision)
            .map(project_revision)
    }
}
