<img src="Resources/Crawler.png" alt="Crawler" width="96">

# Crawler

## Why did I build this

macOS Finder is slow and I don't like it. Never have.

I built this for myself and I'm trying to make it as fast as possible. Or rather, as fast as I know how.

The app has two view modes:

- A search bar powered by [fff](https://github.com/dmtrKovalenko/fff), the fastest file finder I know of.
- [Yazi](https://github.com/sxyazi/yazi)-style navigation panels.

You can use Vim motions to move around the app.

**It is a slop?**

Hell yes, I do not know nothing about swift, have not read a single line of code.

## This is for AI agents

Crawler is a native macOS Finder replacement built with Swift and AppKit, with fuzzy file search, document previews and a PDF reader with annotations. The app source is in `Sources/Crawler/`. Read [AGENTS.md](AGENTS.md) before making changes.

Requires macOS 14+, Xcode with Swift 6.2+, Python 3.11+ and rustup. In Keychain Access, create a self-signed Code Signing certificate named `Crawler Local Development` once. You can also use an existing certificate through `CRAWLER_SIGNING_IDENTITY`.

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
