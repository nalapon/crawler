# Working on Crawler

## Product and current scope

Crawler is a native macOS app for finding and reading files. The shipping app is the Rust port built on Rune in `crawler-rune/`. The Swift/AppKit app in `Sources/Crawler/` is the reference implementation that the rest of this document describes; the port must match it feature for feature. The title bar holds five sections: Search, Folders, Activity, Transfers and Settings. Search and Folders share the scope funnel. `prototype/` contains HTML mockups, including the historical Finder mockup and the section shell prototype; changes there do not affect the native app.

Folders shows places for Home, Downloads, Desktop, at most one user-pinned folder and connected volumes. Search starts at Home, with recent files inside the empty-query view. Activity is the process monitor, Transfers is AirDrop and Settings holds the theme and default text editor. There are no tabs, sidebar, Columns view, System Settings workspace or Trash workspace. Crawler does not move, rename or delete files. Do not restore synthetic data or add destructive disk operations unless the maintainer asks for them.

## Collaboration rules

- Keep code and explanations simple and concrete. Read and apply `~/.agents/skills/unslop/SKILL.md` before user-facing messages when available.
- Questions are read-only unless the user also requests implementation.
- Preserve unrelated local changes. This repository can contain substantial uncommitted native-app work.
- Do not add test files or test-only helpers/fixtures without explicit approval. Prefer existing checks and direct runtime verification.
- Use one agent for ordinary tasks. Delegate only when authorized and useful for breadth or independent review; assign file ownership first.
- Prefer native platform APIs and strong Swift types over adding dependencies or generic abstractions.

## Build and run

Requirements: macOS 14+, rustup (the crate pins its toolchain in `crawler-rune/rust-toolchain.toml`) and Python 3.11+. The Swift reference app additionally needs Xcode with Swift 6.2+ and the macOS SDK. There is no WebView.

Build and launch the signed release app from the repository root:

```sh
scripts/build.sh
open "$PWD/build/Crawler.app"
```

The script builds the Rust port in release, packages the icon, licenses and generated Rust notices (`scripts/rust-notices.py`), strips the packaged executable, signs `build/Crawler.app` and swaps it in place of the previous bundle. It retains the unstripped executable in `crawler-rune/target/release`. `scripts/build-swift.sh` still builds the Swift reference app as `build/Crawler-Swift.app`, with both dependency bootstraps and VLCKit. Signing requires a persistent `Crawler Local Development` certificate in the login keychain, or an existing certificate named by `CRAWLER_SIGNING_IDENTITY`. Never replace it with ad-hoc signing or recreate it for each build: changing the certificate breaks remembered privacy grants. Keep the bundle identifier `studio.crawler.native-prototype` unless the maintainer explicitly requests a migration. Both apps share it, so the port keeps the Swift app's privacy grants and imports its settings once on first launch.

