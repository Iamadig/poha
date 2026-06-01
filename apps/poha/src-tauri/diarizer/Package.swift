// swift-tools-version: 5.9
import PackageDescription

let package = Package(
  name: "PohaDiarizer",
  platforms: [
    .macOS("14.2")
  ],
  products: [
    .executable(name: "poha-diarizer", targets: ["PohaDiarizer"])
  ],
  dependencies: [
    .package(url: "https://github.com/FluidInference/FluidAudio.git", "0.12.6"..<"0.13.0")
  ],
  targets: [
    .executableTarget(
      name: "PohaDiarizer",
      dependencies: [
        .product(name: "FluidAudio", package: "FluidAudio")
      ],
      path: "Sources/PohaDiarizer"
    )
  ],
  cxxLanguageStandard: .cxx17
)
