//! Crawler's eleven themes. Every theme supplies the same roles, and views read them through
//! [`Look`]; no view picks its own colours.

use rune::{Color, Family, TextStyle};

pub const FONTS: &[&[u8]] = &[
    include_bytes!("../../Resources/Fonts/IBMPlexMono-Regular.ttf"),
    include_bytes!("../../Resources/Fonts/IBMPlexMono-Medium.ttf"),
    include_bytes!("../../Resources/Fonts/IBMPlexMono-SemiBold.ttf"),
];
use rune_kit::{Kit, PaletteSeed};

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize,
)]
pub enum Theme {
    #[default]
    Cupertino,
    Dark,
    Catppuccin,
    Github,
    Vercel,
    System7,
    Terminal,
    WindowsRetro,
    Windows,
    Linux,
    LinuxRetro,
}

/// How a theme marks the active pane and draws shell buttons, after the desktop it imitates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Era {
    /// Tinted blocks and an accent dot on the active pane.
    Modern,
    /// Inverted blocks, pinstripes behind the active title (System 7).
    Classic,
    /// Raised and pressed bevels, a navy gradient title (Windows Retro).
    WindowsBevel,
    /// Raised and pressed bevels, a green accent title (Linux Retro).
    UnixBevel,
    /// Inverted bar, monospaced faces (Terminal).
    Terminal,
}

impl Theme {
    pub const ALL: [Theme; 11] = [
        Theme::Cupertino,
        Theme::Dark,
        Theme::Catppuccin,
        Theme::Github,
        Theme::Vercel,
        Theme::System7,
        Theme::Terminal,
        Theme::WindowsRetro,
        Theme::Windows,
        Theme::Linux,
        Theme::LinuxRetro,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Theme::Cupertino => "Cupertino",
            Theme::Dark => "Dark",
            Theme::Catppuccin => "Catppuccin Mocha",
            Theme::Github => "GitHub",
            Theme::Vercel => "Vercel",
            Theme::System7 => "System 7",
            Theme::Terminal => "Terminal",
            Theme::WindowsRetro => "Windows Retro",
            Theme::Windows => "Windows",
            Theme::Linux => "Linux",
            Theme::LinuxRetro => "Linux Retro",
        }
    }

    pub fn detail(self) -> &'static str {
        match self {
            Theme::Cupertino => "macOS light",
            Theme::Dark => "Graphite",
            Theme::Catppuccin => "Pastel night",
            Theme::Github => "Paper & blue",
            Theme::Vercel => "Black & white",
            Theme::System7 => "Retro Macintosh",
            Theme::Terminal => "Green phosphor",
            Theme::WindowsRetro => "Windows 95",
            Theme::Windows => "Soft blue",
            Theme::Linux => "Ubuntu aubergine",
            Theme::LinuxRetro => "Classic Unix",
        }
    }

    /// Accepts the display name or a lowercase id without spaces (`windowsretro`).
    pub fn parse(s: &str) -> Option<Theme> {
        let squash = |v: &str| v.to_ascii_lowercase().replace([' ', '-', '_'], "");
        Theme::ALL.into_iter().find(|t| {
            squash(t.name()) == squash(s) || format!("{t:?}").to_ascii_lowercase() == squash(s)
        })
    }

    pub fn era(self) -> Era {
        match self {
            Theme::System7 => Era::Classic,
            Theme::WindowsRetro => Era::WindowsBevel,
            Theme::LinuxRetro => Era::UnixBevel,
            Theme::Terminal => Era::Terminal,
            _ => Era::Modern,
        }
    }

    pub fn look(self) -> Look {
        let p = |dark,
                 background,
                 raised,
                 sidebar,
                 text,
                 muted,
                 line,
                 accent,
                 accent_text,
                 selection,
                 selection_text| Look {
            theme: self,
            dark,
            background: rgb(background),
            raised: rgb(raised),
            sidebar: rgb(sidebar),
            text: rgb(text),
            muted: rgb(muted),
            line: rgb(line),
            accent: rgb(accent),
            accent_text: rgb(accent_text),
            selection: rgb(selection),
            selection_text: rgb(selection_text),
        };
        match self {
            Theme::Cupertino => p(
                false, 0xffffff, 0xf8f8fb, 0xf1f0f7, 0x292b34, 0x626b7a, 0xe5e7ed, 0x086cdc,
                0xffffff, 0xe5efff, 0x174d99,
            ),
            Theme::Dark => p(
                true, 0x202124, 0x292a2e, 0x191a1d, 0xededf0, 0xa6a8b1, 0x404147, 0x80b6ff,
                0x101d30, 0x304564, 0xd5e7ff,
            ),
            Theme::Catppuccin => p(
                true, 0x1e1e2e, 0x313244, 0x181825, 0xcdd6f4, 0xa6adc8, 0x45475a, 0xcba6f7,
                0x1e1e2e, 0x45405e, 0xf5e0dc,
            ),
            Theme::Github => p(
                false, 0xffffff, 0xf6f8fa, 0xf6f8fa, 0x1f2328, 0x59636e, 0xd1d9e0, 0x0969da,
                0xffffff, 0xddf4ff, 0x0550ae,
            ),
            Theme::Vercel => p(
                true, 0x000000, 0x111111, 0x0a0a0a, 0xededed, 0xa1a1a1, 0x333333, 0xffffff,
                0x000000, 0x303030, 0xffffff,
            ),
            Theme::System7 => p(
                false, 0xffffff, 0xffffff, 0xffffff, 0x000000, 0x666666, 0x999999, 0x111111,
                0xffffff, 0x111111, 0xffffff,
            ),
            Theme::Terminal => p(
                true, 0x0b120d, 0x101e14, 0x080e09, 0xb3edb4, 0x80b184, 0x30563a, 0x7ce38b,
                0x0b120d, 0x234e2f, 0xd7ffdc,
            ),
            Theme::WindowsRetro => p(
                false, 0xffffff, 0xd4d0c8, 0xd4d0c8, 0x101010, 0x505050, 0x808080, 0x000080,
                0xffffff, 0x000080, 0xffffff,
            ),
            Theme::Windows => p(
                false, 0xffffff, 0xf3f6fb, 0xeaf0f9, 0x1a2330, 0x546477, 0xd5dfea, 0x005fb8,
                0xffffff, 0xcce4fa, 0x003c75,
            ),
            Theme::Linux => p(
                true, 0x2c202d, 0x392b3a, 0x221923, 0xf2eaf0, 0xbfaebc, 0x59455a, 0xffa077,
                0x321307, 0x613e47, 0xffe8de,
            ),
            Theme::LinuxRetro => p(
                false, 0xe8e7df, 0xc8c7bd, 0xbdbfb7, 0x202921, 0x424c41, 0x7c8378, 0x365b47,
                0xffffff, 0x365b47, 0xffffff,
            ),
        }
    }
}

