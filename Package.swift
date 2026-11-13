// swift-tools-version: 6.0
import PackageDescription
import Foundation

let videoFrameworkDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
  .appendingPathComponent(".build/video-dependencies/VLCKit - binary package/VLCKit.xcframework/macos-arm64_x86_64").path

let package = Package(
  name: "Crawler",
  platforms: [.macOS(.v14)],
  products: [.executable(name: "Crawler", targets: ["Crawler"])],
  dependencies: [
    .package(url: "https://github.com/tree-sitter/swift-tree-sitter", exact: "0.25.0"),
    .package(url: "https://github.com/alex-pinkus/tree-sitter-swift", revision: "31d17fe7e818a2048c808b5c6fdc2dc792f4f5b5"),
    .package(url: "https://github.com/tree-sitter/tree-sitter-javascript", exact: "0.23.1"),
    .package(url: "https://github.com/tree-sitter/tree-sitter-typescript", exact: "0.23.2"),
    .package(url: "https://github.com/tree-sitter/tree-sitter-go", exact: "0.25.0"),
    .package(url: "https://github.com/tree-sitter/tree-sitter-python", exact: "0.23.6"),
    .package(url: "https://github.com/tree-sitter/tree-sitter-json", exact: "0.24.8"),
    .package(url: "https://github.com/tree-sitter-grammars/tree-sitter-yaml", exact: "0.7.0"),
    .package(url: "https://github.com/tree-sitter/tree-sitter-bash", exact: "0.25.1"),
    .package(url: "https://github.com/DerekStride/tree-sitter-sql", revision: "39fdb006403747241244326e8af3b3e96b85381c"),
    .package(url: "https://github.com/swiftlang/swift-markdown", exact: "0.9.0"),
  ],
  targets: [
    .target(name: "FFFBridge", exclude: ["engine.rs"], linkerSettings: [
      .unsafeFlags(["-L", URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        .appendingPathComponent(".build/fff/lib").path]),
      .linkedLibrary("fff_search"), .linkedLibrary("z"), .linkedLibrary("iconv"),
      .linkedFramework("Security"), .linkedFramework("SystemConfiguration"),
      .linkedFramework("CoreServices"),
    ]),
    .target(name: "AirDropBridge", cSettings: [.unsafeFlags(["-fobjc-arc"])]),
    .target(name: "VideoFallback", cSettings: [
      .unsafeFlags(["-F", videoFrameworkDirectory, "-fobjc-arc"]),
    ]),
    .executableTarget(name: "Crawler", dependencies: [
    "FFFBridge",
    "AirDropBridge",
    "VideoFallback",
    .product(name: "SwiftTreeSitter", package: "swift-tree-sitter"),
    .product(name: "TreeSitterSwift", package: "tree-sitter-swift"),
    .product(name: "TreeSitterJavaScript", package: "tree-sitter-javascript"),
    .product(name: "TreeSitterTypeScript", package: "tree-sitter-typescript"),
    .product(name: "TreeSitterGo", package: "tree-sitter-go"),
    .product(name: "TreeSitterPython", package: "tree-sitter-python"),
    .product(name: "TreeSitterJSON", package: "tree-sitter-json"),
    .product(name: "TreeSitterYAML", package: "tree-sitter-yaml"),
    .product(name: "TreeSitterBash", package: "tree-sitter-bash"),
    .product(name: "TreeSitterSql", package: "tree-sitter-sql"),
    .product(name: "Markdown", package: "swift-markdown"),
  ])]
)
