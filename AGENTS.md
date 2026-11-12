# Working on Crawler

## Product and current scope

Crawler is a native macOS Finder replacement built with Swift and AppKit. The shipping source is `Sources/Crawler/`. Scope funnel is the default workspace, with Columns as the other view mode. `prototype/` contains the historical HTML/CSS/JavaScript mockup; changes there do not affect the native app.

Favorites contain Downloads, Desktop and at most one user-pinned folder. The initial workspace is Home, with recent files inside the empty-query view. Home and Recents are not sidebar destinations. Connected volumes, AirDrop, Activity Monitor, System Settings and read-only Trash have native workspaces. Some file mutation commands remain placeholders. Do not restore synthetic data or turn a placeholder into a destructive disk operation.

## Collaboration rules

- Keep code and explanations simple and concrete. Read and apply `~/.agents/skills/unslop/SKILL.md` before user-facing messages when available.
- Questions are read-only unless the user also requests implementation.
- Preserve unrelated local changes. This repository can contain substantial uncommitted native-app work.
- Do not add test files or test-only helpers/fixtures without explicit approval. Prefer existing checks and direct runtime verification.
- Use one agent for ordinary tasks. Delegate only when authorized and useful for breadth or independent review; assign file ownership first.
- Prefer native platform APIs and strong Swift types over adding dependencies or generic abstractions.

## Build and run

Requirements: macOS 14+, Xcode with Swift 6.2+ and the macOS SDK, Python 3.11+ and rustup. The app uses Swift Package Manager with pinned Tree-sitter/grammar and Swift Markdown dependencies. There is no WebView.

Build and launch the signed release app from the repository root:

```sh
scripts/build.sh
open "$PWD/build/Crawler.app"
```

The script runs both dependency bootstraps, builds release, packages resources and VLCKit, strips the packaged executable and signs `build/Crawler.app`. It retains the original executable and dSYM in `.build/release`. Signing requires a persistent `Crawler Local Development` certificate in the login keychain, or an existing certificate named by `CRAWLER_SIGNING_IDENTITY`. Never replace it with ad-hoc signing or recreate it for each build: changing the certificate breaks remembered privacy grants. Keep the bundle identifier `studio.crawler.native-prototype` unless the maintainer explicitly requests a migration.

For a direct debug build, prepare the dependencies first:

```sh
scripts/bootstrap-video.sh
scripts/bootstrap-fff.sh
swift build
```

Prefer the packaged app when checking permissions, fonts, previews and launch behavior. The debug executable alone is not equivalent. macOS controls protected-folder access; show failures and never bypass privacy controls. Usage descriptions do not grant access. Keep [LICENSE](LICENSE), [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md), bundled dependency licenses and generated Rust notices in packaged builds. Manual installation commands are in [README.md](README.md); there is no Homebrew package yet.

## Source map

Paths in the table are relative to `Sources/Crawler/` unless stated otherwise.

