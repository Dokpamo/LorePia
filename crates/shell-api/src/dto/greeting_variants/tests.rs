use super::classify;
use crate::dto::greeting_preview::CharacterGreetingPreviewDto;

fn project(texts: &[String]) -> Vec<CharacterGreetingPreviewDto> {
    let ids: Vec<_> = (0..texts.len()).map(|i| format!("alternate-{i}")).collect();
    let sources: Vec<_> = ids
        .iter()
        .zip(texts)
        .map(|(id, text)| (id.as_str(), text.as_str()))
        .collect();
    let mut previews: Vec<_> = ids
        .iter()
        .cloned()
        .map(|id| CharacterGreetingPreviewDto {
            id,
            title: None,
            excerpt: String::new(),
            language: None,
            group_id: None,
        })
        .collect();
    classify(&sources, &mut previews);
    previews
}
fn ko(media: &str) -> String {
    format!(
        "{}\n{media}",
        "조용한 역에서 우리는 다음 열차가 도착하기를 기다리고 있었다. ".repeat(8)
    )
}
fn en(media: &str) -> String {
    format!(
        "{}\n{media}",
        "We waited together at the quiet station for the next train to arrive. ".repeat(8)
    )
}

#[test]
fn languages_group_only_unique_matching_scenes_in_stable_source_order() {
    let a = "<img=\"station_wide\">\n<img=\"station_gate\">";
    let b = "<img=\"library_wide\">\n<img=\"library_door\">";
    let result = project(&[ko(a), ko(b), en(a), en(b), "Start".into()]);
    assert_eq!(result[0].language.as_deref(), Some("ko"));
    assert_eq!(result[2].language.as_deref(), Some("en"));
    assert_eq!(result[0].group_id, result[2].group_id);
    assert_eq!(result[0].group_id.as_deref(), Some("alternate-0"));
    assert_eq!(result[1].group_id.as_deref(), Some("alternate-1"));
    assert_eq!(result[4].language, None);
    assert_eq!(result[4].group_id, None);
}

#[test]
fn extra_expression_or_illustration_does_not_split_a_unique_variant() {
    let result = project(&[
        ko("<img=\"figure_smile\"><img=\"figure_neutral\">"),
        en("<img=\"figure_smile\"><img=\"figure_curious\"><img=\"figure_neutral\">"),
    ]);
    assert!(
        result
            .iter()
            .all(|item| item.group_id.as_deref() == Some("alternate-0"))
    );
    let note = format!(
        "[Scene chronology] {}\n\n",
        "The scene begins on the first morning before the city closes. ".repeat(2)
    );
    let result = project(&[
        format!("{note}{}", ko("<img=\"illustration1\">")),
        format!(
            "{}{}",
            note.replace(". \n\n", "\n\n"),
            en("<img=\"illustration2\">")
        ),
    ]);
    assert!(
        result
            .iter()
            .all(|item| item.group_id.as_deref() == Some("alternate-0"))
    );
}

#[test]
fn unrelated_or_ambiguous_scenes_and_same_languages_stay_separate() {
    let media = "<img=\"person_smile\">";
    for texts in [
        vec![ko(media), en("<img=\"other\">")],
        vec![ko(media), ko(media)],
        vec![ko(media), en(media), en(media)],
        vec![ko(""), en("")],
    ] {
        assert!(project(&texts).iter().all(|item| item.group_id.is_none()));
    }
}
