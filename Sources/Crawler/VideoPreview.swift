import AppKit
import AVKit
import VideoFallback

/// Owns only the selected video's decoder. Both the pane and Space window use it.
@MainActor
final class VideoPreview: Surface {
  private enum Playback {
    case idle, loading
    case native(AVPlayer)
    case fallback(CrawlerVLCPlayer)
    case failed
  }
  private var playback: Playback = .idle
  private let picture = NSView()
  private let nativeView = AVPlayerView()
  private let controls = Surface()
  private let play = NSButton()
  private let mute = NSButton()
  private let seek = NSSlider(value: 0, minValue: 0, maxValue: 1, target: nil, action: nil)
  private let time = label("0:00", size: 11)
  private let message = label("Loading video…", size: 12)
  private var loadTask: Task<Void, Never>?
  private var readiness: NSKeyValueObservation?
  private var timeObserver: Any?
  private var generation = UUID()
  private var url: URL?
  private var requestedPlayback = false
  private var muted = false
  private var ready = false
  private var pausedSeekSeconds: Double?
  var didChange: (() -> Void)?
  private(set) var state = "empty"
  var seconds: Double {
    switch playback {
    case .native(let player): return finite(player.currentTime().seconds)
    case .fallback(let player): return player.seconds
    default: return 0
    }
  }
  private var duration: Double {
    switch playback {
    case .native(let player): return finite(player.currentItem?.duration.seconds ?? 0)
    case .fallback(let player): return player.duration
    default: return 0
    }
  }
  private var playing: Bool {
    switch playback {
    case .native(let player): return player.rate != 0
    case .fallback(let player): return player.playing
    default: return false
    }
  }

  override init(frame: NSRect) {
    super.init(frame: frame)
    picture.wantsLayer = true
    picture.layer?.backgroundColor = NSColor.black.cgColor
    nativeView.controlsStyle = .none
    nativeView.allowsVideoFrameAnalysis = false
    nativeView.updatesNowPlayingInfoCenter = false
    nativeView.videoGravity = .resizeAspect
    nativeView.autoresizingMask = [.width, .height]
    picture.addSubview(nativeView)
    for view in [picture, controls, message] { addSubview(view) }
    for view in [play, seek, time, mute] { controls.addSubview(view) }
    play.target = self; play.action = #selector(togglePlayback)
    mute.target = self; mute.action = #selector(toggleMute)
    for button in [play, mute] { button.bezelStyle = .texturedRounded; button.imagePosition = .imageOnly }
    seek.target = self; seek.action = #selector(seekChanged)
    seek.isContinuous = false
    seek.setAccessibilityLabel("Video position")
    message.alignment = .center
    message.maximumNumberOfLines = 3
    message.lineBreakMode = .byWordWrapping
    message.textColor = .white
    updateControls()
  }
  required init?(coder: NSCoder) { nil }
  deinit { loadTask?.cancel() }

  func configure(_ url: URL) {
    stop()
    self.url = url
    playback = .loading
    state = "video-loading"
    message.stringValue = "Loading video…"
    message.isHidden = false
    updateControls()
    let token = generation
    loadTask = Task { [weak self] in
      do {
        // Arrow-key travel should not open every video the selection passes over.
        try await Task.sleep(for: .milliseconds(80))
        let asset = AVURLAsset(url: url)
        let playable = (try? await asset.load(.isPlayable)) == true
        try Task.checkCancellation()
        guard let self, self.generation == token else { return }
        if playable { self.useNative(asset, token: token) }
        else { self.useFallback(url, token: token) }
      } catch is CancellationError {} catch {
        self?.fail(error.localizedDescription)
      }
    }
  }

  private func useNative(_ asset: AVURLAsset, token: UUID) {
    let item = AVPlayerItem(asset: asset)
    item.preferredForwardBufferDuration = 1
    let player = AVPlayer(playerItem: item)
    playback = .native(player)
    nativeView.isHidden = false
    nativeView.player = player
    readiness = item.observe(\.status, options: [.initial, .new]) { [weak self] _, _ in
      Task { @MainActor [weak self] in
        guard let self, self.generation == token, case .native(let active) = self.playback,
          active.currentItem === item else { return }
        switch item.status {
        case .readyToPlay:
          self.ready = true
          self.message.isHidden = true
          self.updateControls()
        case .failed:
          if let url = self.url { self.releaseNative(); self.useFallback(url, token: token) }
        case .unknown: break
        @unknown default: break
        }
      }
    }
    timeObserver = player.addPeriodicTimeObserver(forInterval: CMTime(seconds: 0.5, preferredTimescale: 600), queue: .main) { [weak self] _ in
      MainActor.assumeIsolated { self?.updateControls() }
    }
    // Seeking a paused item displays its initial frame without playing audio.
    player.seek(to: .zero, toleranceBefore: .zero, toleranceAfter: .zero)
  }