| Files | Responsibility |
| --- | --- |
| `main.swift` | App lifecycle, window, menus and launch/state diagnostics. |
| `Model.swift` | Typed paths, metadata, destinations, tabs, sorting and search modes. |
| `Workspace.swift`, `WorkspaceCommands.swift` | Navigation/history, selection, keyboard routing, pane layout and command actions. |
| `FileSystem.swift`, `FilePane.swift` | Background immediate-directory reads, bounded cache/watchers/sizes and recycled native tables. |
| `FileSearchPalette.swift`, `ScopeRail.swift` | Shared funnel workspace/search overlay, query/focus/scope history, recent files, context rail and folder panels. |
| `QuickFileSearch.swift` | Progressive discovery, query scheduling, cache reconciliation, FSEvents and idle release. |
| `FFFSearch.swift`, `SearchPaths.swift`, `Sources/FFFBridge/engine.rs` | FFF C ABI, immutable/mapped path snapshots, matching, ranking and highlights. |
| `SearchPerformance.swift` | Numeric search-stage tracing and first title draw diagnostics. |
| `FileSearch.swift` | Retained Spotlight backend, outside the current FFF search UI. |
| `FileCopy.swift` | Drag-and-drop copies with copyfile(3) byte progress and cancellation. |
| `FileOpening.swift`, `EditorPicker.swift` | Installed editor discovery, shared opening policy and recent-file history. |
| `PreviewPane.swift`, `DocumentPreview.swift`, `PreviewContent.swift`, `MarkdownPreview.swift` | Shared previews, metadata, bounded text reads, Tree-sitter and native Markdown rendering. |
| `MarkdownReader.swift` | Full-window Markdown reader. |
| `WordDocument.swift` | Word Open XML to Markdown conversion for the read-only Markdown reader. |
| `PDFReader.swift`, `NativePDFView.swift`, `PDFRendering.swift` | Full-window PDF reader, retained page layers and actor-owned Core Graphics rasterization/text/link metadata. |
| `PDFAnnotations.swift`, `PDFNoteEditor.swift` | Shared annotation editing/undo/save state and native note popovers. |
| `VideoPreview.swift`, `Sources/VideoFallback/` | AVFoundation playback and on-demand VLCKit fallback. |
| `FileIcons.swift`, `SidebarIcons.swift` | Shared file-type icons and sidebar artwork. |
| `MountedVolumes.swift`, `Sidebar.swift` | Volume discovery/ejection, favorites and native destinations. |
| `ActivitySampling.swift`, `ActivityMonitorView.swift`, `ActivityTreemap.swift`, `ProcessSearch.swift` | Real libproc samples, process table/treemap and independent process-name filtering. |
| `AirDropView.swift`, `AirDropFilePicker.swift`, `AirDropNetwork.swift`, `Sources/AirDropBridge/` | Recipient UI, file picker, public DNS-SD and experimental HTTPS transfers. |
| `SystemSettings*.swift`, `Sources/SystemSettingsBridge/` | Native settings catalogue, hardware operations, permissions, preferences and compatibility bridge. |
| `TrashView.swift`, `Utilities.swift` | Read-only Trash, utility composition and native preview helpers. |
| `Design.swift`, `ThemePalette.swift`, `TabStrip.swift`, `CommandPalette.swift`, `ShortcutSheet.swift` | Native controls/themes, tabs, command choices and keyboard help. |

## Directory browsing

Columns uses `Workspace` and its `FileSystemLibrary`; the funnel's empty-query folder panels use a separate bounded library in `FileSearchPalette`. Both enumerate immediate children in the background.

`FolderID` wraps a standardized absolute path. `ItemID` identifies the displayed file path. Normalize file IDs as well as folder IDs with `standardizedFileURL` because enumeration can return `/private/var` for a folder entered as `/var`. These are path identities, not inode identities: an external rename is a removed path plus a new path. Symlink paths are preserved for back/forward navigation. Directory symlinks can be entered; packages such as `.app` bundles are displayed as files and opened through macOS.

1. `Workspace.navigate` saves selection and scroll offset for the departing folder, updates the active tab's location and history, restores remembered selection, and renders.
2. `Workspace.items` asks `FileSystemLibrary.children` for the active directory. Cached rows are returned immediately. An uncached folder starts an asynchronous read and initially shows a loading state.
3. `DirectoryReader.read` runs in a detached task. Foundation enumerates only that directory, prefetches resource metadata, and sorts folders first using natural filename order. It does not recursively walk the disk or read file contents.
4. The result returns to the main actor and updates only that folder's cache entry. Unchanged cached snapshots do not trigger an update. The workspace redraws only if that directory is still visible; changes in a neighboring column update that column without rebuilding the current pane. A late response cannot navigate the user back to an old location.
5. Columns show parent, current folder, and selected child folder or file preview. Reading the selected child's listing also warms the cache before the user enters it.
6. File and preview panels scroll vertically only, with horizontal elasticity disabled. The table width follows its viewport. `NSTableView` reuses visible cells rather than creating an NSView for every item. Selection-only updates change the existing row selection and preview without reconfiguring or hiding the current and parent tables. Keyboard navigation scrolls only when the selection leaves the viewport; scrollbar overlays appear on mouse or trackpad scrolling, hide after 1.5 seconds of inactivity, and hide immediately on keyboard navigation. After pointer scrolling, the first arrow or Vim navigation key starts from the hovered row in that pane, then subsequent keys follow the keyboard selection. Clicking or navigating clears that handoff; momentum events do not re-arm it. Row hover is calculated from the current pointer position when painted, and visible row backgrounds are invalidated during scrolling so reused rows cannot leave hover trails.

