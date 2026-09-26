import Foundation
import Carbon
import IOKit.hidsystem

/// HID repeat parameters are nanoseconds. These operations affect the current login session.
enum SettingsKeyboard {
  enum Timing: String, Sendable {
    case interval, delay
    var key: String { self == .interval ? kIOHIDKeyRepeatKey : kIOHIDInitialKeyRepeatKey }
    var title: String { self == .interval ? "Key repeat interval" : "Delay before repeat" }
    var choices: [Int] { self == .interval ? [16, 33, 50, 66, 100, 200] : [167, 250, 417, 500, 750, 1000] }
  }
  private static func withConnection<T>(_ body: (io_connect_t) throws -> T) throws -> T {
    let service = IOServiceGetMatchingService(kIOMainPortDefault, IOServiceMatching("IOHIDSystem"))
    guard service != 0 else { throw SettingsFailure(message: "Keyboard controller unavailable.") }
    defer { IOObjectRelease(service) }
    var connection: io_connect_t = 0
    let status = IOServiceOpen(service, mach_task_self_, UInt32(kIOHIDParamConnectType), &connection)
    guard status == KERN_SUCCESS else { throw SettingsFailure(message: "Cannot access keyboard timing (\(status)).") }
    defer { IOServiceClose(connection) }
    return try body(connection)
  }
  private static func read(_ timing: Timing, connection: io_connect_t) throws -> Int64 {
    var value: Unmanaged<CFTypeRef>?
    let status = IOHIDCopyCFTypeParameter(connection, timing.key as CFString, &value)
    guard status == KERN_SUCCESS, let number = value?.takeRetainedValue() as? NSNumber else {
      throw SettingsFailure(message: "Cannot read keyboard timing (\(status)).")
    }
    return number.int64Value
  }
  static func milliseconds(_ timing: Timing) throws -> Int {
    try withConnection { Int((Double(try read(timing, connection: $0)) / 1_000_000).rounded()) }
  }
  static func set(_ timing: Timing, milliseconds: Int) throws {
    guard (10...2000).contains(milliseconds) else { throw SettingsFailure(message: "Keyboard timing is out of range.") }
    try withConnection { connection in
      let nanoseconds = Int64(milliseconds) * 1_000_000
      let status = IOHIDSetCFTypeParameter(connection, timing.key as CFString, NSNumber(value: nanoseconds))
      guard status == KERN_SUCCESS else { throw SettingsFailure(message: "macOS rejected the keyboard timing (\(status)).") }
      guard abs(try read(timing, connection: connection) - nanoseconds) < 1_000_000 else {
        throw SettingsFailure(message: "macOS did not retain the keyboard timing.")
      }
    }
  }
}

extension SettingsKeyboard {
  @MainActor static func shortcutLabel(keyCode: Int, modifiers: Int) -> String {
    guard (0...127).contains(keyCode) else { return "Not assigned" }
    let symbols = [(1 << 18, "⌃"), (1 << 19, "⌥"), (1 << 17, "⇧"), (1 << 20, "⌘")]
    let prefix = symbols.filter { modifiers & $0.0 != 0 }.map(\.1).joined()
    let special = [36: "Return", 48: "Tab", 49: "Space", 51: "Delete", 53: "Escape", 123: "←", 124: "→", 125: "↓", 126: "↑"]
    if let key = special[keyCode] { return prefix + key }
    let source = TISCopyCurrentKeyboardLayoutInputSource().takeRetainedValue()
    guard let pointer = TISGetInputSourceProperty(source, kTISPropertyUnicodeKeyLayoutData) else { return prefix + "Key \(keyCode)" }
    let data = Unmanaged<CFData>.fromOpaque(pointer).takeUnretainedValue()
    guard let bytes = CFDataGetBytePtr(data) else { return prefix + "Key \(keyCode)" }
    let layout = UnsafeRawPointer(bytes).assumingMemoryBound(to: UCKeyboardLayout.self)
    var deadKey: UInt32 = 0, length = 0
    var characters = [UniChar](repeating: 0, count: 8)
    let result = UCKeyTranslate(layout, UInt16(keyCode), UInt16(kUCKeyActionDisplay), 0, UInt32(LMGetKbdType()), UInt32(kUCKeyTranslateNoDeadKeysBit), &deadKey, characters.count, &length, &characters)
    guard result == noErr, length > 0 else { return prefix + "Key \(keyCode)" }
    return prefix + String(utf16CodeUnits: characters, count: length).uppercased()
  }
}
