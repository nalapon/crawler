import Foundation

// Every theme supplies the same roles. Views never need to choose their own palette.
struct ThemePalette {
  let dark: Bool
  let background: UInt32
  let raised: UInt32
  let sidebar: UInt32
  let text: UInt32
  let muted: UInt32
  let line: UInt32
  let accent: UInt32
  let accentText: UInt32
  let selection: UInt32
  let selectionText: UInt32
}

extension Theme {
  var palette: ThemePalette {
    switch self {
    case .cupertino:
      ThemePalette(dark: false, background: 0xffffff, raised: 0xf8f8fb, sidebar: 0xf1f0f7,
        text: 0x292b34, muted: 0x626b7a, line: 0xe5e7ed, accent: 0x086cdc,
        accentText: 0xffffff, selection: 0xe5efff, selectionText: 0x174d99)
    case .dark:
      ThemePalette(dark: true, background: 0x202124, raised: 0x292a2e, sidebar: 0x191a1d,
        text: 0xededf0, muted: 0xa6a8b1, line: 0x404147, accent: 0x80b6ff,
        accentText: 0x101d30, selection: 0x304564, selectionText: 0xd5e7ff)
    case .catppuccin:
      ThemePalette(dark: true, background: 0x1e1e2e, raised: 0x313244, sidebar: 0x181825,
        text: 0xcdd6f4, muted: 0xa6adc8, line: 0x45475a, accent: 0xcba6f7,
        accentText: 0x1e1e2e, selection: 0x45405e, selectionText: 0xf5e0dc)
    case .github:
      ThemePalette(dark: false, background: 0xffffff, raised: 0xf6f8fa, sidebar: 0xf6f8fa,
        text: 0x1f2328, muted: 0x59636e, line: 0xd1d9e0, accent: 0x0969da,
        accentText: 0xffffff, selection: 0xddf4ff, selectionText: 0x0550ae)
    case .vercel:
      ThemePalette(dark: true, background: 0x000000, raised: 0x111111, sidebar: 0x0a0a0a,
        text: 0xededed, muted: 0xa1a1a1, line: 0x333333, accent: 0xffffff,
        accentText: 0x000000, selection: 0x303030, selectionText: 0xffffff)
    case .system7:
      ThemePalette(dark: false, background: 0xffffff, raised: 0xffffff, sidebar: 0xffffff,
        text: 0x000000, muted: 0x666666, line: 0x999999, accent: 0x111111,
        accentText: 0xffffff, selection: 0x111111, selectionText: 0xffffff)
    case .terminal:
      ThemePalette(dark: true, background: 0x0b120d, raised: 0x101e14, sidebar: 0x080e09,
        text: 0xb3edb4, muted: 0x80b184, line: 0x30563a, accent: 0x7ce38b,
        accentText: 0x0b120d, selection: 0x234e2f, selectionText: 0xd7ffdc)
    case .windowsRetro:
      ThemePalette(dark: false, background: 0xffffff, raised: 0xd4d0c8, sidebar: 0xd4d0c8,
        text: 0x101010, muted: 0x505050, line: 0x808080, accent: 0x000080,
        accentText: 0xffffff, selection: 0x000080, selectionText: 0xffffff)
    case .windows:
      ThemePalette(dark: false, background: 0xffffff, raised: 0xf3f6fb, sidebar: 0xeaf0f9,
        text: 0x1a2330, muted: 0x546477, line: 0xd5dfea, accent: 0x005fb8,
        accentText: 0xffffff, selection: 0xcce4fa, selectionText: 0x003c75)
    case .linux:
      ThemePalette(dark: true, background: 0x2c202d, raised: 0x392b3a, sidebar: 0x221923,
        text: 0xf2eaf0, muted: 0xbfaebc, line: 0x59455a, accent: 0xffa077,
        accentText: 0x321307, selection: 0x613e47, selectionText: 0xffe8de)
    case .linuxRetro:
      ThemePalette(dark: false, background: 0xe8e7df, raised: 0xc8c7bd, sidebar: 0xbdbfb7,
        text: 0x202921, muted: 0x424c41, line: 0x7c8378, accent: 0x365b47,
        accentText: 0xffffff, selection: 0x365b47, selectionText: 0xffffff)
    }
  }
}