For a direct debug build of the Swift reference app, prepare the dependencies first:

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
| `Model.swift` | Typed paths, metadata, sorting, search modes and saved funnel sessions. |
| `Workspace.swift`, `WorkspaceCommands.swift` | Sections, Folders history, funnel sessions, keyboard routing and command actions. |
| `Shell.swift`, `SettingsView.swift` | Section bar, Folders places bar with drop-to-copy, status bar, progress bar, theme-drawn shell buttons and Crawler settings. |
| `FileSystem.swift`, `FilePane.swift` | Background immediate-directory reads, bounded cache/watchers/sizes and recycled native tables. |
| `FileSearchPalette.swift`, `ScopeRail.swift` | Scope funnel for Search and Folders: query/focus/scope history, recent files, context rail and folder panels. |
| `QuickFileSearch.swift` | Progressive discovery, query scheduling, cache reconciliation, FSEvents history replay and idle release. |
| `FFFSearch.swift`, `SearchPaths.swift`, `Sources/FFFBridge/engine.rs` | FFF C ABI, immutable/mapped path snapshots, matching, ranking and highlights. |
| `SearchPerformance.swift` | Numeric search-stage tracing and first title draw diagnostics. |
| `FileSearch.swift` | Retained Spotlight backend, outside the current FFF search UI. |
| `FileCopy.swift` | Drag-and-drop copies with copyfile(3) byte progress and cancellation, and the Copies pane in Transfers. |
| `FileOpening.swift`, `EditorPicker.swift` | Installed editor discovery, shared opening policy and recent-file history. |
| `PreviewPane.swift`, `DocumentPreview.swift`, `PreviewContent.swift`, `MarkdownPreview.swift` | Shared previews, metadata, bounded text reads, Tree-sitter and native Markdown rendering. |
| `MarkdownReader.swift` | Full-window Markdown reader. |
| `WordDocument.swift` | Word Open XML to Markdown conversion for the read-only Markdown reader. |
| `PDFReader.swift`, `NativePDFView.swift`, `PDFRendering.swift` | Full-window PDF reader, retained page layers and actor-owned Core Graphics rasterization/text/link metadata. |
| `PDFAnnotations.swift`, `PDFNoteEditor.swift` | Shared annotation editing/undo/save state and native note popovers. |
| `VideoPreview.swift`, `Sources/VideoFallback/` | AVFoundation playback and on-demand VLCKit fallback. |
| `FileIcons.swift`, `ShellIcons.swift` | Shared file-type icons and theme-drawn section and place icons. |
| `MountedVolumes.swift` | Volume discovery and ejection. |
| `ActivitySampling.swift`, `ActivityMonitorView.swift`, `ActivityTreemap.swift`, `ProcessSearch.swift` | Real libproc samples, process table/treemap and independent process-name filtering. |
| `AirDropView.swift`, `AirDropFilePicker.swift`, `AirDropNetwork.swift`, `Sources/AirDropBridge/` | Recipient UI, file picker, public DNS-SD and experimental HTTPS transfers. |
| `Utilities.swift` | Alerts, the Space preview window, AirDrop staging and the shortcut sheet. |
| `Design.swift`, `ThemePalette.swift`, `CommandPalette.swift`, `ShortcutSheet.swift` | Native controls, pane title strips, bevels, era faces and themes, command choices and keyboard help. |

## Directory browsing

Folders uses the scope funnel's two folder panels, backed by a bounded `FileSystemLibrary` in `FileSearchPalette`. It enumerates immediate children in the background.

`FolderID` wraps a standardized absolute path. `ItemID` identifies the displayed file path. Normalize file IDs as well as folder IDs with `standardizedFileURL` because enumeration can return `/private/var` for a folder entered as `/var`. These are path identities, not inode identities: an external rename is a removed path plus a new path. Symlink paths are preserved for back/forward navigation. Directory symlinks can be entered; packages such as `.app` bundles are displayed as files and opened through macOS.

1. `Workspace.openFolder` records the folder in Folders' Back/Forward history and asks the funnel to browse it. Inside a connected drive, the funnel's searches are scoped to that drive.
2. `FileSearchPalette.browse` asks `FileSystemLibrary.children` for the folder. Cached rows are returned immediately. An uncached folder starts an asynchronous read and initially shows a loading state. Browsing the funnel's root lists that folder rather than the Home view; `ScopeSession.listsRoot` keeps that distinction across section switches.
3. `DirectoryReader.read` runs in a detached task. Foundation enumerates only that directory, prefetches resource metadata, and sorts folders first using natural filename order. It does not recursively walk the disk or read file contents.
4. The result returns to the main actor and updates only that folder's cache entry. Unchanged cached snapshots do not trigger an update. The panels redraw only if that directory is still visible. A late response cannot navigate the user back to an old location.
5. The panels show the current folder and the selected child folder or file preview. Reading the selected child's listing also warms the cache before the user enters it.
6. File and preview panels scroll vertically only, with horizontal elasticity disabled. The table width follows its viewport. `NSTableView` reuses visible cells rather than creating an NSView for every item. Selection-only updates change the existing row selection and preview without reconfiguring or hiding the current table. Keyboard navigation scrolls only when the selection leaves the viewport; scrollbar overlays appear on mouse or trackpad scrolling, hide after 1.5 seconds of inactivity, and hide immediately on keyboard navigation. Row hover is calculated from the current pointer position when painted, and visible row backgrounds are invalidated during scrolling so reused rows cannot leave hover trails.