The cache retains up to 32 directory listings after completed reads, protecting visible and in-flight entries from eviction. Returning to a cached folder displays its previous rows while refreshing in the background. Cache capacity counts directories, not bytes; unusually large directories still consume memory proportional to their entry count.

`FileSystemLibrary.watch` opens watcher descriptors in detached tasks, then installs dispatch filesystem sources for visible directories and closes their descriptors when the directories leave the viewport. Events debounce for 120 ms. An event received during enumeration waits for that read, then refreshes again. Watchers observe directory entry changes; they are not recursive content watchers. Command-R forces a refresh, including metadata changes that do not produce a directory event. A deleted/replaced watched directory may require revisiting it to attach a new watcher.

Each tab has its own history, selected path, and per-folder scroll offsets. Tabs and navigation state are session-only. The toolbar pins the current folder directly; the sidebar also offers a native folder picker. The one pinned path persists in UserDefaults under `pinnedFolder`; choosing another folder replaces it. A missing pin remains visible and reports the actual read error when opened.

## Navigation and layout

The compact header uses a read-only breadcrumb; segments are not buttons. The keyboard path editor remains available. Columns uses three equal-width parent/current/child-or-preview panels, hides back/forward controls and keeps dividers stable while selection changes. Scope funnel owns its search and two-panel folder browsing layouts.

File tables reuse native NSTableView cells. Preserve selection and scroll without rebuilding unrelated panels. Keyboard monitoring is limited to the main workspace window and disabled while a sheet is attached so native pickers and preview windows receive their own keys.

## Scope funnel and FFF search

Scope funnel is the default view, initially at Home. The internal `ViewMode.list` case names this workspace, not the old list UI. `FileSearchPalette.swift` owns the shared main-workspace/search-overlay composition, selection, scope history and focus. `ScopeRail.swift` supplies recycled native scope and nearby-entry lists. Columns remains available. Cmd+P focuses the current funnel query; in Columns it opens the funnel over Home.

Scope changes filter the resident index before top-result truncation. Child counts are merged from the full background matching pass for diagnostics; the left rail displays folder names without numeric counts. Navigation and ordinary narrowing reuse the Mac-wide index. Explicit current-folder searches inside directory symlinks add one temporary alias subtree, preserving entered paths without following every symlink on the Mac. Recursive discovery and cache loading start when a nonempty file query needs the index. Empty Home and immediate-folder browsing do not load it.

The context reader stops at 80 immediate entries and keeps four parent snapshots, plus the selected entry. It does not recursively populate a tree. Metadata, parent reads and previews reject stale completions; parent enumeration cooperatively cancels. Scope funnel tabs retain their query, scope history, selection and scroll position. Enter, Cmd+Enter, Space and the existing opening-editor policy remain native.

Scope funnel search uses upstream fff-search 0.11.0 at c188e7a8f90e4ee4aabe18f93d5869dfb4289f12. `scripts/bootstrap-fff.sh` compiles the Rust static library with the C ABI adapter in `Sources/FFFBridge/engine.rs`; upstream scoring is unchanged. Multiword queries try FFF literal term constraints first and retry with FFF fuzzy matching once cached paths have no literal result, without waiting for disk reconciliation. Newly discovered literal matches take priority over that fallback. Swift discovery supplies bounded immutable snapshots and filters scopes before FFF scoring. The batch scheduler owns search parallelism. Cache saves contain original paths/folder flags and FFF’s exact prepared SIMD chunks and path references. Trigram signatures only prioritize cache-block arrival; they must never filter candidates. Preserve whole snapshot boundaries. Convert highlights only for the global top 100 and retain access-history scoring. Keep the live total-match count out of the UI; scope counts and diagnostic totals remain available. Measure physical footprint, including compressed memory, rather than presenting RSS as total memory. The build strips the packaged executable before signing and retains the original executable and dSYM in `.build/release`.

