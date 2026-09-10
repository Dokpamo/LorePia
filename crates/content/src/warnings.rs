use lorepia_domain::ImportWarning;

use crate::adapters;

pub(crate) fn promoted_card(metadata: &adapters::CardMetadata) -> Vec<ImportWarning> {
    if !metadata.promoted_from_v2 {
        return Vec::new();
    }
    vec![ImportWarning {
        code: "character_card_v2_promoted".to_owned(),
        message: "Card declares the V2 specification and was promoted to V3. \
                  Fields that only V3 defines are empty."
            .to_owned(),
    }]
}

pub(crate) fn extension_mismatch(extension: &str, detected: &str) -> Vec<ImportWarning> {
    let expected = match detected {
        "JSON" => extension == "json",
        "PNG" => extension == "png",
        _ => matches!(extension, "charx" | "zip"),
    };
    if expected {
        return Vec::new();
    }
    let actual = if extension.is_empty() {
        "no extension".to_owned()
    } else {
        format!(".{extension}")
    };
    vec![ImportWarning {
        code: "extension_mismatch".to_owned(),
        message: format!("File contents are {detected}, but the file has {actual}."),
    }]
}
