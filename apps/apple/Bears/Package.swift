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
            exclude: [
                "README.md"
            ],
            resources: [
                .copy("Resources/Adapter")
            ],
            swiftSettings: [
                .enableExperimentalFeature("StrictConcurrency")
            ]
        )
    ]
)
