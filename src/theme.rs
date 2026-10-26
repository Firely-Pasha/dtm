//! Colours: the `[theme]` slots, their presets, and the attributes drawing code asks for.

use crate::term::cell::{flags, Attrs, Color};

/// The colour slots a theme sets. `Color::Default` means "the terminal's own colour".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Theme {
    /// Box borders and separators.
    pub border: Color,
    /// The border of whatever has the keyboard.
    pub border_focus: Color,
    /// Dim text: subheaders, hints, placeholders.
    pub muted: Color,
    /// Titles (the session name, prompts).
    pub title: Color,
    /// The row the sidebar's keyboard selection is on; `Default` background means inverted.
    pub selection_bg: Color,
    pub selection_fg: Color,
    /// The shown window in the sidebar, and the dock's current tab; `Default` means inverted.
    pub active_bg: Color,
    pub active_fg: Color,
    /// Copy-mode search matches; `Default` background means inverted.
    pub match_bg: Color,
    pub match_current_bg: Color,
}

/// Preset names, for the config's error message.
pub const PRESETS: &[&str] = &["default", "mono", "nord", "gruvbox"];

const fn rgb(hex: u32) -> Color {
    Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

impl Theme {
    /// What dtm looked like before themes: palette colours, so it follows the terminal's.
    pub const DEFAULT: Theme = Theme {
        border: Color::Idx(8),
        border_focus: Color::Idx(2),
        muted: Color::Idx(8),
        title: Color::Default,
        selection_bg: Color::Default,
        selection_fg: Color::Default,
        active_bg: Color::Default,
        active_fg: Color::Default,
        match_bg: Color::Idx(3),
        match_current_bg: Color::Idx(5),
    };

    pub fn preset(name: &str) -> Option<Theme> {
        Some(match name {
            "default" => Theme::DEFAULT,
            // No colour of its own: grey borders, a plain focused one, inverted selections.
            "mono" => Theme {
                border_focus: Color::Default,
                match_bg: Color::Default,
                match_current_bg: Color::Default,
                ..Theme::DEFAULT
            },
            "nord" => Theme {
                border: rgb(0x4c566a),
                border_focus: rgb(0x88c0d0),
                muted: rgb(0x616e88),
                title: rgb(0xeceff4),
                selection_bg: rgb(0x434c5e),
                selection_fg: rgb(0xeceff4),
                active_bg: rgb(0x3b4252),
                active_fg: rgb(0xd8dee9),
                match_bg: rgb(0xebcb8b),
                match_current_bg: rgb(0xd08770),
            },
            "gruvbox" => Theme {
                border: rgb(0x504945),
                border_focus: rgb(0xfabd2f),
                muted: rgb(0x928374),
                title: rgb(0xebdbb2),
                selection_bg: rgb(0x504945),
                selection_fg: rgb(0xfbf1c7),
                active_bg: rgb(0x3c3836),
                active_fg: rgb(0xebdbb2),
                match_bg: rgb(0xfabd2f),
                match_current_bg: rgb(0xfe8019),
            },
            _ => return None,
        })
    }

    fn fg(color: Color, flags: u16) -> Attrs {
        Attrs { fg: color, bg: Color::Default, flags }
    }

    /// A row style: the given colours, or inverted when the background is not set.
    fn row(fg: Color, bg: Color, bold: bool) -> Attrs {
        let bold = if bold { flags::BOLD } else { 0 };
        if bg == Color::Default {
            Attrs { fg: Color::Default, bg: Color::Default, flags: flags::REVERSE | bold }
        } else {
            Attrs { fg, bg, flags: bold }
        }
    }

    pub fn border(&self, focused: bool) -> Attrs {
        Theme::fg(if focused { self.border_focus } else { self.border }, 0)
    }

    pub fn muted(&self) -> Attrs {
        Theme::fg(self.muted, 0)
    }

    pub fn title(&self) -> Attrs {
        Theme::fg(self.title, flags::BOLD)
    }

    /// The sidebar's keyboard selection (and a focused dock tab).
    pub fn selection(&self) -> Attrs {
        Theme::row(self.selection_fg, self.selection_bg, true)
    }

    /// The shown window in the sidebar (and the dock's current tab).
    pub fn active(&self) -> Attrs {
        Theme::row(self.active_fg, self.active_bg, false)
    }

    /// A copy-mode search match; the current one stands out more.
    pub fn found(&self, current: bool) -> Attrs {
        let bg = if current { self.match_current_bg } else { self.match_bg };
        if bg == Color::Default {
            let extra = if current { flags::BOLD | flags::UNDERLINE } else { 0 };
            Attrs { fg: Color::Default, bg: Color::Default, flags: flags::REVERSE | extra }
        } else {
            Attrs { fg: Color::Idx(0), bg, flags: if current { flags::BOLD } else { 0 } }
        }
    }
}

impl Default for Theme {
    fn default() -> Theme {
        Theme::DEFAULT
    }
}

/// A colour as written in the config: a name, a 0-255 palette index or `#rrggbb`.
pub fn parse_color(text: &str) -> Result<Color, String> {
    let t = text.trim().to_ascii_lowercase();
    const NAMES: [&str; 8] = ["black", "red", "green", "yellow", "blue", "magenta", "cyan", "white"];
    if t == "default" || t == "none" {
        return Ok(Color::Default);
    }
    if let Some(hex) = t.strip_prefix('#') {
        let ok = hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit());
        let v = u32::from_str_radix(hex, 16).ok().filter(|_| ok);
        return v.map(rgb).ok_or_else(|| format!("{text:?} is not a #rrggbb colour"));
    }
    if let Ok(n) = t.parse::<u16>() {
        return u8::try_from(n).map(Color::Idx).map_err(|_| format!("{text:?}: palette colours are 0-255"));
    }
    if t == "grey" || t == "gray" {
        return Ok(Color::Idx(8));
    }
    let (bright, name) = match t.strip_prefix("bright-") {
        Some(rest) => (8, rest),
        None => (0, t.as_str()),
    };
    NAMES
        .iter()
        .position(|n| *n == name)
        .map(|i| Color::Idx(i as u8 + bright))
        .ok_or_else(|| format!("unknown colour {text:?} (a name like red or bright-black, 0-255, or #rrggbb)"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_indexes_and_hex() {
        assert_eq!(parse_color("red"), Ok(Color::Idx(1)));
        assert_eq!(parse_color("Bright-Black"), Ok(Color::Idx(8)));
        assert_eq!(parse_color("bright-white"), Ok(Color::Idx(15)));
        assert_eq!(parse_color("gray"), Ok(Color::Idx(8)));
        assert_eq!(parse_color("208"), Ok(Color::Idx(208)));
        assert_eq!(parse_color("#88C0d0"), Ok(Color::Rgb(0x88, 0xc0, 0xd0)));
        assert_eq!(parse_color("default"), Ok(Color::Default));
        assert!(parse_color("256").is_err());
        assert!(parse_color("#12345").is_err());
        assert!(parse_color("#12345g").is_err());
        assert!(parse_color("bright-").is_err());
        assert!(parse_color("mauve").unwrap_err().contains("unknown colour"));
    }

    #[test]
    fn the_default_theme_is_what_dtm_always_drew() {
        let t = Theme::DEFAULT;
        assert_eq!(t.border(false), Attrs { fg: Color::Idx(8), bg: Color::Default, flags: 0 });
        assert_eq!(t.border(true), Attrs { fg: Color::Idx(2), bg: Color::Default, flags: 0 });
        assert_eq!(t.muted(), Attrs { fg: Color::Idx(8), bg: Color::Default, flags: 0 });
        assert_eq!(t.title(), Attrs { fg: Color::Default, bg: Color::Default, flags: flags::BOLD });
        assert_eq!(t.selection(), Attrs { fg: Color::Default, bg: Color::Default, flags: flags::BOLD | flags::REVERSE });
        assert_eq!(t.active(), Attrs { fg: Color::Default, bg: Color::Default, flags: flags::REVERSE });
    }

    #[test]
    fn rows_use_their_colours_once_a_background_is_set() {
        let t = Theme::preset("nord").unwrap();
        let sel = t.selection();
        assert_eq!((sel.fg, sel.bg), (rgb(0xeceff4), rgb(0x434c5e)));
        assert!(sel.has(flags::BOLD) && !sel.has(flags::REVERSE));
        let act = t.active();
        assert!(!act.has(flags::REVERSE) && !act.has(flags::BOLD));
    }

    #[test]
    fn matches_are_inverted_without_a_colour_and_the_current_one_is_louder() {
        let mono = Theme::preset("mono").unwrap();
        assert!(mono.found(false).has(flags::REVERSE));
        assert!(mono.found(true).has(flags::UNDERLINE));
        let t = Theme::DEFAULT;
        assert_eq!(t.found(false).bg, Color::Idx(3));
        assert_eq!(t.found(true).bg, Color::Idx(5));
    }

    #[test]
    fn every_listed_preset_exists() {
        for name in PRESETS {
            assert!(Theme::preset(name).is_some(), "{name}");
        }
        assert!(Theme::preset("solarized").is_none());
    }
}