When Home has an empty or whitespace-only query, show a fixed centered bordered search field above two panels: up to eight recently opened files and the selected file preview. Keep the search field in the same position during searches. Skip empty-query ranking, scope counts and nearby-context loading there; explicit folder browsing keeps the normal funnel. Recent files come from successful FileOpening operations in the separate bounded `recentOpenedFiles` preference, never from ranking visits, folder navigation or previews. Read recent metadata off the main actor.

Explicit folder navigation with an empty query uses two recycled FilePane instances: current directory and selected child directory or file preview. These read only immediate children through a bounded FileSystemLibrary with watchers for visible directories. Do not use the recursive top-100 search response as a folder listing. Enter/right/l enters a folder; left/h returns to its parent. Typing restores scoped search; clearing restores directory browsing.

Search defaults to This Mac using the resident fuzzy index rooted at `/`, independent of navigation. Typing `$` activates a current-folder prefix filter without inserting the symbol or changing engines. A pasted leading `$` is consumed too. Store mode separately from visible query text, preserve both in tab state, and use a subtle tinted field plus a noninteractive scope label. Clearing the query or Backspace in an empty local field restores This Mac. Do not add a mode toggle. All result paths resolve against `/`. Discovery/cache loading starts on a nonempty query, publishes progressively, and persists a separate Mac-wide FFF path-snapshot cache. Leaving search or clearing the query cancels expensive work immediately, retains batches for a 60-second grace period, then drops index buffers and stops its watchers. Returning within that grace period reuses resident batches. Older cache formats migrate to `.fff3.bin` prepared snapshots. Cache headers validate the root and pinned engine revision. Swift metadata and Rust scoring borrow the same read-only mapping; snapshots retain it through an Arc until their last query finishes. Cache writes atomically replace the file, never truncate a mapped inode. Version 3 aligns prepared index tables; older unaligned tables alone are copied during migration. Keep compact ChunkedString references resident and materialize bounded FileItems only for scoring. Matching and ranking belong to FFF. Reconciliation reuses unchanged batches instead of preparing a second complete index. Skip `/dev` and the duplicate macOS mount hierarchy `/System/Volumes`; ordinary permission, hidden-entry, package, and symlink exclusions remain. Do not label Home-only coverage as computer-wide. Keep old rows visible during replacement queries, but do not activate them as results of the new query before its first publication.

Both search modes show “Narrow the search” and “Where it lives”. This Mac narrows from `/` using an explicit search-only scope, independent of the folder being browsed; clearing the query restores the full Mac scope. Keep the same bounded context reader and fuzzy matcher. Do not hide these panels when changing search modes.

Search presents its first nonempty usable response immediately, including after an initial empty publication. Later UI updates share a 40 ms window while FFF keeps searching without a typing debounce. Preserve the selected path, loaded preview and scrolled viewport anchor as results arrive; skip reconfiguring unchanged visible cells. Final index reconciliation replaces the same query atomically, without exposing intermediate subsets or resetting the selected preview. Numeric scope counts remain diagnostic only.

## Preserve seamless search

Never replace or bypass the resident fuzzy search engine when changing search scope, navigation, themes, or controls. Those changes must preserve matching, ranking, progressive results, and typing responsiveness unless the maintainer explicitly requests an engine or behavior change. This Mac and `$` current-folder mode must use the same fuzzy matcher; `$` changes only the scope filter. Do not substitute query-time Spotlight matching, add a debounce, or clear visible results while a replacement query is running. Verify rapid typing, Escape/refocus, fuzzy abbreviations, and scope changes in the native app before declaring a search change complete. An optimization must not silently remove existing behavior.

## Search query cache and tracing

