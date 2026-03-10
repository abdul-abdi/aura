// swift-tools-version: 5.9
import PackageDescription

let package = Package(
    name: "AuraApp",
    platforms: [
        .macOS(.v14),
    ],
    targets: [
        .executableTarget(
            name: "AuraApp",
            path: "Sources"
        ),
    ]
)
