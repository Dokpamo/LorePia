// swift-tools-version:5.9

import PackageDescription

let package = Package(
    name: "tauri-plugin-lorepia-platform",
    platforms: [
        .iOS(.v17),
    ],
    products: [
        .library(
            name: "tauri-plugin-lorepia-platform",
            type: .static,
            targets: ["LorepiaPlatformPlugin"]
        ),
    ],
    dependencies: [
        .package(name: "Tauri", path: "../.tauri/tauri-api"),
    ],
    targets: [
        .target(
            name: "LorepiaPlatformPlugin",
            dependencies: [
                .byName(name: "Tauri"),
            ],
            path: "Sources"
        ),
        .testTarget(
            name: "LorepiaPlatformPluginTests",
            dependencies: ["LorepiaPlatformPlugin"],
            path: "Tests"
        ),
    ]
)
