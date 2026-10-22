//! User configuration (`~/.config/dtm/config.toml`) merged over built-in defaults.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::Deserialize;

use crate::action::Action;
use crate::keys::{format_key, parse_key, KeyCombo, CTRL};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarMode {
    Windows,
    Sessions,
}

#[derive(Clone, Debug)]
pub struct SidebarCfg {
    pub position: Position,
    pub width: u16,
    pub visible: bool,
    pub default_mode: SidebarMode,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub prefix: KeyCombo,
    pub shell: String,
    pub history_limit: usize,
    /// Ask the terminal for mouse reports (sidebar clicks, wheel scrolling).
    pub mouse: bool,
    /// Send copied text to the terminal clipboard with OSC 52.
    pub clipboard: bool,
    pub sidebar: SidebarCfg,
    pub prefix_keys: HashMap<KeyCombo, Action>,
    pub root_keys: HashMap<KeyCombo, Action>,
    pub copy_keys: HashMap<KeyCombo, Action>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    options: RawOptions,
    sidebar: RawSidebar,
    keys: RawKeys,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawOptions {
    prefix: Option<String>,
    shell: Option<String>,
    history_limit: Option<usize>,
    mouse: Option<bool>,
    clipboard: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawSidebar {
    position: Option<Position>,
    width: Option<u16>,
    visible: Option<bool>,
    default_mode: Option<SidebarMode>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawKeys {
    prefix: BTreeMap<String, String>,
    root: BTreeMap<String, String>,
    copy: BTreeMap<String, String>,
}

const DEFAULT_PREFIX_KEYS: &[(&str, &str)] = &[
    ("c", "new-window"),
    ("n", "next-window"),
    ("p", "prev-window"),
    ("1", "select-window 1"),
    ("2", "select-window 2"),
    ("3", "select-window 3"),
    ("4", "select-window 4"),
    ("5", "select-window 5"),
    ("6", "select-window 6"),
    ("7", "select-window 7"),
    ("8", "select-window 8"),
    ("9", "select-window 9"),
    ("|", "split-vertical"),
    ("-", "split-horizontal"),
    ("Left", "focus-left"),
    ("Right", "focus-right"),
    ("Up", "focus-up"),
    ("Down", "focus-down"),
    ("h", "focus-left"),
    ("l", "focus-right"),
    ("k", "focus-up"),
    ("j", "focus-down"),
    ("H", "resize-left 5"),
    ("L", "resize-right 5"),
    ("K", "resize-up 5"),
    ("J", "resize-down 5"),
    ("o", "focus-next"),
    ("z", "zoom-pane"),
    ("x", "kill-pane"),
    ("&", "kill-window"),
    (",", "rename-window"),
    ("$", "rename-session"),
    ("b", "toggle-sidebar"),
    ("s", "toggle-sidebar-mode"),
    ("S", "new-session"),
    ("(", "prev-session"),
    (")", "next-session"),
    ("d", "detach"),
    ("a", "send-prefix"),
    ("r", "reload-config"),
    ("[", "copy-mode"),
    ("PageUp", "copy-mode"),
    ("]", "paste-buffer"),
];

const DEFAULT_COPY_KEYS: &[(&str, &str)] = &[
    ("k", "copy-up"),
    ("Up", "copy-up"),
    ("j", "copy-down"),
    ("Down", "copy-down"),
    ("h", "copy-left"),
    ("Left", "copy-left"),
    ("l", "copy-right"),
    ("Right", "copy-right"),
    ("PageUp", "copy-page-up"),
    ("C-b", "copy-page-up"),
    ("PageDown", "copy-page-down"),
    ("C-f", "copy-page-down"),
    ("C-u", "copy-half-page-up"),
    ("C-d", "copy-half-page-down"),
    ("g", "copy-top"),
    ("Home", "copy-top"),
    ("G", "copy-bottom"),
    ("End", "copy-bottom"),
    ("0", "copy-line-start"),
    ("$", "copy-line-end"),
    ("v", "copy-select"),
    ("Space", "copy-select"),
    ("y", "copy-yank"),
    ("Enter", "copy-yank"),
    ("q", "copy-cancel"),
    ("Escape", "copy-cancel"),
];

fn default_table(defaults: &[(&str, &str)]) -> HashMap<KeyCombo, Action> {
    defaults
        .iter()
        .map(|(k, a)| (parse_key(k).expect("default key"), a.parse().expect("default action")))
        .collect()
}

impl Default for Config {
    fn default() -> Self {
        Config {
            prefix: (crate::keys::Key::Char('a'), CTRL),
            shell: crate::pty::default_shell(),
            history_limit: 10_000,
            mouse: true,
            clipboard: true,
            sidebar: SidebarCfg {
                position: Position::Left,
                width: 24,
                visible: true,
                default_mode: SidebarMode::Windows,
            },
            prefix_keys: default_table(DEFAULT_PREFIX_KEYS),
            root_keys: HashMap::new(),
            copy_keys: default_table(DEFAULT_COPY_KEYS),
        }
    }
}

fn apply_table(
    table: &mut HashMap<KeyCombo, Action>,
    raw: &BTreeMap<String, String>,
    name: &str,
) -> Result<(), String> {
    for (k, v) in raw {
        let combo = parse_key(k).map_err(|e| format!("keys.{name}: {e}"))?;
        if v.trim() == "none" {
            table.remove(&combo);
        } else {
            let action = v.parse().map_err(|e| format!("keys.{name}[{}]: {e}", format_key(combo)))?;
            table.insert(combo, action);
        }
    }
    Ok(())
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Config, String> {
        let raw: RawConfig = toml::from_str(text).map_err(|e| e.message().to_string())?;
        let mut cfg = Config::default();
        if let Some(p) = raw.options.prefix {
            cfg.prefix = parse_key(&p).map_err(|e| format!("options.prefix: {e}"))?;
        }
        if let Some(s) = raw.options.shell {
            cfg.shell = s;
        }
        if let Some(h) = raw.options.history_limit {
            cfg.history_limit = h;
        }
        if let Some(m) = raw.options.mouse {
            cfg.mouse = m;
        }
        if let Some(c) = raw.options.clipboard {
            cfg.clipboard = c;
        }
        let sb = &mut cfg.sidebar;
        if let Some(v) = raw.sidebar.position {
            sb.position = v;
        }
        if let Some(v) = raw.sidebar.width {
            if !(4..=200).contains(&v) {
                return Err("sidebar.width must be between 4 and 200".into());
            }
            sb.width = v;
        }
        if let Some(v) = raw.sidebar.visible {
            sb.visible = v;
        }
        if let Some(v) = raw.sidebar.default_mode {
            sb.default_mode = v;
        }
        apply_table(&mut cfg.prefix_keys, &raw.keys.prefix, "prefix")?;
        apply_table(&mut cfg.root_keys, &raw.keys.root, "root")?;
        apply_table(&mut cfg.copy_keys, &raw.keys.copy, "copy")?;
        Ok(cfg)
    }

    /// Load from `path`; a missing file means defaults.
    pub fn load(path: &Path) -> Result<Config, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => Config::from_toml(&text).map_err(|e| format!("{}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::{CopyCmd, Dir};
    use crate::keys::{Key, ALT};

    #[test]
    fn defaults_are_valid() {
        let c = Config::default();
        assert_eq!(c.prefix, (Key::Char('a'), CTRL));
        assert_eq!(c.prefix_keys[&(Key::Char('|'), 0)], Action::SplitVertical);
        assert_eq!(c.prefix_keys[&(Key::Char('H'), 0)], Action::Resize(Dir::Left, 5));
        assert_eq!(c.sidebar.position, Position::Left);
        assert_eq!(c.sidebar.default_mode, SidebarMode::Windows);
        assert!(c.mouse && c.clipboard);
        assert_eq!(c.prefix_keys[&(Key::Char('['), 0)], Action::CopyMode);
        assert_eq!(c.copy_keys[&(Key::Char('v'), 0)], Action::Copy(CopyCmd::Select));
    }

    #[test]
    fn empty_file_gives_defaults() {
        let c = Config::from_toml("").unwrap();
        assert_eq!(c.prefix_keys.len(), Config::default().prefix_keys.len());
    }

    #[test]
    fn overrides_and_removal() {
        let c = Config::from_toml(
            r#"
            [options]
            prefix = "C-b"
            shell = "/bin/zsh"
            mouse = false
            clipboard = false
            [sidebar]
            position = "right"
            width = 30
            default_mode = "sessions"
            visible = false
            [keys.prefix]
            "c" = "new-session"
            "d" = "none"
            "M-x" = "kill-pane"
            [keys.root]
            "M-n" = "next-window"
            [keys.copy]
            "x" = "copy-yank"
            "q" = "none"
            "#,
        )
        .unwrap();
        assert_eq!(c.prefix, (Key::Char('b'), CTRL));
        assert_eq!(c.shell, "/bin/zsh");
        assert!(!c.mouse && !c.clipboard);
        assert_eq!(c.sidebar.position, Position::Right);
        assert_eq!(c.sidebar.width, 30);
        assert!(!c.sidebar.visible);
        assert_eq!(c.sidebar.default_mode, SidebarMode::Sessions);
        assert_eq!(c.prefix_keys[&(Key::Char('c'), 0)], Action::NewSession);
        assert!(!c.prefix_keys.contains_key(&(Key::Char('d'), 0)));
        assert_eq!(c.prefix_keys[&(Key::Char('x'), ALT)], Action::KillPane);
        assert_eq!(c.root_keys[&(Key::Char('n'), ALT)], Action::NextWindow);
        assert_eq!(c.copy_keys[&(Key::Char('x'), 0)], Action::Copy(CopyCmd::Yank));
        assert!(!c.copy_keys.contains_key(&(Key::Char('q'), 0)));
        assert_eq!(c.copy_keys[&(Key::Escape, 0)], Action::Copy(CopyCmd::Cancel));
        // untouched defaults remain
        assert_eq!(c.prefix_keys[&(Key::Char('z'), 0)], Action::ZoomPane);
    }

    #[test]
    fn errors_are_descriptive() {
        let e = Config::from_toml("[keys.prefix]\n\"x\" = \"bogus\"").unwrap_err();
        assert!(e.contains("keys.prefix[x]") && e.contains("bogus"), "{e}");
        let e = Config::from_toml("[keys.prefix]\n\"Nope\" = \"detach\"").unwrap_err();
        assert!(e.contains("Nope"), "{e}");
        assert!(Config::from_toml("[sidebar]\nposition = \"top\"").is_err());
        assert!(Config::from_toml("[sidebar]\nwidth = 1").is_err());
        assert!(Config::from_toml("[unknown]\nx = 1").is_err());
        assert!(Config::from_toml("this is not toml").is_err());
    }

    #[test]
    fn example_config_parses_and_matches_defaults() {
        let c = Config::from_toml(include_str!("../config.example.toml")).unwrap();
        let d = Config::default();
        assert_eq!(c.prefix, d.prefix);
        assert_eq!(c.history_limit, d.history_limit);
        assert_eq!(c.sidebar.width, d.sidebar.width);
        // every binding documented in the example is a real default, and vice versa
        for (k, a) in &d.prefix_keys {
            if let (Key::Char(ch), 0) = k
                && ch.is_ascii_digit() && *ch != '1' {
                    continue;
                }
            if matches!(k.0, Key::Left | Key::Right | Key::Up | Key::Down) {
                continue;
            }
            assert_eq!(c.prefix_keys.get(k), Some(a), "{}", format_key(*k));
        }
        assert_eq!(c.copy_keys.len(), d.copy_keys.len());
        for (k, a) in &d.copy_keys {
            assert_eq!(c.copy_keys.get(k), Some(a), "{}", format_key(*k));
        }
    }

    #[test]
    fn missing_file_is_defaults() {
        assert!(Config::load(Path::new("/nonexistent/dtm/config.toml")).is_ok());
    }
}
