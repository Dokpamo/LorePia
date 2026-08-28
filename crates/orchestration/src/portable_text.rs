//! Deterministic renderer for portable character-card text macros.

use lorepia_domain::{PromptResolutionContext, VariableValue};
use sha2::{Digest, Sha256};

const MAX_RENDER_PASSES: usize = 256;
const MAX_RENDER_CHARS: usize = 262_144;

/// Expands the portable macro subset used by character fields and knowledge.
/// Unknown macros are retained verbatim so ordinary brace-heavy prose and
/// future extensions round-trip without data loss.
#[must_use]
pub fn render_portable_text(source: &str, context: &PromptResolutionContext) -> String {
    if !source.contains("{{") {
        return source.to_owned();
    }
    let mut output = source.to_owned();
    for pass in 0..MAX_RENDER_PASSES {
        let Some((start, end, token)) = next_evaluable_token(&output) else {
            break;
        };
        let Some(value) = evaluate_token(token, context, source, pass) else {
            break;
        };
        if replacement_would_exceed(&output, start, end, &value) {
            return source.to_owned();
        }
        output.replace_range(start..end, &value);
    }
    for pass in 0..MAX_RENDER_PASSES {
        let Some(block) = innermost_block(&output) else {
            break;
        };
        let replacement = if evaluate_block_condition(block.token, context, source, pass) {
            output[block.body_start..block.else_start.unwrap_or(block.body_end)].to_owned()
        } else if let Some(else_end) = block.else_end {
            output[else_end..block.body_end].to_owned()
        } else {
            String::new()
        };
        if replacement_would_exceed(&output, block.start, block.end, &replacement) {
            return source.to_owned();
        }
        output.replace_range(block.start..block.end, &replacement);
        for expression_pass in 0..MAX_RENDER_PASSES {
            let Some((start, end, token)) = next_evaluable_token(&output) else {
                break;
            };
            let Some(value) = evaluate_token(token, context, source, expression_pass) else {
                break;
            };
            if replacement_would_exceed(&output, start, end, &value) {
                return source.to_owned();
            }
            output.replace_range(start..end, &value);
        }
    }
    output
}

fn replacement_would_exceed(source: &str, start: usize, end: usize, replacement: &str) -> bool {
    source[..start].chars().count() + replacement.chars().count() + source[end..].chars().count()
        > MAX_RENDER_CHARS
}

fn next_evaluable_token(source: &str) -> Option<(usize, usize, &str)> {
    let mut stack = Vec::new();
    let bytes = source.as_bytes();
    let mut cursor = 0;
    while cursor + 1 < bytes.len() {
        if &bytes[cursor..cursor + 2] == b"{{" {
            stack.push(cursor);
            cursor += 2;
            continue;
        }
        if &bytes[cursor..cursor + 2] == b"}}" {
            if let Some(start) = stack.pop() {
                let token = &source[start + 2..cursor];
                if !is_block_token(token) && token_is_known(token) {
                    return Some((start, cursor + 2, token));
                }
            }
            cursor += 2;
            continue;
        }
        cursor += 1;
    }
    None
}

fn is_block_token(token: &str) -> bool {
    let token = token.trim();
    token.starts_with('#') || token.starts_with('/') || token == "else"
}

fn token_is_known(token: &str) -> bool {
    let name = token
        .trim()
        .split_once("::")
        .map_or_else(|| token.trim(), |(name, _)| name.trim());
    matches!(
        name.to_ascii_lowercase().as_str(),
        "user"
            | "char"
            | "character"
            | "date"
            | "time"
            | "getvar"
            | "getglobalvar"
            | "equal"
            | "notequal"
            | "greater"
            | "greater_equal"
            | "greaterequal"
            | "less"
            | "less_equal"
            | "lessequal"
            | "and"
            | "or"
            | "not"
            | "contains"
            | "startswith"
            | "roll"
            | "pick"
            | "lastcharmessage"
            | "lastmessageid"
            | "chat_index"
            | "raw"
            | "slot"
            | "?"
    ) || token.trim_start().starts_with("? ")
}

