use lorepia_domain::{PromptBlock, PromptConversationMessage, PromptResolutionContext};

use super::DraftMessage;
use crate::render_portable_text;

pub(super) fn is_compat_block(block: &PromptBlock) -> bool {
    block.provenance.source_id.as_deref().is_some_and(|source| {
        source.starts_with("imported-preset:")
            || source.starts_with("imported-memory-preset:")
            || source.starts_with("risu-preset:")
            || source.starts_with("risu-memory-preset:")
    })
}

pub(super) fn render_compat_text(
    block: &PromptBlock,
    source: &str,
    context: &PromptResolutionContext,
) -> String {
    if is_compat_block(block) {
        render_portable_text(source, context)
    } else {
        source.to_owned()
    }
}

pub(super) fn select_relative_message_range(
    messages: Vec<&PromptConversationMessage>,
    start: i32,
    end: Option<i32>,
) -> Result<Vec<&PromptConversationMessage>, String> {
    let length = i64::try_from(messages.len()).unwrap_or(i64::MAX);
    let resolve = |offset: i32| {
        let offset = i64::from(offset);
        if offset < 0 {
            length.saturating_add(offset)
        } else {
            offset
        }
        .clamp(0, length)
    };
    let start = resolve(start);
    let end = end.map_or(length, resolve);
    if start > end {
        return Err("relative history range start occurs after its end".to_owned());
    }
    let start = usize::try_from(start).unwrap_or(messages.len());
    let end = usize::try_from(end).unwrap_or(messages.len());
    Ok(messages[start..end].to_vec())
}

pub(super) fn merge_same_role_messages(messages: &mut Vec<DraftMessage>) {
    let mut merged: Vec<DraftMessage> = Vec::with_capacity(messages.len());
    for message in messages.drain(..) {
        if let Some(previous) = merged.last_mut()
            && previous.requested_role == message.requested_role
            && previous.authority == message.authority
            && previous.provenance == message.provenance
        {
            previous.content.push_str("\n\n");
            previous.content.push_str(&message.content);
            previous
                .source_message_ids
                .extend(message.source_message_ids);
            previous
                .source_memory_record_ids
                .extend(message.source_memory_record_ids);
            continue;
        }
        merged.push(message);
    }
    *messages = merged;
}