Up to eight complete queries retain bounded owned hits, diagnostic totals and scope counts. Keys include normalized text, mode, scope, volume and exact integer history boosts. Index path/kind changes, batch appends, history changes and idle release invalidate them. Content-only reconciliation reuses unchanged snapshots without restarting the query or clearing this cache. Default search parallelism is min(12, active processor count); `--search-workers N` overrides it for diagnostics. Cache-reload diagnostics distinguish a true resident engine pass from a subsequent query-cache hit.

`SearchPerformance.swift` retains 32 numeric stage samples and emits the `Search to first title draw` Points of Interest interval. `Search Performance` logs input dispatch, publication, row preparation and the first visible title's AppKit draw. The separately named window notification can arrive late and must not be presented as rendering completion. Draw timings exclude compositor presentation. Normal native profiling must exclude state/event report writes and accessibility-tree polling during capture; the latter caused substantial measured main-thread work.

`SearchPaths.swift` stores mapped original UTF-8 paths as offsets and flags, with owned arrays for new discoveries. Deduplication/reconciliation use bounded batch/row locations and process-seeded hash lookups, always verifying full path equality after a hash match. Non-ASCII comparisons retain Swift canonical Unicode equality. Neither hash nor trigram hints may exclude FFF candidates.

## Command palette and themes

Command-Shift-P opens the command palette for navigation, selected-file actions, tabs, views, themes and utilities. `WorkspaceCommands.swift` captures the visible selection/folder before presenting it. Hide unavailable actions, preserve native file-opening policy, and use NSWorkspace URLs for Terminal/editor actions rather than executing shell strings. Theme/editor/drive choices stay inside the palette; Escape returns to commands before dismissing it. Confirmed themes persist under `theme`; previews and cancellation must not alter that saved preference.

## Documents and PDF annotations

Code, Markdown, plain text, PDFs and video share `DocumentPreview` between panes and the Space window. Keep file reads, Markdown parsing and Tree-sitter work off the main actor, bounded and cancellation-aware. `QLThumbnailGenerator` provides asynchronous fallbacks for other formats; missing thumbnails show real metadata and an icon.

Enter/double-click and search activation open PDFs in `PDFReader` inside the Crawler window. X, two consecutive non-repeated Escape presses within 700 ms, or Command-W restore the existing workspace and focus. The header button beside the title copies the file name with its extension. Reuse an already loaded preview's `PDFSource` and bitmap cache; otherwise read only the selected PDF in the background. Suspend hidden previews while the reader or Space window is open. Space opens the shared native renderer in a resizable window and closes with Space or Escape.

`NativePDFView` composites retained page layers. `PDFRasterizer` owns Core Graphics parsing/rasterization and PDFKit metadata/editing on its actor. PDFKit supplies text, selection, links and annotation serialization; never restore PDFView or PDFKit page rendering. Read only the first page geometry before initial display, estimate unread layout slots, and resolve them near the viewport. Retain visible surfaces plus one neighboring page on each side, cap the worker bitmap cache at 128 MiB and use bounded crops at large zoom levels. Do not eagerly parse or rasterize a long PDF.

PDF tools stay active between strokes. Choose Highlight, Underline or Note, then drag over text to see the mark while drawing. Notes use dotted underlines with a hover popover and a click-to-edit popover. `PDFAnnotationSession` belongs to the shared source so previews and the reader see the same marks. Keep pending strokes, Undo/Redo and Save ordered. Remove annotations clears supported highlights, underlines and text notes on every page, including saved marks and unvisited pages; scan annotation metadata only for that explicit action and preserve links, forms and unsupported annotations. Removal is undoable.

Command-S saves annotations into the PDF; Shift-Command-S saves a copy. Closing the reader/window or quitting must handle unsaved changes and in-flight operations. Failed saves retain edits. Detect external file changes before replacing the original; require Save As on conflict. Encrypted PDFs and those that disallow annotations remain read-only. Text tools require a text layer; there is no OCR or editing of the underlying document text.

