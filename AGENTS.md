# Working on Crawler

## Product and current scope

Crawler is a native macOS app for finding and reading files. It is written in Rust on Rune and lives in `crawler-rune/`. There is no other implementation: the Swift/AppKit app it replaced has been removed. The title bar holds five sections: Search, Folders, Activity, Transfers and Settings. Search and Folders share the scope funnel. `prototype/` contains HTML mockups, including the historical Finder mockup and the section shell prototype; changes there do not affect the app.

Folders shows places for Home, Downloads, Desktop, at most one user-pinned folder and connected volumes. Search starts at Home, with recent files inside the empty-query view. Activity is the process monitor, Transfers is AirDrop and Settings holds the theme. There are no tabs, sidebar, Columns view, System Settings workspace or Trash workspace. Crawler does not move, rename or delete files. Do not restore synthetic data or add destructive disk operations unless the maintainer asks for them.

## Collaboration rules

- Keep code and explanations simple and concrete. Read and apply `~/.agents/skills/unslop/SKILL.md` before user-facing messages when available.
- Questions are read-only unless the user also requests implementation.
- Preserve unrelated local changes.
- Do not add test files or test-only helpers/fixtures without explicit approval. Prefer existing checks and direct runtime verification.
- Use one agent for ordinary tasks. Delegate only when authorized and useful for breadth or independent review; assign file ownership first.
- Prefer native platform APIs and strong Rust types over adding dependencies or generic abstractions.

## Build and run

Requirements: macOS 14+, rustup (the crate pins its toolchain in `crawler-rune/rust-toolchain.toml`) and Python 3.11+. There is no WebView.

Build and launch the signed release app from the repository root:

```sh
scripts/build.sh
open "$PWD/build/Crawler.app"
```

The script builds `crawler-rune` in release, packages the icon, licenses and generated Rust notices (`scripts/rust-notices.py`), strips the packaged executable, signs `build/Crawler.app` and swaps it in place of the previous bundle. It retains the unstripped executable in `crawler-rune/target/release`. Signing requires a persistent `Crawler Local Development` certificate in the login keychain, or an existing certificate named by `CRAWLER_SIGNING_IDENTITY`. Never replace it with ad-hoc signing or recreate it for each build: changing the certificate breaks remembered privacy grants. Keep the bundle identifier `studio.crawler.native-prototype` unless the maintainer explicitly requests a migration. Preferences live under `studio.crawler.rune`; `swift_settings.rs` imports the removed Swift app's settings once, so an upgrade keeps the theme, pinned folder and recent files.

For a debug build, run `cargo run` inside `crawler-rune/`.

Prefer the packaged app when checking permissions, fonts, previews and launch behavior. The debug executable alone is not equivalent. macOS controls protected-folder access; show failures and never bypass privacy controls. Usage descriptions do not grant access. Keep [LICENSE](LICENSE), [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md), bundled dependency licenses and generated Rust notices in packaged builds. Manual installation commands are in [README.md](README.md); there is no Homebrew package yet.

## Source map

Paths in the table are relative to `crawler-rune/src/`. Every module keeps its own state struct inside `App` and reaches another module only through that module's public functions.

