use std::path::PathBuf;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use super::layout::{Node, PaneId, Rect};
use crate::config::SidebarMode;
use crate::project::PlanNode;
use crate::protocol::ServerMsg;
use crate::pty::Pty;
use crate::term::canvas::Canvas;
use crate::term::screen::Screen;

pub type WindowId = u32;
pub type SessionId = u32;
pub type ClientId = u32;

/// How a pane was started, so it can be started again.
#[derive(Clone, Debug, Default)]
pub struct PaneSpec {
    pub cwd: Option<PathBuf>,
    /// Typed into the shell after it starts.
    pub command: Option<String>,
}

pub struct Pane {
    pub id: PaneId,
    pub spec: PaneSpec,
    pub screen: Screen,
    pub pty: Pty,
    pub last_title: String,
    pub copy: Option<super::copy::CopyMode>,
}

impl Pane {
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if self.screen.cols() != cols as usize || self.screen.rows() != rows as usize {
            self.screen.resize(cols as usize, rows as usize);
            self.pty.resize(cols, rows);
            if let Some(c) = self.copy.as_mut() {
                c.clamp(&self.screen);
            }
        }
    }
}

pub struct Window {
    pub id: WindowId,
    /// Set by rename-window; wins over the automatic name.
    pub pinned_name: Option<String>,
    pub auto_name: String,
    pub root: Node,
    pub active: PaneId,
    /// The pane that was focused before `active`.
    pub last: Option<PaneId>,
    pub zoomed: bool,
    pub activity: bool,
    pub bell: bool,
    /// The project layout this window came from (kept for respawn and export).
    pub plan: Option<PlanNode>,
    /// A project window that has no panes yet: it starts when first shown.
    pub pending: bool,
}

impl Window {
    /// Focus `pane`, remembering the previous one for `focus-last`.
    pub fn focus(&mut self, pane: PaneId) {
        if pane != self.active {
            self.last = Some(self.active);
            self.active = pane;
        }
    }

    pub fn name(&self) -> &str {
        self.pinned_name.as_deref().unwrap_or(&self.auto_name)
    }

    pub fn rects(&self, size: (u16, u16)) -> Vec<(PaneId, Rect)> {
        let full = Rect::new(0, 0, size.0, size.1);
        if self.zoomed {
            return vec![(self.active, full)];
        }
        let mut v = Vec::new();
        self.root.layout(full, &mut v);
        v
    }
}

pub struct Session {
    pub id: SessionId,
    pub name: String,
    pub windows: Vec<Window>,
    pub active: usize,
    /// Size of the pane area, set by the owning client.
    pub size: (u16, u16),
    pub owner: Option<ClientId>,
    pub cwd: PathBuf,
    /// The project file this session was opened from.
    pub project: Option<PathBuf>,
}

impl Session {
    pub fn active_window(&self) -> &Window {
        &self.windows[self.active]
    }

    pub fn active_window_mut(&mut self) -> &mut Window {
        &mut self.windows[self.active]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    RenameWindow(WindowId),
    RenameSession(SessionId),
    ConfirmKillWindow(WindowId),
    ConfirmKillSession(SessionId),
    ConfirmRespawnPane(PaneId),
    ConfirmRespawnWindow(WindowId),
}

impl PromptKind {
    /// Confirmation prompts take a single y/n key instead of text.
    pub fn is_confirm(&self) -> bool {
        matches!(
            self,
            PromptKind::ConfirmKillWindow(_)
                | PromptKind::ConfirmKillSession(_)
                | PromptKind::ConfirmRespawnPane(_)
                | PromptKind::ConfirmRespawnWindow(_)
        )
    }
}

pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    pub label: String,
}

pub struct Client {
    pub id: ClientId,
    pub tx: UnboundedSender<ServerMsg>,
    pub cols: u16,
    pub rows: u16,
    pub session: Option<SessionId>,
    pub sidebar_visible: bool,
    pub sidebar_mode: SidebarMode,
    pub key_buf: Vec<u8>,
    pub flush_scheduled: bool,
    /// Raw bytes of the prefix key while waiting for the follow-up key.
    pub prefix: Option<Vec<u8>>,
    pub prompt: Option<Prompt>,
    /// Selected row while the sidebar has keyboard focus.
    pub sidebar_focus: Option<usize>,
    /// Plain resize keys are accepted until this moment.
    pub resize_until: Option<Instant>,
    pub prev: Option<Canvas>,
    pub last_active: Instant,
    pub message: Option<(String, Instant)>,
}

impl Client {
    pub fn new(id: ClientId, tx: UnboundedSender<ServerMsg>) -> Client {
        Client {
            id,
            tx,
            cols: 80,
            rows: 24,
            session: None,
            sidebar_visible: true,
            sidebar_mode: SidebarMode::Windows,
            key_buf: Vec::new(),
            flush_scheduled: false,
            prefix: None,
            prompt: None,
            sidebar_focus: None,
            resize_until: None,
            prev: None,
            last_active: Instant::now(),
            message: None,
        }
    }

    pub fn send(&self, msg: ServerMsg) {
        let _ = self.tx.send(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Window {
        Window {
            id: 1,
            pinned_name: None,
            auto_name: String::new(),
            root: Node::Leaf(1),
            active: 1,
            last: None,
            zoomed: false,
            activity: false,
            bell: false,
            plan: None,
            pending: false,
        }
    }

    #[test]
    fn focus_remembers_the_previous_pane() {
        let mut w = window();
        w.focus(1);
        assert_eq!(w.last, None, "focusing the active pane changes nothing");
        w.focus(2);
        assert_eq!((w.active, w.last), (2, Some(1)));
        w.focus(3);
        assert_eq!((w.active, w.last), (3, Some(2)));
        w.focus(2);
        assert_eq!((w.active, w.last), (2, Some(3)));
    }
}