/// The resolved colours of a theme. Cheap to copy; build once per view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub theme: Theme,
    pub dark: bool,
    pub background: Color,
    pub raised: Color,
    pub sidebar: Color,
    pub text: Color,
    pub muted: Color,
    pub line: Color,
    pub accent: Color,
    pub accent_text: Color,
    pub selection: Color,
    pub selection_text: Color,
}

impl Look {
    pub fn era(&self) -> Era {
        self.theme.era()
    }

    pub fn font(&self, size: f32, weight: f32) -> TextStyle {
        let family = match self.theme {
            Theme::System7 => Family::Named("Geneva"),
            Theme::WindowsRetro => Family::Named("Tahoma"),
            Theme::LinuxRetro => Family::Named("Helvetica"),
            Theme::Terminal => Family::Named("IBM Plex Mono"),
            _ => Family::Named("System Font"),
        };
        TextStyle::new(size, weight).family(family)
    }

    /// Cupertino's titlebar material resolves to this neutral gray in the reference captures.
    pub fn chrome(&self) -> Color {
        if self.theme == Theme::Cupertino {
            rgb(0xe4e4e4)
        } else {
            self.sidebar
        }
    }
    pub fn separator(&self) -> Color {
        if self.era() == Era::Classic {
            Color::BLACK
        } else {
            self.line
        }
    }

    /// The rune-kit components' tokens, derived from the same semantic roles as the shell.
    pub fn kit(&self) -> Kit {
        let mut k = Kit::from_palette(PaletteSeed {
            bg: self.background,
            fg: self.text,
            accent: self.accent,
        });
        if self.era() != Era::Modern {
            // Retro desktops draw square corners.
            k.radius = rune_kit::Radius {
                xs: 0.0,
                sm: 0.0,
                md: 0.0,
                lg: 0.0,
                xl: 0.0,
            };
        }
        k.c.page = self.background;
        k.c.bg_base = self.background;
        k.c.layer_01 = self.background;
        k.c.layer_02 = self.raised;
        k.c.surface_base = self.background;
        k.c.surface_raised = self.raised;
        k.c.surface_float = self.raised;
        k.c.bg_button_neutral = self.raised;
        k.c.text_base = self.text;
        k.c.text_strong = self.text;
        k.c.text_muted = self.muted;
        k.c.text_weak = self.muted;
        k.c.text_faint = self.muted;
        k.c.text_accent = self.accent;
        k.c.border_base = self.line;
        k.c.border_weak = self.line;
        k.c.border_muted = self.line;
        k.c.border_strong = self.line;
        k.c.border_focus = self.accent;
        k.c.icon_base = self.text;
        k.c.icon_muted = self.muted;
        k.c.bg_accent = self.accent;
        k.c.text_inverse = self.accent_text;
        k.c.text_invert_strong = self.accent_text;
        k.ty.ui = self.font(13.0, 400.0).lh(20.0);
        k.ty.ui_medium = self.font(13.0, 600.0).lh(20.0);
        k.ty.ui_compact = self.font(13.0, 400.0).lh(16.0);
        k.ty.small = self.font(11.0, 400.0);
        k.ty.small_medium = self.font(11.0, 600.0);
        k.ty.body = self.font(14.0, 400.0);
        k.ty.body_medium = self.font(14.0, 600.0);
        k.ty.text_small = self.font(13.0, 400.0);
        k.ty.text_small_medium = self.font(13.0, 600.0);
        k.ty.text_xsmall = self.font(12.0, 400.0);
        k.ty.text_xsmall_medium = self.font(12.0, 600.0);
        k
    }
}

/// `0xRRGGBB` (sRGB) to Rune's linear colour.
pub fn rgb(hex: u32) -> Color {
    let ch = |shift: u32| rune::color::srgb_decode(((hex >> shift) & 0xff) as f32 / 255.0);
    Color {
        r: ch(16),
        g: ch(8),
        b: ch(0),
        a: 1.0,
    }
}
