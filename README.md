<img src="Resources/Crawler.png" alt="Crawler" width="96">

# Crawler

## Why did I build this

macOS Finder is slow and I don't like it. Never have.

I built this for myself and I'm trying to make it as fast as possible. Or rather, as fast as I know how.

The title bar has five sections:

- **Search**, powered by [fff](https://github.com/dmtrKovalenko/fff), the fastest file finder I know of.
- **Folders**, [Yazi](https://github.com/sxyazi/yazi)-style panels for browsing and reading a folder.
- **Activity**, **Transfers** (AirDrop) and **Settings**.

You can use Vim motions to move around the app.

**It is a slop?**

Hell yes, I do not know nothing about swift, have not read a single line of code.

## This is for AI agents

Crawler is a native macOS app for finding and reading files, with fuzzy file search, document previews, Markdown and Word reading and a PDF reader with annotations. The app is written in Rust on [Rune](https://github.com/anomalyco/rune), in `crawler-rune/`. The original Swift and AppKit version in `Sources/Crawler/` remains as the reference. Read [AGENTS.md](AGENTS.md) before making changes.

Requires macOS 14+, Python 3.11+ and rustup. The Swift reference app (`scripts/build-swift.sh`) also needs Xcode with Swift 6.2+. In Keychain Access, create a self-signed Code Signing certificate named `Crawler Local Development` once. You can also use an existing certificate through `CRAWLER_SIGNING_IDENTITY`.

From the repository root, build and install in Applications:

```sh
scripts/build.sh
sudo ditto "build/Crawler.app" "/Applications/Crawler.app"
open "/Applications/Crawler.app"
```

The build script downloads the pinned dependencies, builds the release app and signs it. Keep the same signing certificate between builds. There is no Homebrew package yet.

To open folders from a terminal, link the `crawler` command into a folder on your `PATH`:

```sh
ln -s "$PWD/scripts/crawler" ~/.local/bin/crawler
crawler .          # current folder
crawler ~/Code     # any folder; a file path opens the file
```
