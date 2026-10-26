//! Named commands that key bindings map to.

use std::str::FromStr;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Left,
    Right,
    Up,
    Down,
}

/// Commands available while the sidebar has focus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarCmd {
    Down,
    Up,
    /// Switch to the selected window or session and leave the sidebar.
    Open,
    Kill,
    Rename,
    MoveDown,
    MoveUp,
    New,
    Respawn,
    Leave,
    /// Write the session to its project file.
    Export,
    /// Filter the windows by typing.
    Search,
    /// Rename the selected window's group.
    RenameGroup,
    /// Give the selected window's group another tag.
    GroupTag,
}

/// Movement and editing commands available in copy mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CopyCmd {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    HalfPageUp,
    HalfPageDown,
    Top,
    Bottom,
    LineStart,
    LineEnd,
    /// Start (or clear) the selection.
    Select,
    /// Select whole lines (switches an existing selection to line mode).
    SelectLine,
    /// Select a rectangle of columns.
    SelectRect,
    /// Search forward / backward (asks for the text), and repeat the last search.
    SearchForward,
    SearchBackward,
    SearchNext,
    SearchPrev,
    /// Word motions, and the top / middle / bottom row of the view.
    WordNext,
    WordPrev,
    WordEnd,
    ViewTop,
    ViewMiddle,
    ViewBottom,
    /// Copy the selection (or current line) and leave copy mode.
    Yank,
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    NewWindow,
    KillWindow,
    NextWindow,
    PrevWindow,
    /// Step through windows in the order they were last opened (`true`: further back).
    RecentWindow(bool),
    /// Window n (1-based, its tag for 1-9) of the first group.
    SelectWindow(usize),
    RenameWindow,
    /// Ask for a name and start a group with a new window in it.
    NewGroup,
    /// Close the shown session (asks first).
    KillSession,
    /// Ask for a command (an action name or one of the `dtm run` commands) and run it.
    CommandPrompt,
    /// A list of the key bindings.
    Help,
    SplitVertical,
    SplitHorizontal,
    Focus(Dir),
    FocusNext,
    Resize(Dir, u16),
    KillPane,
    ZoomPane,
    NewSession,
    NextSession,
    PrevSession,
    SelectSession(usize),
    RenameSession,
    ToggleSidebar,
    ToggleSidebarMode,
    Detach,
    SendPrefix,
    ReloadConfig,
    CopyMode,
    Copy(CopyCmd),
    PasteBuffer,
    FocusSidebar,
    Sidebar(SidebarCmd),
    /// Focus the previously focused pane.
    FocusLast,
    /// Like `Focus`, but hand the key to an editor in the pane (which navigates
    /// its own splits and asks dtm to move on at its edge).
    Navigate(Dir),
    NavigateLast,
    /// Send the key that triggered this binding to the pane.
    SendKey,
    /// Restart the focused pane the way it was started (asks first).
    RespawnPane,
    /// Restart the window from its project layout (asks first).
    RespawnWindow,
    /// Write the session to its project file.
    ExportProject,
    /// Focus the sidebar and filter its windows by typing.
    SearchWindows,
    /// The focused pane (window, dock or popup) or sidebar fills the whole terminal.
    ToggleMaximize,
    /// Show or hide the popup shell (it keeps running while hidden).
    TogglePopup,
    ToggleDock,
    FocusDock,
    DockNext,
    DockPrev,
    /// Ask for a command and add a pane running it to the dock.
    DockAdd,
    /// Close the dock's current pane (asks first).
    DockClose,
    DockRename,
}