  private func useFallback(_ url: URL, token: UUID) {
    playback = .loading
    loadTask = Task { [weak self] in
      do {
        let worker = Task.detached(priority: .userInitiated) { try CrawlerVLCPlayer.loadFramework() }
        try await worker.value
        try Task.checkCancellation()
        guard let self, self.generation == token else { return }
        let player = CrawlerVLCPlayer(url: url)
        self.playback = .fallback(player)
        self.nativeView.isHidden = true
        player.videoView.frame = self.picture.bounds
        player.videoView.autoresizingMask = [.width, .height]
        self.picture.addSubview(player.videoView)
        player.muted = true
        player.changed = { [weak self, weak player] in
          guard let self, let player, self.generation == token else { return }
          if player.failed { self.fail("This video could not be played. Open it in your video application."); return }
          if player.ready {
            self.ready = true
            self.message.isHidden = true
            if self.requestedPlayback { player.muted = self.muted }
          }
          self.updateControls()
        }
        player.prepare()
      } catch is CancellationError {} catch {
        guard let self, self.generation == token else { return }
        self.fail(error.localizedDescription)
      }
    }
  }

  @objc private func togglePlayback() {
    guard ready else { return }
    requestedPlayback = true
    switch playback {
    case .native(let player):
      if playing { player.pause() }
      else {
        if duration > 0, seconds >= duration - 0.1 { player.seek(to: .zero) }
        player.play()
      }
    case .fallback(let player):
      player.muted = muted
      if player.playing { player.pause() } else { player.play() }
    default: break
    }
    updateControls()
  }
  @objc private func toggleMute() {
    muted.toggle()
    switch playback {
    case .native(let player): player.isMuted = muted
    case .fallback(let player): player.muted = requestedPlayback ? muted : true
    default: break
    }
    updateControls()
  }
  @objc private func seekChanged() {
    guard ready, duration > 0 else { return }
    switch playback {
    case .native(let player): player.seek(to: CMTime(seconds: seek.doubleValue * duration, preferredTimescale: 600))
    case .fallback(let player):
      // VLC does not publish its new clock until playback resumes.
      pausedSeekSeconds = playing ? nil : seek.doubleValue * duration
      player.position = seek.floatValue
    default: break
    }
    updateControls()
  }

  private func updateControls() {
    controls.fill = Design.raised
    time.textColor = Design.text
    play.image = NSImage(systemSymbolName: playing ? "pause.fill" : "play.fill", accessibilityDescription: nil)
    play.setAccessibilityLabel(playing ? "Pause video" : "Play video")
    play.toolTip = playing ? "Pause video" : "Play video"
    mute.image = NSImage(systemSymbolName: muted ? "speaker.slash.fill" : "speaker.wave.2.fill", accessibilityDescription: nil)
    mute.setAccessibilityLabel(muted ? "Unmute video" : "Mute video")
    play.isEnabled = ready
    mute.isEnabled = ready
    seek.isEnabled = ready && duration > 0
    if case .fallback(let player) = playback { seek.isEnabled = seek.isEnabled && player.seekable }
    if playing { pausedSeekSeconds = nil }
    let displayedSeconds = pausedSeekSeconds ?? seconds
    seek.doubleValue = duration > 0 ? displayedSeconds / duration : 0
    time.stringValue = clock(displayedSeconds) + " / " + clock(duration)
    if ready {
      let engine = if case .native = playback { "native" } else { "vlc" }
      state = "video-\(engine)-\(playing ? "playing" : "paused")"
    }
    didChange?()
  }
  private func finite(_ value: Double) -> Double { value.isFinite ? max(0, value) : 0 }
  private func clock(_ value: Double) -> String {
    let seconds = Int(finite(value))
    return seconds >= 3600
      ? String(format: "%d:%02d:%02d", seconds / 3600, seconds / 60 % 60, seconds % 60)
      : String(format: "%d:%02d", seconds / 60, seconds % 60)
  }
  private func fail(_ reason: String) {
    stop()
    playback = .failed
    state = "video-failed"
    message.stringValue = reason
    message.isHidden = false
    updateControls()
  }
  private func releaseNative() {
    readiness = nil
    if case .native(let player) = playback {
      player.pause()
      if let timeObserver { player.removeTimeObserver(timeObserver) }
      player.replaceCurrentItem(with: nil)
    }
    timeObserver = nil
    nativeView.player = nil
  }
  func stop() {
    generation = UUID()
    loadTask?.cancel(); loadTask = nil
    releaseNative()
    if case .fallback(let player) = playback {
      player.videoView.isHidden = true
      player.stop()
    }
    playback = .idle
    url = nil; ready = false; requestedPlayback = false; muted = false
    pausedSeekSeconds = nil
    state = "empty"
    updateControls()
  }
  override func layout() {
    super.layout()
    let height: CGFloat = 44
    picture.frame = NSRect(x: 0, y: 0, width: bounds.width, height: max(0, bounds.height - height))
    nativeView.frame = picture.bounds
    controls.frame = NSRect(x: 0, y: picture.frame.maxY, width: bounds.width, height: height)
    play.frame = NSRect(x: 10, y: 8, width: 32, height: 28)
    mute.frame = NSRect(x: bounds.width - 42, y: 8, width: 32, height: 28)
    let timeWidth: CGFloat = bounds.width > 360 ? 120 : 90
    time.frame = NSRect(x: bounds.width - 50 - timeWidth, y: 14, width: timeWidth, height: 18)
    seek.frame = NSRect(x: 52, y: 12, width: max(20, bounds.width - timeWidth - 114), height: 20)
    message.frame = NSRect(x: 20, y: max(0, picture.bounds.midY - 30), width: max(0, bounds.width - 40), height: 60)
  }
}
