// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "AuraV3",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "AuraV3", targets: ["AuraV3App"]),
        .executable(name: "aura", targets: ["AuraCLI"]),
    ],
    targets: [
        // Library: everything testable lives here.
        .target(
            name: "AuraV3",
            path: "Sources/AuraV3",
            resources: [.process("../../Resources")],
            swiftSettings: [
                .enableExperimentalFeature("StrictConcurrency"),
            ]
        ),
        // Thin app launcher (just main.swift wiring AppCoordinator).
        .executableTarget(
            name: "AuraV3App",
            dependencies: ["AuraV3"],
            path: "Sources/AuraV3App"
        ),
        // CLI target (Plan 4 fills its source files).
        .executableTarget(
            name: "AuraCLI",
            dependencies: ["AuraV3"],
            path: "Sources/aura"
        ),
        .testTarget(
            name: "AuraV3Tests",
            dependencies: ["AuraV3"],
            path: "Tests/AuraV3Tests",
            // Plan 4 fixtures live here so Bundle.module finds them.
            resources: [.copy("Fixtures")]
        ),
    ]
)
