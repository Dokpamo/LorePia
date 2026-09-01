use std::collections::BTreeSet;

use sha2::{Digest, Sha256};

#[derive(Default)]
pub(crate) struct KnowledgeEntryIds(BTreeSet<String>);

impl KnowledgeEntryIds {
    pub(crate) fn make_unique(&mut self, id: &mut String, source_sha256: &str, index: usize) {
        if self.0.insert(id.clone()) {
            return;
        }
        *id = derived_knowledge_entry_id(source_sha256, index, "duplicate");
        while !self.0.insert(id.clone()) {
            id.push('x');
        }
    }
}

pub(crate) fn normalized_knowledge_entry_id(
    raw_id: Option<String>,
    source_sha256: &str,
    index: usize,
) -> String {
    raw_id
        .map(|value| value.trim().to_owned())
        .filter(|value| {
            let chars = value.chars().count();
            (1..=256).contains(&chars) && !value.contains('\0')
        })
        .unwrap_or_else(|| derived_knowledge_entry_id(source_sha256, index, "missing"))
}

fn derived_knowledge_entry_id(source_sha256: &str, index: usize, reason: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"portable-knowledge-entry-v1\0");
    digest.update(source_sha256.as_bytes());
    digest.update([0]);
    digest.update(index.to_le_bytes());
    digest.update([0]);
    digest.update(reason.as_bytes());
    format!("card-entry:{}", hex::encode(digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_and_duplicate_entry_ids_are_deterministically_replaced() {
        let source = "a".repeat(64);
        let missing = normalized_knowledge_entry_id(Some(String::new()), &source, 0);
        let oversized = normalized_knowledge_entry_id(Some("x".repeat(257)), &source, 1);
        assert_ne!(missing, oversized);
        assert!(missing.starts_with("card-entry:"));

        let mut ids = KnowledgeEntryIds::default();
        let mut first = "duplicate".to_owned();
        let mut second = "duplicate".to_owned();
        ids.make_unique(&mut first, &source, 2);
        ids.make_unique(&mut second, &source, 3);
        assert_eq!(first, "duplicate");
        assert_ne!(second, first);
        assert!(second.starts_with("card-entry:"));
    }
}