The cache retains up to 32 directory listings after completed reads, protecting visible and in-flight entries from eviction. Returning to a cached folder displays its previous rows while refreshing in the background. Cache capacity counts directories, not bytes; unusually large directories still consume memory proportional to their entry count.

`FileSystemLibrary.watch` opens watcher descriptors in detached tasks, then installs dispatch filesystem sources for visible directories and closes their descriptors when the directories leave the viewport. Events debounce for 120 ms. An event received during enumeration waits for that read, then refreshes again. Watchers observe directory entry changes; they are not recursive content watchers. Command-R forces a refresh, including metadata changes that do not produce a directory event. A deleted/replaced watched directory may require revisiting it to attach a new watcher.

Folders keeps one Back/Forward history for the session. Search and Folders each keep their funnel state (query, scope, selection and scroll) while the other section is visible. The Folders bar pins the current folder; the command palette also offers a native folder picker. The one pinned path persists in UserDefaults under `pinnedFolder`; choosing another folder replaces it. A missing pin remains visible and reports the actual read error when opened.

## Navigation and layout

The title bar (`SectionBar`) shows Search ⌘1, Folders ⌘2, Activity ⌘3, Transfers ⌘4 and Settings ⌘, after the window controls, then the Open menu, the command palette and keyboard help. `ShellButton` draws section and place buttons in the current theme: a tinted block in modern themes, an inverted block in System 7 and raised or pressed bevels in Windows Retro and Linux Retro. Folders adds `PlacesBar`: Back and Forward (⌘[ and ⌘]), the places, refresh and pin. ⌘L swaps the places for a path field. The scope funnel owns its search and two-panel folder layouts below these bars.

Every pane starts with a `PaneTitle` strip. The active pane, where keyboard navigation goes, is drawn the way each theme marks an active window: pinstripes behind the title in System 7, a navy gradient in Windows Retro, the green accent in Linux Retro, an inverted bar in Terminal and an accent dot in the modern themes. Do not add focus rings around panes. Folders shows the folder name and its parent in the current pane's strip, not as a separate path line.

`StatusBar` runs along the bottom of every section. Its left side shows the funnel's status and key hints in Search and Folders, and Activity's key hints; its right side shows the running copy with byte progress and Cancel, and clicking it opens Transfers. Sections must not draw their own bottom hint rows.

`Design.font` gives retro themes the interface faces of their era: Geneva for System 7, Tahoma for Windows Retro and Helvetica for Linux Retro. Terminal uses the bundled IBM Plex Mono. `Design.readingFont` sets document text in previews and the reader: New York, Verdana and Charter respectively; other themes use the interface face. Hand-drawn text truncates without tightening, which overlaps letters in these faces.

Activity has a toolbar row (sample summary, process filter, Pause) above two flush panes: the process table and the map with its details. Transfers shows AirDrop beside the Copies pane, which lists the running copy and this session's finished, cancelled and failed copies.

File tables reuse native NSTableView cells. Preserve selection and scroll without rebuilding unrelated panels. Keyboard monitoring is limited to the main workspace window and disabled while a sheet is attached so native pickers and preview windows receive their own keys.

## Scope funnel and FFF search

Search is the default section, initially at Home. `FileSearchPalette.swift` owns the funnel composition, selection, scope history and focus for Search and Folders. `Workspace` saves a `ScopeSession` for one of them while the other is visible, and suspends the funnel in the other sections. `ScopeRail.swift` supplies recycled native scope and nearby-entry lists. Cmd+P shows Search and focuses its query.