| Files | Responsibility |
| --- | --- |
| `main.rs`, `app.rs`, `args.rs` | Window and menus, the whole app's state, section and reader routing, global keys, launch flags and state reports. |
| `documents.rs` | The open-documents Apple event: folders and files handed over by `open`, the `crawler` CLI, Finder or the Dock. |
| `shell.rs`, `theme.rs`, `icons.rs` | Section bar, status bar, pane title strips, toasts, theme-drawn buttons, themes and era faces, file-type icons. |
| `funnel/mod.rs`, `funnel/model.rs` | Scope funnel for Search and Folders: query, focus, scope history, selection, places, typed paths and `DocumentKind`, the only extension table. |
| `funnel/library.rs`, `funnel/scroll.rs` | Background immediate-directory reads, bounded cache, watchers, folder sizes and scrollbar chrome. |
| `index/mod.rs`, `index/worker.rs` | The resident Mac-wide fuzzy index: the engine's contract with the funnel, query scheduling, scoring passes, cache reconciliation and idle release. |
| `index/cache.rs`, `index/snapshot.rs`, `index/matching.rs` | The mapped path-snapshot cache, immutable snapshots and FFF matching, ranking and highlights. |
| `index/discovery.rs`, `index/watcher.rs`, `index/performance.rs` | The disk walk, FSEvents history replay and search-stage signposts. |
| `index/fff-core/` | Pinned source copy of upstream fff-search, patched in through `Cargo.toml`. Its scoring is unchanged. |
| `opening/` | The Enter policy, successful-open history, volumes and ejection, Terminal and Reveal in Finder. |
| `preview/mod.rs`, `preview/content.rs` | Pane previews, the Space window and the document reader; bounded loading and the `Document` model. |
| `preview/native.rs`, `preview/word.rs`, `preview/sheet.rs`, `preview/controls.rs` | ImageIO and Quick Look decoding, Word to Markdown conversion, spreadsheets and reader controls. |
| `pdf/` | Full-window PDF reader: retained page surfaces (`view.rs`), the worker that owns Core Graphics rasterization and PDFKit text, links and annotations (`worker.rs`), shared bitmap storage (`bitmap.rs`). |
| `activity/` | libproc samples, the process table and treemap and the process-name filter. |
| `transfers/` | AirDrop discovery, the file picker and the experimental HTTPS transfer, the macOS AirDrop fallback, and drag-and-drop copies with byte progress. |
| `settings/` | Settings, the command palette and the keyboard shortcut sheet. |
| `swift_settings.rs` | One-time import of the removed Swift app's preferences. |

## Directory browsing

Folders uses the scope funnel's two folder panels, backed by a bounded `Library` (`funnel/library.rs`). It enumerates immediate children in the background.

`FolderID` wraps a standardized absolute path. `ItemID` identifies the displayed file path. These are path identities, not inode identities: an external rename is a removed path plus a new path. Symlink paths are preserved for back/forward navigation. Directory symlinks can be entered; packages such as `.app` bundles are displayed as files.

1. Opening a folder records it in Folders' Back/Forward history and asks the funnel to browse it. Inside a connected drive, the funnel's searches are scoped to that drive.
2. The funnel asks the library for the folder. Cached rows are returned immediately. An uncached folder starts an asynchronous read and initially shows a loading state.
3. The read runs on a worker. It enumerates only that directory, reads metadata with one `lstat` per entry (the hidden flag comes from that same `lstat`) and sorts folders first using natural filename order. It does not recursively walk the disk or read file contents.
4. The result returns to the main thread and updates only that folder's cache entry. Unchanged listings do not trigger an update. The panels redraw only if that directory is still visible. A late response cannot navigate the user back to an old location.
5. The panels show the current folder and the selected child folder or file preview. Reading the selected child's listing also warms the cache before the user enters it.
6. File and preview panels scroll vertically only. Lists are virtual: only the rows on screen are built. Selection-only updates change the existing selection and preview without rebuilding the list. Keyboard navigation scrolls only when the selection leaves the viewport; scrollbar overlays appear on mouse or trackpad scrolling, hide after 1.5 seconds of inactivity, and hide immediately on keyboard navigation.

The cache retains up to 32 directory listings after completed reads, protecting visible and in-flight entries from eviction. Returning to a cached folder displays its previous rows while refreshing in the background. Cache capacity counts directories, not bytes; unusually large directories still consume memory proportional to their entry count.

The library watches visible directories and stops watching them when they leave the viewport. Events debounce for 120 ms. An event received during enumeration waits for that read, then refreshes again. Watchers observe directory entry changes; they are not recursive content watchers. Command-R forces a refresh, including metadata changes that do not produce a directory event. A deleted or replaced watched directory may require revisiting it to attach a new watcher.

Folders keeps one Back/Forward history for the session. Search and Folders each keep their funnel state (query, scope, selection and scroll) while the other section is visible. The Folders bar pins the current folder; the command palette also offers a native folder picker. The one pinned path persists in the preferences under `pinnedFolder`; choosing another folder replaces it. A missing pin remains visible and reports the actual read error when opened.

