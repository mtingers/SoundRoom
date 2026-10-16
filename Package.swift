// swift-tools-version: 5.9
import PackageDescription

let package = Package(name: "Soundroom", platforms: [.macOS(.v14)], products: [
    .executable(name: "Soundroom", targets: ["Soundroom"]),
    .library(name: "SoundroomCore", targets: ["SoundroomCore"]),
    .library(name: "SoundroomEngineControl", targets: ["SoundroomEngineControl"]),
    .library(name: "SoundroomRemote", targets: ["SoundroomRemote"]),
    .executable(name: "SoundroomTUI", targets: ["SoundroomTUI"]),
    .executable(name: "DrumPackGenerator", targets: ["DrumPackGenerator"]),
], targets: [
    .target(name: "SoundroomCore"),
    .target(name: "SoundroomEngineControl", dependencies: ["SoundroomCore", "SoundroomRemote"]),
    .target(name: "SoundroomRemote", dependencies: ["SoundroomCore"]),
    .executableTarget(name: "SoundroomTUI", dependencies: ["SoundroomCore", "SoundroomRemote"]),
    .executableTarget(name: "DrumPackGenerator", dependencies: ["SoundroomCore"]),
    .executableTarget(name: "Soundroom", dependencies: ["SoundroomCore", "SoundroomRemote", "SoundroomEngineControl"]),
    .testTarget(name: "SoundroomCoreTests", dependencies: ["SoundroomCore"]),
    .testTarget(name: "SoundroomEngineControlTests", dependencies: ["SoundroomEngineControl", "SoundroomRemote"]),
    .testTarget(name: "SoundroomIntegrationTests", dependencies: ["Soundroom", "SoundroomCore", "SoundroomRemote"]),
    .testTarget(name: "SoundroomRemoteTests", dependencies: ["SoundroomRemote", "SoundroomCore"]),
    .testTarget(name: "SoundroomTUITests", dependencies: ["SoundroomTUI", "SoundroomRemote", "SoundroomCore"])
])