Scope changes filter the resident index before top-result truncation. Child counts are merged from the full background matching pass for diagnostics; the left rail displays folder names without numeric counts. Navigation and ordinary narrowing reuse the Mac-wide index. Explicit current-folder searches inside directory symlinks add one temporary alias subtree, preserving entered paths without following every symlink on the Mac. Recursive discovery and cache loading start when a nonempty file query needs the index. Empty Home and immediate-folder browsing do not load it.

The context reader stops at 80 immediate entries and keeps four parent snapshots, plus the selected entry. It does not recursively populate a tree. Metadata, parent reads and previews reject stale completions; parent enumeration cooperatively cancels. Search and Folders retain their query, scope history, selection and scroll position across section switches. Enter, Cmd+Enter, Space and the existing opening-editor policy remain native.

Scope funnel search uses upstream fff-search 0.11.0 at c188e7a8f90e4ee4aabe18f93d5869dfb4289f12. `scripts/bootstrap-fff.sh` compiles the Rust static library with the C ABI adapter in `Sources/FFFBridge/engine.rs`; upstream scoring is unchanged. Multiword queries try FFF literal term constraints first and retry with FFF fuzzy matching once cached paths have no literal result, without waiting for disk reconciliation. Newly discovered literal matches take priority over that fallback. Swift discovery supplies bounded immutable snapshots and filters scopes before FFF scoring. The batch scheduler owns search parallelism. Cache saves contain original paths/folder flags and FFF’s exact prepared SIMD chunks and path references. Trigram signatures only prioritize cache-block arrival; they must never filter candidates. Preserve whole snapshot boundaries. Convert highlights only for the global top 100 and retain access-history scoring. Keep the live total-match count out of the UI; scope counts and diagnostic totals remain available. Measure physical footprint, including compressed memory, rather than presenting RSS as total memory. The build strips the packaged executable before signing and retains the original executable and dSYM in `.build/release`.

When Home has an empty or whitespace-only query, show a fixed centered bordered search field above two panels: up to eight recently opened files and the selected file preview. Keep the search field in the same position during searches. Skip empty-query ranking, scope counts and nearby-context loading there; explicit folder browsing keeps the normal funnel. Recent files come from successful FileOpening operations in the separate bounded `recentOpenedFiles` preference, never from ranking visits, folder navigation or previews. Read recent metadata off the main actor.

Folders, an explicit folder navigation with an empty query, uses two recycled FilePane instances: current directory and selected child directory or file preview. These read only immediate children through a bounded FileSystemLibrary with watchers for visible directories. Do not use the recursive top-100 search response as a folder listing. Enter/right/l enters a folder; left/h returns to its parent. Typing restores scoped search; clearing restores directory browsing. The current folder's pane allows multiple selection: Command-click, Shift-click, Shift-J/K or Shift-arrows from an anchor, and Command-A. Escape or plain j/k collapse it to the focused row, which drives the preview, Enter and the Open menu. Reloads and preview updates keep a multiple selection that still contains the focus. Dragging any selected row carries every selected file as a copy, to other apps or onto a Folders place; AirDrop staging sends them all. The status bar shows the selection count.

Search defaults to This Mac using the resident fuzzy index rooted at `/`, independent of navigation. Typing `$` activates a current-folder prefix filter without inserting the symbol or changing engines. A pasted leading `$` is consumed too. Store mode separately from visible query text, preserve both in the section's funnel state, and use a subtle tinted field plus a noninteractive scope label. Clearing the query or Backspace in an empty local field restores This Mac. Do not add a mode toggle. All result paths resolve against `/`. Discovery/cache loading starts on a nonempty query, publishes progressively, and persists a separate Mac-wide FFF path-snapshot cache. Leaving search or clearing the query cancels expensive work immediately, retains batches for a 60-second grace period, then drops index buffers and stops its watchers. Returning within that grace period reuses resident batches. Older cache formats migrate to `.fff3.bin` prepared snapshots. Cache headers validate the root and pinned engine revision; version 4 headers also record the saved FSEvents ID described below. Swift metadata and Rust scoring borrow the same read-only mapping; snapshots retain it through an Arc until their last query finishes. Cache writes atomically replace the file, never truncate a mapped inode. Versions 3 and 4 align prepared index tables; older unaligned tables alone are copied during migration. Keep compact ChunkedString references resident and materialize bounded FileItems only for scoring. Matching and ranking belong to FFF. Reconciliation reuses unchanged batches instead of preparing a second complete index. Skip `/dev` and the duplicate macOS mount hierarchy `/System/Volumes`; ordinary permission, hidden-entry, package, and symlink exclusions remain. Do not label Home-only coverage as computer-wide. Keep old rows visible during replacement queries, but do not activate them as results of the new query before its first publication.

