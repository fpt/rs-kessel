// swift-tools-version: 6.1
import PackageDescription

let package = Package(
    name: "Kessel",
    platforms: [.macOS("26.0")],
    products: [
        .executable(name: "kessel-cli", targets: ["KesselCli"]),
    ],
    dependencies: [
        // YAML parsing
        .package(url: "https://github.com/jpsim/Yams.git", from: "5.0.0"),
    ],
    targets: [
        .target(
            name: "AgentKit",
            dependencies: ["Util", "AgentBridge", "TTS", "Watcher", "ScreenCapture"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .executableTarget(
            name: "KesselCli",
            dependencies: [
                "AgentKit",
                "Util",
                "AgentBridge",
                "CEditline",
                "Audio",
                "ScreenCapture",
            ],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "Util",
            dependencies: ["Yams"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "TTS",
            dependencies: ["Util"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "Audio",
            dependencies: [
                "Util",
            ],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "Watcher",
            dependencies: ["Util"],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .target(
            name: "ScreenCapture",
            dependencies: [],
            swiftSettings: [.swiftLanguageMode(.v5)]
        ),
        .systemLibrary(
            name: "CEditline",
            path: "Sources/CEditline"
        ),
        .systemLibrary(
            name: "AgentBridgeFFI",
            path: "Sources/AgentBridgeFFI",
            pkgConfig: nil,
            providers: nil
        ),
        .target(
            name: "AgentBridge",
            dependencies: ["AgentBridgeFFI"],
            swiftSettings: [
                .swiftLanguageMode(.v5),
            ],
            linkerSettings: [
                .unsafeFlags([
                    "-L../crates/target/release",
                    "-lkessel_core",
                ])
            ]
        ),
    ]
)