/// Every name `Action::from_str` accepts (the ones that take a number are listed bare).
pub const ACTION_NAMES: &[&str] = &[
    "command-prompt", "copy-bottom", "copy-cancel", "copy-down", "copy-half-page-down", "copy-half-page-up",
    "copy-left", "copy-line-end", "copy-line-start", "copy-mode", "copy-page-down", "copy-page-up",
    "copy-right", "copy-search-backward", "copy-search-forward", "copy-search-next", "copy-search-prev",
    "copy-select", "copy-select-line", "copy-select-rect", "copy-top", "copy-up", "copy-view-bottom",
    "copy-view-middle", "copy-view-top", "copy-word-end", "copy-word-next", "copy-word-prev", "copy-yank",
    "detach", "dock-add", "dock-close", "dock-next", "dock-prev", "dock-rename", "export-project",
    "focus-dock", "focus-down", "focus-last", "focus-left", "focus-next", "focus-right", "focus-sidebar",
    "focus-up", "help", "kill-pane", "kill-session", "kill-window", "navigate-down", "navigate-last",
    "navigate-left", "navigate-right", "navigate-up", "new-group", "new-session", "new-window",
    "next-session", "next-window", "paste-buffer", "prev-session", "prev-window", "recent-window",
    "recent-window-reverse", "reload-config", "rename-session", "rename-window", "resize-down", "resize-left",
    "resize-right", "resize-up", "respawn-pane", "respawn-window", "search-windows", "select-session",
    "select-window", "send-key", "send-prefix", "sidebar-down", "sidebar-export", "sidebar-group-tag",
    "sidebar-kill", "sidebar-leave", "sidebar-move-down", "sidebar-move-up", "sidebar-new", "sidebar-open",
    "sidebar-rename", "sidebar-rename-group", "sidebar-respawn", "sidebar-search", "sidebar-up",
    "split-horizontal", "split-vertical", "toggle-dock", "toggle-maximize", "toggle-popup", "toggle-sidebar",
    "toggle-sidebar-mode", "zoom-pane",
];

fn number<T: FromStr>(arg: Option<&str>, name: &str) -> Result<T, String> {
    arg.ok_or_else(|| format!("{name} needs a number"))?
        .parse()
        .map_err(|_| format!("{name}: invalid number"))
}

impl std::fmt::Display for Dir {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Dir::Left => "left",
            Dir::Right => "right",
            Dir::Up => "up",
            Dir::Down => "down",
        })
    }
}

/// The text `Action::from_str` reads back: `select-window 3`, `resize-left 5`, `toggle-maximize`.
impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::SelectWindow(n) => write!(f, "select-window {n}"),
            Action::SelectSession(n) => write!(f, "select-session {n}"),
            Action::Resize(dir, n) => write!(f, "resize-{dir} {n}"),
            _ => match ACTION_NAMES.iter().find(|n| n.parse::<Action>().as_ref() == Ok(self)) {
                Some(name) => f.write_str(name),
                None => write!(f, "{self:?}"),
            },
        }
    }
}

impl FromStr for Action {
    type Err = String;