Each cache save records the FSEvents ID up to which the index is complete, and its date. When a search start finds a saved ID less than 24 hours old, the watcher replays events from that ID instead of walking the disk: the cache loads as usual, changed paths collect until `HistoryDone`, and one incremental scan reconciles them. The start walks the whole disk instead when the saved ID is older, the replay passes 30 seconds, more than 50,000 changed paths are pending, an event needs a full rescan (dropped events, `MustScanSubDirs`, a root change), or the loaded cache is not the one the ID was saved with. A save claims only an ID its batches already include: a full walk claims the event current when it started, an incremental scan the last event delivered before it started. A partly loaded cache is never saved. An unchanged incremental scan saves only when the file's ID is more than an hour behind. Incremental reconciliation looks up each indexed row's ancestors in the set of changed paths, so thousands of changes cost one pass over the index. Paths beneath hidden folders, packages and folder symlinks are dropped before that pass, and the hidden folders join the watcher's exclusions. A folder whose open or listing times out keeps its previously indexed contents. The state report's `file_search_catch_up` shows the method, the fallback reason, the replay time and the number of replayed paths.

Both search modes show “Narrow the search” and “Where it lives”. This Mac narrows from `/` using an explicit search-only scope, independent of the folder being browsed; clearing the query restores the full Mac scope. Keep the same bounded context reader and fuzzy matcher. Do not hide these panels when changing search modes.

Search presents its first nonempty usable response immediately, including after an initial empty publication. Later UI updates share a 40 ms window while FFF keeps searching without a typing debounce. Preserve the selected path, loaded preview and scrolled viewport anchor as results arrive; skip reconfiguring unchanged visible cells. Final index reconciliation replaces the same query atomically, without exposing intermediate subsets or resetting the selected preview. Numeric scope counts remain diagnostic only.

## Preserve seamless search

Never replace or bypass the resident fuzzy search engine when changing search scope, navigation, themes, or controls. Those changes must preserve matching, ranking, progressive results, and typing responsiveness unless the maintainer explicitly requests an engine or behavior change. This Mac and `$` current-folder mode must use the same fuzzy matcher; `$` changes only the scope filter. Do not substitute query-time Spotlight matching, add a debounce, or clear visible results while a replacement query is running. Verify rapid typing, Escape/refocus, fuzzy abbreviations, and scope changes in the native app before declaring a search change complete. An optimization must not silently remove existing behavior.

## Search query cache and tracing

Up to eight complete queries retain bounded owned hits, diagnostic totals and scope counts. Keys include normalized text, mode, scope, volume and exact integer history boosts. Index path/kind changes, batch appends, history changes and idle release invalidate them. Content-only reconciliation reuses unchanged snapshots without restarting the query or clearing this cache. Default search parallelism is min(12, active processor count); `--search-workers N` overrides it for diagnostics. Cache-reload diagnostics distinguish a true resident engine pass from a subsequent query-cache hit.

`SearchPerformance.swift` retains 32 numeric stage samples and emits the `Search to first title draw` Points of Interest interval. `Search Performance` logs input dispatch, publication, row preparation and the first visible title's AppKit draw. The separately named window notification can arrive late and must not be presented as rendering completion. Draw timings exclude compositor presentation. Normal native profiling must exclude state/event report writes and accessibility-tree polling during capture; the latter caused substantial measured main-thread work.

