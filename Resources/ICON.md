# Crawler icon

`Crawler.png` is the source artwork: a "C" made of six retro stripes on night blue, ending in a found point. `scripts/draw-icon.swift` draws it in code. `Crawler.iconset` and `Crawler.icns` contain its macOS sizes.

Regenerate them from the repository root:

```sh
swift scripts/draw-icon.swift
swift scripts/make-icon.swift Resources
iconutil -c icns Resources/Crawler.iconset -o Resources/Crawler.icns
scripts/build.sh
```
