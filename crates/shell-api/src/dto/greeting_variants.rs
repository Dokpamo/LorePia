use super::greeting_preview::CharacterGreetingPreviewDto;

struct Anchors {
    language: Option<&'static str>,
    media: Vec<String>,
    preamble: Option<String>,
}

// Bounded, deterministic display classification. Ambiguous matches stay separate;
// this does not assert authored translation metadata or alter source/selector IDs.
pub(super) fn classify(sources: &[(&str, &str)], previews: &mut [CharacterGreetingPreviewDto]) {
    let anchors: Vec<_> = sources.iter().map(|(_, source)| anchors(source)).collect();
    let best: Vec<Option<usize>> = anchors
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let scores: Vec<_> = anchors
                .iter()
                .enumerate()
                .filter_map(|(j, b)| {
                    (i != j
                        && a.language.is_some()
                        && b.language.is_some()
                        && a.language != b.language)
                        .then(|| (j, similarity(a, b)))
                        .filter(|(_, score)| *score > 0)
                })
                .collect();
            let highest = scores.iter().map(|(_, score)| *score).max()?;
            let mut matches = scores.iter().filter(|(_, score)| *score == highest);
            let first = matches.next()?.0;
            matches.next().is_none().then_some(first)
        })
        .collect();
    for (i, preview) in previews.iter_mut().enumerate() {
        preview.language = anchors[i].language.map(str::to_owned);
        preview.group_id = best[i]
            .filter(|j| best[*j] == Some(i))
            .map(|j| sources[i.min(j)].0.to_owned());
    }
}

fn similarity(a: &Anchors, b: &Anchors) -> u8 {
    if a.preamble.is_some() && a.preamble == b.preamble {
        return 3;
    }
    if !a.media.is_empty() && a.media == b.media {
        return 2;
    }
    // Preserve sequence while allowing an illustration or expression to be added.
    let common: Vec<_> = a.media.iter().filter(|id| b.media.contains(id)).collect();
    let reverse: Vec<_> = b.media.iter().filter(|id| a.media.contains(id)).collect();
    let unique: std::collections::BTreeSet<_> = common.iter().collect();
    u8::from(
        unique.len() >= 2
            && common == reverse
            && common.len() * 3 >= a.media.len().max(b.media.len()) * 2,
    )
}

fn anchors(source: &str) -> Anchors {
    let bounded: String = source.chars().take(8_192).collect();
    let text = bounded.replace("\r\n", "\n");
    let preamble = text
        .trim_start()
        .split_once("\n\n")
        .map(|(first, _)| first.trim())
        .filter(|first| {
            first.starts_with('[') && first.contains(']') && (80..=1024).contains(&first.len())
        })
        .map(|first| {
            first
                .trim_end_matches(['.', '!', '?'])
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
        });
    let body = if preamble.is_some() {
        text.split_once("\n\n")
            .map_or(text.as_str(), |(_, rest)| rest)
    } else {
        &text
    };
    let mut media = Vec::new();
    let mut plain = String::new();
    let mut rest = body;
    // Only literal portable image aliases. No HTML evaluation, network or scripts.
    while let Some(start) = rest.find("<img=\"") {
        plain.push_str(&rest[..start]);
        let tail = &rest[start + 6..];
        let Some(end) = tail.find("\">") else {
            break;
        };
        let id = &tail[..end];
        if !id.is_empty() && id.len() <= 160 && media.len() < 64 {
            media.push(id.to_owned());
        }
        rest = &tail[end + 2..];
    }
    plain.push_str(rest);
    let mut hangul = 0;
    let mut latin = 0;
    let mut kana = 0;
    let mut han = 0;
    for ch in plain.chars() {
        match ch as u32 {
            0xac00..=0xd7a3 => hangul += 1,
            0x3040..=0x30ff => kana += 1,
            0x4e00..=0x9fff => han += 1,
            _ if ch.is_ascii_alphabetic() => latin += 1,
            _ => (),
        }
    }
    let language = if hangul >= 24 && hangul * 4 > latin {
        Some("ko")
    } else if kana >= 16 {
        Some("ja")
    } else if han >= 24 {
        Some("zh")
    } else if latin >= 40 {
        Some("en")
    } else {
        None
    };
    Anchors {
        language,
        media,
        preamble,
    }
}

#[cfg(test)]
mod tests;
