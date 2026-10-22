//! Session, window and pane operations, and the dispatcher for key actions.

use std::path::PathBuf;

use super::layout::{neighbor, Axis, Node, PaneId, Rect};
use super::state::{ClientId, Pane, Prompt, PromptKind, Session, SessionId, Window};
use super::{Event, Server};
use crate::action::{Action, Dir};
use crate::config::SidebarMode;
use crate::protocol::ServerMsg;
use crate::pty::{Pty, SpawnOpts};
use crate::term::screen::Screen;

impl Server {
    // ---- creation ----

    pub fn spawn_pane(&mut self, id: PaneId, cwd: Option<PathBuf>, size: (u16, u16)) -> Result<(), String> {
        let (cols, rows) = (size.0.max(1), size.1.max(1));
        let tx = self.events.clone();
        let pty = Pty::spawn(
            SpawnOpts { shell: self.cfg.shell.clone(), args: vec![], env: vec![], cwd, cols, rows },
            move |ev| {
                let _ = tx.send(Event::Pane { id, ev });
            },
        )
        .map_err(|e| format!("{e:#}"))?;
        let screen = Screen::new(cols as usize, rows as usize, self.cfg.history_limit);
        self.panes.insert(id, Pane { id, screen, pty, last_title: String::new() });
        Ok(())
    }

    fn make_window(&mut self, cwd: Option<PathBuf>, size: (u16, u16)) -> Result<Window, String> {
        let pane = self.alloc_pane_id();
        self.spawn_pane(pane, cwd, size)?;
        Ok(Window {
            id: self.alloc_window_id(),
            pinned_name: None,
            auto_name: String::new(),
            root: Node::Leaf(pane),
            active: pane,
            zoomed: false,
            activity: false,
            bell: false,
        })
    }

    pub fn create_session(
        &mut self,
        name: Option<String>,
        cwd: Option<PathBuf>,
        size: (u16, u16),
    ) -> Result<SessionId, String> {
        let name = match name {
            Some(n) if n.trim().is_empty() => return Err("session name is empty".into()),
            Some(n) => n,
            None => (0..).map(|i| i.to_string()).find(|n| !self.sessions.iter().any(|s| &s.name == n)).unwrap(),
        };
        let window = self.make_window(cwd.clone(), size)?;
        let id = self.alloc_session_id();
        self.sessions.push(Session {
            id,
            name,
            windows: vec![window],
            active: 0,
            size,
            owner: None,
            cwd: cwd.unwrap_or_default(),
        });
        self.had_session = true;
        let si = self.sessions.len() - 1;
        self.refresh_name(si, 0);
        Ok(id)
    }