## Navigation and layout

The title bar shows Search ⌘1, Folders ⌘2, Activity ⌘3, Transfers ⌘4 and Settings ⌘, after the window controls, then the command palette and keyboard help. There is no Open button. Section and place buttons are drawn in the current theme: a tinted block in modern themes, an inverted block in System 7 and raised or pressed bevels in Windows Retro and Linux Retro. Folders adds the places bar: Back and Forward (⌘[ and ⌘]), the places, refresh and pin. ⌘L swaps the places for a path field. The scope funnel owns its search and two-panel folder layouts below these bars.

Every pane starts with a title strip. The active pane, where keyboard navigation goes, is drawn the way each theme marks an active window: pinstripes behind the title in System 7, a navy gradient in Windows Retro, the green accent in Linux Retro, an inverted bar in Terminal and an accent dot in the modern themes. Do not add focus rings around panes. Folders shows the folder name and its parent in the current pane's strip, not as a separate path line.

The status bar runs along the bottom of every section. Its left side shows the funnel's status and key hints in Search and Folders, and Activity's key hints; its right side shows the running copy with byte progress and Cancel, and clicking it opens Transfers. Sections must not draw their own bottom hint rows. Readers cover the whole window and have no status bar.

Retro themes use the interface faces of their era: Geneva for System 7, Tahoma for Windows Retro and Helvetica for Linux Retro. Terminal uses the bundled IBM Plex Mono. Document text in previews and the reader uses New York, Verdana and Charter respectively; other themes use the interface face.

Activity has a toolbar row (sample summary, process filter, Pause) above two flush panes: the process table and the map with its details. Transfers shows AirDrop beside the Copies pane, which lists the running copy and this session's finished, cancelled and failed copies.

Preserve selection and scroll without rebuilding unrelated panels. Global keys go to the open reader first, then to the visible section.

## Scope funnel and FFF search

Search is the default section, initially at Home. `funnel/mod.rs` owns the funnel composition, selection, scope history and focus for Search and Folders. Each of the two keeps its own session while the other is visible, and the funnel is suspended in the other sections. Cmd+P shows Search and focuses its query.

Scope changes filter the resident index before top-result truncation. Child counts are merged from the full background matching pass for diagnostics; the left rail displays folder names without numeric counts. Navigation and ordinary narrowing reuse the Mac-wide index. Recursive discovery and cache loading start when a nonempty file query needs the index. Empty Home and immediate-folder browsing do not load it.

The context reader stops at 80 immediate entries and keeps four parent snapshots, plus the selected entry. It does not recursively populate a tree. Metadata, parent reads and previews reject stale completions. Search and Folders retain their query, scope history, selection and scroll position across section switches.

Search uses upstream fff-search 0.11.0 at c188e7a8f90e4ee4aabe18f93d5869dfb4289f12, as the pinned source copy in `index/fff-core/`; upstream scoring is unchanged. Multiword queries try FFF literal term constraints first and retry with FFF fuzzy matching once cached paths have no literal result, without waiting for disk reconciliation. Newly discovered literal matches take priority over that fallback. Discovery supplies bounded immutable snapshots and scopes are filtered before FFF scoring. Cache saves contain original paths, folder flags and FFF's exact prepared SIMD chunks and path references. Trigram signatures only prioritize cache-block arrival; they must never filter candidates. Preserve whole snapshot boundaries. Convert highlights only for the global top 100 and retain access-history scoring. Keep the live total-match count out of the UI; scope counts and diagnostic totals remain available. Measure physical footprint, including compressed memory, rather than presenting RSS as total memory.

Cache blocks are verified and decoded on up to eight threads, in the query's priority order. A scoring pass is one parallel pass with a single merge. When a pass has the same query over the same batches plus new ones at the end, it scores only the new ones and merges them into the previous pass (`Partial` in `index/worker.rs`); the top 100, scope counts and total are the same as a full pass. The first usable result is sent on its own before the final response, because a final response is dropped while batches keep arriving.

