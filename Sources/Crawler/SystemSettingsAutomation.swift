import AppKit

/// Only terms from Apple's installed System Events scripting dictionary are accepted.
/// No preference-file writes, UI clicking, or interpolation of user-provided script text.
enum SettingsScriptProperty: String, CaseIterable, Sendable {
  case darkMode, dockSize, dockMagnification, magnificationSize, dockAutohide, dockPosition
  case animateOpening, minimizeEffect, minimizeIntoApp, showIndicators, showRecents
  var term: String {
    switch self {
    case .darkMode: "dark mode"
    case .dockSize: "dock size"
    case .dockMagnification: "magnification"
    case .magnificationSize: "magnification size"
    case .dockAutohide: "autohide"
    case .dockPosition: "screen edge"
    case .animateOpening: "animate"
    case .minimizeEffect: "minimize effect"
    case .minimizeIntoApp: "minimize into application"
    case .showIndicators: "show indicators"
    case .showRecents: "show recents"
    }
  }
  var title: String {
    switch self {
    case .darkMode: "Dark appearance"
    case .dockSize: "Dock size"
    case .dockMagnification: "Magnification"
    case .magnificationSize: "Magnification size"
    case .dockAutohide: "Automatically hide the Dock"
    case .dockPosition: "Position on screen"
    case .animateOpening: "Animate opening applications"
    case .minimizeEffect: "Minimize effect"
    case .minimizeIntoApp: "Minimize into application icon"
    case .showIndicators: "Show open application indicators"
    case .showRecents: "Show recent applications"
    }
  }
  var object: String { self == .darkMode ? "appearance preferences" : "dock preferences" }
  var options: [String] {
    switch self {
    case .dockPosition: ["bottom", "left", "right"]
    case .minimizeEffect: ["genie", "scale"]
    default: []
    }
  }
  var isScalar: Bool { self == .dockSize || self == .magnificationSize }
}

