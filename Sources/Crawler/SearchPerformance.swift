import Foundation
import os

// AppKit update/draw timings are submission timings, not compositor presentation.
// Signposts correlate these stages with Instruments without logging query text.
@MainActor
final class SearchPerformance {
  private static let signposter = OSSignposter(subsystem: "studio.crawler.native-prototype", category: .pointsOfInterest)
  private static let logger = Logger(subsystem: "studio.crawler.native-prototype", category: "Search Performance")
  private var samples: [[String: Double]] = []
  private var start = ProcessInfo.processInfo.systemUptime
  private var eventTime: TimeInterval?
  private var interval: OSSignpostIntervalState?
  private var waitingForWindow = false
  private var waitingForTitle = false
  private var sequence = 0
  var didMeasure: (() -> Void)?
  var diagnostics: [[String: Double]] { samples }

  func input(timestamp: TimeInterval?) { eventTime = timestamp }
  func cancel() {
    if let interval { Self.signposter.endInterval("Search to first title draw", interval, "cancelled") }
    interval = nil
    waitingForWindow = false
    waitingForTitle = false
  }
  func begin() {
    cancel()
    let now = ProcessInfo.processInfo.systemUptime
    let keyboardInput = eventTime != nil
    start = eventTime ?? now
    eventTime = nil
    waitingForWindow = false
    waitingForTitle = false
    if samples.count == 32 { samples.removeFirst() }
    sequence += 1
    samples.append(["sequence": Double(sequence), "input_dispatch_ms": max(0, now - start) * 1000,
      "keyboard_input": keyboardInput ? 1 : 0])
    interval = Self.signposter.beginInterval("Search to first title draw")
  }
  private var elapsed: Double { (ProcessInfo.processInfo.systemUptime - start) * 1000 }
  func published(complete: Bool, cacheHit: Bool) {
    guard !samples.isEmpty else { return }
    let i = samples.count - 1
    if samples[i]["first_publish_ms"] == nil { samples[i]["first_publish_ms"] = elapsed }
    if complete { samples[i]["complete_publish_ms"] = elapsed }
    samples[i]["cache_hit"] = cacheHit ? 1 : 0
    Self.signposter.emitEvent("Results published", "complete=\(complete) cache=\(cacheHit)")
  }
  func rowsUpdated(since: TimeInterval) {
    guard !samples.isEmpty else { return }
    let i = samples.count - 1
    samples[i]["row_update_work_ms", default: 0] += (ProcessInfo.processInfo.systemUptime - since) * 1000
    if samples[i]["first_rows_ms"] == nil {
      samples[i]["first_rows_ms"] = elapsed
      waitingForWindow = true
      waitingForTitle = true
    }
  }
  func drawn(since: TimeInterval) {
    guard waitingForWindow, !samples.isEmpty else { return }
    let i = samples.count - 1
    samples[i]["table_background_draw_work_ms", default: 0] += (ProcessInfo.processInfo.systemUptime - since) * 1000
    if samples[i]["table_background_draw_ms"] == nil { samples[i]["table_background_draw_ms"] = elapsed }
  }
  func titleDrawn(since: TimeInterval) {
    guard waitingForTitle, !samples.isEmpty else { return }
    waitingForTitle = false
    let ms = elapsed, i = samples.count - 1
    samples[i]["first_title_draw_ms"] = ms
    samples[i]["title_draw_work_ms"] = (ProcessInfo.processInfo.systemUptime - since) * 1000
    if let interval { Self.signposter.endInterval("Search to first title draw", interval) }
    interval = nil
    let dispatch = samples[i]["input_dispatch_ms"] ?? 0
    let publish = samples[i]["first_publish_ms"] ?? 0
    let rows = samples[i]["first_rows_ms"] ?? 0
    Self.logger.info("title_draw_ms=\(ms, privacy: .public) input_dispatch_ms=\(dispatch, privacy: .public) publish_ms=\(publish, privacy: .public) rows_ms=\(rows, privacy: .public)")
    // Reporting is diagnostic I/O; keep it outside the view's drawing call.
    if didMeasure != nil { DispatchQueue.main.async { [weak self] in self?.didMeasure?() } }
  }
  func windowUpdated() {
    guard waitingForWindow, !samples.isEmpty else { return }
    waitingForWindow = false
    samples[samples.count - 1]["window_update_notification_ms"] = elapsed
    // This notification can lag drawing, so it never ends the rendering interval.
  }
}
