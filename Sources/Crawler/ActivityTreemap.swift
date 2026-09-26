import AppKit

/// A node's value includes its descendants. A process with children gets a separate leaf for its own usage.
struct ActivityMapNode: Equatable {
  enum ID: Hashable {
    case group(String)
    case process(ActivityProcessID)
    case own(ActivityProcessID)
    case other
    indirect case remainder(ID)
  }
  let id: ID
  let title: String
  let value: Double
  let children: [ActivityMapNode]
  let process: ActivityProcessID?

  static func limited(_ nodes: [Self], parent: ID) -> [Self] {
    let sorted = nodes.filter { $0.value > 0 }.sorted { $0.value == $1.value ? $0.title < $1.title : $0.value > $1.value }
    guard sorted.count > 8 else { return sorted }
    let tail = Array(sorted.dropFirst(8))
    return Array(sorted.prefix(8)) + [Self(id: .remainder(parent), title: "\(tail.count) more processes",
      value: tail.reduce(0) { $0 + $1.value }, children: tail, process: nil)]
  }

  static func forest(_ processes: [ActivityProcess], metric: ActivityMetric) -> [Self] {
    let ids = Set(processes.map { $0.id.pid })
    let children = Dictionary(grouping: processes.filter { ids.contains($0.parent) && $0.parent != $0.id.pid }, by: \.parent)
    func node(_ process: ActivityProcess, seen: Set<Int32>) -> Self {
      let own = metric.value(process) ?? 0
      let descendants = (children[process.id.pid] ?? []).filter { !seen.contains($0.id.pid) }.map {
        node($0, seen: seen.union([$0.id.pid]))
      }
      let leaves = descendants.isEmpty ? [] :
        [Self(id: .own(process.id), title: process.name, value: own, children: [], process: process.id)] + descendants
      return Self(id: .process(process.id), title: process.name,
        value: own + descendants.reduce(0) { $0 + $1.value }, children: leaves, process: process.id)
    }
    return processes.filter { !ids.contains($0.parent) || $0.parent == $0.id.pid }
      .map { node($0, seen: [$0.id.pid]) }
  }
}

@MainActor
final class ActivityTreemap: Surface {
  struct Tile: Equatable {
    let node: ActivityMapNode
    let rect: NSRect
    let depth: Int
    let color: NSColor
    let container: Bool
    let valueText: String
  }
  private var nodes: [ActivityMapNode] = []
  private var showChildren = false
  private var renderedSize = NSSize.zero
  private(set) var redrawCount = 0
  private(set) var layoutCount = 0
  func update(nodes: [ActivityMapNode], metric: ActivityMetric, showChildren: Bool) {
    guard self.nodes != nodes || self.metric != metric || self.showChildren != showChildren else { return }
    self.nodes = nodes; self.metric = metric; self.showChildren = showChildren
    rebuild()
  }
  var visibleNodes: [ActivityMapNode] { nodes }