Enter/double-click and search activation open Markdown in `MarkdownReader` inside the Crawler window, sharing the PDF reader's cover/restore path (`presentReader`) and close keys. It reads up to 4 MB through `PreviewTextLoader` with the `.reader` budget; panes keep the 256 KB `.preview` budget. `MarkdownStyle` styles both from the same semantic spans; only the typography differs. The reader centers a column of about 70 characters. It embeds local images (at most 48, downsampled off the main actor) and never fetches remote ones. `#fragment` links scroll to GitHub-style heading slugs, and links to other Markdown files open in the reader with Command-[ to go back. Command-F finds, the header zoom buttons, a trackpad pinch and Command-plus/minus/0 zoom in 10% steps from 70% to 200%, reflowing the text (persisted as `markdownReaderScale`), and E or Open in editor hands the file to the chosen editor. Raw HTML never runs: inline tags are dropped, comments hidden and other HTML blocks shown as source.

Enter/double-click and search activation open Word documents (`.docx`, `.docm`, `.dotx`) read-only in `MarkdownReader`: `WordDocument` reads the package parts with the system `unzip` inside `PreviewTextLoader` and converts headings (Title and heading styles), bold, italics, strikethrough, lists, tables and external links to Markdown, so zoom, column, find and themes match Markdown. Fonts, colors, images and page layout are dropped. Panes keep the Quick Look thumbnail. E or Open in app opens the original in its default app. `.doc`, `.odt` and `.pages` still open in their default app.

Code and plain text open through `FileOpening` and the chosen installed editor; other formats use macOS defaults. Crawler Settings and the Open dropdown share `defaultTextEditorBundleID`. Discover app declarations in the background; do not hard-code editors or modify global file associations. Record recent-file history only after a successful open/display.

## Video previews

`VideoPreview.swift` is owned by `DocumentPreview`, shared between panes and the Space window. Native AVFoundation playback is preferred; the small Objective-C `VideoFallback` target loads the bundled VLCKit framework on demand for MKV and unsupported native formats. `scripts/bootstrap-video.sh` downloads a pinned, checksummed official distribution before compilation; `scripts/build.sh` embeds and signs it with the existing identity. Keep playback opt-in, selected-file-only, and cancellation-aware. Pause preparation must remain silent. Serialize VLC transport and teardown off the main actor, retain its drawable through shutdown, and stop hidden players. Space suspends the pane's decoder and restores its paused preview when closed. Never introduce background transcoding or scan video contents across a directory.

## Connected volumes

`MountedVolumes.swift` discovers browsable mounted volumes off the main actor and observes macOS mount/unmount/rename notifications. Locations shows these volumes directly, excluding the startup and hidden system volumes. Clicking a drive opens the existing folder-and-preview funnel. The default query scope is that drive, retained across folder navigation and tabs; `$` still narrows to the current folder. Empty queries restore immediate directory browsing. The selected volume is indexed progressively through the existing resident fuzzy matcher; its supplementary batches replace duplicate Mac-cache coverage during drive searches. Native ejection reports errors and never forces an unmount. Dragging search results, recent files or folder rows onto a sidebar folder or drive copies them there in the background (`FileCopy.swift`) without changing the view. A bottom card shows byte progress and Cancel; one copy runs at a time. Existing names are skipped, never replaced (`COPYFILE_EXCL`), and a cancelled or failed item removes only its own partial target. Read-only volumes, such as NTFS drives that macOS mounts read-only, are reported before copying. Drags to other apps offer only copy, never move.

## Background sizes

Regular file sizes come from metadata. The active directory gets one utility-priority scan that totals logical file bytes in its immediate child folders and packages, including hidden contents. Scans do not follow symlinks and stop cooperatively when the active directory changes or refreshes. Errors leave an unknown size instead of publishing a partial total. Results update cached items, visible cells, and the selected preview without rebuilding the file panels. Parent and child preview columns do not independently launch recursive scans. Totals are logical bytes, not allocated disk space; hard-linked paths count separately. Visible-directory watchers do not detect every nested change, so Command-R recomputes totals. Large folders can take time to finish.

## Performance and correctness constraints

