use lorepia_core::CharacterContentV1;
use serde::{Deserialize, Serialize};

const MAX_SOURCE_CHARS: usize = 8_192;
const MAX_EXCERPT_CHARS: usize = 320;
const MAX_TITLE_CHARS: usize = 96;

/// Inert, bounded reading aid, not a greeting selector or conversation input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterGreetingPreviewDto {
    pub id: String,
    /// Only an explicit leading Markdown heading; never an invented scene name.
    pub title: Option<String>,
    pub excerpt: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub group_id: Option<String>,
}

pub(super) fn greeting_previews(content: &CharacterContentV1) -> Vec<CharacterGreetingPreviewDto> {
    let sources: Vec<_> = std::iter::once(("default".to_owned(), content.first_message.as_str()))
        .chain(
            content
                .alternate_greetings
                .iter()
                .take(128)
                .enumerate()
                .map(|(index, text)| (format!("alternate-{index}"), text.as_str())),
        )
        .filter(|(_, text)| !text.is_empty())
        .collect();
    let mut previews: Vec<_> = sources
        .iter()
        .map(|(id, text)| preview(id.clone(), text))
        .collect();
    let borrowed: Vec<_> = sources
        .iter()
        .map(|(id, text)| (id.as_str(), *text))
        .collect();
    super::greeting_variants::classify(&borrowed, &mut previews);
    previews
}

fn preview(id: String, source: &str) -> CharacterGreetingPreviewDto {
    let bounded: String = source.chars().take(MAX_SOURCE_CHARS).collect();
    let normalized = bounded.replace("\r\n", "\n");
    // Skip standalone media markup, but do not interpret or execute any markup,
    // card macros, display transforms or scripts. The renderer uses text nodes.
    let mut lines = excerpt_start(&normalized)
        .lines()
        .map(str::trim)
        .filter(|line| !(line.is_empty() || line.starts_with('<') && line.ends_with('>')));
    let first = lines.next().unwrap_or_default();
    let title = heading(first).map(str::to_owned);
    let excerpt = if title.is_some() {
        lines.collect::<Vec<_>>().join("\n\n")
    } else {
        std::iter::once(first)
            .chain(lines)
            .collect::<Vec<_>>()
            .join("\n\n")
    };
    CharacterGreetingPreviewDto {
        id,
        title,
        excerpt: truncate(&excerpt, MAX_EXCERPT_CHARS),
        language: None,
        group_id: None,
    }
}

fn excerpt_start(source: &str) -> &str {
    let trimmed = source.trim_start();
    // A leading bracketed annotation is a display preamble, not a scene title.
    // Prefer a following paragraph for the reading aid only. Do not evaluate the
    // annotation, discard the actual greeting, or mistake Markdown links for it.
    if let Some((preamble, body)) = trimmed.split_once("\n\n")
        && preamble.starts_with('[')
        && let Some((label, suffix)) = preamble.split_once(']')
        && label.chars().count() <= 128
        && !suffix.starts_with('(')
        && !body.trim().is_empty()
    {
        return body;
    }
    source
}

fn heading(line: &str) -> Option<&str> {
    let text = line.trim_start_matches('#');
    let level = line.len() - text.len();
    if !(1..=6).contains(&level) || !text.starts_with(' ') {
        return None;
    }
    let text = text.trim().trim_end_matches('#').trim_end();
    (!text.is_empty() && text.chars().count() <= MAX_TITLE_CHARS).then_some(text)
}

fn truncate(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    let mut shortened: String = text.chars().take(limit - 1).collect();
    shortened.push('…');
    shortened
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_display_text_without_renumbering_omitted_greetings() {
        let content = CharacterContentV1 {
            first_message: "Start".into(),
            alternate_greetings: vec![String::new(), "# The station\n\nA train arrives.".into()],
            ..CharacterContentV1::default()
        };
        let previews = greeting_previews(&content);
        assert_eq!(previews.len(), 2);
        assert_eq!(previews[0].id, "default");
        assert_eq!(previews[0].title, None);
        assert_eq!(previews[0].excerpt, "Start");
        assert_eq!(previews[1].id, "alternate-1");
        assert_eq!(previews[1].title.as_deref(), Some("The station"));
        assert_eq!(previews[1].excerpt, "A train arrives.");
    }

    #[test]
    fn bounds_unicode_and_does_not_invent_titles_or_execute_card_markup() {
        let body = "봄".repeat(5_000);
        let result = preview("alternate-0".into(), &format!("<img=\"cover\">\n\n{body}"));
        assert_eq!(result.title, None);
        assert_eq!(result.excerpt.chars().count(), MAX_EXCERPT_CHARS);
        assert!(result.excerpt.ends_with('…'));
        let literal = preview("default".into(), "Hello <img onerror=alert(1)> {{user}}");
        assert_eq!(literal.excerpt, "Hello <img onerror=alert(1)> {{user}}");
        assert!(greeting_previews(&CharacterContentV1::default()).is_empty());
    }

    #[test]
    fn serializes_preview_metadata_separately_and_accepts_older_profile_responses() {
        let content = CharacterContentV1 {
            alternate_greetings: vec!["# A supplied heading\n\nAn opening.".into(); 129],
            ..CharacterContentV1::default()
        };
        let profile = crate::CharacterRenderProfileDto::from_content(
            "character-1".into(),
            Some("revision-1".into()),
            content,
        );
        let mut wire = serde_json::to_value(profile).expect("profile JSON");
        assert_eq!(
            wire["greeting_previews"]
                .as_array()
                .expect("preview list")
                .len(),
            128
        );
        assert_eq!(wire["greeting_previews"][0]["title"], "A supplied heading");
        wire.as_object_mut()
            .expect("profile object")
            .remove("greeting_previews");
        let older: crate::CharacterRenderProfileDto =
            serde_json::from_value(wire).expect("older response");
        assert!(older.greeting_previews.is_empty());
    }

    #[test]
    fn previews_a_scene_after_a_leading_annotation_without_treating_it_as_a_title() {
        let annotated =
            "[Scene note] Use the earlier date.\r\n\r\n<img=\"cover\">\r\n\r\nA train arrives.";
        let result = preview("alternate-0".into(), annotated);
        assert_eq!(result.title, None);
        assert_eq!(result.excerpt, "A train arrives.");
        let only_note = "[Scene note] Use the earlier date.";
        assert_eq!(preview("default".into(), only_note).excerpt, only_note);
        let link = "[A station](https://example.com)\n\nA train arrives.";
        assert!(
            preview("default".into(), link)
                .excerpt
                .starts_with("[A station]")
        );
    }
}
