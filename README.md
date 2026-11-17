<img src="Resources/Crawler.png" alt="Crawler" width="96">

# Crawler

## Why did I build this

macOS Finder is slow and I don't like it. Never have.

It was first built in Swift. Now it is written in Rust on [Rune](https://github.com/anomalyco/rune).

I built this for myself and I'm trying to make it as fast as possible. Or rather, as fast as I know how.

The title bar has five sections:

- **Search**, powered by [fff](https://github.com/dmtrKovalenko/fff), the fastest file finder I know of.
- **Folders**, [Yazi](https://github.com/sxyazi/yazi)-style panels for browsing and reading a folder.
- **Activity**, **Transfers** (AirDrop) and **Settings**.

Every file opens inside Crawler: PDFs, Markdown, Word, spreadsheets, code, plain text, images and video. It never hands a file to another app.

You can use Vim motions to move around the app.

**Is it slop?**

Hell yes. I don't know anything about Rust and I haven't read a single line of the code.

## This is for AI agents

Crawler is a native macOS app for finding and reading files, with fuzzy file search, document previews, readers for Markdown, Word, spreadsheets, code and images, and a PDF reader with annotations. Every file opens inside the app. The code is in `crawler-rune/`; the Swift version has been removed. Read [AGENTS.md](AGENTS.md) before making changes.

Requires macOS 14+, Python 3.11+ and rustup. In Keychain Access, create a self-signed Code Signing certificate named `Crawler Local Development` once. You can also use an existing certificate through `CRAWLER_SIGNING_IDENTITY`.

From the repository root, build and install in Applications:

```sh
scripts/build.sh
sudo ditto "build/Crawler.app" "/Applications/Crawler.app"
open "/Applications/Crawler.app"
```

The build script builds the release app and signs it; Cargo downloads the pinned dependencies. Keep the same signing certificate between builds. There is no Homebrew package yet.

To open folders from a terminal, link the `crawler` command into a folder on your `PATH`:

```sh
ln -s "$PWD/scripts/crawler" ~/.local/bin/crawler
crawler .          # current folder
crawler ~/Code     # any folder; a file path opens the file
```
