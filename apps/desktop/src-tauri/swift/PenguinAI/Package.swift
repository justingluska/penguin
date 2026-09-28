// swift-tools-version:5.9
// Penguin's bridge to Apple's on-device Foundation Models framework (thread
// summaries, docs/SUMMARIES.md). Built by src-tauri/build.rs through swift-rs
// as a static library, macOS only, and called from Rust through the C
// functions in Sources/PenguinAI/PenguinAI.swift (src/summary/apple.rs).
//
// Swift 5 language mode on purpose: the bridge hands C pointers to tasks,
// which Swift 6's strict concurrency checking would reject without adding
// any safety here (the Rust side owns those pointers; see the comments).
import PackageDescription

let package = Package(
    name: "PenguinAI",
    // Below macOS 26 on purpose: the app still launches on older macOS, where
    // FoundationModels is absent. Every use of it is behind
    // `#available(macOS 26.0, *)`, and build.rs weak-links the framework.
    platforms: [.macOS(.v10_15)],
    products: [
        .library(name: "PenguinAI", type: .static, targets: ["PenguinAI"]),
    ],
    targets: [
        .target(name: "PenguinAI", path: "Sources/PenguinAI"),
    ]
)
