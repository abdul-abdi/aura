// swift-tools-version:5.10
import PackageDescription

let package = Package(
    name: "Aura",
    platforms: [.macOS(.v14)],
    products: [
        .executable(name: "Aura", targets: ["AuraApp"]),
        // Spec §14.7 — `aura support-bundle` diagnostics CLI. Empty placeholder
        // in Plan 1; Plan 4's Phase F fills in `Sources/AuraCLI/main.swift`.
        // Product is `aura-support` (not `aura`) to dodge case-insensitive-FS
        // collision with the `Aura` app binary on default APFS volumes —
        // Plan 4 may rename or split into a sub-package to honor the literal
        // `aura support-bundle` command.
        .executable(name: "aura-support", targets: ["AuraCLI"]),
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
        // CLI placeholder for Plan 4. Path uses `Sources/AuraCLI` (not
        // `Sources/aura`) — see comment on the `aura-support` product above.
        .executableTarget(
            name: "AuraCLI",
            dependencies: ["Aura"],
            path: "Sources/AuraCLI"
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