`SearchPaths.swift` stores mapped original UTF-8 paths as offsets and flags, with owned arrays for new discoveries. Deduplication/reconciliation use bounded batch/row locations and process-seeded hash lookups, always verifying full path equality after a hash match. Non-ASCII comparisons retain Swift canonical Unicode equality. Neither hash nor trigram hints may exclude FFF candidates.

## Command palette and themes

Command-K or Command-Shift-P opens the command palette for sections, places, selected-file actions and themes. `WorkspaceCommands.swift` captures the visible selection/folder before presenting it. Hide unavailable actions, preserve native file-opening policy, and use NSWorkspace URLs for Terminal/editor actions rather than executing shell strings. Theme/editor/drive choices stay inside the palette; Escape returns to commands before dismissing it. Confirmed themes persist under `theme`; previews and cancellation must not alter that saved preference.

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

`MountedVolumes.swift` discovers browsable mounted volumes off the main actor and observes macOS mount/unmount/rename notifications. The Folders places show these volumes directly, excluding the startup and hidden system volumes. Clicking a drive browses it in Folders. The default query scope is that drive while Folders stays inside it; `$` still narrows to the current folder. Empty queries restore immediate directory browsing. The selected volume is indexed progressively through the existing resident fuzzy matcher; its supplementary batches replace duplicate Mac-cache coverage during drive searches. Native ejection reports errors and never forces an unmount. Dragging search results, recent files or folder rows onto a Folders place or drive copies them there in the background (`FileCopy.swift`) without changing the view. The status bar shows byte progress and Cancel from every section, and Transfers lists the running copy and this session's finished copies; one copy runs at a time. Existing names are skipped, never replaced (`COPYFILE_EXCL`), and a cancelled or failed item removes only its own partial target. Read-only volumes, such as NTFS drives that macOS mounts read-only, are reported before copying. Drags to other apps offer only copy, never move.

## Background sizes

Regular file sizes come from metadata. The folder listed in Folders gets one utility-priority scan that totals logical file bytes in its immediate child folders and packages, including hidden contents. Scans do not follow symlinks and stop cooperatively when the active directory changes or refreshes. Errors leave an unknown size instead of publishing a partial total. Results update cached items and visible cells without rebuilding the file panels. The child panel never launches its own recursive scan. Totals are logical bytes, not allocated disk space; hard-linked paths count separately. Visible-directory watchers do not detect every nested change, so Command-R recomputes totals. Large folders can take time to finish.

## Performance and correctness constraints

Keep enumeration, resource metadata reads, and default directory sorting off the main actor. Do not pre-walk the user's home directory or load file contents to draw browsing rows. Nonempty search starts progressive Mac-wide discovery; empty-query browsing does not start the recursive index. Typing must never wait for a complete scan. Recursive size scans are separate cancellable background work for the active folder, never a prerequisite for drawing its entries. Avoid synchronous per-file icon lookups in table rendering. The main actor owns AppKit and cache publication; detached work returns Sendable file values.

Uncached reads currently publish a complete sorted directory snapshot. They do not stream batches or cancel an OS enumeration already in progress. Explicit non-default sorting, filtering, and display-row preparation run on the main actor. Profile these before claiming responsiveness for enormous folders or slow/network volumes.

Display loading, empty, and failed states distinctly. Do not convert permission errors into empty folders. Missing per-file metadata is represented as unknown, not invented dates or sizes. Ordinary directory listings omit hidden entries.

## AirDrop

AirDrop uses `AirDropView.swift`, a custom `AirDropFilePicker.swift`, public DNS-SD in `AirDropBridge`, and the experimental HTTPS protocol in `AirDropNetwork.swift`. Current receivers require `/Ask` to declare `TransferID` as `{id: UUID}` and `TransferType` as `{files: {}}`; `/Upload` must repeat that UUID in its `TransferID` header. Keep these together in `AirDropPreparedFiles`.

