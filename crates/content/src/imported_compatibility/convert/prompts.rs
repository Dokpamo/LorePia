use lorepia_domain::{
    BlockSource, CacheBoundary, CacheBoundaryId, CacheMode, CacheRoleFilter, CoreResult,
    HistorySelector, InstructionAuthority, MergePolicy, OverflowPolicy, PlacementZone, PromptBlock,
    PromptBlockId, PromptBlockKind, Provenance, RoleHint, SafeTemplate, TemplatePart, TokenPolicy,
};
use serde_json::Value;

use super::{MAX_TEMPLATE_CHARS, bounded_name, unsupported};

pub(super) fn prompt_blocks(
    value: Option<&Value>,
    prefix: &str,
    provenance: &Provenance,
) -> CoreResult<(Vec<PromptBlock>, Vec<CacheBoundary>, usize)> {
    let items = value
        .and_then(Value::as_array)
        .ok_or_else(|| unsupported("imported preset promptTemplate must be an array"))?;
    let mut blocks: Vec<PromptBlock> = Vec::new();
    let mut boundaries = Vec::new();
    let mut skipped = 0_usize;
    for (index, item) in items.iter().enumerate() {
        let Some(object) = item.as_object() else {
            skipped += 1;
            continue;
        };
        let kind = object
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("plain");
        if kind == "cache" {
            if let Some(previous) = blocks.last() {
                boundaries.push(CacheBoundary {
                    id: CacheBoundaryId::from(format!("risu-cache-{prefix}-{index}")),
                    after_block_id: previous.id.clone(),
                    role_filter: CacheRoleFilter::All,
                    ttl: lorepia_domain::CacheTtl::ProviderDefault,
                    mode: CacheMode::Explicit,
                });
            } else {
                skipped += 1;
            }
            continue;
        }
        if let Some(block) = prompt_block(object, index, prefix, provenance)? {
            blocks.push(block);
        } else {
            skipped += 1;
        }
    }
    Ok((blocks, boundaries, skipped))
}

fn prompt_block(
    object: &serde_json::Map<String, Value>,
    index: usize,
    prefix: &str,
    provenance: &Provenance,
) -> CoreResult<Option<PromptBlock>> {
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("plain");
    let name = bounded_name(
        object
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default(),
        &format!("Imported prompt item {}", index + 1),
    );
    let text = object
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let role = role_hint(object.get("role").and_then(Value::as_str));
    let id = format!("risu-block-{prefix}-{index}");
    let inner_template = imported_inner_template(object)?;
    let block = match kind {
        "persona" => dynamic_prompt_block(
            id,
            name,
            PromptBlockKind::UserPersona,
            BlockSource::UserPersona,
            RoleHint::User,
            PlacementZone::CharacterContext,
            None,
            inner_template,
            provenance,
        ),
        "description" => dynamic_prompt_block(
            id,
            name,
            PromptBlockKind::CharacterDescription,
            BlockSource::CharacterField {
                field: lorepia_domain::CharacterField::Description,
            },
            role,
            PlacementZone::CharacterContext,
            None,
            inner_template,
            provenance,
        ),
        "lorebook" => dynamic_prompt_block(
            id,
            name,
            PromptBlockKind::WorldKnowledge,
            BlockSource::SelectedKnowledge,
            role,
            PlacementZone::RetrievedContext,
            None,
            inner_template,
            provenance,
        ),
        "memory" => dynamic_prompt_block(
            id,
            name,
            PromptBlockKind::RetrievedMemory,
            BlockSource::SelectedMemory,
            role,
            PlacementZone::RetrievedContext,
            None,
            inner_template,
            provenance,
        ),
        "authornote" => dynamic_prompt_block(
            id,
            name,
            PromptBlockKind::AuthorNote,
            BlockSource::AuthorNote,
            role,
            PlacementZone::RecentEnhancement,
            None,
            inner_template,
            provenance,
        ),
        "chat" if imported_latest_user_range(object) => {
            latest_user_prompt_block(id, name, provenance)
        }
        "chat" | "chatML" if text.trim().is_empty() => dynamic_prompt_block(
            id,
            name,
            PromptBlockKind::HistorySlice,
            BlockSource::History,
            role,
            PlacementZone::RecentHistory,
            Some(imported_history_selector(object)),
            inner_template,
            provenance,
        ),
        "plain" | "postEverything" | "chatML" if !text.trim().is_empty() => {
            let mut block = static_prompt_block(id, name, text, role, index, provenance)?;
            if kind == "postEverything" {
                block.placement_zone = PlacementZone::PostHistory;
            }
            block
        }
        _ => return Ok(None),
    };
    Ok(Some(block))
}

