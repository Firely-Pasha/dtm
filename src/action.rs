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
    Leave,
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
    /// 1-based, as shown in the sidebar.
    SelectWindow(usize),
    RenameWindow,
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
}

fn number<T: FromStr>(arg: Option<&str>, name: &str) -> Result<T, String> {
    arg.ok_or_else(|| format!("{name} needs a number"))?
        .parse()
        .map_err(|_| format!("{name}: invalid number"))
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
            "select-window" => {
                let n: usize = number(arg, name)?;
                if n == 0 { Err("select-window is 1-based".into()) } else { Ok(Action::SelectWindow(n)) }
            }
            "rename-window" => no_arg(Action::RenameWindow),
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
            "focus-sidebar" => no_arg(Action::FocusSidebar),
            "focus-last" => no_arg(Action::FocusLast),
            "navigate-left" => no_arg(Action::Navigate(Dir::Left)),
            "navigate-right" => no_arg(Action::Navigate(Dir::Right)),
            "navigate-up" => no_arg(Action::Navigate(Dir::Up)),
            "navigate-down" => no_arg(Action::Navigate(Dir::Down)),
            "navigate-last" => no_arg(Action::NavigateLast),
            "send-key" => no_arg(Action::SendKey),
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
        assert_eq!("focus-sidebar".parse(), Ok(Action::FocusSidebar));
        assert_eq!("navigate-left".parse(), Ok(Action::Navigate(Dir::Left)));
        assert_eq!("navigate-down".parse(), Ok(Action::Navigate(Dir::Down)));
        assert_eq!("navigate-last".parse(), Ok(Action::NavigateLast));
        assert_eq!("focus-last".parse(), Ok(Action::FocusLast));
        assert_eq!("send-key".parse(), Ok(Action::SendKey));
        assert_eq!("sidebar-move-up".parse(), Ok(Action::Sidebar(SidebarCmd::MoveUp)));
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
}