Keep enumeration, resource metadata reads, and default directory sorting off the main actor. Do not pre-walk the user's home directory or load file contents to draw browsing rows. Nonempty search starts progressive Mac-wide discovery; empty-query browsing does not start the recursive index. Typing must never wait for a complete scan. Recursive size scans are separate cancellable background work for the active folder, never a prerequisite for drawing its entries. Avoid synchronous per-file icon lookups in table rendering. The main actor owns AppKit and cache publication; detached work returns Sendable file values.

Uncached reads currently publish a complete sorted directory snapshot. They do not stream batches or cancel an OS enumeration already in progress. Explicit non-default sorting, filtering, and display-row preparation run on the main actor. Profile these before claiming responsiveness for enormous folders or slow/network volumes.

Display loading, empty, and failed states distinctly. Do not convert permission errors into empty folders. Missing per-file metadata is represented as unknown, not invented dates or sizes. Ordinary directory listings omit hidden entries; Trash has its own policy.

## System settings workspace

`SystemSettingsView.swift` owns the ten-section native workspace and controls. `SystemSettingsModel.swift` owns the catalogue/background row reads; `SystemSettingsAudio.swift` owns Core Audio. Sound supports input/output selection, output volume/mute, input gain, balance when exposed, and effects output. `SystemSettingsWireless.swift` retains scan objects on an actor, connects through CoreWLAN, and connects/disconnects paired Bluetooth devices. Scan requests Location authorization through `SystemSettingsPermissions.swift`; network passwords stay in memory for the request and never enter logs or preferences. The native Bluetooth pairing assistant handles pairing confirmation.

`SystemSettingsAutomation.swift` serializes public System Events scripting for dark appearance and Dock properties. Permission is requested only by the explicit Enable controls action. Scripts accept only enum properties and validated values. Privileged commands escape each shell argument and then the containing AppleScript literal separately. `SystemSettingsPower.swift` reads pmset values; low-power/display-sleep changes use fixed commands with macOS administrator authorization and readback, without installing a privileged helper. Keyboard uses public TIS input-source selection.

The maintainer authorized private API integration to prioritize settings coverage. `SystemSettingsBridge` isolates dynamic calls with symbol/signature checks; `SystemSettingsPrivate` confirms resulting states. Bluetooth power uses the private IOBluetooth preference function. AirPods listening modes use advertised Core Audio `lstm`/`lsms` properties. AVOutputContext advanced controls are capability-gated; a missing system audio context or required entitlement must leave them unavailable. Do not forge entitlements or claim inaccessible controls work. `SystemSettingsPreferences` uses CFPreferences for language, locale, accent, repeat timing and configured symbolic shortcuts, preserving unrelated keys; some consumers need a new login. Keyboard repeat also applies through IOHID and checks readback. General uses validated, independently shell/AppleScript-escaped systemsetup/scutil commands with native admin authorization, and softwareupdate for explicit checks/downloads/installs. Never run updates just to verify UI. Keep permissions in the signed bundle. Hardware refreshes stop offscreen, stale reads cannot change sections, and pending operations must not prevent browsing another settings section.

Carbon TIS input-source enumeration, property reads and selection must run on MainActor. Reading source properties on a worker triggered a macOS dispatch-queue assertion during native UI verification.

Sound and AirPods audio values subscribe to Core Audio property notifications through SettingsAudioObservation while the settings workspace is active. Register off the main actor, reattach on hardware-list changes, and remove listeners offscreen. Coalesce events arriving during a read into one follow-up refresh. The five-second refresh remains a fallback for other settings, not the normal volume-update path.

See [the current settings guide](docs/system-settings-design.md) for supported operations and capability limits.

## AirDrop

AirDrop uses `AirDropView.swift`, a custom `AirDropFilePicker.swift`, public DNS-SD in `AirDropBridge`, and the experimental HTTPS protocol in `AirDropNetwork.swift`. Current receivers require `/Ask` to declare `TransferID` as `{id: UUID}` and `TransferType` as `{files: {}}`; `/Upload` must repeat that UUID in its `TransferID` header. Keep these together in `AirDropPreparedFiles`.