    fn session_index(&self, sid: SessionId) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == sid)
    }

    fn active_cwd(&self, sid: SessionId) -> Option<PathBuf> {
        let s = self.sessions.iter().find(|s| s.id == sid)?;
        let from_pane = self.panes.get(&s.active_window().active).and_then(|p| p.pty.cwd());
        from_pane.or_else(|| Some(s.cwd.clone()).filter(|p| !p.as_os_str().is_empty()))
    }

    pub fn new_window(&mut self, sid: SessionId) -> Result<(), String> {
        let cwd = self.active_cwd(sid);
        let size = self.sessions.iter().find(|s| s.id == sid).map(|s| s.size).ok_or("no session")?;
        let window = self.make_window(cwd, size)?;
        let si = self.session_index(sid).ok_or("no session")?;
        let s = &mut self.sessions[si];
        s.windows.push(window);
        let wi = s.windows.len() - 1;
        self.refresh_name(si, wi);
        self.select_window(sid, wi);
        Ok(())
    }

    pub fn split(&mut self, sid: SessionId, axis: Axis) -> Result<(), String> {
        let cwd = self.active_cwd(sid);
        let si = self.session_index(sid).ok_or("no session")?;
        let (active, size, mut tree) = {
            let s = &self.sessions[si];
            let w = s.active_window();
            (w.active, s.size, w.root.clone())
        };
        let new_id = self.alloc_pane_id();
        tree.split(active, axis, new_id);
        let mut rects = Vec::new();
        tree.layout(Rect::new(0, 0, size.0, size.1), &mut rects);
        let r = rects.iter().find(|(id, _)| *id == new_id).map(|(_, r)| *r).ok_or("split failed")?;
        if r.w == 0 || r.h == 0 {
            return Err("pane too small to split".into());
        }
        self.spawn_pane(new_id, cwd, (r.w, r.h))?;
        let w = self.sessions[si].active_window_mut();
        w.root = tree;
        w.active = new_id;
        w.zoomed = false;
        let wi = self.sessions[si].active;
        self.refresh_name(si, wi);
        self.relayout(sid);
        Ok(())
    }

    // ---- selection ----

    pub fn select_window(&mut self, sid: SessionId, wi: usize) {
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) else { return };
        if wi >= s.windows.len() {
            return;
        }
        s.active = wi;
        let w = &mut s.windows[wi];
        w.activity = false;
        w.bell = false;
    }

    fn step_window(&mut self, sid: SessionId, forward: bool) {
        let Some(s) = self.sessions.iter().find(|s| s.id == sid) else { return };
        let n = s.windows.len();
        let wi = if forward { (s.active + 1) % n } else { (s.active + n - 1) % n };
        self.select_window(sid, wi);
    }

    // ---- teardown ----

    /// A pane's process exited (or it was killed): remove it and anything left empty.
    pub fn close_pane(&mut self, pid: PaneId) {
        if self.panes.remove(&pid).is_none() {
            return;
        }
        let Some((si, wi)) = self.locate_pane(pid) else { return };
        let sid = self.sessions[si].id;
        let w = &mut self.sessions[si].windows[wi];
        let root = std::mem::replace(&mut w.root, Node::Leaf(0));
        match root.remove(pid) {
            (Some(root), focus) => {
                w.root = root;
                if w.active == pid {
                    w.active = focus.unwrap_or_else(|| w.root.first_leaf());
                }
                if w.zoomed && w.root.leaves().len() < 2 {
                    w.zoomed = false;
                }
                if w.zoomed && !w.root.contains(w.active) {
                    w.zoomed = false;
                }
                self.refresh_name(si, wi);
                self.relayout(sid);
            }
            (None, _) => {
                let s = &mut self.sessions[si];
                s.windows.remove(wi);
                if s.windows.is_empty() {
                    self.remove_session(sid);
                } else {
                    if wi < s.active {
                        s.active -= 1;
                    } else if wi == s.active {
                        s.active = wi.min(s.windows.len() - 1);
                        let w = &mut s.windows[s.active];
                        w.activity = false;
                        w.bell = false;
                    }
                    s.active = s.active.min(s.windows.len() - 1);
                }
            }
        }
        self.mark_dirty();
    }

    fn locate_pane(&self, pane: PaneId) -> Option<(usize, usize)> {
        self.sessions.iter().enumerate().find_map(|(si, s)| {
            s.windows.iter().position(|w| w.root.contains(pane)).map(|wi| (si, wi))
        })
    }

    fn remove_session(&mut self, sid: SessionId) {
        let Some(idx) = self.session_index(sid) else { return };
        let removed = self.sessions.remove(idx);
        // Any panes still alive are dropped (and hung up).
        for w in &removed.windows {
            for p in w.root.leaves() {
                self.panes.remove(&p);
            }
        }
        let attached: Vec<ClientId> =
            self.clients.values().filter(|c| c.session == Some(sid)).map(|c| c.id).collect();
        if self.sessions.is_empty() {
            for cid in attached {
                self.send(cid, ServerMsg::Exited);
            }
            return;
        }
        let fallback = self.sessions[idx.min(self.sessions.len() - 1)].id;
        for cid in attached {
            if let Some(c) = self.clients.get_mut(&cid) {
                c.session = None;
            }
            self.switch_client(cid, fallback);
        }
    }

    // ---- key actions ----

    pub fn run_action(&mut self, cid: ClientId, action: Action, prefix_raw: Option<Vec<u8>>) {
        let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else { return };
        let Some(si) = self.session_index(sid) else { return };
        let result: Result<(), String> = match action {
            Action::NewWindow => self.new_window(sid),
            Action::KillWindow => {
                let leaves = self.sessions[si].active_window().root.leaves();
                for p in leaves {
                    self.close_pane(p);
                }
                Ok(())
            }
            Action::NextWindow => {
                self.step_window(sid, true);
                Ok(())
            },
            Action::PrevWindow => {
                self.step_window(sid, false);
                Ok(())
            },
            Action::SelectWindow(n) => {
                self.select_window(sid, n - 1);
                Ok(())
            },
            Action::RenameWindow => {
                let text = self.sessions[si].active_window().name().to_string();
                self.set_prompt(cid, PromptKind::RenameWindow, text);
                Ok(())
            }
            Action::RenameSession => {
                let text = self.sessions[si].name.clone();
                self.set_prompt(cid, PromptKind::RenameSession, text);
                Ok(())
            }
            Action::SplitVertical => self.split(sid, Axis::Cols),
            Action::SplitHorizontal => self.split(sid, Axis::Rows),
            Action::Focus(dir) => {
                self.focus_dir(si, dir);
                Ok(())
            },
            Action::FocusNext => {
                self.focus_next(si);
                Ok(())
            },
            Action::Resize(dir, n) => {
                self.resize_pane(si, dir, n);
                Ok(())
            },
            Action::KillPane => {
                let p = self.sessions[si].active_window().active;
                self.close_pane(p);
                Ok(())
            }
            Action::ZoomPane => {
                let w = self.sessions[si].active_window_mut();
                if w.zoomed || w.root.leaves().len() > 1 {
                    w.zoomed = !w.zoomed;
                }
                self.relayout(sid);
                Ok(())
            }
            Action::NewSession => {
                let cwd = self.active_cwd(sid);
                let size = self.content_size(cid);
                self.create_session(None, cwd, size).map(|new| self.switch_client(cid, new))
            }
            Action::NextSession => {
                self.step_session(cid, si, true);
                Ok(())
            },
            Action::PrevSession => {
                self.step_session(cid, si, false);
                Ok(())
            },
            Action::SelectSession(n) => {
                if let Some(s) = self.sessions.get(n - 1) {
                    let id = s.id;
                    self.switch_client(cid, id);
                }
                Ok(())
            }
            Action::ToggleSidebar => {
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.sidebar_visible = !c.sidebar_visible;
                }
                self.update_session_size(sid);
                Ok(())
            }
            Action::ToggleSidebarMode => {
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.sidebar_mode = match c.sidebar_mode {
                        SidebarMode::Windows => SidebarMode::Sessions,
                        SidebarMode::Sessions => SidebarMode::Windows,
                    };
                    c.sidebar_visible = true;
                }
                self.update_session_size(sid);
                Ok(())
            }
            Action::Detach => {
                self.detach(cid);
                return;
            }
            Action::SendPrefix => {
                if let (Some(raw), Some(p)) = (prefix_raw, self.panes.get(&self.sessions[si].active_window().active)) {
                    p.pty.write(raw);
                }
                Ok(())
            }
            Action::ReloadConfig => match self.reload() {
                Ok(()) => {
                    self.message(cid, "config reloaded");
                    Ok(())
                }
                Err(e) => Err(format!("config error: {e}")),
            },
        };
        if let Err(e) = result {
            self.message(cid, e);
        }
        self.mark_dirty();
    }

    fn set_prompt(&mut self, cid: ClientId, kind: PromptKind, text: String) {
        if let Some(c) = self.clients.get_mut(&cid) {
            c.prompt = Some(Prompt { kind, text });
            c.sidebar_visible = true;
        }
    }

    fn step_session(&mut self, cid: ClientId, si: usize, forward: bool) {
        let n = self.sessions.len();
        if n < 2 {
            return;
        }
        let next = if forward { (si + 1) % n } else { (si + n - 1) % n };
        let id = self.sessions[next].id;
        self.switch_client(cid, id);
    }

    fn unzoom(&mut self, si: usize) {
        let sid = self.sessions[si].id;
        let w = self.sessions[si].active_window_mut();
        if w.zoomed {
            w.zoomed = false;
            self.relayout(sid);
        }
    }

    fn focus_dir(&mut self, si: usize, dir: Dir) {
        self.unzoom(si);
        let s = &mut self.sessions[si];
        let size = s.size;
        let w = s.active_window_mut();
        let rects = w.rects(size);
        if let Some(n) = neighbor(&rects, w.active, dir) {
            w.active = n;
        }
        let wi = self.sessions[si].active;
        self.refresh_name(si, wi);
    }

    fn focus_next(&mut self, si: usize) {
        self.unzoom(si);
        let w = self.sessions[si].active_window_mut();
        let leaves = w.root.leaves();
        if let Some(i) = leaves.iter().position(|&p| p == w.active) {
            w.active = leaves[(i + 1) % leaves.len()];
        }
        let wi = self.sessions[si].active;
        self.refresh_name(si, wi);
    }

    fn resize_pane(&mut self, si: usize, dir: Dir, n: u16) {
        let sid = self.sessions[si].id;
        let size = self.sessions[si].size;
        let w = self.sessions[si].active_window_mut();
        if w.zoomed {
            return;
        }
        let active = w.active;
        if w.root.resize(active, dir, n, Rect::new(0, 0, size.0, size.1)) {
            self.relayout(sid);
        }
    }

    /// Apply the result of a rename prompt.
    pub fn commit_prompt(&mut self, cid: ClientId, kind: PromptKind, text: String) {
        let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else { return };
        let Some(si) = self.session_index(sid) else { return };
        let text = text.trim().to_string();
        match kind {
            PromptKind::RenameWindow => {
                let w = self.sessions[si].active_window_mut();
                w.pinned_name = (!text.is_empty()).then_some(text);
            }
            PromptKind::RenameSession => {
                if text.is_empty() {
                    return;
                }
                if self.sessions.iter().any(|s| s.name == text && s.id != sid) {
                    self.message(cid, format!("duplicate session: {text}"));
                } else {
                    self.sessions[si].name = text;
                }
            }
        }
        self.mark_dirty();
    }
}
