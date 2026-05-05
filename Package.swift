// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "Aura",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Aura", targets: ["AuraApp"]),
    ],
    targets: [
        // Library: everything testable lives here.
        .target(
            name: "Aura",
            path: "Sources/Aura",
            resources: [.process("../../Resources")],
            swiftSettings: [
                .enableExperimentalFeature("StrictConcurrency"),
            ]
        ),
        // Thin app launcher (main.swift wires AppCoordinator + UI).
        .executableTarget(
            name: "AuraApp",
            dependencies: ["Aura"],
            path: "Sources/AuraApp"
        ),
        .testTarget(
            name: "AuraTests",
            dependencies: ["Aura"],
            path: "Tests/AuraTests",
            // Plan 4 fixtures live here so Bundle.module finds them.
            resources: [.copy("Fixtures")]
        ),
    ]
)