pub(super) fn static_prompt_block(
    id: String,
    name: String,
    text: &str,
    role_hint: RoleHint,
    order: usize,
    provenance: &Provenance,
) -> CoreResult<PromptBlock> {
    if text.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(unsupported(format!(
            "imported prompt block exceeds {MAX_TEMPLATE_CHARS} characters: {name}"
        )));
    }
    Ok(PromptBlock {
        id: PromptBlockId::from(id),
        name,
        kind: PromptBlockKind::StaticInstruction,
        enabled: true,
        role_hint,
        authority: InstructionAuthority::ImportedContent,
        template: Some(SafeTemplate {
            parts: vec![TemplatePart::Text {
                value: text.to_owned(),
            }],
            max_output_chars: u32::try_from(text.chars().count().max(1))
                .unwrap_or(262_144)
                .min(262_144),
        }),
        condition: None,
        source: BlockSource::Template,
        placement_zone: PlacementZone::PresetInstruction,
        history_selector: None,
        token_policy: TokenPolicy {
            priority: u16::try_from(1_000_usize.saturating_sub(order)).unwrap_or(0),
            min_tokens: None,
            max_tokens: None,
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::TrimTail,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: provenance.clone(),
    })
}

#[allow(clippy::too_many_arguments)]
fn dynamic_prompt_block(
    id: String,
    name: String,
    kind: PromptBlockKind,
    source: BlockSource,
    role_hint: RoleHint,
    placement_zone: PlacementZone,
    history_selector: Option<HistorySelector>,
    template: Option<SafeTemplate>,
    provenance: &Provenance,
) -> PromptBlock {
    PromptBlock {
        id: PromptBlockId::from(id),
        name,
        kind,
        enabled: true,
        role_hint,
        authority: InstructionAuthority::ImportedContent,
        template,
        condition: None,
        source,
        placement_zone,
        history_selector,
        token_policy: TokenPolicy {
            priority: 500,
            min_tokens: None,
            max_tokens: None,
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::TrimHead,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: provenance.clone(),
    }
}

pub(super) fn append_latest_user_block(
    blocks: &mut Vec<PromptBlock>,
    prefix: &str,
    provenance: &Provenance,
) {
    if blocks
        .iter()
        .any(|block| block.kind == PromptBlockKind::LatestUserTurn)
    {
        return;
    }
    blocks.push(latest_user_prompt_block(
        format!("risu-latest-user-{prefix}"),
        "Latest user message".to_owned(),
        provenance,
    ));
}

fn latest_user_prompt_block(id: String, name: String, provenance: &Provenance) -> PromptBlock {
    PromptBlock {
        id: PromptBlockId::from(id),
        name,
        kind: PromptBlockKind::LatestUserTurn,
        enabled: true,
        role_hint: RoleHint::User,
        authority: InstructionAuthority::ImportedContent,
        template: None,
        condition: None,
        source: BlockSource::LatestUser,
        placement_zone: PlacementZone::LatestUser,
        history_selector: None,
        token_policy: TokenPolicy {
            priority: u16::MAX,
            min_tokens: None,
            max_tokens: None,
            reserve_tokens: None,
        },
        overflow_policy: OverflowPolicy::Reject,
        merge_policy: MergePolicy::SeparateMessage,
        provenance: provenance.clone(),
    }
}

fn imported_latest_user_range(object: &serde_json::Map<String, Value>) -> bool {
    object.get("rangeStart").and_then(Value::as_i64) == Some(-1)
        && object.get("rangeEnd").and_then(Value::as_str) == Some("end")
}

fn imported_history_selector(object: &serde_json::Map<String, Value>) -> HistorySelector {
    let Some(start) = object
        .get("rangeStart")
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
    else {
        return HistorySelector::All;
    };
    let end = match object.get("rangeEnd") {
        Some(Value::String(value)) if value == "end" => None,
        Some(value) => value.as_i64().and_then(|value| i32::try_from(value).ok()),
        None => None,
    };
    HistorySelector::RelativeMessageRange { start, end }
}

fn imported_inner_template(
    object: &serde_json::Map<String, Value>,
) -> CoreResult<Option<SafeTemplate>> {
    let Some(source) = object
        .get("innerFormat")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    if source.chars().count() > MAX_TEMPLATE_CHARS {
        return Err(unsupported(
            "imported innerFormat exceeds the safe template limit",
        ));
    }
    let mut parts = Vec::new();
    let mut remaining = source;
    while let Some(index) = remaining.find("{{slot}}") {
        if index > 0 {
            parts.push(TemplatePart::Text {
                value: remaining[..index].to_owned(),
            });
        }
        parts.push(TemplatePart::Slot {
            name: "block_content".to_owned(),
        });
        remaining = &remaining[index + "{{slot}}".len()..];
    }
    if !remaining.is_empty() || parts.is_empty() {
        parts.push(TemplatePart::Text {
            value: remaining.to_owned(),
        });
    }
    Ok(Some(SafeTemplate {
        parts,
        max_output_chars: lorepia_domain::MAX_TEMPLATE_OUTPUT_CHARS,
    }))
}

fn role_hint(value: Option<&str>) -> RoleHint {
    match value {
        Some("system") => RoleHint::System,
        Some("user") => RoleHint::User,
        Some("bot" | "assistant") => RoleHint::Assistant,
        _ => RoleHint::ProviderDefault,
    }
}
