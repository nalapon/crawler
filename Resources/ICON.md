# Crawler icon

`Crawler.png` is the source artwork for the blue filing-cabinet icon. `Crawler.iconset` and `Crawler.icns` contain its macOS sizes.

Regenerate them from the repository root:

```sh
swift scripts/make-icon.swift Resources
iconutil -c icns Resources/Crawler.iconset -o Resources/Crawler.icns
scripts/build.sh
```