When Home has an empty or whitespace-only query, show a fixed centered bordered search field above two panels: up to eight recently opened files and the selected file preview. Keep the search field in the same position during searches. Skip empty-query ranking, scope counts and nearby-context loading there; explicit folder browsing keeps the normal funnel. Recent files come from successful opens in the separate bounded `recentOpenedFiles` preference, never from ranking visits, folder navigation or previews. Read recent metadata off the main thread.

Folders, an explicit folder navigation with an empty query, uses two panels: current directory and selected child directory or file preview. These read only immediate children through the bounded library with watchers for visible directories. Do not use the recursive top-100 search response as a folder listing. Enter/right/l enters a folder; left/h returns to its parent. Typing restores scoped search; clearing restores directory browsing. The current folder's pane allows multiple selection: Command-click, Shift-click, Shift-J/K or Shift-arrows from an anchor, and Command-A. Escape or plain j/k collapse it to the focused row, which drives the preview and Enter. Reloads and preview updates keep a multiple selection that still contains the focus. Dragging any selected row carries every selected file as a copy, to other apps or onto a Folders place; AirDrop staging sends them all. The status bar shows the selection count.

Search defaults to This Mac using the resident fuzzy index rooted at `/`, independent of navigation. Typing `$` activates a current-folder prefix filter without inserting the symbol or changing engines. A pasted leading `$` is consumed too. Store mode separately from visible query text, preserve both in the section's funnel state, and use a subtle tinted field plus a noninteractive scope label. Clearing the query or Backspace in an empty local field restores This Mac. Do not add a mode toggle. All result paths resolve against `/`. Discovery and cache loading start on a nonempty query, publish progressively, and persist a Mac-wide path-snapshot cache (`mac-paths.fff4.bin` under `~/Library/Application Support/studio.crawler.rune`). Leaving search or clearing the query cancels expensive work immediately, retains batches for a 60-second grace period, then drops index buffers and stops its watchers. Returning within that grace period reuses resident batches. Cache headers validate the root and pinned engine revision and record the saved FSEvents ID described below. Metadata and scoring borrow the same read-only mapping; snapshots retain it through an `Arc` until their last query finishes. Cache writes atomically replace the file, never truncate a mapped inode. Matching and ranking belong to FFF. Reconciliation reuses unchanged batches instead of preparing a second complete index. Skip `/dev` and the duplicate macOS mount hierarchy `/System/Volumes`; ordinary permission, hidden-entry, package and symlink exclusions remain. Do not label Home-only coverage as computer-wide. Keep old rows visible during replacement queries, but do not activate them as results of the new query before its first publication.

Each cache save records the FSEvents ID up to which the index is complete, and its date. When a search start finds a saved ID less than 24 hours old, the watcher replays events from that ID instead of walking the disk: the cache loads as usual, changed paths collect until `HistoryDone`, and one incremental scan reconciles them. The start walks the whole disk instead when the saved ID is older, the replay passes 30 seconds, more than 50,000 changed paths are pending, an event needs a full rescan (dropped events, `MustScanSubDirs`, a root change), or the loaded cache is not the one the ID was saved with. A save claims only an ID its batches already include. A partly loaded cache is never saved. An unchanged incremental scan saves only when the file's ID is more than an hour behind. Paths beneath hidden folders, packages and folder symlinks are dropped before reconciliation. A folder whose open or listing times out keeps its previously indexed contents. The state report's `file_search_catch_up` shows the method, the fallback reason, the replay time and the number of replayed paths.

Both search modes show "Narrow the search" and "Where it lives". This Mac narrows from `/` using an explicit search-only scope, independent of the folder being browsed; clearing the query restores the full Mac scope. Do not hide these panels when changing search modes.

Search presents its first nonempty usable response immediately, including after an initial empty publication. Later UI updates share a 40 ms window while FFF keeps searching without a typing debounce. Preserve the selected path, loaded preview and scrolled viewport anchor as results arrive. Final index reconciliation replaces the same query atomically, without resetting the selected preview.

## Preserve seamless search