The AirDrop log category records acknowledgment times and submitted archive bytes without filenames or identity credentials. Discovery is intermittent when the legacy service disappears while Finder still sees the device. Custom sending requires macOS 15 or later, Everyone mode, and a self-issued client certificate imported only into memory. Never add private discovery entitlements, extract Apple-account credentials, invent devices or progress, or bypass TLS validation. The explicit macOS AirDrop fallback uses `NSSharingService.sendViaAirDrop`; its didShare callback also fires on picker cancellation and is not a delivery receipt.

## Verification

Match verification to the change. For documentation-only changes, check links, paths, commands and the diff. For native behavior changes, build the release app and exercise the affected UI; a build alone does not prove behavior. Use disposable data outside personal folders. Never modify user files to test mutations.

For navigation changes, check direct paths, parent/child traversal, back/forward, section switches, empty/missing folders, pinned-folder persistence, connected volumes and external directory changes. For search changes, check rapid typing, Escape/refocus, fuzzy abbreviations, both modes, scope changes, progressive results and selection/preview preservation. For PDF changes, verify annotations after saving and reopening disposable copies, Undo/Redo, removal across unvisited pages, unsaved-close handling and the measurements below.

```sh
open -n "$PWD/build/Crawler.app" --args \
  --state-report /tmp/crawler-state.json \
  --launch-report /tmp/crawler-launch.json
```

Existing diagnostics:

- `scripts/crawler [path]` sends a folder or file to Crawler through `open`, launched or already running. `AppDelegate.application(_:open:)` shows folders in Folders and opens files with the Enter policy; `CFBundleDocumentTypes` declares folders with rank None, so Crawler never becomes the default folder handler.
- `--folder <absolute-path>` opens Folders at that directory.
- `--find <query>` opens/focuses search with an initial query.
- `--open <absolute-path>` opens a file at launch through the normal Enter policy, including the PDF and Markdown readers. Combine it with `--snapshot` to check reader rendering without driving the UI.
- `--search-root <absolute-path>` sets the initial browsing/local-search context for diagnostics. This Mac still uses the `/` index; use `$` folder mode to constrain a disposable search.
- `--search-workers N` overrides the bounded matching worker count.
- `--section <name>` launches into Search, Folders, Activity, Transfers or Settings. `--activity` and `--settings` remain shortcuts for two of them.
- `--snapshot <path>` saves an app-side screenshot 0.2 seconds after launch; `--snapshot-delay <seconds>` waits longer, for example until Activity has its first samples. `--quit-after <seconds>` exits automatically.
- `--event-log <existing-path>` appends state JSON when state reporting is active.

State reports include the section, the Folders location and history, search status/results/selection, query/index/cache timing, preview readiness, PDF rendering/annotation state, Markdown reader state/load time and Activity Monitor samples. Reports can contain private paths and filenames; keep them local. `scripts/benchmark-startup.py` measures app-side window readiness, not asynchronous directory loading or compositor presentation. Do not claim cold-disk results from warm files or infer FPS from callback timers. Profile normal launches without state/event writes or accessibility polling during capture.

## PDF performance

For every PDF viewer change, measure the packaged app on representative real documents, including the longest available one. Record time to first bitmap, time to the first display-resolution bitmap, parsing/raster costs and peak scroll-update cost separately. The `PDF Performance` log category records numeric timings without paths or contents; `--state-report` exposes `pdf_reader_load_ms` and `pdf_reader_rendering` (including `first_sharp_ms`, `parse_ms`, `max_raster_ms`, cache bytes and hits). Bitmap-ready timings exclude compositor presentation. Profile main-thread work with Time Profiler on a normal launch without state/event reports, because diagnostic file writes can distort responsiveness. Never infer FPS from a callback timer or claim cold-disk results from a warm local file. Prioritize first visible content and responsive input over finishing all pages; do not eagerly parse or rasterize an entire long PDF.
