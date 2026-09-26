import Foundation

/// Read-only commands never invoke a shell. Privileged changes use SettingsAutomation.
actor SettingsGeneral {
  private var updates: String?
  func updateRows() -> [SettingsRow] {
    var rows: [SettingsRow] = [.init(title: "Software Update", detail: "Check Apple for available updates.", control: .checkUpdates)]
    if let updates { rows.append(.init(title: "Available updates", detail: "Results of the last check", control: .updateDetails(updates))) }
    return rows
  }
  func checkUpdates() throws {
    updates = try Self.run("/usr/sbin/softwareupdate", ["--list"])
  }
  static func run(_ executable: String, _ arguments: [String]) throws -> String {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    let pipe = Pipe()
    process.standardOutput = pipe
    process.standardError = pipe
    try process.run()
    let data = pipe.fileHandleForReading.readDataToEndOfFile()
    process.waitUntilExit()
    let output = String(decoding: data, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
    guard process.terminationStatus == 0 else { throw SettingsFailure(message: output.isEmpty ? "System command failed (\(process.terminationStatus))." : output) }
    return output
  }
}