fn evaluate_token(
    token: &str,
    context: &PromptResolutionContext,
    source: &str,
    pass: usize,
) -> Option<String> {
    let token = token.trim();
    let mut parts = token.split("::");
    let name = parts.next()?.trim().to_ascii_lowercase();
    let args = parts.map(str::trim).collect::<Vec<_>>();
    let value = match name.as_str() {
        "user" => context.user_name.clone(),
        "char" | "character" => context.character.name.clone(),
        "date" => context.current_date.clone(),
        "time" => context.current_time.clone(),
        "getvar" | "getglobalvar" => args
            .first()
            .and_then(|name| variable_value(context, name))
            .unwrap_or_else(|| "0".to_owned()),
        "equal" => bool_text(compare_values(args.first()?, args.get(1)?) == 0),
        "notequal" => bool_text(compare_values(args.first()?, args.get(1)?) != 0),
        "greater" => bool_text(compare_values(args.first()?, args.get(1)?) > 0),
        "greater_equal" | "greaterequal" => {
            bool_text(compare_values(args.first()?, args.get(1)?) >= 0)
        }
        "less" => bool_text(compare_values(args.first()?, args.get(1)?) < 0),
        "less_equal" | "lessequal" => bool_text(compare_values(args.first()?, args.get(1)?) <= 0),
        "and" => bool_text(args.iter().all(|value| truthy(value))),
        "or" => bool_text(args.iter().any(|value| truthy(value))),
        "not" => bool_text(!args.first().is_some_and(|value| truthy(value))),
        "contains" => bool_text(
            args.first()
                .zip(args.get(1))
                .is_some_and(|(value, needle)| value.contains(needle)),
        ),
        "startswith" => bool_text(
            args.first()
                .zip(args.get(1))
                .is_some_and(|(value, prefix)| value.starts_with(prefix)),
        ),
        "roll" => {
            let sides = args.first()?.parse::<u64>().ok()?.max(1);
            (deterministic_number(context.session_seed, source, token, pass) % sides + 1)
                .to_string()
        }
        "pick" => {
            if args.is_empty() {
                String::new()
            } else {
                let index = usize::try_from(
                    deterministic_number(context.session_seed, source, token, pass)
                        % u64::try_from(args.len()).unwrap_or(u64::MAX),
                )
                .unwrap_or(0);
                args[index].to_owned()
            }
        }
        "lastcharmessage" => context
            .messages
            .iter()
            .rev()
            .find(|message| message.role == lorepia_domain::PromptMessageRole::Assistant)
            .map(|message| message.content.clone())
            .unwrap_or_default(),
        "lastmessageid" | "chat_index" => context.messages.len().saturating_sub(1).to_string(),
        "raw" => args.first().copied().unwrap_or_default().to_owned(),
        "slot" => args
            .first()
            .and_then(|name| context.slots.iter().find(|slot| slot.name == *name))
            .map(|slot| slot.value.clone())
            .unwrap_or_default(),
        "?" => evaluate_arithmetic(args.join("::").trim()),
        _ if token.starts_with("? ") => evaluate_arithmetic(token[1..].trim()),
        _ => return None,
    };
    Some(value)
}

fn variable_value(context: &PromptResolutionContext, requested: &str) -> Option<String> {
    let requested = requested.trim();
    let aliases = [
        requested,
        requested.strip_prefix("toggle_").unwrap_or(requested),
    ];
    context.variables.values.iter().find_map(|binding| {
        let id = binding.variable.id.as_str();
        let qualified = binding
            .variable
            .namespace
            .as_ref()
            .map(|namespace| format!("{}.{}", namespace.as_str(), id));
        aliases
            .iter()
            .any(|candidate| *candidate == id || qualified.as_deref() == Some(*candidate))
            .then(|| format_variable(&binding.value))
    })
}

fn format_variable(value: &VariableValue) -> String {
    match value {
        VariableValue::Bool(value) => u8::from(*value).to_string(),
        VariableValue::Integer(value) => value.to_string(),
        VariableValue::Decimal(value) => value.to_string(),
        VariableValue::Text(value) | VariableValue::Enum(value) => value.clone(),
        VariableValue::StringList(values) => values.join(","),
    }
}

fn bool_text(value: bool) -> String {
    u8::from(value).to_string()
}

fn truthy(value: &str) -> bool {
    !matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "0" | "false" | "null" | "none"
    )
}

fn compare_values(left: &str, right: &str) -> i8 {
    match (left.trim().parse::<f64>(), right.trim().parse::<f64>()) {
        (Ok(left), Ok(right)) => {
            if (left - right).abs() < f64::EPSILON {
                0
            } else if left < right {
                -1
            } else {
                1
            }
        }
        _ => match left.trim().cmp(right.trim()) {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        },
    }
}

fn evaluate_arithmetic(expression: &str) -> String {
    let expression = expression.trim().replace(' ', "");
    for (index, character) in expression.char_indices().skip(1) {
        if matches!(character, '+' | '-') {
            let (left, right) = expression.split_at(index);
            let right = &right[character.len_utf8()..];
            if let (Ok(left), Ok(right)) = (left.parse::<i64>(), right.parse::<i64>()) {
                return if character == '+' {
                    left.saturating_add(right)
                } else {
                    left.saturating_sub(right)
                }
                .to_string();
            }
        }
    }
    expression
}

