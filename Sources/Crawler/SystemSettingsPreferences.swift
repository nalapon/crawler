import Foundation

/// Undocumented macOS preference keys, explicitly allowed for this settings workspace.
/// CFPreferences changes one key, preserving unrelated values. Some changes need a new login.
actor SettingsPreferences {
  enum Choice: String, Sendable {
    case language, region, accent, repeatInterval, repeatDelay
    var key: String {
      switch self {
      case .language: "AppleLanguages"
      case .region: "AppleLocale"
      case .accent: "AppleAccentColor"
      case .repeatInterval: "KeyRepeat"
      case .repeatDelay: "InitialKeyRepeat"
      }
    }
  }
  private let global = kCFPreferencesAnyApplication
  func read(_ choice: Choice) -> String? {
    let value = CFPreferencesCopyValue(choice.key as CFString, global, kCFPreferencesCurrentUser, kCFPreferencesAnyHost)
    if choice == .language { return (value as? [String])?.first }
    if let number = value as? NSNumber { return number.stringValue }
    return value as? String
  }
  func set(_ choice: Choice, value: String) throws {
    let object: CFPropertyList
    switch choice {
    case .language:
      guard Locale.availableIdentifiers.contains(value) || Locale.LanguageCode.isoLanguageCodes.map(\.identifier).contains(value) else { throw SettingsFailure(message: "Unknown language.") }
      let previous = CFPreferencesCopyValue(choice.key as CFString, global, kCFPreferencesCurrentUser, kCFPreferencesAnyHost) as? [String] ?? []
      object = ([value] + previous.filter { $0 != value }) as CFArray
    case .region:
      guard Locale.availableIdentifiers.contains(value) else { throw SettingsFailure(message: "Unknown locale.") }
      object = value as CFString
    case .accent:
      guard let color = Int(value), (-2...6).contains(color) else { throw SettingsFailure(message: "Unknown accent color.") }
      object = NSNumber(value: color)
    case .repeatInterval, .repeatDelay:
      guard let ticks = Int(value), (1...120).contains(ticks) else { throw SettingsFailure(message: "Keyboard timing out of range.") }
      object = NSNumber(value: ticks)
    }
    CFPreferencesSetValue(choice.key as CFString, object, global, kCFPreferencesCurrentUser, kCFPreferencesAnyHost)
    guard CFPreferencesSynchronize(global, kCFPreferencesCurrentUser, kCFPreferencesAnyHost), read(choice) == value else {
      throw SettingsFailure(message: "macOS did not save the preference.")
    }
    if choice == .repeatInterval || choice == .repeatDelay {
      let timing: SettingsKeyboard.Timing = choice == .repeatInterval ? .interval : .delay
      guard let ticks = Int(value) else { return }
      try SettingsKeyboard.set(timing, milliseconds: Int((Double(ticks) * 1000 / 60).rounded()))
    }
  }
  struct Shortcut: Sendable {
    let id: String
    let enabled: Bool
    let keyCode: Int
    let modifiers: Int
  }
  private let shortcutsDomain = "com.apple.symbolichotkeys" as CFString
  private func shortcutDictionary() -> [String: [String: Any]] {
    CFPreferencesCopyValue("AppleSymbolicHotKeys" as CFString, shortcutsDomain, kCFPreferencesCurrentUser, kCFPreferencesAnyHost) as? [String: [String: Any]] ?? [:]
  }
  func shortcuts() -> [Shortcut] {
    shortcutDictionary().compactMap { id, entry in
      guard let enabled = entry["enabled"] as? Bool, let value = entry["value"] as? [String: Any],
            value["type"] as? String == "standard", let parameters = value["parameters"] as? [Int], parameters.count == 3 else { return nil }
      return Shortcut(id: id, enabled: enabled, keyCode: parameters[1], modifiers: parameters[2])
    }.sorted { (Int($0.id) ?? 0) < (Int($1.id) ?? 0) }
  }
  func setShortcut(_ id: String, enabled: Bool, keyCode: Int, modifiers: Int) throws {
    guard ((0...127).contains(keyCode) || (!enabled && keyCode == 65535)), modifiers >= 0 else { throw SettingsFailure(message: "Invalid keyboard shortcut.") }
    if enabled, shortcuts().contains(where: { $0.id != id && $0.enabled && $0.keyCode == keyCode && $0.modifiers & 0x1e0000 == modifiers & 0x1e0000 }) {
      throw SettingsFailure(message: "Another system shortcut already uses this combination.")
    }
    var dictionary = shortcutDictionary()
    guard var entry = dictionary[id], var value = entry["value"] as? [String: Any], var parameters = value["parameters"] as? [Int], parameters.count == 3 else {
      throw SettingsFailure(message: "This macOS version has no editable definition for the shortcut.")
    }
    entry["enabled"] = enabled
    if parameters[1] != keyCode || parameters[2] != modifiers { parameters = [65535, keyCode, modifiers] }
    value["parameters"] = parameters; entry["value"] = value; dictionary[id] = entry
    CFPreferencesSetValue("AppleSymbolicHotKeys" as CFString, dictionary as CFDictionary, shortcutsDomain, kCFPreferencesCurrentUser, kCFPreferencesAnyHost)
    guard CFPreferencesSynchronize(shortcutsDomain, kCFPreferencesCurrentUser, kCFPreferencesAnyHost),
          let actual = shortcuts().first(where: { $0.id == id }), actual.enabled == enabled, actual.keyCode == keyCode, actual.modifiers == modifiers else {
      throw SettingsFailure(message: "macOS did not save this shortcut.")
    }
    _ = try SettingsGeneral.run("/System/Library/PrivateFrameworks/SystemAdministration.framework/Resources/activateSettings", ["-u"])
  }
}