Never replace or bypass the resident fuzzy search engine when changing search scope, navigation, themes or controls. Those changes must preserve matching, ranking, progressive results and typing responsiveness unless the maintainer explicitly requests an engine or behavior change. This Mac and `$` current-folder mode must use the same fuzzy matcher; `$` changes only the scope filter. Do not substitute query-time Spotlight matching, add a debounce, or clear visible results while a replacement query is running. An optimization must not silently remove existing behavior: check any index change by comparing `file_search_results`, `total_matches` and the scope counts of the old and new builds on the same cache.

## Search query cache and tracing

Up to eight complete queries retain bounded owned hits, diagnostic totals and scope counts. Keys include normalized text, mode, scope, volume and exact integer history boosts. Index path or kind changes, batch appends, history changes and idle release invalidate them. Default search parallelism is min(12, available cores); `--search-workers N` overrides it for diagnostics.

The index keeps 32 numeric stage samples (`index["stages"]` in the state report): input dispatch, first result, complete publication, row update and the first visible title's draw. Draw timings exclude compositor presentation. Profile normal launches without state or event report writes.

## Command palette and themes

Command-K or Command-Shift-P opens the command palette for sections, places, selected-file actions and themes. It captures the visible selection and folder before presenting. Hide unavailable actions. Theme and drive choices stay inside the palette; Escape returns to commands before dismissing it. Confirmed themes persist under `theme`; previews and cancellation must not alter that saved preference. Terminal for the current folder and Reveal in Finder are palette actions.

## Opening files

Every file opens inside Crawler. There is no Open button, no default text editor, no editor discovery and no hand-off to another app: do not add them back. Enter on a file Crawler cannot draw (archives, disk images, audio, apps and other packages) does nothing.

`DocumentKind::for_path` in `funnel/model.rs` is the only extension table. `preview::content::Format::of` groups its kinds and `opening::dispatch` routes them: folders to Folders, PDFs to the PDF reader and Markdown, Word, spreadsheets, code, plain text, images and video to the document reader (`Reader::Document`, `preview/mod.rs`). Add a format to that table, never to a second list. Record recent-file history only after a document has loaded.

The document reader has one header for every format: Back, the name, the position, then only the controls that document can use. X, two consecutive non-repeated Escape presses within 700 ms, or Command-W close a reader and restore the workspace and its focus. A `Document` has one `Body`:

- `Blocks`: Markdown, `.docx` and, through `/usr/bin/textutil` into a temporary `.docx`, `.doc`, `.rtf` and `.odt`. The reader reads up to 4 MB and centers a column of about 70 characters; panes read 256 KB. It embeds local images (at most 48) and never fetches remote ones. `#fragment` links scroll to GitHub-style heading slugs. Raw HTML never runs: inline tags are dropped, comments hidden and other HTML blocks shown as source. Word conversion keeps headings, bold, italics, strikethrough, lists, tables and external links; fonts, colors, images and page layout are dropped. O or the header shows the heading outline.
- `Text`: code and plain text up to 16 MB. One buffer, a `u32` offset per line and one sorted highlight list; never a `String` per line. Rune colours the first 128 KiB. Lines show their numbers, W toggles wrapping (unwrapped text scrolls sideways) and Command-L goes to a line.
- `Sheet`: one sheet at a time in a flat cell arena (`preview/sheet.rs`). `.xlsx`, `.xlsm` and `.xlsb` stream cell by cell through calamine and stop at 200,000 rows, 512 columns or 4 million cells; `.xls` and `.ods` have no streaming reader, so calamine reads their sheet whole and panes show their Quick Look thumbnail instead. `.csv` and `.tsv` use the same grid. Column letters and the first row stay at the top, row numbers at the left, sheet tabs at the bottom, and Command-L goes to a row.
- `None`: images and video. Images decode through ImageIO's `SubsampleFactor`, so the decoder drops pixels by two, four or eight; the single draw into the RGBA buffer does the last step down and applies the EXIF orientation (`preview/native.rs`). The reader decodes to 3072 px and panes to 1280 px. Do not go back to `CGImageSourceCreateThumbnail` for files: its resampling took 42 ms on a 2400 px JPEG that subsampling decodes in 11 ms. Other formats in panes use Quick Look thumbnails; a missing thumbnail shows real metadata and an icon.