fn deterministic_number(seed: Option<u64>, source: &str, token: &str, pass: usize) -> u64 {
    let mut digest = Sha256::new();
    digest.update(b"portable-character-macro-v1\0");
    digest.update(seed.unwrap_or_default().to_le_bytes());
    digest.update(source.as_bytes());
    digest.update([0]);
    digest.update(token.as_bytes());
    digest.update(pass.to_le_bytes());
    let digest = digest.finalize();
    u64::from_le_bytes(digest[..8].try_into().expect("eight digest bytes"))
}

struct BlockMatch<'a> {
    start: usize,
    body_start: usize,
    body_end: usize,
    else_start: Option<usize>,
    else_end: Option<usize>,
    end: usize,
    token: &'a str,
}

fn innermost_block(source: &str) -> Option<BlockMatch<'_>> {
    let tokens = flat_tokens(source);
    let mut stack = Vec::<(usize, usize, &str, Option<(usize, usize)>)>::new();
    for (start, end, token) in tokens {
        let trimmed = token.trim();
        if trimmed.starts_with("#if") || trimmed.starts_with("#when") {
            stack.push((start, end, token, None));
            continue;
        }
        if trimmed == ":else" {
            if let Some((_, _, _, marker)) = stack.last_mut() {
                *marker = Some((start, end));
            }
            continue;
        }
        if (trimmed == "/" || trimmed == "/if" || trimmed == "/when")
            && let Some((open_start, open_end, open_token, else_marker)) = stack.pop()
        {
            return Some(BlockMatch {
                start: open_start,
                body_start: open_end,
                body_end: start,
                else_start: else_marker.map(|marker| marker.0),
                else_end: else_marker.map(|marker| marker.1),
                end,
                token: open_token,
            });
        }
    }
    None
}

fn flat_tokens(source: &str) -> Vec<(usize, usize, &str)> {
    let mut result = Vec::new();
    let mut cursor = 0;
    while let Some(relative_start) = source[cursor..].find("{{") {
        let start = cursor + relative_start;
        let content_start = start + 2;
        let Some(relative_end) = source[content_start..].find("}}") else {
            break;
        };
        let content_end = content_start + relative_end;
        if !source[content_start..content_end].contains("{{") {
            result.push((start, content_end + 2, &source[content_start..content_end]));
        }
        cursor = content_end + 2;
    }
    result
}

fn evaluate_block_condition(
    token: &str,
    context: &PromptResolutionContext,
    source: &str,
    pass: usize,
) -> bool {
    let token = token.trim();
    if let Some(condition) = token.strip_prefix("#if") {
        return truthy(condition.trim_start_matches("::").trim());
    }
    let Some(condition) = token.strip_prefix("#when") else {
        return false;
    };
    let args = condition
        .trim_start_matches("::")
        .split("::")
        .map(str::trim)
        .collect::<Vec<_>>();
    evaluate_when_expression(&args, context, source, pass)
}

fn evaluate_when_expression(
    args: &[&str],
    context: &PromptResolutionContext,
    _source: &str,
    _pass: usize,
) -> bool {
    // Nested tokens are expanded before block evaluation. Keeping operators
    // verbatim here prevents words such as `not` and `or` from being mistaken
    // for standalone value macros.
    let mut statement = args
        .iter()
        .map(|value| (*value).to_owned())
        .collect::<Vec<_>>();
    if statement.is_empty() {
        return false;
    }
    while statement.len() > 1 {
        let condition = statement.pop().unwrap_or_default();
        let operator = statement.pop().unwrap_or_default();
        let result = match operator.as_str() {
            "not" => bool_text(!when_truthy(&condition)),
            "keep" | "legacy" => condition,
            "and" => bool_text(
                statement.pop().is_some_and(|left| when_truthy(&left)) && when_truthy(&condition),
            ),
            "or" => bool_text(
                statement.pop().is_some_and(|left| when_truthy(&left)) || when_truthy(&condition),
            ),
            "is" | "=" | "==" | "===" => {
                bool_text(statement.pop().is_some_and(|left| left == condition))
            }
            "isnot" | "!=" | "!==" => {
                bool_text(statement.pop().is_none_or(|left| left != condition))
            }
            "var" => bool_text(
                variable_value(context, &condition).is_some_and(|value| when_truthy(&value)),
            ),
            "toggle" => bool_text(
                variable_value(context, &format!("toggle_{condition}"))
                    .is_some_and(|value| when_truthy(&value)),
            ),
            "vis" => bool_text(
                statement
                    .pop()
                    .and_then(|name| variable_value(context, &name))
                    .is_some_and(|value| value == condition),
            ),
            "visnot" => bool_text(
                statement
                    .pop()
                    .and_then(|name| variable_value(context, &name))
                    .is_none_or(|value| value != condition),
            ),
            "tis" => bool_text(
                statement
                    .pop()
                    .and_then(|name| variable_value(context, &format!("toggle_{name}")))
                    .is_some_and(|value| value == condition),
            ),
            "tisnot" => bool_text(
                statement
                    .pop()
                    .and_then(|name| variable_value(context, &format!("toggle_{name}")))
                    .is_none_or(|value| value != condition),
            ),
            ">" | ">=" | "<" | "<=" => {
                let left = statement.pop().unwrap_or_default();
                let comparison = compare_values(&left, &condition);
                bool_text(match operator.as_str() {
                    ">" => comparison > 0,
                    ">=" => comparison >= 0,
                    "<" => comparison < 0,
                    "<=" => comparison <= 0,
                    _ => false,
                })
            }
            _ => bool_text(when_truthy(&condition)),
        };
        statement.push(result);
    }
    statement.first().is_some_and(|value| when_truthy(value))
}

