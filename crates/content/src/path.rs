use unicode_normalization::UnicodeNormalization;

pub(crate) fn validate_archive_path(name: &str) -> Result<String, &'static str> {
    if name.is_empty() {
        return Err("archive entry has an empty path");
    }
    if name.chars().any(char::is_control) {
        return Err("archive entry path contains a control character");
    }
    if name.starts_with('/') || name.starts_with('\\') {
        return Err("archive entry uses an absolute path");
    }

    let normalized_slashes = name.replace('\\', "/");
    let is_directory = normalized_slashes.ends_with('/');
    let components = normalized_slashes.split('/').collect::<Vec<_>>();
    let mut normalized_components = Vec::new();
    for (index, &component) in components.iter().enumerate() {
        if component.is_empty() {
            if is_directory && index + 1 == components.len() {
                continue;
            }
            return Err("archive entry path contains an empty component");
        }
        if matches!(component, "." | "..") {
            return Err("archive entry attempts path traversal");
        }
        if component.contains(':') {
            return Err("archive entry path contains a Windows drive or stream separator");
        }
        if component.ends_with([' ', '.']) {
            return Err("archive entry path has a platform-ambiguous suffix");
        }

        let collision_component = component.nfkc().collect::<String>().to_lowercase();
        if is_windows_reserved_name(&collision_component) {
            return Err("archive entry uses a reserved Windows device name");
        }
        normalized_components.push(collision_component);
    }
    if normalized_components.is_empty() {
        return Err("archive entry has an empty normalized path");
    }

    Ok(normalized_components.join("/"))
}

fn is_windows_reserved_name(component: &str) -> bool {
    let base = component.split('.').next().unwrap_or(component);
    matches!(base, "con" | "prn" | "aux" | "nul")
        || base
            .strip_prefix("com")
            .or_else(|| base.strip_prefix("lpt"))
            .is_some_and(|suffix| {
                suffix.len() == 1 && matches!(suffix.as_bytes().first(), Some(b'1'..=b'9'))
            })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_traversal_and_absolute_paths() {
        assert!(validate_archive_path("../secret").is_err());
        assert!(validate_archive_path("/etc/passwd").is_err());
        assert!(validate_archive_path(r"C:\secret").is_err());
        assert!(validate_archive_path(r"folder\..\secret").is_err());
        assert!(validate_archive_path("folder/./card.json").is_err());
        assert!(validate_archive_path("folder//card.json").is_err());
    }

    #[test]
    fn normalizes_case_for_collision_detection() {
        assert_eq!(
            validate_archive_path("Assets/Hero.PNG").expect("valid"),
            "assets/hero.png"
        );
        assert_eq!(
            validate_archive_path("Assets/").expect("valid directory"),
            "assets"
        );
    }

    #[test]
    fn rejects_cross_platform_ambiguous_paths() {
        assert!(validate_archive_path("card.json:stream").is_err());
        assert!(validate_archive_path("folder./card.json").is_err());
        assert!(validate_archive_path("CON/avatar.png").is_err());
        assert!(validate_archive_path("lpt1.txt").is_err());
    }
}
