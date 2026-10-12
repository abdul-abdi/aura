// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "Aura",
    platforms: [.macOS(.v14)],
    targets: [
        .executableTarget(name: "Aura", path: "Sources/Aura"),
        .testTarget(name: "AuraTests", dependencies: ["Aura"], path: "Tests/AuraTests"),
    ]
)
