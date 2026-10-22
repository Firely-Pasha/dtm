//! Session, window and pane operations, and the dispatcher for key actions.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::copy::{base64, CopyMode, Outcome};
use super::layout::{neighbor, Axis, Node, PaneId, Rect};
use super::state::{ClientId, Pane, Prompt, PromptKind, Session, SessionId, Window};
use super::{Event, Server};
use crate::action::{Action, CopyCmd, Dir, SidebarCmd};
use crate::config::{Position, SidebarMode};
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
        self.panes.insert(id, Pane { id, screen, pty, last_title: String::new(), copy: None });
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
                let wi = self.sessions[si].active;
                self.kill_window(sid, wi);
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
                let w = self.sessions[si].active_window();
                let (id, text) = (w.id, w.name().to_string());
                self.set_prompt(cid, PromptKind::RenameWindow(id), text, "rename window");
                Ok(())
            }
            Action::RenameSession => {
                let text = self.sessions[si].name.clone();
                self.set_prompt(cid, PromptKind::RenameSession(sid), text, "rename session");
                Ok(())
            }
            Action::SplitVertical => self.split(sid, Axis::Cols),
            Action::SplitHorizontal => self.split(sid, Axis::Rows),
            Action::Focus(dir) => {
                self.focus_command(cid, si, dir);
                Ok(())
            },
            Action::FocusNext => {
                self.focus_next(si);
                Ok(())
            },
            Action::Resize(dir, n) => {
                self.resize_pane(si, dir, n);
                let repeat = self.cfg.repeat_time;
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.resize_until = (repeat > 0).then(|| Instant::now() + Duration::from_millis(repeat));
                }
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
            Action::CopyMode => {
                if let Some(p) = self.panes.get_mut(&self.sessions[si].active_window().active)
                    && p.copy.is_none() {
                        p.copy = Some(CopyMode::new(&p.screen));
                    }
                Ok(())
            }
            Action::Copy(cmd) => {
                self.copy_command(cid, si, cmd);
                Ok(())
            }
            Action::FocusSidebar => self.enter_sidebar(cid),
            Action::Sidebar(cmd) => self.sidebar_command(cid, cmd),
            Action::PasteBuffer => {
                self.paste_buffer(si);
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

    fn copy_command(&mut self, cid: ClientId, si: usize, cmd: CopyCmd) {
        let active = self.sessions[si].active_window().active;
        let Some(p) = self.panes.get_mut(&active) else { return };
        let Some(copy) = p.copy.as_mut() else { return };
        match copy.apply(cmd, &p.screen) {
            Outcome::Stay => {}
            Outcome::Exit => p.copy = None,
            Outcome::Yanked(text) => {
                p.copy = None;
                let note = format!("copied {} characters", text.chars().count());
                if self.cfg.clipboard && !text.is_empty() {
                    let osc = format!("\x1b]52;c;{}\x07", base64(text.as_bytes()));
                    self.send(cid, ServerMsg::Output(osc.into_bytes()));
                }
                self.paste_text = text;
                self.message(cid, note);
            }
        }
    }

    fn paste_buffer(&mut self, si: usize) {
        if self.paste_text.is_empty() {
            return;
        }
        let active = self.sessions[si].active_window().active;
        let Some(p) = self.panes.get_mut(&active) else { return };
        p.copy = None;
        let mut data = Vec::new();
        let bracketed = p.screen.modes().bracketed_paste;
        if bracketed {
            data.extend_from_slice(b"\x1b[200~");
        }
        data.extend_from_slice(self.paste_text.replace('\n', "\r").as_bytes());
        if bracketed {
            data.extend_from_slice(b"\x1b[201~");
        }
        p.pty.write(data);
    }

    fn set_prompt(&mut self, cid: ClientId, kind: PromptKind, text: String, label: impl Into<String>) {
        if let Some(c) = self.clients.get_mut(&cid) {
            c.prompt = Some(Prompt { kind, text, label: label.into() });
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

    /// Move pane focus; false when there is no pane in that direction.
    fn focus_dir(&mut self, si: usize, dir: Dir) -> bool {
        self.unzoom(si);
        let s = &mut self.sessions[si];
        let size = s.size;
        let w = s.active_window_mut();
        let rects = w.rects(size);
        let moved = match neighbor(&rects, w.active, dir) {
            Some(n) => {
                w.active = n;
                true
            }
            None => false,
        };
        let wi = self.sessions[si].active;
        self.refresh_name(si, wi);
        moved
    }

    /// Directional focus: panes first, then on into the sidebar; and back out of it.
    fn focus_command(&mut self, cid: ClientId, si: usize, dir: Dir) {
        let toward_sidebar = matches!(
            (self.cfg.sidebar.position, dir),
            (Position::Left, Dir::Left) | (Position::Right, Dir::Right)
        );
        if self.clients.get(&cid).is_some_and(|c| c.sidebar_focus.is_some()) {
            let cmd = match dir {
                Dir::Up => Some(SidebarCmd::Up),
                Dir::Down => Some(SidebarCmd::Down),
                _ if !toward_sidebar => Some(SidebarCmd::Leave),
                _ => None,
            };
            if let Some(cmd) = cmd {
                let _ = self.sidebar_command(cid, cmd);
            }
            return;
        }
        if !self.focus_dir(si, dir) && toward_sidebar {
            let _ = self.enter_sidebar(cid);
        }
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

    /// Apply the result of a prompt (rename text, or a confirmed kill).
    pub fn commit_prompt(&mut self, cid: ClientId, kind: PromptKind, text: String) {
        let text = text.trim().to_string();
        match kind {
            PromptKind::RenameWindow(id) => {
                if let Some(w) = self.sessions.iter_mut().flat_map(|s| s.windows.iter_mut()).find(|w| w.id == id) {
                    w.pinned_name = (!text.is_empty()).then_some(text);
                }
            }
            PromptKind::RenameSession(id) => {
                if text.is_empty() {
                    return;
                }
                if self.sessions.iter().any(|s| s.name == text && s.id != id) {
                    self.message(cid, format!("duplicate session: {text}"));
                } else if let Some(s) = self.sessions.iter_mut().find(|s| s.id == id) {
                    s.name = text;
                }
            }
            PromptKind::ConfirmKillWindow(id) => {
                let found = self.sessions.iter().find_map(|s| {
                    s.windows.iter().position(|w| w.id == id).map(|wi| (s.id, wi))
                });
                if let Some((sid, wi)) = found {
                    self.kill_window(sid, wi);
                }
            }
            PromptKind::ConfirmKillSession(id) => self.kill_session(id),
        }
        self.mark_dirty();
    }

    // ---- killing and reordering ----

    pub fn kill_window(&mut self, sid: SessionId, wi: usize) {
        let Some(s) = self.sessions.iter().find(|s| s.id == sid) else { return };
        let Some(w) = s.windows.get(wi) else { return };
        for p in w.root.leaves() {
            self.close_pane(p);
        }
    }

    pub fn kill_session(&mut self, sid: SessionId) {
        let Some(s) = self.sessions.iter().find(|s| s.id == sid) else { return };
        let panes: Vec<PaneId> = s.windows.iter().flat_map(|w| w.root.leaves()).collect();
        for p in panes {
            self.close_pane(p);
        }
    }

    fn move_window(&mut self, sid: SessionId, from: usize, to: usize) {
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) else { return };
        if from >= s.windows.len() || to >= s.windows.len() {
            return;
        }
        s.windows.swap(from, to);
        if s.active == from {
            s.active = to;
        } else if s.active == to {
            s.active = from;
        }
    }

    // ---- sidebar focus ----

    fn set_sidebar_focus(&mut self, cid: ClientId, sel: Option<usize>) {
        if let Some(c) = self.clients.get_mut(&cid) {
            c.sidebar_focus = sel;
        }
    }

    /// Give the sidebar keyboard focus, with the current window/session selected.
    pub fn enter_sidebar(&mut self, cid: ClientId) -> Result<(), String> {
        let Some(c) = self.clients.get_mut(&cid) else { return Ok(()) };
        c.sidebar_visible = true;
        let c = &self.clients[&cid];
        if self.sidebar_width(c) == 0 {
            return Err("terminal too narrow for the sidebar".into());
        }
        let Some(si) = c.session.and_then(|sid| self.session_index(sid)) else { return Ok(()) };
        let sel = match c.sidebar_mode {
            SidebarMode::Windows => self.sessions[si].active,
            SidebarMode::Sessions => si,
        };
        let sid = self.sessions[si].id;
        self.update_session_size(sid);
        self.set_sidebar_focus(cid, Some(sel));
        Ok(())
    }

    fn sidebar_command(&mut self, cid: ClientId, cmd: SidebarCmd) -> Result<(), String> {
        let Some(c) = self.clients.get(&cid) else { return Ok(()) };
        let (Some(sel), Some(sid)) = (c.sidebar_focus, c.session) else { return Ok(()) };
        let mode = c.sidebar_mode;
        let si = self.session_index(sid).ok_or("no session")?;
        let count = match mode {
            SidebarMode::Windows => self.sessions[si].windows.len(),
            SidebarMode::Sessions => self.sessions.len(),
        };
        let sel = sel.min(count.saturating_sub(1));
        match cmd {
            SidebarCmd::Down => self.set_sidebar_focus(cid, Some((sel + 1).min(count - 1))),
            SidebarCmd::Up => self.set_sidebar_focus(cid, Some(sel.saturating_sub(1))),
            SidebarCmd::Leave => self.set_sidebar_focus(cid, None),
            SidebarCmd::Open => {
                self.set_sidebar_focus(cid, None);
                match mode {
                    SidebarMode::Windows => self.select_window(sid, sel),
                    SidebarMode::Sessions => {
                        let id = self.sessions[sel].id;
                        self.switch_client(cid, id);
                    }
                }
            }
            SidebarCmd::Kill => match mode {
                SidebarMode::Windows => {
                    let w = &self.sessions[si].windows[sel];
                    let (id, label) = (w.id, format!("kill window {} ({})? (y/n)", sel + 1, w.name()));
                    self.set_prompt(cid, PromptKind::ConfirmKillWindow(id), String::new(), label);
                }
                SidebarMode::Sessions => {
                    let s = &self.sessions[sel];
                    let (id, label) = (s.id, format!("kill session {} ({})? (y/n)", sel + 1, s.name));
                    self.set_prompt(cid, PromptKind::ConfirmKillSession(id), String::new(), label);
                }
            },
            SidebarCmd::Rename => match mode {
                SidebarMode::Windows => {
                    let w = &self.sessions[si].windows[sel];
                    let (id, text) = (w.id, w.name().to_string());
                    self.set_prompt(cid, PromptKind::RenameWindow(id), text, "rename window");
                }
                SidebarMode::Sessions => {
                    let s = &self.sessions[sel];
                    let (id, text) = (s.id, s.name.clone());
                    self.set_prompt(cid, PromptKind::RenameSession(id), text, "rename session");
                }
            },
            SidebarCmd::MoveDown | SidebarCmd::MoveUp => {
                let to = if cmd == SidebarCmd::MoveDown { sel + 1 } else { sel.wrapping_sub(1) };
                if to < count {
                    match mode {
                        SidebarMode::Windows => self.move_window(sid, sel, to),
                        SidebarMode::Sessions => self.sessions.swap(sel, to),
                    }
                    self.set_sidebar_focus(cid, Some(to));
                }
            }
            SidebarCmd::New => match mode {
                SidebarMode::Windows => {
                    self.new_window(sid)?;
                    let active = self.sessions[si].active;
                    self.set_sidebar_focus(cid, Some(active));
                }
                SidebarMode::Sessions => {
                    let cwd = self.active_cwd(sid);
                    let size = self.content_size(cid);
                    let new = self.create_session(None, cwd, size)?;
                    self.switch_client(cid, new);
                    let idx = self.session_index(new).unwrap_or(0);
                    self.set_sidebar_focus(cid, Some(idx));
                }
            },
        }
        self.mark_dirty();
        Ok(())
    }
}