actor SettingsAutomation {
  private var enabled = false
  func enable() throws {
    _ = try execute("tell application \"System Events\" to get dark mode of appearance preferences")
    enabled = true
  }
  func rows(for section: SystemSettingsSection) throws -> [SettingsRow] {
    guard enabled else {
      return [.init(title: "Allow system controls", detail: "macOS will ask permission to control System Events. This enables settings here in Crawler.", control: .enableAutomation)]
    }
    let properties: [SettingsScriptProperty] = section == .appearance ? [.darkMode]
      : SettingsScriptProperty.allCases.filter { $0 != .darkMode }
    return try properties.map { property in
      let value = try execute("tell application \"System Events\" to get \(property.term) of \(property.object)")
      if property.isScalar {
        return .init(title: property.title, detail: "\(Int(value.doubleValue * 100))%", control: .scriptScalar(property, value.doubleValue))
      }
      if !property.options.isEmpty {
        let selected = try execute("tell application \"System Events\" to return (\(property.term) of \(property.object)) as text").stringValue ?? ""
        return .init(title: property.title, detail: selected.capitalized, control: .scriptChoice(property, selected))
      }
      return .init(title: property.title, detail: value.booleanValue ? "On" : "Off", control: .scriptToggle(property, value.booleanValue))
    }
  }
  func set(_ property: SettingsScriptProperty, boolean: Bool) throws {
    guard !property.isScalar, property.options.isEmpty else { throw SettingsFailure(message: "Invalid control type.") }
    let result = try set(property, literal: boolean ? "true" : "false")
    guard result.booleanValue == boolean else { throw SettingsFailure(message: "macOS did not retain this change.") }
  }
  func set(_ property: SettingsScriptProperty, scalar: Double) throws {
    guard property.isScalar, scalar.isFinite, (0...1).contains(scalar) else { throw SettingsFailure(message: "Invalid control value.") }
    let result = try set(property, literal: String(scalar))
    guard abs(result.doubleValue - scalar) < 0.02 else { throw SettingsFailure(message: "macOS did not retain this change.") }
  }
  func set(_ property: SettingsScriptProperty, choice: String) throws {
    guard property.options.contains(choice) else { throw SettingsFailure(message: "Invalid control choice.") }
    _ = try set(property, literal: choice)
    let result = try execute("tell application \"System Events\" to return (\(property.term) of \(property.object)) as text")
    guard result.stringValue == choice else { throw SettingsFailure(message: "macOS did not retain this change.") }
  }
  func setPower(_ source: SettingsPowerSource, lowPower: Bool) throws {
    // All shell text is fixed or selected from enums/booleans, never user input.
    _ = try execute("do shell script \"/usr/bin/pmset \(source.argument) lowpowermode \(lowPower ? 1 : 0)\" with administrator privileges")
    guard try SettingsPower.read().first(where: { $0.source == source })?.lowPower == lowPower else {
      throw SettingsFailure(message: "macOS did not retain the energy mode change.")
    }
  }
  func setDisplaySleep(_ source: SettingsPowerSource, minutes: Int) throws {
    guard (0...180).contains(minutes) else { throw SettingsFailure(message: "Choose a value between 0 and 180 minutes.") }
    _ = try execute("do shell script \"/usr/bin/pmset \(source.argument) displaysleep \(minutes)\" with administrator privileges")
    guard try SettingsPower.read().first(where: { $0.source == source })?.displaySleep == minutes else {
      throw SettingsFailure(message: "macOS did not retain the display sleep change.")
    }
  }
  func applyUpdates(install: Bool) throws {
    _ = try administratorCommand("/usr/sbin/softwareupdate", arguments: [install ? "--install" : "--download", "--all"])
  }
  func setDate(_ date: Date) throws {
    let formatter = DateFormatter()
    formatter.locale = Locale(identifier: "en_US_POSIX")
    formatter.calendar = Calendar(identifier: .gregorian)
    formatter.dateFormat = "MM:dd:yyyy"
    let day = formatter.string(from: date)
    formatter.dateFormat = "HH:mm:ss"
    let time = formatter.string(from: date)
    _ = try administratorCommand("/usr/sbin/systemsetup", arguments: ["-setusingnetworktime", "off"])
    _ = try administratorCommand("/usr/sbin/systemsetup", arguments: ["-setdate", day])
    _ = try administratorCommand("/usr/sbin/systemsetup", arguments: ["-settime", time])
    guard abs(Date().timeIntervalSince(date)) < 30 else { throw SettingsFailure(message: "macOS did not confirm the requested time.") }
  }
  func setTimeZone(_ identifier: String) throws {
    guard TimeZone.knownTimeZoneIdentifiers.contains(identifier) else { throw SettingsFailure(message: "Unknown time zone.") }
    _ = try administratorCommand("/usr/sbin/systemsetup", arguments: ["-settimezone", identifier])
    let actual = try administratorCommand("/usr/sbin/systemsetup", arguments: ["-gettimezone"])
    guard actual.contains(identifier) else { throw SettingsFailure(message: "macOS did not retain the time zone.") }
    NSTimeZone.resetSystemTimeZone()
  }
  func setNetworkTime(_ enabled: Bool) throws {
    _ = try administratorCommand("/usr/sbin/systemsetup", arguments: ["-setusingnetworktime", enabled ? "on" : "off"])
    let actual = try administratorCommand("/usr/sbin/systemsetup", arguments: ["-getusingnetworktime"])
    guard actual.lowercased().contains(enabled ? "on" : "off") else { throw SettingsFailure(message: "macOS did not retain automatic time.") }
  }
  func setComputerName(_ name: String) throws {
    guard !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, name.utf8.count <= 255,
          !name.unicodeScalars.contains(where: { CharacterSet.controlCharacters.contains($0) }) else {
      throw SettingsFailure(message: "Enter a computer name of 1–255 bytes without control characters.")
    }
    _ = try administratorCommand("/usr/sbin/scutil", arguments: ["--set", "ComputerName", name])
    guard try SettingsGeneral.run("/usr/sbin/scutil", ["--get", "ComputerName"]) == name else {
      throw SettingsFailure(message: "macOS did not retain the computer name.")
    }
  }
  // Escape shell arguments and then the containing AppleScript string independently.
  private func administratorCommand(_ executable: String, arguments: [String]) throws -> String {
    let command = ([executable] + arguments).map { "'" + $0.replacingOccurrences(of: "'", with: "'\"'\"'") + "'" }.joined(separator: " ")
    let literal = command.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
    return try execute("with timeout of 86400 seconds\ndo shell script \"" + literal + "\" with administrator privileges\nend timeout").stringValue ?? ""
  }
  private func set(_ property: SettingsScriptProperty, literal: String) throws -> NSAppleEventDescriptor {
    try execute("""
    tell application "System Events"
      set \(property.term) of \(property.object) to \(literal)
      return \(property.term) of \(property.object)
    end tell
    """)
  }
  private func execute(_ source: String) throws -> NSAppleEventDescriptor {
    guard let script = NSAppleScript(source: source) else { throw SettingsFailure(message: "Could not create the macOS settings request.") }
    var error: NSDictionary?
    let result = script.executeAndReturnError(&error)
    if let error {
      let message = error[NSAppleScript.errorMessage] as? String ?? "System Events rejected this operation."
      throw SettingsFailure(message: message)
    }
    return result
  }
}