fn when_truthy(value: &str) -> bool {
    matches!(value.trim(), "1" | "true")
}

#[cfg(test)]
mod tests {
    use lorepia_domain::{
        CharacterPromptContent, ConversationBranchId, ConversationId, MessageId,
        PromptConversationMessage, PromptMessageRole, PromptResolutionContext, VariableMap,
    };

    use super::*;

    fn context() -> PromptResolutionContext {
        PromptResolutionContext {
            conversation_id: ConversationId("conversation".into()),
            branch_id: ConversationBranchId("branch".into()),
            character: CharacterPromptContent {
                character_id: "character".into(),
                name: "Guide".into(),
                aliases: Vec::new(),
                description: String::new(),
                personality: String::new(),
                scenario: String::new(),
                first_message: String::new(),
                dialogue_examples: Vec::new(),
                system_instruction: String::new(),
                post_history_instruction: String::new(),
                alternate_greetings: Vec::new(),
                knowledge_book_ids: Vec::new(),
                asset_ids: Vec::new(),
            },
            persona: None,
            user_name: "Player".into(),
            messages: Vec::new(),
            latest_user_message_id: MessageId("latest".into()),
            selected_knowledge: Vec::new(),
            selected_memory: Vec::new(),
            summary_boundaries: Vec::new(),
            conversation_summary: None,
            author_note: None,
            group_context: None,
            variables: VariableMap::default(),
            slots: Vec::new(),
            current_date: "2026-08-28".into(),
            current_time: "00:00:00+09:00".into(),
            supported_capabilities: Vec::new(),
            session_seed: Some(7),
            context_snapshot: None,
        }
    }

    #[test]
    fn renders_nested_condition_and_builtin_names() {
        let rendered = render_portable_text(
            "{{#if {{equal::{{getvar::missing}}::0}}}}Hi {{user}} from {{char}}{{/}}",
            &context(),
        );
        assert_eq!(rendered, "Hi Player from Guide");
    }

    #[test]
    fn deterministic_roll_blocks_are_stable() {
        let source = "{{#when::{{roll::1}}::<=::1}}event{{/when}}";
        assert_eq!(render_portable_text(source, &context()), "event");
        assert_eq!(render_portable_text(source, &context()), "event");
    }

    #[test]
    fn compound_when_operators_and_else_follow_right_to_left_evaluation() {
        assert_eq!(
            render_portable_text("{{#when::1::and::1}}yes{{:else}}no{{/when}}", &context(),),
            "yes"
        );
        assert_eq!(
            render_portable_text(
                "{{#when::0::or::not::true}}yes{{:else}}no{{/when}}",
                &context(),
            ),
            "no"
        );
    }

    #[test]
    fn compound_event_condition_can_read_the_previous_character_message() {
        let mut context = context();
        context.messages.push(PromptConversationMessage {
            id: MessageId("assistant".into()),
            branch_id: context.branch_id.clone(),
            role: PromptMessageRole::Assistant,
            content: "Scene: Combat".to_owned(),
            turn_index: 0,
        });
        assert_eq!(
            render_portable_text(
                "{{#when::{{contains::{{lastcharmessage}}::Scene: Combat}}::and::{{lessequal::{{roll::1}}::3}}}}event{{/when}}",
                &context,
            ),
            "event"
        );
    }

    #[test]
    fn unknown_image_prompt_braces_are_preserved() {
        assert_eq!(
            render_portable_text("solo, {{muscular female}}", &context()),
            "solo, {{muscular female}}"
        );
    }
}
