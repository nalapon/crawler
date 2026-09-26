import Foundation

enum SettingsPowerSource: String, Sendable, CaseIterable {
  case battery = "Battery Power", adapter = "AC Power"
  var argument: String { self == .battery ? "-b" : "-c" }
  var title: String { self == .battery ? "On battery" : "On power adapter" }
}

enum SettingsPower {
  struct Values: Sendable {
    let source: SettingsPowerSource
    let lowPower: Bool?
    let displaySleep: Int?
  }
  /// pmset is Apple's administrative interface; this path only reads it.
  static func read() throws -> [Values] {
    let process = Process(), pipe = Pipe()
    process.executableURL = URL(fileURLWithPath: "/usr/bin/pmset")
    process.arguments = ["-g", "custom"]
    process.standardOutput = pipe
    process.standardError = FileHandle.nullDevice
    try process.run()
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    guard process.terminationStatus == 0, let text = String(data: data, encoding: .utf8) else {
      throw SettingsFailure(message: "macOS did not return energy settings.")
    }
    var current: SettingsPowerSource?
    var values: [SettingsPowerSource: [String: Int]] = [:]
    for line in text.split(separator: "\n") {
      if line.hasSuffix(":"), let source = SettingsPowerSource(rawValue: String(line.dropLast())) { current = source; continue }
      let fields = line.split(whereSeparator: \.isWhitespace)
      if let current, fields.count == 2, let number = Int(fields[1]) { values[current, default: [:]][String(fields[0])] = number }
    }
    return SettingsPowerSource.allCases.compactMap { source in
      guard let fields = values[source] else { return nil }
      let lowPower = fields["lowpowermode"].flatMap { (0...1).contains($0) ? $0 == 1 : nil }
      return Values(source: source, lowPower: lowPower, displaySleep: fields["displaysleep"])
    }
  }
}