  var metric: ActivityMetric = .memory
  var selected: ActivityMapNode.ID? { didSet { if oldValue != selected { needsDisplay = true } } }
  var keyboardFocus = false { didSet { if oldValue != keyboardFocus { needsDisplay = true } } }
  var choose: ((ActivityMapNode, Bool) -> Void)?
  private(set) var tiles: [Tile] = []
  private var hovered: ActivityMapNode.ID?
  private var tracking: NSTrackingArea?
  override init(frame: NSRect) {
    super.init(frame: frame)
    setAccessibilityElement(false)
    setAccessibilityRole(.group)
    setAccessibilityLabel("Resource map. Rectangle area represents measured usage.")
  }
  required init?(coder: NSCoder) { nil }
  override func layout() {
    super.layout()
    if renderedSize != bounds.size { renderedSize = bounds.size; rebuild() }
  }
  override func updateTrackingAreas() {
    super.updateTrackingAreas()
    if let tracking { removeTrackingArea(tracking) }
    let area = NSTrackingArea(rect: .zero, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
    addTrackingArea(area); tracking = area
  }
  override func mouseMoved(with event: NSEvent) {
    let next = hit(convert(event.locationInWindow, from: nil))?.node.id
    if hovered != next { hovered = next; needsDisplay = true }
  }
  override func mouseExited(with event: NSEvent) { hovered = nil; needsDisplay = true }
  override func mouseDown(with event: NSEvent) {
    guard let tile = hit(convert(event.locationInWindow, from: nil)) else { return }
    choose?(tile.node, event.clickCount > 1)
  }
  private func hit(_ point: NSPoint) -> Tile? { tiles.last { $0.rect.contains(point) } }

  /// Balanced binary partitions preserve area while keeping rectangles readable at every window size.
  static func partition(_ nodes: [ActivityMapNode], in rect: NSRect) -> [(ActivityMapNode, NSRect)] {
    let positive = nodes.filter { $0.value > 0 }.sorted {
      $0.value == $1.value ? $0.title < $1.title : $0.value > $1.value
    }
    guard !positive.isEmpty, rect.width > 0, rect.height > 0 else { return [] }
    func split(_ list: ArraySlice<ActivityMapNode>, _ box: NSRect) -> [(ActivityMapNode, NSRect)] {
      guard list.count > 1 else { return list.map { ($0, box) } }
      let total = list.reduce(0) { $0 + $1.value }
      var accumulated = 0.0
      var cut = list.startIndex + 1
      for i in list.indices.dropLast() {
        accumulated += list[i].value
        cut = i + 1
        if accumulated >= total / 2 { break }
      }
      let first = list[..<cut], second = list[cut...]
      let fraction = CGFloat(first.reduce(0) { $0 + $1.value } / total)
      if box.width >= box.height {
        let width = box.width * fraction
        return split(first, NSRect(x: box.minX, y: box.minY, width: width, height: box.height))
          + split(second, NSRect(x: box.minX + width, y: box.minY, width: box.width - width, height: box.height))
      }
      let height = box.height * fraction
      return split(first, NSRect(x: box.minX, y: box.minY, width: box.width, height: height))
        + split(second, NSRect(x: box.minX, y: box.minY + height, width: box.width, height: box.height - height))
    }
    return split(positive[...], rect)
  }
  func rebuild(force: Bool = false) {
    layoutCount += 1
    var nextTiles: [Tile] = []
    func add(_ nodes: [ActivityMapNode], rect: NSRect, depth: Int, color: NSColor?) {
      for (node, box) in Self.partition(nodes, in: rect) {
        let palette: [UInt32] = Design.dark
          ? [0x2c4058, 0x2b453b, 0x4b4030, 0x42364f, 0x503931, 0x2b444b]
          : [0xd9e9fc, 0xdbece4, 0xeee2cd, 0xe8dff4, 0xf5dfd9, 0xd8ebef]
        let hash = node.title.utf8.reduce(UInt32(0)) { ($0 &* 31) &+ UInt32($1) }
        let fill = color ?? Design.color(palette[Int(hash % UInt32(palette.count))])
        let tileRect = box.integral.insetBy(dx: 1.5, dy: 1.5)
        let container = showChildren && depth < 1 && !node.children.isEmpty && tileRect.width > 110 && tileRect.height > 95
        nextTiles.append(Tile(node: node, rect: tileRect, depth: depth, color: fill, container: container, valueText: metric.formatted(node.value)))
        if container {
          add(ActivityMapNode.limited(node.children, parent: node.id), rect: NSRect(x: tileRect.minX + 5, y: tileRect.minY + 39,
            width: tileRect.width - 10, height: tileRect.height - 44), depth: depth + 1,
            color: fill.blended(withFraction: 0.17, of: Design.raised))
        }
      }
    }
    add(nodes, rect: bounds.insetBy(dx: 2, dy: 2), depth: 0, color: nil)
    // Compare what is painted, not the full hidden subtree or subpixel counter changes.
    let changed = force || nextTiles.count != tiles.count || zip(nextTiles, tiles).contains { next, old in
      next.node.id != old.node.id || next.node.title != old.node.title || next.rect != old.rect
        || next.valueText != old.valueText || next.container != old.container || next.color != old.color
    }
    tiles = nextTiles
    guard changed else { return }
    let elements = tiles.filter { $0.rect.width > 24 && $0.rect.height > 20 }.map { tile in
      let element = ActivityMapAccessibility { [weak self] in self?.choose?(tile.node, true) }
      element.setAccessibilityParent(self)
      element.setAccessibilityRole(.button)
      element.setAccessibilityEnabled(true)
      element.setAccessibilityLabel("\(tile.node.title), \(tile.valueText)")
      element.setAccessibilityFrameInParentSpace(tile.rect)
      return element
    }
    setAccessibilityChildren(elements)
    needsDisplay = true
  }
  var navigable: [Tile] { tiles.filter { $0.rect.width >= 28 && $0.rect.height >= 24 } }
  func move(_ delta: Int, edge: Bool = false) -> ActivityMapNode? {
    let list = navigable
    guard !list.isEmpty else { return nil }
    let index = list.firstIndex { $0.node.id == selected } ?? 0
    let next = edge ? (delta < 0 ? 0 : list.count - 1) : max(0, min(list.count - 1, index + delta))
    return list[next].node
  }
  override func draw(_ dirtyRect: NSRect) {
    super.draw(dirtyRect)
    redrawCount += 1
    if tiles.isEmpty {
      drawText(metric == .memory ? "No measurable memory in this scope." : "No measured activity in this sample.",
        in: bounds.insetBy(dx: 24, dy: max(24, bounds.height / 2 - 12)), size: 13, color: Design.muted, align: .center)
      return
    }
    for tile in tiles where tile.rect.width > 2 && tile.rect.height > 2 {
      let active = tile.node.id == selected
      let container = tile.container
      let path = NSBezierPath(roundedRect: tile.rect, xRadius: Design.classic ? 0 : 5, yRadius: Design.classic ? 0 : 5)
      (Design.classic ? NSColor.white : tile.color).setFill(); path.fill()
      (Design.classic ? NSColor.black : tile.color.blended(withFraction: 0.25, of: Design.text) ?? Design.line).setStroke()
      path.lineWidth = 1; path.stroke()
      if active || tile.node.id == hovered {
        Design.accent.setStroke()
        let outline = NSBezierPath(roundedRect: tile.rect.insetBy(dx: 1, dy: 1), xRadius: Design.classic ? 0 : 4, yRadius: Design.classic ? 0 : 4)
        outline.lineWidth = active && keyboardFocus ? 3 : 2; outline.stroke()
      }
      guard tile.rect.width > 48, tile.rect.height > 25 else { continue }
      let x = tile.rect.minX + 9, width = tile.rect.width - 18
      drawText(tile.node.title, in: NSRect(x: x, y: tile.rect.minY + 7, width: width, height: 17),
        size: tile.depth == 0 ? 12 : 11, color: Design.text, weight: .medium)
      if tile.rect.height > 43, !container {
        drawText(tile.valueText, in: NSRect(x: x, y: tile.rect.minY + 28, width: width, height: 17), size: 11, color: Design.text)
      } else if container, tile.rect.width > 150 {
        drawText(tile.valueText, in: NSRect(x: x, y: tile.rect.minY + 23, width: width, height: 14), size: 10, color: Design.text)
      }
    }
  }
}

@MainActor
private final class ActivityMapAccessibility: NSAccessibilityElement {
  nonisolated let press: @MainActor @Sendable () -> Void
  init(press: @escaping @MainActor @Sendable () -> Void) { self.press = press; super.init() }
  nonisolated override func accessibilityPerformPress() -> Bool {
    let action = press
    return MainActor.assumeIsolated { action(); return true }
  }
}
