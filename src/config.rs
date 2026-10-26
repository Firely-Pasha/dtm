//! User configuration (`~/.config/dtm/config.toml`) merged over built-in defaults.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use serde::Deserialize;

use crate::action::Action;
use crate::keys::{format_key, parse_key, KeyCombo, CTRL};
use crate::project::{parse_size, Size, SizeSpec};

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
    /// Moving the selection while the sidebar has focus switches to that item right away.
    pub follow: bool,
}

/// The popup shell's size, as a share of the pane area or in cells.
#[derive(Clone, Debug)]
pub struct PopupCfg {
    pub width: Size,
    pub height: Size,
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
    /// How long (ms) after a resize the plain resize keys keep working; 0 disables.
    pub repeat_time: u64,
    /// Extra program names (besides vim, nvim, fzf, ...) that get Ctrl-h/j/k/l themselves.
    pub navigator_apps: Vec<String>,
    /// Switching to another window focuses its top-left pane (false: the pane focused last).
    pub focus_top_left: bool,
    /// Draw the sidebar, every pane and the dock as separate rounded boxes.
    pub islands: bool,
    pub sidebar: SidebarCfg,
    pub popup: PopupCfg,
    pub prefix_keys: HashMap<KeyCombo, Action>,
    pub root_keys: HashMap<KeyCombo, Action>,
    pub copy_keys: HashMap<KeyCombo, Action>,
    pub resize_keys: HashMap<KeyCombo, Action>,
    pub sidebar_keys: HashMap<KeyCombo, Action>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    options: RawOptions,
    sidebar: RawSidebar,
    popup: RawPopup,
    keys: RawKeys,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawPopup {
    width: Option<SizeSpec>,
    height: Option<SizeSpec>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawOptions {
    prefix: Option<String>,
    shell: Option<String>,
    history_limit: Option<usize>,
    mouse: Option<bool>,
    clipboard: Option<bool>,
    repeat_time: Option<u64>,
    navigator_apps: Option<Vec<String>>,
    window_focus: Option<WindowFocus>,
    style: Option<Style>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Style {
    Plain,
    Islands,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum WindowFocus {
    TopLeft,
    Last,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawSidebar {
    position: Option<Position>,
    width: Option<u16>,
    visible: Option<bool>,
    default_mode: Option<SidebarMode>,
    follow: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
struct RawKeys {
    prefix: BTreeMap<String, String>,
    root: BTreeMap<String, String>,
    copy: BTreeMap<String, String>,
    resize: BTreeMap<String, String>,
    sidebar: BTreeMap<String, String>,
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
    ("h", "resize-left 5"),
    ("l", "resize-right 5"),
    ("k", "resize-up 5"),
    ("j", "resize-down 5"),
    ("H", "resize-left 5"),
    ("L", "resize-right 5"),
    ("K", "resize-up 5"),
    ("J", "resize-down 5"),
    ("o", "focus-next"),
    ("z", "zoom-pane"),
    ("x", "kill-pane"),
    ("&", "kill-window"),
    (",", "rename-window"),
    ("C", "new-group"),
    ("$", "rename-session"),
    ("b", "toggle-sidebar"),
    ("s", "toggle-sidebar-mode"),
    ("S", "new-session"),
    ("(", "prev-session"),
    (")", "next-session"),
    ("d", "detach"),
    ("a", "send-prefix"),
    ("r", "reload-config"),
    ("R", "respawn-pane"),
    ("M-r", "respawn-window"),
    ("f", "search-windows"),
    ("/", "search-windows"),
    ("m", "toggle-maximize"),
    // Then plain Tab / Shift-Tab keep stepping for the repeat time.
    ("Tab", "recent-window"),
    ("S-Tab", "recent-window-reverse"),
    ("P", "toggle-popup"),
    ("D", "toggle-dock"),
    ("}", "dock-next"),
    ("{", "dock-prev"),
    ("A", "dock-add"),
    ("X", "dock-close"),
    ("[", "copy-mode"),
    ("PageUp", "copy-mode"),
    ("]", "paste-buffer"),
    ("w", "focus-sidebar"),
    (";", "focus-last"),
    // Ctrl-l is taken by navigation; this sends it to the shell (clear screen).
    ("C-l", "send-key"),
];

const DEFAULT_ROOT_KEYS: &[(&str, &str)] = &[
    ("C-h", "navigate-left"),
    ("C-j", "navigate-down"),
    ("C-k", "navigate-up"),
    ("C-l", "navigate-right"),
    ("C-Tab", "recent-window"),
    ("C-S-Tab", "recent-window-reverse"),
];

const DEFAULT_RESIZE_KEYS: &[(&str, &str)] = &[
    ("h", "resize-left 1"),
    ("Left", "resize-left 1"),
    ("j", "resize-down 1"),
    ("Down", "resize-down 1"),
    ("k", "resize-up 1"),
    ("Up", "resize-up 1"),
    ("l", "resize-right 1"),
    ("Right", "resize-right 1"),
];

const DEFAULT_SIDEBAR_KEYS: &[(&str, &str)] = &[
    ("j", "sidebar-down"),
    ("Down", "sidebar-down"),
    ("k", "sidebar-up"),
    ("Up", "sidebar-up"),
    ("Enter", "sidebar-open"),
    ("o", "sidebar-open"),
    ("Space", "sidebar-open"),
    ("x", "sidebar-kill"),
    ("r", "sidebar-rename"),
    (",", "sidebar-rename"),
    ("J", "sidebar-move-down"),
    ("K", "sidebar-move-up"),
    ("n", "sidebar-new"),
    ("R", "sidebar-respawn"),
    ("e", "sidebar-export"),
    ("/", "sidebar-search"),
    ("g", "sidebar-rename-group"),
    ("t", "sidebar-group-tag"),
    ("Tab", "toggle-sidebar-mode"),
    ("q", "sidebar-leave"),
    ("Escape", "sidebar-leave"),
];

/// Sidebar defaults, plus the keys that step back towards the panes.
fn default_sidebar_keys(position: Position) -> HashMap<KeyCombo, Action> {
    let mut t = default_table(DEFAULT_SIDEBAR_KEYS);
    let back: &[&str] = match position {
        Position::Left => &["l", "Right"],
        Position::Right => &["h", "Left"],
    };
    for k in back {
        t.insert(parse_key(k).expect("default key"), Action::Sidebar(crate::action::SidebarCmd::Leave));
    }
    t
}

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
    ("V", "copy-select-line"),
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
            repeat_time: 1000,
            navigator_apps: Vec::new(),
            focus_top_left: true,
            islands: true,
            sidebar: SidebarCfg {
                position: Position::Left,
                width: 24,
                visible: true,
                default_mode: SidebarMode::Windows,
                follow: true,
            },
            prefix_keys: default_table(DEFAULT_PREFIX_KEYS),
            root_keys: default_table(DEFAULT_ROOT_KEYS),
            copy_keys: default_table(DEFAULT_COPY_KEYS),
            resize_keys: default_table(DEFAULT_RESIZE_KEYS),
            popup: PopupCfg { width: Size::Percent(80.0), height: Size::Percent(80.0) },
            sidebar_keys: default_sidebar_keys(Position::Left),
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
        if let Some(t) = raw.options.repeat_time {
            cfg.repeat_time = t;
        }
        if let Some(a) = raw.options.navigator_apps {
            cfg.navigator_apps = a;
        }
        if let Some(f) = raw.options.window_focus {
            cfg.focus_top_left = f == WindowFocus::TopLeft;
        }
        if let Some(s) = raw.options.style {
            cfg.islands = s == Style::Islands;
        }
        let sb = &mut cfg.sidebar;
        if let Some(v) = raw.sidebar.position {
            sb.position = v;
            cfg.sidebar_keys = default_sidebar_keys(v);
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
        if let Some(v) = raw.sidebar.follow {
            sb.follow = v;
        }
        if let Some(v) = &raw.popup.width {
            cfg.popup.width = parse_size(v).map_err(|e| format!("popup.width: {e}"))?;
        }
        if let Some(v) = &raw.popup.height {
            cfg.popup.height = parse_size(v).map_err(|e| format!("popup.height: {e}"))?;
        }
        apply_table(&mut cfg.prefix_keys, &raw.keys.prefix, "prefix")?;
        apply_table(&mut cfg.root_keys, &raw.keys.root, "root")?;
        apply_table(&mut cfg.copy_keys, &raw.keys.copy, "copy")?;
        apply_table(&mut cfg.resize_keys, &raw.keys.resize, "resize")?;
        apply_table(&mut cfg.sidebar_keys, &raw.keys.sidebar, "sidebar")?;
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
    use crate::action::{CopyCmd, Dir, SidebarCmd};
    use crate::keys::{Key, ALT};

    #[test]
    fn defaults_are_valid() {
        let c = Config::default();
        assert_eq!(c.prefix, (Key::Char('a'), CTRL));
        assert_eq!(c.prefix_keys[&(Key::Char('|'), 0)], Action::SplitVertical);
        assert_eq!(c.prefix_keys[&(Key::Char('H'), 0)], Action::Resize(Dir::Left, 5));
        assert_eq!(c.sidebar.position, Position::Left);
        assert_eq!(c.sidebar.default_mode, SidebarMode::Windows);
        assert!(c.sidebar.follow);
        assert!(c.mouse && c.clipboard && c.focus_top_left && c.islands);
        assert!(!Config::from_toml("[options]\nstyle = \"plain\"").unwrap().islands);
        assert!(Config::from_toml("[options]\nstyle = \"classic\"").is_err());
        assert!(Config::from_toml("[options]\nstyle = \"fancy\"").is_err());
        assert_eq!(c.root_keys[&(Key::Tab, CTRL)], Action::RecentWindow(true));
        assert_eq!(c.prefix_keys[&(Key::Tab, 0)], Action::RecentWindow(true));
        assert_eq!(c.prefix_keys[&(Key::Char('/'), 0)], Action::SearchWindows);
        assert_eq!(c.prefix_keys[&(Key::Char('m'), 0)], Action::ToggleMaximize);
        assert_eq!(Config::from_toml("[options]\nwindow_focus = \"last\"").map(|c| c.focus_top_left), Ok(false));
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
            follow = false
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
        assert!(!c.sidebar.visible && !c.sidebar.follow);
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
    fn repeat_time_resize_and_sidebar_tables() {
        let d = Config::default();
        assert_eq!(d.repeat_time, 1000);
        assert_eq!(d.resize_keys[&(Key::Char('h'), 0)], Action::Resize(Dir::Left, 1));
        assert_eq!(d.prefix_keys[&(Key::Char('w'), 0)], Action::FocusSidebar);
        assert_eq!(d.sidebar_keys[&(Key::Char('x'), 0)], Action::Sidebar(SidebarCmd::Kill));
        assert_eq!(d.copy_keys[&(Key::Char('V'), 0)], Action::Copy(CopyCmd::SelectLine));
        // stepping back towards the panes depends on the sidebar side
        assert_eq!(d.sidebar_keys[&(Key::Char('l'), 0)], Action::Sidebar(SidebarCmd::Leave));
        assert!(!d.sidebar_keys.contains_key(&(Key::Char('h'), 0)));

        let c = Config::from_toml(
            r#"
            [options]
            repeat_time = 250
            [sidebar]
            position = "right"
            [keys.resize]
            "a" = "resize-left 3"
            "h" = "none"
            [keys.sidebar]
            "d" = "sidebar-kill"
            "x" = "none"
            "#,
        )
        .unwrap();
        assert_eq!(c.repeat_time, 250);
        assert_eq!(c.resize_keys[&(Key::Char('a'), 0)], Action::Resize(Dir::Left, 3));
        assert!(!c.resize_keys.contains_key(&(Key::Char('h'), 0)));
        assert_eq!(c.sidebar_keys[&(Key::Char('d'), 0)], Action::Sidebar(SidebarCmd::Kill));
        assert!(!c.sidebar_keys.contains_key(&(Key::Char('x'), 0)));
        assert_eq!(c.sidebar_keys[&(Key::Char('h'), 0)], Action::Sidebar(SidebarCmd::Leave));
        assert!(!c.sidebar_keys.contains_key(&(Key::Char('l'), 0)));
        assert!(Config::from_toml("[keys.sidebar]\n\"x\" = \"bogus\"").is_err());
    }

    #[test]
    fn prefix_hjkl_resize_and_arrows_focus() {
        let d = Config::default();
        assert_eq!(d.prefix_keys[&(Key::Char('h'), 0)], Action::Resize(Dir::Left, 5));
        assert_eq!(d.prefix_keys[&(Key::Char('j'), 0)], Action::Resize(Dir::Down, 5));
        assert_eq!(d.prefix_keys[&(Key::Char('k'), 0)], Action::Resize(Dir::Up, 5));
        assert_eq!(d.prefix_keys[&(Key::Char('l'), 0)], Action::Resize(Dir::Right, 5));
        assert_eq!(d.prefix_keys[&(Key::Left, 0)], Action::Focus(Dir::Left));
        assert_eq!(d.prefix_keys[&(Key::Down, 0)], Action::Focus(Dir::Down));
        assert_eq!(d.prefix_keys[&(Key::Char('o'), 0)], Action::FocusNext);
    }

    #[test]
    fn ctrl_hjkl_navigation_defaults() {
        use crate::keys::CTRL;
        let d = Config::default();
        assert_eq!(d.root_keys[&(Key::Char('h'), CTRL)], Action::Navigate(Dir::Left));
        assert_eq!(d.root_keys[&(Key::Char('j'), CTRL)], Action::Navigate(Dir::Down));
        assert_eq!(d.root_keys[&(Key::Char('k'), CTRL)], Action::Navigate(Dir::Up));
        assert_eq!(d.root_keys[&(Key::Char('l'), CTRL)], Action::Navigate(Dir::Right));
        // the shell's own Ctrl-l (clear screen) stays reachable through the prefix
        assert_eq!(d.prefix_keys[&(Key::Char('l'), CTRL)], Action::SendKey);
        assert_eq!(d.prefix_keys[&(Key::Char(';'), 0)], Action::FocusLast);
        assert!(d.navigator_apps.is_empty());

        let c = Config::from_toml(
            r#"
            [options]
            navigator_apps = ["hx"]
            [keys.root]
            "C-h" = "none"
            "C-\\" = "navigate-last"
            "M-h" = "navigate-left"
            "#,
        )
        .unwrap();
        assert_eq!(c.navigator_apps, vec!["hx".to_string()]);
        assert!(!c.root_keys.contains_key(&(Key::Char('h'), CTRL)));
        assert_eq!(c.root_keys[&(Key::Char('\\'), CTRL)], Action::NavigateLast);
        assert_eq!(c.root_keys[&(Key::Char('h'), ALT)], Action::Navigate(Dir::Left));
        assert_eq!(c.root_keys[&(Key::Char('l'), CTRL)], Action::Navigate(Dir::Right));
    }

    #[test]
    fn example_config_parses_and_matches_defaults() {
        let c = Config::from_toml(include_str!("../examples/config.toml")).unwrap();
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
