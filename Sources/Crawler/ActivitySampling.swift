import Foundation
import Darwin

struct ActivityProcessID: Hashable, Sendable {
  let pid: Int32
  let started: UInt64
}

struct ActivityProcess: Sendable {
  let id: ActivityProcessID
  let parent: Int32
  let name: String
  let executable: String
  let memory: UInt64?
  let cpu: Double?
  let readRate: Double?
  let writeRate: Double?
  var disk: Double? {
    guard let readRate, let writeRate else { return nil }
    return readRate + writeRate
  }
}

enum ActivityProcessTermination: Sendable {
  case quit, forceQuit
  var signal: Int32 { self == .quit ? SIGTERM : SIGKILL }
}

enum ActivityProcessControl {
  /// Recheck identity immediately before signalling; a stale sample must not target a reused PID.
  static func terminate(_ id: ActivityProcessID, mode: ActivityProcessTermination) throws {
    guard id.pid > 1, id.started > 0 else {
      throw NSError(domain: "Crawler.Process", code: 1,
        userInfo: [NSLocalizedDescriptionKey: "The identity of this process cannot be verified."])
    }
    var info = proc_bsdinfo()
    guard proc_pidinfo(id.pid, PROC_PIDTBSDINFO, 0, &info, Int32(MemoryLayout.size(ofValue: info))) == MemoryLayout.size(ofValue: info) else {
      throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
    }
    guard info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec == id.started else {
      throw NSError(domain: "Crawler.Process", code: 2,
        userInfo: [NSLocalizedDescriptionKey: "This process has exited. Its PID now belongs to another process."])
    }
    guard Darwin.kill(id.pid, mode.signal) == 0 else {
      throw NSError(domain: NSPOSIXErrorDomain, code: Int(errno))
    }
  }
}

enum ActivityMetric: String, CaseIterable, Sendable {
  case memory = "Memory", cpu = "CPU", disk = "Disk"
  func value(_ process: ActivityProcess) -> Double? {
    switch self {
    case .memory: process.memory.map { Double($0) }
    case .cpu: process.cpu
    case .disk: process.disk
    }
  }
  func formatted(_ value: Double?) -> String {
    guard let value else { return "—" }
    switch self {
    case .cpu: return String(format: "%.1f%%", value)
    case .memory, .disk:
      let bytes = ByteCountFormatter.string(fromByteCount: Int64(min(value, Double(Int64.max - 1024))), countStyle: .memory)
      return bytes + (self == .disk ? "/s" : "")
    }
  }
}

struct ActivityCounters: Sendable {
  let started: UInt64
  let cpu: UInt64
  let read: UInt64
  let written: UInt64
  let time: TimeInterval
}

struct ActivitySnapshot: Sendable {
  var processes: [ActivityProcess] = []
  var counters: [Int32: ActivityCounters] = [:]
  var sampledAt: Date = .now
  var unavailable = 0
  var failure: String?
  var milliseconds = 0.0
}

