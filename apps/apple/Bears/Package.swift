// swift-tools-version: 5.10
import PackageDescription

let strictConcurrency: [SwiftSetting] = [
    .enableExperimentalFeature("StrictConcurrency"),
    .unsafeFlags(["-strict-concurrency=complete"])
]

var products: [Product] = [
    .library(name: "BearsClient", targets: ["BearsClient"])
]
var targets: [Target] = [
    .target(name: "BearsClient", path: "Shared/Client", swiftSettings: strictConcurrency),
    .testTarget(name: "BearsClientTests", dependencies: ["BearsClient"],
                path: "Tests/BearsClientTests", swiftSettings: strictConcurrency)
]

#if os(macOS)
products.append(.executable(name: "Bears", targets: ["BearsApp"]))
targets.append(.executableTarget(
    name: "BearsApp",
    dependencies: ["BearsClient"],
    path: "BearsApp",
    exclude: ["README.md"],
    resources: [.copy("Resources/Adapter")],
    swiftSettings: strictConcurrency
))
targets.append(.testTarget(
    name: "BearsAppTests",
    dependencies: ["BearsApp", "BearsClient"],
    path: "Tests/BearsAppTests",
    swiftSettings: strictConcurrency
))
#endif

let package = Package(
    name: "Bears",
    platforms: [.macOS(.v13), .iOS(.v16)],
    products: products,
    targets: targets
)
