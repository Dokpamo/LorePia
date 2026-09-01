//! Renderer-selected appearance projected onto native system chrome.

use serde::Deserialize;
use tauri::AppHandle;
#[cfg(target_os = "android")]
use tauri_plugin_lorepia_platform::LorepiaPlatformExt;

use crate::error::CommandResult;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SystemBarStyleRequest {
    dark: bool,
}

#[tauri::command]
pub async fn set_system_bar_style(
    app: AppHandle,
    request: SystemBarStyleRequest,
) -> CommandResult<()> {
    #[cfg(target_os = "android")]
    app.lorepia_platform()
        .set_system_bar_style(request.dark)
        .await?;

    #[cfg(not(target_os = "android"))]
    let _ = (app, request);

    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::SystemBarStyleRequest;

    const COMMAND_SOURCE: &str = include_str!("appearance_commands.rs");
    const INVOKE_REGISTRY_SOURCE: &str = include_str!("lib.rs");
    const APP_COMMANDS: &str = include_str!("../generated/app_commands.rs");
    const DEVELOPMENT_CAPABILITY: &str = include_str!("../capabilities/main-development.json");
    const RELEASE_CAPABILITY: &str = include_str!("../capabilities/main-release.json");
    const PLATFORM_PLUGIN_BUILD_SOURCE: &str =
        include_str!("../../../../plugins/lorepia-platform/build.rs");
    const ANDROID_PLUGIN_SOURCE: &str = include_str!(
        "../../../../plugins/lorepia-platform/android/src/main/java/dev/lorepia/tauri/platform/LorepiaPlatformPlugin.kt"
    );

    #[test]
    fn request_is_one_strict_renderer_owned_appearance_bit() {
        let light: SystemBarStyleRequest =
            serde_json::from_value(json!({"dark": false})).expect("light system bar style");
        let dark: SystemBarStyleRequest =
            serde_json::from_value(json!({"dark": true})).expect("dark system bar style");
        assert!(!light.dark);
        assert!(dark.dark);
        assert!(serde_json::from_value::<SystemBarStyleRequest>(json!({})).is_err());
        assert!(
            serde_json::from_value::<SystemBarStyleRequest>(json!({
                "dark": true,
                "status_bar_color": "#000000"
            }))
            .is_err()
        );
    }

    #[test]
    fn route_is_allowlisted_once_and_native_plugin_stays_rust_only() {
        assert_eq!(
            INVOKE_REGISTRY_SOURCE
                .matches("appearance_commands::set_system_bar_style")
                .count(),
            1
        );
        assert_eq!(APP_COMMANDS.matches("\"set_system_bar_style\"").count(), 1);
        assert!(COMMAND_SOURCE.contains(".set_system_bar_style(request.dark)"));
        assert!(PLATFORM_PLUGIN_BUILD_SOURCE.contains("const COMMANDS: &[&str] = &[];"));
        assert!(ANDROID_PLUGIN_SOURCE.contains("fun setSystemBarStyle(invoke: Invoke)"));
        assert!(
            ANDROID_PLUGIN_SOURCE.contains("WindowInsetsController.APPEARANCE_LIGHT_STATUS_BARS")
        );

        for (kind, source) in [
            ("development", DEVELOPMENT_CAPABILITY),
            ("release", RELEASE_CAPABILITY),
        ] {
            let capability: Value =
                serde_json::from_str(source).unwrap_or_else(|error| panic!("{kind}: {error}"));
            assert_eq!(
                capability["permissions"]
                    .as_array()
                    .expect("permission array")
                    .iter()
                    .filter(|permission| {
                        permission.as_str() == Some("allow-set-system-bar-style")
                    })
                    .count(),
                1,
                "{kind} capability"
            );
        }
    }
}