/// Only cumulative counters cross samples. PID plus start time prevents PID reuse from producing false rates.
enum ActivitySampler {
  static func read(previous: [Int32: ActivityCounters]) -> ActivitySnapshot {
    let began = ProcessInfo.processInfo.systemUptime
    var snapshot = ActivitySnapshot()
    var timebase = mach_timebase_info_data_t()
    mach_timebase_info(&timebase)
    // rusage CPU times use Mach absolute-time units, including on Apple Silicon.
    let secondsPerTick = Double(timebase.numer) / Double(timebase.denom) / 1_000_000_000
    let capacity = proc_listallpids(nil, 0)
    guard capacity > 0 else {
      snapshot.failure = "Unable to read the process list: \(String(cString: strerror(errno)))"
      return snapshot
    }
    var pids = [Int32](repeating: 0, count: Int(capacity) + 256)
    let count = pids.withUnsafeMutableBytes { proc_listallpids($0.baseAddress, Int32($0.count)) }
    guard count > 0 else {
      snapshot.failure = "The process list could not be sampled."
      return snapshot
    }
    for pid in pids.prefix(Int(count)) where pid > 0 {
      if Task.isCancelled { break }
      var info = proc_bsdinfo()
      guard proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &info, Int32(MemoryLayout.size(ofValue: info))) == MemoryLayout.size(ofValue: info) else {
        if errno == EPERM || errno == EACCES {
          snapshot.unavailable += 1
          // The PID is public even when macOS withholds its name, parent and usage.
          snapshot.processes.append(ActivityProcess(id: .init(pid: pid, started: 0), parent: 0,
            name: "PID \(pid)", executable: "", memory: nil, cpu: nil, readRate: nil, writeRate: nil))
        }
        continue
      }
      let name = withUnsafeBytes(of: info.pbi_name) { bytes in
        String(decoding: bytes.prefix(while: { $0 != 0 }), as: UTF8.self)
      }
      let command = withUnsafeBytes(of: info.pbi_comm) { bytes in
        String(decoding: bytes.prefix(while: { $0 != 0 }), as: UTF8.self)
      }
      var path = [UInt8](repeating: 0, count: 4 * Int(MAXPATHLEN))
      let pathLength = path.withUnsafeMutableBytes { proc_pidpath(pid, $0.baseAddress, UInt32($0.count)) }
      let executable = pathLength > 0 ? String(decoding: path.prefix(while: { $0 != 0 }), as: UTF8.self) : ""
      var usage = rusage_info_v4()
      // libproc's historical void** signature actually receives the address of the rusage structure.
      let readable = withUnsafeMutablePointer(to: &usage) { pointer in
        pointer.withMemoryRebound(to: Optional<rusage_info_t>.self, capacity: 1) {
          proc_pid_rusage(pid, RUSAGE_INFO_V4, $0) == 0
        }
      }
      let started = info.pbi_start_tvsec * 1_000_000 + info.pbi_start_tvusec
      let now = ProcessInfo.processInfo.systemUptime
      var cpu: Double?, readRate: Double?, writeRate: Double?
      if readable {
        let next = ActivityCounters(started: started, cpu: usage.ri_user_time + usage.ri_system_time,
          read: usage.ri_diskio_bytesread, written: usage.ri_diskio_byteswritten, time: now)
        if let old = previous[pid], old.started == started, now > old.time {
          let elapsed = now - old.time
          if next.cpu >= old.cpu { cpu = Double(next.cpu - old.cpu) * secondsPerTick / elapsed * 100 }
          if next.read >= old.read { readRate = Double(next.read - old.read) / elapsed }
          if next.written >= old.written { writeRate = Double(next.written - old.written) / elapsed }
        }
        snapshot.counters[pid] = next
      } else { snapshot.unavailable += 1 }
      snapshot.processes.append(ActivityProcess(id: .init(pid: pid, started: started), parent: Int32(info.pbi_ppid),
        name: name.isEmpty ? command : name, executable: executable,
        memory: readable ? usage.ri_phys_footprint : nil, cpu: cpu, readRate: readRate, writeRate: writeRate))
    }
    snapshot.sampledAt = .now
    snapshot.milliseconds = (ProcessInfo.processInfo.systemUptime - began) * 1000
    return snapshot
  }
}

struct ActivityGroup: Sendable {
  let key: String
  let name: String
  let processes: [ActivityProcess]
  func value(_ metric: ActivityMetric) -> Double { processes.reduce(0) { $0 + (metric.value($1) ?? 0) } }

  static func collect(_ processes: [ActivityProcess]) -> [Self] {
    let byPID = Dictionary(uniqueKeysWithValues: processes.map { ($0.id.pid, $0) })
    func app(_ process: ActivityProcess) -> String? {
      guard let range = process.executable.range(of: ".app/") else { return nil }
      return String(process.executable[..<range.lowerBound]) + ".app"
    }
    func root(_ process: ActivityProcess) -> ActivityProcess {
      var current = process
      var seen: Set<Int32> = [current.id.pid]
      while current.parent > 1, let parent = byPID[current.parent], app(parent) == nil,
        seen.insert(parent.id.pid).inserted { current = parent }
      return current
    }
    let grouped = Dictionary(grouping: processes) { process in
      app(process) ?? "pid:\(root(process).id.pid):\(root(process).id.started)"
    }
    return grouped.map { key, members in
      let name = key.hasPrefix("pid:") ? root(members[0]).name
        : URL(fileURLWithPath: key).deletingPathExtension().lastPathComponent
      return Self(key: key, name: name, processes: members)
    }
  }
}
