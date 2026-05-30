// swift-tools-version: 5.10
import PackageDescription

let package = Package(
    name: "Bears",
    platforms: [
        .macOS(.v13)
    ],
    products: [
        .executable(
            name: "Bears",
            targets: ["BearsApp"]
        )
    ],
    targets: [
        .executableTarget(
            name: "BearsApp",
            path: "BearsApp",
            resources: [
                .copy("../Resources/Adapter/bears-acp-adapter")
            ],
            swiftSettings: [
                .enableExperimentalFeature("StrictConcurrency")
            ]
        )
    ]
)