The AirDrop log category records acknowledgment times and submitted archive bytes without filenames or identity credentials. Discovery is intermittent when the legacy service disappears while Finder still sees the device. Custom sending requires macOS 15 or later, Everyone mode, and a self-issued client certificate imported only into memory. Never add private discovery entitlements, extract Apple-account credentials, invent devices or progress, or bypass TLS validation. The explicit macOS AirDrop fallback uses `NSSharingService.sendViaAirDrop`; its didShare callback also fires on picker cancellation and is not a delivery receipt.

## Trash workspace

`TrashView.swift` resolves the current user’s Trash with Foundation on the home and mounted volumes without creating directories. It reads immediate entries off the main actor, includes hidden files except `.DS_Store`, deduplicates Trash paths, and displays metadata in the existing native FilePane table. Column headers sort; arrow keys and optional j/k select rows. Refresh and Command-R reread the list; a three-second refresh runs only while visible, including mount changes. Missing Trash directories are empty; other failures remain visible alongside any readable items. macOS may require Full Disk Access. This view is read-only; restoration and emptying are not implemented.

## Verification

Match verification to the change. For documentation-only changes, check links, paths, commands and the diff. For native behavior changes, build the release app and exercise the affected UI; a build alone does not prove behavior. Use disposable data outside personal folders. Never modify user files to test mutations.

For navigation changes, check direct paths, parent/child traversal, back/forward, tabs, empty/missing folders, pinned-folder persistence, connected volumes and external directory changes. For search changes, check rapid typing, Escape/refocus, fuzzy abbreviations, both modes, scope changes, progressive results and selection/preview preservation. For PDF changes, verify annotations after saving and reopening disposable copies, Undo/Redo, removal across unvisited pages, unsaved-close handling and the measurements below.

```sh
open -n "$PWD/build/Crawler.app" --args \
  --state-report /tmp/crawler-state.json \
  --launch-report /tmp/crawler-launch.json
```

Existing diagnostics:

- `scripts/crawler [path]` sends a folder or file to Crawler through `open`, launched or already running. `AppDelegate.application(_:open:)` navigates folders and opens files with the Enter policy; `CFBundleDocumentTypes` declares folders with rank None, so Crawler never becomes the default folder handler.
- `--folder <absolute-path>` changes the initial Home workspace to that directory.
- `--find <query>` opens/focuses search with an initial query.
- `--open <absolute-path>` opens a file at launch through the normal Enter policy, including the PDF and Markdown readers. Combine it with `--snapshot` to check reader rendering without driving the UI.
- `--search-root <absolute-path>` sets the initial browsing/local-search context for diagnostics. This Mac still uses the `/` index; use `$` folder mode to constrain a disposable search.
- `--search-workers N` overrides the bounded matching worker count.
- `--activity` and `--settings` launch directly into those utility workspaces.
- `--snapshot <path>` saves an app-side startup screenshot; `--quit-after <seconds>` exits automatically.
- `--event-log <existing-path>` appends state JSON when state reporting is active.

State reports include search status/results/selection, query/index/cache timing, preview readiness, PDF rendering/annotation state, Markdown reader state/load time and Activity Monitor samples. Reports can contain private paths and filenames; keep them local. `scripts/benchmark-startup.py` measures app-side window readiness, not asynchronous directory loading or compositor presentation. Do not claim cold-disk results from warm files or infer FPS from callback timers. Profile normal launches without state/event writes or accessibility polling during capture.

## PDF performance

For every PDF viewer change, measure the packaged app on representative real documents, including the longest available one. Record time to first bitmap, time to the first display-resolution bitmap, parsing/raster costs and peak scroll-update cost separately. The `PDF Performance` log category records numeric timings without paths or contents; `--state-report` exposes `pdf_reader_load_ms` and `pdf_reader_rendering` (including `first_sharp_ms`, `parse_ms`, `max_raster_ms`, cache bytes and hits). Bitmap-ready timings exclude compositor presentation. Profile main-thread work with Time Profiler on a normal launch without state/event reports, because diagnostic file writes can distort responsiveness. Never infer FPS from a callback timer or claim cold-disk results from a warm local file. Prioritize first visible content and responsive input over finishing all pages; do not eagerly parse or rasterize an entire long PDF.