Find (Command-F) works in blocks, text lines and sheet rows. Zoom is the header buttons, a trackpad pinch and Command-plus/minus/0, in 10% steps from 70% to 200% for text (persisted as `markdownReaderScale`). Command-[ goes back through documents opened from links, whatever their format; closing a PDF opened from a link returns to that document. All rows are virtual: build nothing per row outside the visible window, and build theme tokens once per view, not once per row.

Keep file reads, Markdown parsing and syntax highlighting off the main thread, bounded and cancellation-aware. Space opens the shared preview in a resizable window and closes with Space or Escape; hidden previews are suspended while the reader or the Space window is open.

## PDF reader and annotations

Enter, double-click and search activation open PDFs in the PDF reader inside the Crawler window. The header button beside the title copies the file name with its extension. Reuse an already loaded preview's source and bitmap cache; otherwise read only the selected PDF in the background.

One worker thread (`pdf/worker.rs`) owns every Cocoa PDF object: Core Graphics parses and rasterizes, PDFKit supplies text, selection, links and annotation serialization. Never use PDFView or PDFKit page rendering. Read only the first page geometry before initial display, estimate unread layout slots, and resolve them near the viewport. Retain visible surfaces plus one neighboring page on each side, cap the bitmap cache at 128 MiB across both windows and use bounded crops at large zoom levels. Do not eagerly parse or rasterize a long PDF. Bitmaps are IOSurface-backed and shared with the GPU without a copy (`pdf/bitmap.rs`).

PDF tools stay active between strokes. Choose Highlight, Underline or Note, then drag over text to see the mark while drawing. Notes use dotted underlines with a hover popover and a click-to-edit popover. Previews and the reader see the same marks. Keep pending strokes, Undo/Redo and Save ordered. Remove annotations clears supported highlights, underlines and text notes on every page, including saved marks and unvisited pages, and preserves links, forms and unsupported annotations. Removal is undoable.

Command-S saves annotations into the PDF; Shift-Command-S saves a copy. Closing the reader or window, or quitting, must handle unsaved changes and in-flight operations. Failed saves retain edits. Detect external file changes before replacing the original; require Save As on conflict. Encrypted PDFs and those that disallow annotations remain read-only. Text tools require a text layer; there is no OCR or editing of the underlying document text.

For every PDF viewer change, measure on representative real documents, including the longest available one. The state report exposes `pdf_reader_load_ms` and `pdf_reader_rendering` (`first_bitmap_ms`, `first_sharp_ms`, `parse_ms`, `raster_ms`, `max_raster_ms`, `metadata_ms`, cache bytes and hits). Bitmap-ready timings exclude compositor presentation. Never infer FPS from a callback timer or claim cold-disk results from a warm local file. Text extraction shares the raster thread; a fast scroll through an 11 MB manual left that thread idle about 80% of the time, so moving text elsewhere measured no gain.

## Video previews

Video plays through Rune's player, shared between panes, the Space window and the document reader. Keep playback opt-in, selected-file-only and cancellation-aware. Hidden players pause. Space suspends the pane's player and restores its paused preview when closed. Never introduce background transcoding or scan video contents across a directory.

## Connected volumes

`opening/platform.rs` discovers browsable mounted volumes off the main thread and observes macOS mount, unmount and rename notifications. The Folders places show these volumes directly, excluding the startup and hidden system volumes. Clicking a drive browses it in Folders. The default query scope is that drive while Folders stays inside it; `$` still narrows to the current folder. The selected volume is indexed progressively through the same resident fuzzy matcher; its supplemental batches replace duplicate Mac-cache coverage during drive searches. Native ejection reports errors and never forces an unmount. Dragging search results, recent files or folder rows onto a Folders place or drive copies them there in the background (`transfers/copy.rs`) without changing the view. The status bar shows byte progress and Cancel from every section; one copy runs at a time. Existing names are skipped, never replaced (`COPYFILE_EXCL`), and a cancelled or failed item removes only its own partial target. Read-only volumes are reported before copying. Drags to other apps offer only copy, never move.

## Background sizes

Regular file sizes come from metadata. The folder listed in Folders gets one utility-priority scan that totals logical file bytes in its immediate child folders and packages, including hidden contents. Scans do not follow symlinks and stop cooperatively when the active directory changes or refreshes. Errors leave an unknown size instead of publishing a partial total. Results update cached items and visible cells without rebuilding the file panels. The child panel never launches its own recursive scan. Totals are logical bytes, not allocated disk space; hard-linked paths count separately. Command-R recomputes totals.

## Performance and correctness constraints

Keep enumeration, metadata reads and directory sorting off the main thread. Do not pre-walk the user's home directory or load file contents to draw browsing rows. Nonempty search starts progressive Mac-wide discovery; empty-query browsing does not start the recursive index. Typing must never wait for a complete scan. Avoid per-file icon lookups in row building.

Uncached reads publish a complete sorted directory snapshot. They do not stream batches or cancel an OS enumeration already in progress. Profile before claiming responsiveness for enormous folders or slow network volumes.

Display loading, empty and failed states distinctly. Do not convert permission errors into empty folders. Missing per-file metadata is represented as unknown, not invented dates or sizes. Ordinary directory listings omit hidden entries.

## AirDrop

AirDrop lives in `transfers/`: recipient UI and picker, public DNS-SD discovery (`discovery.rs`) and the experimental HTTPS protocol (`network.rs`). Current receivers require `/Ask` to declare `TransferID` as `{id: UUID}` and `TransferType` as `{files: {}}`; `/Upload` must repeat that UUID in its `TransferID` header.

Discovery is intermittent when the legacy service disappears while Finder still sees the device. Custom sending requires macOS 15 or later, Everyone mode, and a self-issued client certificate imported only into memory. Never add private discovery entitlements, extract Apple-account credentials, invent devices or progress, or bypass TLS validation. The explicit macOS AirDrop fallback uses `NSSharingService` (`transfers/system.rs`); its callback also fires on picker cancellation and is not a delivery receipt.

## Verification

Match verification to the change. For documentation-only changes, check links, paths, commands and the diff. For behavior changes, build the release app and exercise the affected UI; a build alone does not prove behavior. Use disposable data outside personal folders. Never modify user files to test mutations. The crate has no test suite.

Rune renders offscreen without opening a window, which is the safe way to check a change while the maintainer is using the Mac:

```sh
crawler-rune/target/release/crawler --open /path/to/file \
  --snap /tmp/out.png --size 1180x760 --at 3 --realtime \
  --state-report /tmp/crawler-state.json
```

- `--snap`, `--size`, `--at`, `--realtime`, `--crop KEY` and `--script FILE.json` are Rune's tooling flags; `--help` lists the rest. A script replays keys (`{"t":2,"type":"key","name":"f","cmd":true}`), typed text, taps and wheel events.
- Headless runs keep preferences in memory and the search cache in a temporary folder. Set `RUNE_PREFS_DIR` to a scratch folder to keep a cache between runs.
- `scripts/crawler [path]` sends a folder or file to the running app through `open`.
- `--folder <absolute-path>` opens Folders at that directory. `--find <query>` opens Search with an initial query. `--open <absolute-path>` opens a file through the normal Enter policy.
- `--search-root <absolute-path>` sets the initial browsing and local-search context. `--search-workers N` overrides the matching worker count.
- `--section <name>` launches into a section; `--activity` and `--settings` are shortcuts. `--crawler-theme <name>` sets the theme for one run.
- `--state-report <path>` writes the state as JSON four times a second, `--launch-report <path>` writes the launch timings once and `--event-log <existing-path>` appends each state.

State reports include the section, the Folders location and history, search status, results and selection, query, index and cache timing, preview readiness, the reader's format and rows, PDF rendering and annotation state and Activity samples. Reports can contain private paths and filenames; keep them local.

For navigation changes, check direct paths, parent/child traversal, back/forward, section switches, empty and missing folders, pinned-folder persistence, connected volumes and external directory changes. For search changes, check rapid typing, Escape and refocus, fuzzy abbreviations, both modes, scope changes, progressive results and selection and preview preservation. For PDF changes, verify annotations after saving and reopening disposable copies, Undo/Redo, removal across unvisited pages and unsaved-close handling.
