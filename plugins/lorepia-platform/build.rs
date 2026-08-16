const COMMANDS: &[&str] = &[];

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").expect("Cargo sets target OS");
    println!("cargo:rerun-if-env-changed=IPHONEOS_DEPLOYMENT_TARGET");
    if target_os == "ios" && std::env::var_os("IPHONEOS_DEPLOYMENT_TARGET").is_none() {
        // SAFETY: this build script has not spawned threads. The variable is
        // set before tauri-plugin invokes swift-rs so bare Cargo cross-checks
        // use the same iOS 17 floor as the app configuration.
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("IPHONEOS_DEPLOYMENT_TARGET", "17.0");
        }
    }

    tauri_plugin::Builder::new(COMMANDS)
        .android_path("android")
        .ios_path("ios")
        .build();

    let mobile = matches!(target_os.as_str(), "android" | "ios");
    cfg_alias("mobile", mobile);
    cfg_alias("desktop", !mobile);
}

fn cfg_alias(alias: &str, enabled: bool) {
    println!("cargo:rustc-check-cfg=cfg({alias})");
    if enabled {
        println!("cargo:rustc-cfg={alias}");
    }
}