    fn from_str(s: &str) -> Result<Action, String> {
        let mut parts = s.split_whitespace();
        let name = parts.next().ok_or("empty action")?;
        let arg = parts.next();
        if parts.next().is_some() {
            return Err(format!("too many arguments for {name}"));
        }
        let no_arg = |a: Action| -> Result<Action, String> {
            if arg.is_some() { Err(format!("{name} takes no argument")) } else { Ok(a) }
        };
        let amount = || -> Result<u16, String> {
            match arg {
                None => Ok(1),
                Some(_) => number(arg, name),
            }
        };
        match name {
            "new-window" => no_arg(Action::NewWindow),
            "kill-window" => no_arg(Action::KillWindow),
            "next-window" => no_arg(Action::NextWindow),
            "prev-window" => no_arg(Action::PrevWindow),
            "recent-window" => no_arg(Action::RecentWindow(true)),
            "recent-window-reverse" => no_arg(Action::RecentWindow(false)),
            "select-window" => {
                let n: usize = number(arg, name)?;
                if n == 0 { Err("select-window is 1-based".into()) } else { Ok(Action::SelectWindow(n)) }
            }
            "rename-window" => no_arg(Action::RenameWindow),
            "new-group" => no_arg(Action::NewGroup),
            "kill-session" => no_arg(Action::KillSession),
            "command-prompt" => no_arg(Action::CommandPrompt),
            "help" => no_arg(Action::Help),
            "split-vertical" => no_arg(Action::SplitVertical),
            "split-horizontal" => no_arg(Action::SplitHorizontal),
            "focus-left" => no_arg(Action::Focus(Dir::Left)),
            "focus-right" => no_arg(Action::Focus(Dir::Right)),
            "focus-up" => no_arg(Action::Focus(Dir::Up)),
            "focus-down" => no_arg(Action::Focus(Dir::Down)),
            "focus-next" => no_arg(Action::FocusNext),
            "resize-left" => Ok(Action::Resize(Dir::Left, amount()?)),
            "resize-right" => Ok(Action::Resize(Dir::Right, amount()?)),
            "resize-up" => Ok(Action::Resize(Dir::Up, amount()?)),
            "resize-down" => Ok(Action::Resize(Dir::Down, amount()?)),
            "kill-pane" => no_arg(Action::KillPane),
            "zoom-pane" => no_arg(Action::ZoomPane),
            "new-session" => no_arg(Action::NewSession),
            "next-session" => no_arg(Action::NextSession),
            "prev-session" => no_arg(Action::PrevSession),
            "select-session" => {
                let n: usize = number(arg, name)?;
                if n == 0 { Err("select-session is 1-based".into()) } else { Ok(Action::SelectSession(n)) }
            }
            "rename-session" => no_arg(Action::RenameSession),
            "toggle-sidebar" => no_arg(Action::ToggleSidebar),
            "toggle-sidebar-mode" => no_arg(Action::ToggleSidebarMode),
            "detach" => no_arg(Action::Detach),
            "send-prefix" => no_arg(Action::SendPrefix),
            "reload-config" => no_arg(Action::ReloadConfig),
            "copy-mode" => no_arg(Action::CopyMode),
            "paste-buffer" => no_arg(Action::PasteBuffer),
            "copy-up" => no_arg(Action::Copy(CopyCmd::Up)),
            "copy-down" => no_arg(Action::Copy(CopyCmd::Down)),
            "copy-left" => no_arg(Action::Copy(CopyCmd::Left)),
            "copy-right" => no_arg(Action::Copy(CopyCmd::Right)),
            "copy-page-up" => no_arg(Action::Copy(CopyCmd::PageUp)),
            "copy-page-down" => no_arg(Action::Copy(CopyCmd::PageDown)),
            "copy-half-page-up" => no_arg(Action::Copy(CopyCmd::HalfPageUp)),
            "copy-half-page-down" => no_arg(Action::Copy(CopyCmd::HalfPageDown)),
            "copy-top" => no_arg(Action::Copy(CopyCmd::Top)),
            "copy-bottom" => no_arg(Action::Copy(CopyCmd::Bottom)),
            "copy-line-start" => no_arg(Action::Copy(CopyCmd::LineStart)),
            "copy-line-end" => no_arg(Action::Copy(CopyCmd::LineEnd)),
            "copy-select" => no_arg(Action::Copy(CopyCmd::Select)),
            "copy-select-line" => no_arg(Action::Copy(CopyCmd::SelectLine)),
            "copy-select-rect" => no_arg(Action::Copy(CopyCmd::SelectRect)),
            "copy-search-forward" => no_arg(Action::Copy(CopyCmd::SearchForward)),
            "copy-search-backward" => no_arg(Action::Copy(CopyCmd::SearchBackward)),
            "copy-search-next" => no_arg(Action::Copy(CopyCmd::SearchNext)),
            "copy-search-prev" => no_arg(Action::Copy(CopyCmd::SearchPrev)),
            "copy-word-next" => no_arg(Action::Copy(CopyCmd::WordNext)),
            "copy-word-prev" => no_arg(Action::Copy(CopyCmd::WordPrev)),
            "copy-word-end" => no_arg(Action::Copy(CopyCmd::WordEnd)),
            "copy-view-top" => no_arg(Action::Copy(CopyCmd::ViewTop)),
            "copy-view-middle" => no_arg(Action::Copy(CopyCmd::ViewMiddle)),
            "copy-view-bottom" => no_arg(Action::Copy(CopyCmd::ViewBottom)),
            "focus-sidebar" => no_arg(Action::FocusSidebar),
            "focus-last" => no_arg(Action::FocusLast),
            "navigate-left" => no_arg(Action::Navigate(Dir::Left)),
            "navigate-right" => no_arg(Action::Navigate(Dir::Right)),
            "navigate-up" => no_arg(Action::Navigate(Dir::Up)),
            "navigate-down" => no_arg(Action::Navigate(Dir::Down)),
            "navigate-last" => no_arg(Action::NavigateLast),
            "send-key" => no_arg(Action::SendKey),
            "respawn-pane" => no_arg(Action::RespawnPane),
            "respawn-window" => no_arg(Action::RespawnWindow),
            "export-project" => no_arg(Action::ExportProject),
            "search-windows" => no_arg(Action::SearchWindows),
            "toggle-maximize" => no_arg(Action::ToggleMaximize),
            "toggle-popup" => no_arg(Action::TogglePopup),
            "toggle-dock" => no_arg(Action::ToggleDock),
            "focus-dock" => no_arg(Action::FocusDock),
            "dock-next" => no_arg(Action::DockNext),
            "dock-prev" => no_arg(Action::DockPrev),
            "dock-add" => no_arg(Action::DockAdd),
            "dock-close" => no_arg(Action::DockClose),
            "dock-rename" => no_arg(Action::DockRename),
            "sidebar-export" => no_arg(Action::Sidebar(SidebarCmd::Export)),
            "sidebar-search" => no_arg(Action::Sidebar(SidebarCmd::Search)),
            "sidebar-rename-group" => no_arg(Action::Sidebar(SidebarCmd::RenameGroup)),
            "sidebar-group-tag" => no_arg(Action::Sidebar(SidebarCmd::GroupTag)),
            "sidebar-respawn" => no_arg(Action::Sidebar(SidebarCmd::Respawn)),
            "sidebar-down" => no_arg(Action::Sidebar(SidebarCmd::Down)),
            "sidebar-up" => no_arg(Action::Sidebar(SidebarCmd::Up)),
            "sidebar-open" => no_arg(Action::Sidebar(SidebarCmd::Open)),
            "sidebar-kill" => no_arg(Action::Sidebar(SidebarCmd::Kill)),
            "sidebar-rename" => no_arg(Action::Sidebar(SidebarCmd::Rename)),
            "sidebar-move-down" => no_arg(Action::Sidebar(SidebarCmd::MoveDown)),
            "sidebar-move-up" => no_arg(Action::Sidebar(SidebarCmd::MoveUp)),
            "sidebar-new" => no_arg(Action::Sidebar(SidebarCmd::New)),
            "sidebar-leave" => no_arg(Action::Sidebar(SidebarCmd::Leave)),
            "copy-yank" => no_arg(Action::Copy(CopyCmd::Yank)),
            "copy-cancel" => no_arg(Action::Copy(CopyCmd::Cancel)),
            _ => Err(format!("unknown action {name:?}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_actions() {
        assert_eq!("new-window".parse(), Ok(Action::NewWindow));
        assert_eq!("select-window 3".parse(), Ok(Action::SelectWindow(3)));
        assert_eq!("resize-left 5".parse(), Ok(Action::Resize(Dir::Left, 5)));
        assert_eq!("resize-down".parse(), Ok(Action::Resize(Dir::Down, 1)));
        assert_eq!("  focus-up ".parse(), Ok(Action::Focus(Dir::Up)));
        assert_eq!("copy-mode".parse(), Ok(Action::CopyMode));
        assert_eq!("copy-half-page-up".parse(), Ok(Action::Copy(CopyCmd::HalfPageUp)));
        assert_eq!("paste-buffer".parse(), Ok(Action::PasteBuffer));
        assert_eq!("copy-select-line".parse(), Ok(Action::Copy(CopyCmd::SelectLine)));
        assert_eq!("copy-select-rect".parse(), Ok(Action::Copy(CopyCmd::SelectRect)));
        assert_eq!("copy-search-forward".parse(), Ok(Action::Copy(CopyCmd::SearchForward)));
        assert_eq!("copy-word-end".parse(), Ok(Action::Copy(CopyCmd::WordEnd)));
        assert_eq!("copy-view-middle".parse(), Ok(Action::Copy(CopyCmd::ViewMiddle)));
        assert_eq!("focus-sidebar".parse(), Ok(Action::FocusSidebar));
        assert_eq!("navigate-left".parse(), Ok(Action::Navigate(Dir::Left)));
        assert_eq!("navigate-down".parse(), Ok(Action::Navigate(Dir::Down)));
        assert_eq!("navigate-last".parse(), Ok(Action::NavigateLast));
        assert_eq!("focus-last".parse(), Ok(Action::FocusLast));
        assert_eq!("send-key".parse(), Ok(Action::SendKey));
        assert_eq!("respawn-pane".parse(), Ok(Action::RespawnPane));
        assert_eq!("respawn-window".parse(), Ok(Action::RespawnWindow));
        assert_eq!("export-project".parse(), Ok(Action::ExportProject));
        assert_eq!("search-windows".parse(), Ok(Action::SearchWindows));
        assert_eq!("toggle-popup".parse(), Ok(Action::TogglePopup));
        assert_eq!("dock-add".parse(), Ok(Action::DockAdd));
        assert_eq!("dock-prev".parse(), Ok(Action::DockPrev));
        assert_eq!("sidebar-export".parse(), Ok(Action::Sidebar(SidebarCmd::Export)));
        assert_eq!("sidebar-search".parse(), Ok(Action::Sidebar(SidebarCmd::Search)));
        assert_eq!("sidebar-respawn".parse(), Ok(Action::Sidebar(SidebarCmd::Respawn)));
        assert_eq!("sidebar-move-up".parse(), Ok(Action::Sidebar(SidebarCmd::MoveUp)));
        assert_eq!("new-group".parse(), Ok(Action::NewGroup));
        assert_eq!("sidebar-group-tag".parse(), Ok(Action::Sidebar(SidebarCmd::GroupTag)));
        assert_eq!("sidebar-rename-group".parse(), Ok(Action::Sidebar(SidebarCmd::RenameGroup)));
    }

    #[test]
    fn rejects_bad_actions() {
        assert!("".parse::<Action>().is_err());
        assert!("bogus".parse::<Action>().is_err());
        assert!("select-window".parse::<Action>().is_err());
        assert!("select-window 0".parse::<Action>().is_err());
        assert!("select-window x".parse::<Action>().is_err());
        assert!("detach now".parse::<Action>().is_err());
        assert!("resize-left 1 2".parse::<Action>().is_err());
    }

    /// The names `from_str` matches, read from this file, so the list cannot fall behind.
    fn names_in_source() -> Vec<String> {
        let src = include_str!("action.rs");
        let body = &src[src.find("fn from_str(s: &str)").unwrap()..src.find("#[cfg(test)]").unwrap()];
        body.lines()
            .filter_map(|l| {
                let l = l.trim_start();
                let rest = l.strip_prefix('"')?;
                let (name, after) = rest.split_once('"')?;
                after.trim_start().starts_with("=>").then(|| name.to_string())
            })
            .collect()
    }

    #[test]
    fn action_names_lists_every_action_once() {
        let mut in_source = names_in_source();
        in_source.sort();
        let mut listed: Vec<String> = ACTION_NAMES.iter().map(|s| s.to_string()).collect();
        let sorted = listed.clone();
        listed.sort();
        assert_eq!(listed, sorted, "ACTION_NAMES is sorted");
        listed.dedup();
        assert_eq!(listed, in_source);
    }

    #[test]
    fn every_action_displays_as_text_that_parses_back() {
        for name in ACTION_NAMES {
            for line in [name.to_string(), format!("{name} 2")] {
                if let Ok(a) = line.parse::<Action>() {
                    assert_eq!(a.to_string().parse::<Action>(), Ok(a.clone()), "{line} -> {a}");
                }
            }
        }
        assert_eq!(Action::Resize(Dir::Left, 5).to_string(), "resize-left 5");
        assert_eq!(Action::SelectWindow(3).to_string(), "select-window 3");
        assert_eq!(Action::Copy(CopyCmd::Yank).to_string(), "copy-yank");
        assert_eq!(Action::Sidebar(SidebarCmd::MoveUp).to_string(), "sidebar-move-up");
        assert_eq!(Action::RecentWindow(false).to_string(), "recent-window-reverse");
        assert_eq!(Action::Help.to_string(), "help");
    }
}

