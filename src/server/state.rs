use std::path::PathBuf;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use super::layout::{Node, PaneId, Rect};
use crate::config::SidebarMode;
use crate::protocol::ServerMsg;
use crate::pty::Pty;
use crate::term::canvas::Canvas;
use crate::term::screen::Screen;

pub type WindowId = u32;
pub type SessionId = u32;
pub type ClientId = u32;

pub struct Pane {
    pub id: PaneId,
    pub screen: Screen,
    pub pty: Pty,
    pub last_title: String,
}

impl Pane {
    pub fn resize(&mut self, cols: u16, rows: u16) {
        let (cols, rows) = (cols.max(1), rows.max(1));
        if self.screen.cols() != cols as usize || self.screen.rows() != rows as usize {
            self.screen.resize(cols as usize, rows as usize);
            self.pty.resize(cols, rows);
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
    pub zoomed: bool,
    pub activity: bool,
    pub bell: bool,
}

impl Window {
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
    RenameWindow,
    RenameSession,
}

pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
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
            prev: None,
            last_active: Instant::now(),
            message: None,
        }
    }

    pub fn send(&self, msg: ServerMsg) {
        let _ = self.tx.send(msg);
    }
}
