// swift-tools-version: 5.9

import PackageDescription

let package = Package(
  name: "poha-calendar-eventkit",
  platforms: [.macOS("14.2")],
  products: [
    .library(
      name: "poha-calendar-eventkit",
      type: .static,
      targets: ["PohaCalendarEventKit"])
  ],
  targets: [
    .target(
      name: "PohaCalendarEventKit",
      path: "Sources/PohaCalendarEventKit"),
    .testTarget(
      name: "PohaCalendarEventKitTests",
      dependencies: ["PohaCalendarEventKit"],
      path: "Tests/PohaCalendarEventKitTests"),
  ]
)
