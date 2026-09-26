import Foundation
import CoreAudio
import AudioToolbox

/// Core Audio values never leave the worker as framework objects.
enum SettingsAudio {
  struct Device: Sendable {
    let id: AudioDeviceID
    let name: String
    let input: Bool
    let output: Bool
  }
  struct Snapshot: Sendable {
    let devices: [Device]
    let output: AudioDeviceID
    let input: AudioDeviceID
    let volume: Float32?
    let muted: Bool?
    let inputVolume: Float32?
    let balance: Float32?
    let effectsOutput: AudioDeviceID
  }
  struct Failure: LocalizedError {
    let status: OSStatus
    var errorDescription: String? { "macOS could not complete the audio operation (\(status))." }
  }
  static func address(_ selector: AudioObjectPropertySelector, scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) -> AudioObjectPropertyAddress {
    .init(mSelector: selector, mScope: scope, mElement: kAudioObjectPropertyElementMain)
  }
  static func read<T>(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, initial: T,
                      scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) throws -> T {
    var property = address(selector, scope: scope), value = initial
    var size = UInt32(MemoryLayout<T>.size)
    let status = withUnsafeMutablePointer(to: &value) {
      AudioObjectGetPropertyData(object, &property, 0, nil, &size, $0)
    }
    guard status == noErr else { throw Failure(status: status) }
    return value
  }
  static func write<T>(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, value: T,
                       scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal) throws {
    var property = address(selector, scope: scope), value = value
    let status = withUnsafePointer(to: &value) {
      AudioObjectSetPropertyData(object, &property, 0, nil, UInt32(MemoryLayout<T>.size), $0)
    }
    guard status == noErr else { throw Failure(status: status) }
  }
  static func editable(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector, scope: AudioObjectPropertyScope = kAudioDevicePropertyScopeOutput) -> Bool {
    var property = address(selector, scope: scope), result: DarwinBoolean = false
    return AudioObjectIsPropertySettable(object, &property, &result) == noErr && result.boolValue
  }
  static func hasStreams(_ object: AudioObjectID, scope: AudioObjectPropertyScope) -> Bool {
    var property = address(kAudioDevicePropertyStreams, scope: scope), size: UInt32 = 0
    return AudioObjectGetPropertyDataSize(object, &property, 0, nil, &size) == noErr && size > 0
  }
  static func snapshot() throws -> Snapshot {
    let system = AudioObjectID(kAudioObjectSystemObject)
    var property = address(kAudioHardwarePropertyDevices), size: UInt32 = 0
    let status = AudioObjectGetPropertyDataSize(system, &property, 0, nil, &size)
    guard status == noErr else { throw Failure(status: status) }
    var ids = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
    let result = ids.withUnsafeMutableBytes { buffer -> OSStatus in
      guard let base = buffer.baseAddress else { return noErr }
      return AudioObjectGetPropertyData(system, &property, 0, nil, &size, base)
    }
    guard result == noErr else { throw Failure(status: result) }
    let devices = ids.compactMap { id -> Device? in
      guard let name = try? read(id, kAudioObjectPropertyName, initial: "" as CFString) else { return nil }
      return Device(id: id, name: name as String, input: hasStreams(id, scope: kAudioDevicePropertyScopeInput),
                    output: hasStreams(id, scope: kAudioDevicePropertyScopeOutput))
    }
    let output = try read(system, kAudioHardwarePropertyDefaultOutputDevice, initial: AudioDeviceID(0))
    let input = (try? read(system, kAudioHardwarePropertyDefaultInputDevice, initial: AudioDeviceID(0))) ?? 0
    let volume: Float32? = editable(output, kAudioHardwareServiceDeviceProperty_VirtualMainVolume)
      ? try? read(output, kAudioHardwareServiceDeviceProperty_VirtualMainVolume, initial: Float32(0), scope: kAudioDevicePropertyScopeOutput) : nil
    let mute: UInt32? = editable(output, kAudioDevicePropertyMute)
      ? try? read(output, kAudioDevicePropertyMute, initial: UInt32(0), scope: kAudioDevicePropertyScopeOutput) : nil
    let inputVolume: Float32? = editable(input, kAudioHardwareServiceDeviceProperty_VirtualMainVolume, scope: kAudioDevicePropertyScopeInput)
      ? try? read(input, kAudioHardwareServiceDeviceProperty_VirtualMainVolume, initial: Float32(0), scope: kAudioDevicePropertyScopeInput) : nil
    let balance: Float32? = editable(output, kAudioHardwareServiceDeviceProperty_VirtualMainBalance)
      ? try? read(output, kAudioHardwareServiceDeviceProperty_VirtualMainBalance, initial: Float32(0.5), scope: kAudioDevicePropertyScopeOutput) : nil
    let effects = (try? read(system, kAudioHardwarePropertyDefaultSystemOutputDevice, initial: AudioDeviceID(0))) ?? 0
    return Snapshot(devices: devices, output: output, input: input, volume: volume, muted: mute.map { $0 != 0 }, inputVolume: inputVolume, balance: balance, effectsOutput: effects)
  }
}

/// Immutable registrations created off the main actor and removed with their owner.
/// Core Audio delivers notifications on its queue; the consumer chooses its UI executor.
final class SettingsAudioObservation: @unchecked Sendable {
  private let registrations: [(AudioObjectID, AudioObjectPropertyAddress, AudioObjectPropertyListenerBlock)]

  init(changed: @escaping @Sendable (Bool) -> Void) throws {
    let snapshot = try SettingsAudio.snapshot()
    var registrations: [(AudioObjectID, AudioObjectPropertyAddress, AudioObjectPropertyListenerBlock)] = []
    func observe(_ object: AudioObjectID, _ selector: AudioObjectPropertySelector,
                 scope: AudioObjectPropertyScope = kAudioObjectPropertyScopeGlobal, topology: Bool = false) {
      var address = SettingsAudio.address(selector, scope: scope)
      guard AudioObjectHasProperty(object, &address) else { return }
      let block: AudioObjectPropertyListenerBlock = { _, _ in changed(topology) }
      if AudioObjectAddPropertyListenerBlock(object, &address, nil, block) == noErr {
        registrations.append((object, address, block))
      }
    }
    let system = AudioObjectID(kAudioObjectSystemObject)
    observe(system, kAudioHardwarePropertyDevices, topology: true)
    observe(system, kAudioHardwarePropertyDefaultOutputDevice)
    observe(system, kAudioHardwarePropertyDefaultInputDevice)
    observe(system, kAudioHardwarePropertyDefaultSystemOutputDevice)
    for device in snapshot.devices {
      for scope in [kAudioDevicePropertyScopeOutput, kAudioDevicePropertyScopeInput] {
        observe(device.id, kAudioHardwareServiceDeviceProperty_VirtualMainVolume, scope: scope)
        observe(device.id, kAudioHardwareServiceDeviceProperty_VirtualMainBalance, scope: scope)
        observe(device.id, kAudioDevicePropertyMute, scope: scope)
        // Drivers may notify scalar/channel properties instead of virtual main volume.
        observe(device.id, kAudioDevicePropertyVolumeScalar, scope: scope)
      }
      observe(device.id, 0x6c73746d) // Bluetooth listening mode, when exposed.
    }
    self.registrations = registrations
  }
  deinit {
    for (object, original, block) in registrations {
      var address = original
      AudioObjectRemovePropertyListenerBlock(object, &address, nil, block)
    }
  }
}
