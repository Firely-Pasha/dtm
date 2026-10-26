//! Session, window and pane operations, and the dispatcher for key actions.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::copy::{base64, CopyMode, Outcome};
use super::layout::{neighbor, Axis, Node, PaneId, Rect};
use super::state::{
    ClientId, Cycle, Dock, Extra, Group, GroupId, Pane, PaneSpec, Prompt, PromptKind, Session, SessionId, Window, WindowId,
    DEFAULT_GROUP,
};
use super::{Event, Server};
use crate::action::{Action, CopyCmd, Dir, SidebarCmd};
use crate::config::{Position, SidebarMode};
use crate::protocol::ServerMsg;
use crate::pty::{Pty, SpawnOpts};
use crate::term::screen::Screen;

/// The key press that fired a binding.
pub struct Trigger {
    /// Raw bytes of the key itself.
    pub key: Vec<u8>,
    /// Raw bytes of the prefix key, when the binding came from the prefix table.
    pub prefix: Option<Vec<u8>>,
}

impl Trigger {
    pub fn key(key: Vec<u8>) -> Trigger {
        Trigger { key, prefix: None }
    }
}

impl Server {
    // ---- creation ----

    /// Variables that let programs in a pane (and editor plugins) reach this server.
    fn pane_env(&self, id: PaneId) -> Vec<(String, String)> {
        vec![
            ("DTM_PANE".into(), id.to_string()),
            ("DTM_SOCKET".into(), self.socket.display().to_string()),
            ("DTM_BIN".into(), self.exe.clone()),
        ]
    }

    pub fn spawn_pane(&mut self, id: PaneId, cwd: Option<PathBuf>, size: (u16, u16)) -> Result<(), String> {
        let (cols, rows) = (size.0.max(1), size.1.max(1));
        let tx = self.events.clone();
        let pty = Pty::spawn(
            SpawnOpts { shell: self.cfg.shell.clone(), args: vec![], env: self.pane_env(id), cwd: cwd.clone(), cols, rows },
            move |ev| {
                let _ = tx.send(Event::Pane { id, ev });
            },
        )
        .map_err(|e| format!("{e:#}"))?;
        let screen = Screen::new(cols as usize, rows as usize, self.cfg.history_limit);
        let spec = PaneSpec { cwd, command: None };
        self.panes.insert(id, Pane { id, spec, screen, pty, last_title: String::new(), copy: None });
        Ok(())
    }

    pub(super) fn make_window(&mut self, cwd: Option<PathBuf>, size: (u16, u16), group: GroupId) -> Result<Window, String> {
        let pane = self.alloc_pane_id();
        self.spawn_pane(pane, cwd, size)?;
        Ok(Window {
            id: self.alloc_window_id(),
            pinned_name: None,
            auto_name: String::new(),
            root: Node::Leaf(pane),
            active: pane,
            last: None,
            zoomed: false,
            activity: false,
            bell: false,
            plan: None,
            pending: false,
            group,
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
        let tag = self.free_tag(&[], DEFAULT_GROUP);
        let window = self.make_window(cwd.clone(), size, 1)?;
        let id = self.alloc_session_id();
        self.sessions.push(Session {
            id,
            name,
            windows: vec![window],
            groups: vec![Group { id: 1, name: DEFAULT_GROUP.into(), tag }],
            active: 0,
            size,
            owner: None,
            cwd: cwd.unwrap_or_default(),
            project: None,
            pane_command: None,
            dock: Dock::default(),
            dock_size: (1, 1),
            popup: None,
            popup_shown: false,
            sidebar_w: None,
            recent: Vec::new(),
            maximized: None,
            full_size: (0, 0),
        });
        self.had_session = true;
        let si = self.sessions.len() - 1;
        self.refresh_name(si, 0);
        Ok(id)
    }

    pub(super) fn session_index(&self, sid: SessionId) -> Option<usize> {
        self.sessions.iter().position(|s| s.id == sid)
    }

    pub(super) fn active_cwd(&self, sid: SessionId) -> Option<PathBuf> {
        let s = self.sessions.iter().find(|s| s.id == sid)?;
        let from_pane = self.panes.get(&s.active_window().active).and_then(|p| p.pty.cwd());
        from_pane.or_else(|| Some(s.cwd.clone()).filter(|p| !p.as_os_str().is_empty()))
    }

    pub fn new_window(&mut self, sid: SessionId) -> Result<(), String> {
        let cwd = self.active_cwd(sid);
        let si = self.session_index(sid).ok_or("no session")?;
        let s = &self.sessions[si];
        // The new window goes at the end of the shown window's group.
        let (size, group, wi) = (s.size, s.active_window().group, s.group_windows(s.group_of(s.active)).end);
        let window = self.make_window(cwd, size, group)?;
        self.setup_pane(sid, window.active, None);
        let s = &mut self.sessions[si];
        s.windows.insert(wi, window);
        self.refresh_name(si, wi);
        self.open_window(sid, wi);
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
        self.setup_pane(sid, new_id, None);
        let w = self.sessions[si].active_window_mut();
        w.root = tree;
        w.focus(new_id);
        w.zoomed = false;
        let wi = self.sessions[si].active;
        self.refresh_name(si, wi);
        self.relayout(sid);
        Ok(())
    }

    // ---- selection ----

    /// Show window `wi` without counting it as opened (the sidebar following its selection).
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

    /// Open window `wi`: show it, focus its top-left pane if it is another window, and remember it.
    pub fn open_window(&mut self, sid: SessionId, wi: usize) {
        let Some(si) = self.session_index(sid) else { return };
        let s = &self.sessions[si];
        let Some(id) = s.windows.get(wi).map(|w| w.id) else { return };
        // A window the sidebar is already showing still counts as another one until it is opened.
        let changed = s.active != wi || s.recent.first().is_some_and(|&r| r != id);
        self.select_window(sid, wi);
        if changed {
            self.focus_top_left(si, wi);
        }
        self.remember_window(si, id);
    }

    /// The window a session shows counts as focused (a session chosen, not just hovered).
    pub(super) fn focus_shown(&mut self, sid: SessionId) {
        let Some(si) = self.session_index(sid) else { return };
        let id = self.sessions[si].active_window().id;
        self.remember_window(si, id);
    }

    fn focus_top_left(&mut self, si: usize, wi: usize) {
        let w = &mut self.sessions[si].windows[wi];
        if !self.cfg.focus_top_left || w.zoomed {
            return;
        }
        let first = w.root.first_leaf();
        w.focus(first);
        self.refresh_name(si, wi);
    }

    fn remember_window(&mut self, si: usize, id: WindowId) {
        let recent = &mut self.sessions[si].recent;
        recent.retain(|&w| w != id);
        recent.insert(0, id);
    }

    fn step_window(&mut self, sid: SessionId, forward: bool) {
        let Some(s) = self.sessions.iter().find(|s| s.id == sid) else { return };
        let n = s.windows.len();
        let wi = if forward { (s.active + 1) % n } else { (s.active + n - 1) % n };
        self.open_window(sid, wi);
    }

    /// Ctrl-Tab: the next window back in the order they were opened. Presses within the
    /// repeat time keep going further back; after that the shown window counts as opened.
    fn recent_window(&mut self, cid: ClientId, si: usize, back: bool) {
        let expired = self.clients.get(&cid).and_then(|c| c.cycle_until).is_none_or(|t| Instant::now() >= t);
        if expired {
            self.end_cycle(cid);
        }
        let s = &self.sessions[si];
        let exists = |id: &WindowId| s.windows.iter().any(|w| w.id == *id);
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let cycle = c.cycle.get_or_insert_with(|| {
            // The shown window, then the opened ones, then any never opened.
            let mut list = vec![s.windows[s.active].id];
            for id in s.recent.iter().copied().filter(exists).chain(s.windows.iter().map(|w| w.id)) {
                if !list.contains(&id) {
                    list.push(id);
                }
            }
            Cycle { list, idx: 0 }
        });
        cycle.list.retain(exists);
        let n = cycle.list.len();
        if n < 2 {
            c.cycle = None;
            return;
        }
        cycle.idx = if back { (cycle.idx + 1) % n } else { (cycle.idx + n - 1) % n };
        let id = cycle.list[cycle.idx];
        let repeat = self.cfg.repeat_time;
        c.cycle_until = Some(Instant::now() + Duration::from_millis(repeat));
        let sid = s.id;
        if let Some(wi) = s.windows.iter().position(|w| w.id == id) {
            let changed = wi != s.active;
            self.select_window(sid, wi);
            if changed {
                self.focus_top_left(si, wi);
            }
        }
    }

    /// Finish a recent-window cycle: the window it left shown counts as opened.
    pub(super) fn end_cycle(&mut self, cid: ClientId) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let (Some(cycle), Some(sid)) = (c.cycle.take(), c.session) else { return };
        c.cycle_until = None;
        let Some(si) = self.session_index(sid) else { return };
        let s = &self.sessions[si];
        let shown = s.windows[s.active].id;
        if cycle.list.get(cycle.idx) == Some(&shown) {
            self.remember_window(si, shown);
        }
    }

    // ---- teardown ----

    /// A pane's process exited (or it was killed): remove it and anything left empty.
    pub fn close_pane(&mut self, pid: PaneId) {
        let extra = self.extra_of(pid);
        if self.panes.remove(&pid).is_none() {
            return;
        }
        match extra {
            Some((si, Extra::Popup)) => {
                let s = &mut self.sessions[si];
                s.popup = None;
                s.popup_shown = false;
                self.mark_dirty();
                return;
            }
            Some((si, Extra::Dock(i))) => {
                // The pane is already gone; the entry goes with it.
                self.sessions[si].dock.entries[i].pane = None;
                self.remove_dock_entry(si, i);
                return;
            }
            None => {}
        }
        let Some((si, wi)) = self.locate_pane(pid) else { return };
        let sid = self.sessions[si].id;
        let w = &mut self.sessions[si].windows[wi];
        let root = std::mem::replace(&mut w.root, Node::Leaf(0));
        match root.remove(pid) {
            (Some(root), focus) => {
                w.root = root;
                if w.last == Some(pid) {
                    w.last = None;
                }
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
            (None, _) => self.drop_window(si, wi),
        }
        self.mark_dirty();
    }

    /// Remove window `wi` (which has no panes left), and the session if it was the last.
    fn drop_window(&mut self, si: usize, wi: usize) {
        let sid = self.sessions[si].id;
        let s = &mut self.sessions[si];
        let was_active = s.active;
        s.windows.remove(wi);
        if s.windows.is_empty() {
            self.remove_session(sid);
            return;
        }
        if wi < s.active {
            s.active -= 1;
        } else if wi == s.active {
            s.active = wi.min(s.windows.len() - 1);
            let w = &mut s.windows[s.active];
            w.activity = false;
            w.bell = false;
        }
        s.active = s.active.min(s.windows.len() - 1);
        s.normalize_groups();
        // Closing the shown window lands on another one, which then counts as focused.
        if wi == was_active {
            let id = s.active_window().id;
            self.remember_window(si, id);
        }
    }

    pub fn locate_pane(&self, pane: PaneId) -> Option<(usize, usize)> {
        self.sessions.iter().enumerate().find_map(|(si, s)| {
            s.windows.iter().position(|w| w.root.contains(pane)).map(|wi| (si, wi))
        })
    }

    pub(super) fn remove_session(&mut self, sid: SessionId) {
        let Some(idx) = self.session_index(sid) else { return };
        let removed = self.sessions.remove(idx);
        // Any panes still alive are dropped (and hung up).
        for w in &removed.windows {
            for p in w.root.leaves() {
                self.panes.remove(&p);
            }
        }
        for p in removed.popup.into_iter().chain(removed.dock.entries.iter().filter_map(|e| e.pane)) {
            self.panes.remove(&p);
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

    pub fn run_action(&mut self, cid: ClientId, action: Action, trigger: Trigger) {
        let Some(sid) = self.clients.get(&cid).and_then(|c| c.session) else { return };
        let Some(si) = self.session_index(sid) else { return };
        let follows = matches!(
            action,
            Action::SelectWindow(_) | Action::NextWindow | Action::PrevWindow | Action::RecentWindow(_)
        );
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
                if n <= self.sessions[si].group_windows(0).len() {
                    self.open_window(sid, n - 1);
                }
                Ok(())
            },
            Action::ToggleMaximize => {
                if let Some(c) = self.clients.get_mut(&cid)
                    && c.sidebar_focus.is_some()
                {
                    c.sidebar_max = !c.sidebar_max;
                } else {
                    let focused = self.focused_pane(si);
                    let full = self.clients.get(&cid).map(|c| (c.cols, c.rows));
                    let s = &mut self.sessions[si];
                    if let Some(full) = full {
                        s.full_size = full;
                    }
                    s.maximized = if s.maximized.is_some() { None } else { focused };
                    self.relayout(sid);
                }
                Ok(())
            }
            Action::RecentWindow(back) => {
                // Each prefix press starts over; plain Tab (and a root binding) go on.
                if trigger.prefix.is_some() {
                    self.end_cycle(cid);
                }
                self.recent_window(cid, si, back);
                Ok(())
            }
            Action::RenameWindow => {
                let w = self.sessions[si].active_window();
                let (id, text) = (w.id, w.name().to_string());
                self.set_prompt(cid, PromptKind::RenameWindow(id), text, "rename window");
                Ok(())
            }
            Action::NewGroup => {
                self.set_prompt(cid, PromptKind::NewGroup(sid), String::new(), "new group");
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
                if !self.resize_panel(cid, si, dir, n) {
                    self.resize_pane(si, dir, n);
                }
                let repeat = self.cfg.repeat_time;
                if let Some(c) = self.clients.get_mut(&cid) {
                    c.resize_until = (repeat > 0).then(|| Instant::now() + Duration::from_millis(repeat));
                }
                if repeat > 0 {
                    // The size indicator goes away when the repeat window ends.
                    let tx = self.events.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(repeat + 30)).await;
                        let _ = tx.send(Event::Redraw);
                    });
                }
                Ok(())
            },
            Action::KillPane => {
                if let Some(p) = self.focused_pane(si) {
                    self.close_pane(p);
                }
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
                    self.focus_shown(id);
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
                if let (Some(raw), Some(p)) = (trigger.prefix, self.focused_pane(si).and_then(|id| self.panes.get(&id))) {
                    p.pty.write(raw);
                }
                Ok(())
            }
            Action::CopyMode => {
                if let Some(id) = self.focused_pane(si)
                    && let Some(p) = self.panes.get_mut(&id)
                    && p.copy.is_none() {
                        p.copy = Some(CopyMode::new(&p.screen));
                    }
                Ok(())
            }
            Action::Copy(cmd) => {
                self.copy_command(cid, si, cmd);
                Ok(())
            }
            Action::FocusLast => {
                self.focus_last(si);
                Ok(())
            }
            Action::Navigate(dir) => {
                self.navigate(cid, si, Some(dir), trigger.key);
                Ok(())
            }
            Action::NavigateLast => {
                self.navigate(cid, si, None, trigger.key);
                Ok(())
            }
            Action::SendKey => {
                self.write_active(si, trigger.key);
                Ok(())
            }
            Action::RespawnPane => {
                if let Some(id) = self.focused_pane(si) {
                    self.set_prompt(cid, PromptKind::ConfirmRespawnPane(id), String::new(), "respawn pane? (y/n)");
                }
                Ok(())
            }
            Action::RespawnWindow => {
                let w = self.sessions[si].active_window();
                let label = format!("respawn window {}? (y/n)", w.name());
                self.set_prompt(cid, PromptKind::ConfirmRespawnWindow(w.id), String::new(), label);
                Ok(())
            }
            Action::ExportProject => self.export_project(si).map(|p| self.message(cid, format!("exported to {}", p.display()))),
            Action::SearchWindows => self.start_search(cid),
            Action::TogglePopup => self.toggle_popup(sid),
            Action::ToggleDock => self.toggle_dock(sid),
            Action::FocusDock => self.focus_dock(sid),
            Action::DockNext => self.step_dock(sid, true),
            Action::DockPrev => self.step_dock(sid, false),
            Action::DockAdd => {
                self.set_prompt(cid, PromptKind::DockAdd(sid), String::new(), "dock command (empty = shell)");
                Ok(())
            }
            Action::DockClose => match self.sessions[si].dock.entries.get(self.sessions[si].dock.active) {
                Some(e) => {
                    let label = format!("close dock pane {}? (y/n)", e.name);
                    self.set_prompt(cid, PromptKind::ConfirmDockClose(sid), String::new(), label);
                    Ok(())
                }
                None => Err("dock is empty".into()),
            },
            Action::DockRename => match self.sessions[si].dock.entries.get(self.sessions[si].dock.active) {
                Some(e) => {
                    let text = e.name.clone();
                    self.set_prompt(cid, PromptKind::DockRename(sid), text, "rename dock pane");
                    Ok(())
                }
                None => Err("dock is empty".into()),
            },
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
        if follows {
            self.sidebar_select_shown(cid);
        }
        self.mark_dirty();
    }

    /// A focused sidebar's selection moves to the window now shown.
    pub(super) fn sidebar_select_shown(&mut self, cid: ClientId) {
        let Some(c) = self.clients.get(&cid) else { return };
        if c.sidebar_focus.is_none() || c.sidebar_mode != SidebarMode::Windows || c.search.is_some() {
            return;
        }
        let Some(s) = c.session.and_then(|sid| self.sessions.iter().find(|s| s.id == sid)) else { return };
        let row = self.sidebar_rows(c, s).iter().position(|&i| i == s.active);
        if let Some(c) = self.clients.get_mut(&cid) {
            c.sidebar_focus = row.or(c.sidebar_focus);
        }
    }

    fn copy_command(&mut self, cid: ClientId, si: usize, cmd: CopyCmd) {
        let Some(active) = self.focused_pane(si) else { return };
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
        let Some(active) = self.focused_pane(si) else { return };
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

    pub(super) fn set_prompt(&mut self, cid: ClientId, kind: PromptKind, text: String, label: impl Into<String>) {
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
        self.focus_shown(id);
    }

    fn unzoom_window(&mut self, si: usize, wi: usize) {
        let sid = self.sessions[si].id;
        let w = &mut self.sessions[si].windows[wi];
        if w.zoomed {
            w.zoomed = false;
            self.relayout(sid);
        }
    }

    fn unzoom(&mut self, si: usize) {
        let wi = self.sessions[si].active;
        self.unzoom_window(si, wi);
    }

    /// Move focus from `origin` in window `wi`; false when there is no pane in that direction.
    pub fn focus_dir_from(&mut self, si: usize, wi: usize, origin: PaneId, dir: Dir) -> bool {
        self.unzoom_window(si, wi);
        let s = &mut self.sessions[si];
        let size = s.size;
        let w = &mut s.windows[wi];
        let rects = w.rects(size);
        let moved = match neighbor(&rects, origin, dir) {
            Some(n) => {
                w.focus(n);
                true
            }
            None => false,
        };
        self.refresh_name(si, wi);
        moved
    }

    fn focus_dir(&mut self, si: usize, dir: Dir) -> bool {
        let wi = self.sessions[si].active;
        let origin = self.sessions[si].windows[wi].active;
        self.focus_dir_from(si, wi, origin, dir)
    }

    /// Is the sidebar on the side `dir` points to?
    pub fn toward_sidebar(&self, dir: Dir) -> bool {
        matches!(
            (self.cfg.sidebar.position, dir),
            (Position::Left, Dir::Left) | (Position::Right, Dir::Right)
        )
    }

    /// Directional focus: panes first, then on into the sidebar; and back out of it.
    fn focus_command(&mut self, cid: ClientId, si: usize, dir: Dir) {
        let toward_sidebar = self.toward_sidebar(dir);
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
        let s = &self.sessions[si];
        let dock_shown = s.dock.visible && !s.dock.entries.is_empty();
        if s.dock.focused && s.shown_dock_pane().is_some() {
            // The dock is on the side away from the sidebar: only the way back counts.
            if toward_sidebar {
                self.sessions[si].dock.focused = false;
            }
            return;
        }
        if !self.focus_dir(si, dir) {
            if toward_sidebar {
                let _ = self.enter_sidebar(cid);
            } else if matches!(dir, Dir::Left | Dir::Right) && dock_shown {
                self.sessions[si].dock.focused = true;
            }
        }
    }

    /// Focus the previously focused pane of the window.
    pub fn focus_last_in(&mut self, si: usize, wi: usize) {
        self.unzoom_window(si, wi);
        let w = &mut self.sessions[si].windows[wi];
        if let Some(l) = w.last.filter(|l| w.root.contains(*l)) {
            w.focus(l);
        }
        self.refresh_name(si, wi);
    }

    fn focus_last(&mut self, si: usize) {
        let wi = self.sessions[si].active;
        self.focus_last_in(si, wi);
    }

    pub(super) fn write_active(&mut self, si: usize, data: Vec<u8>) {
        if let Some(p) = self.focused_pane(si).and_then(|id| self.panes.get(&id)) {
            p.pty.write(data);
        }
    }

    /// Is an editor (one that does its own Ctrl-h/j/k/l navigation) in the pane's foreground?
    fn active_pane_runs_editor(&self, si: usize) -> bool {
        let Some(active) = self.focused_pane(si) else { return false };
        self.panes.get(&active).is_some_and(|p| {
            p.copy.is_none()
                && p.pty
                    .foreground_commands()
                    .iter()
                    .any(|c| crate::procinfo::is_editor(c, &self.cfg.navigator_apps))
        })
    }

    /// Ctrl-h/j/k/l: give the key to an editor if one is running, else move focus.
    fn navigate(&mut self, cid: ClientId, si: usize, dir: Option<Dir>, key: Vec<u8>) {
        let sidebar_focused = self.clients.get(&cid).is_some_and(|c| c.sidebar_focus.is_some());
        if !sidebar_focused && (self.sessions[si].shown_popup().is_some() || self.active_pane_runs_editor(si)) {
            self.write_active(si, key);
            return;
        }
        match dir {
            Some(d) => self.focus_command(cid, si, d),
            None if !sidebar_focused => self.focus_last(si),
            None => {}
        }
    }

    fn focus_next(&mut self, si: usize) {
        self.unzoom(si);
        let w = self.sessions[si].active_window_mut();
        let leaves = w.root.leaves();
        if let Some(i) = leaves.iter().position(|&p| p == w.active) {
            w.focus(leaves[(i + 1) % leaves.len()]);
        }
        let wi = self.sessions[si].active;
        self.refresh_name(si, wi);
    }

    /// Resize the focused sidebar or dock instead of a pane. False when neither has focus.
    fn resize_panel(&mut self, cid: ClientId, si: usize, dir: Dir, n: u16) -> bool {
        let Some(c) = self.clients.get(&cid) else { return false };
        let sidebar = c.sidebar_focus.is_some();
        let s = &self.sessions[si];
        if !sidebar && !(s.dock.focused && s.shown_dock_pane().is_some()) {
            return false;
        }
        let sid = s.id;
        // The edge facing the panes moves the way the key points.
        let sidebar_left = self.cfg.sidebar.position == Position::Left;
        let grow = match dir {
            Dir::Right => sidebar_left == sidebar,
            Dir::Left => sidebar_left != sidebar,
            _ => return true,
        };
        let step = |w: u16| if grow { w.saturating_add(n) } else { w.saturating_sub(n) };
        let sw = self.sidebar_width(c);
        let dw = self.dock_width(c, s);
        let cols = c.cols;
        if sidebar {
            let max = cols.saturating_sub(super::MIN_CONTENT + dw).max(4);
            let w = step(sw).clamp(4, max.min(200));
            self.sessions[si].sidebar_w = Some(w);
        } else {
            let max = cols.saturating_sub(sw + super::MIN_CONTENT).max(super::MIN_DOCK);
            let w = step(dw).clamp(super::MIN_DOCK, max);
            let d = &mut self.sessions[si].dock;
            d.width = match d.width {
                crate::project::Size::Percent(_) => crate::project::Size::Percent(f32::from(w) * 100.0 / f32::from(cols)),
                crate::project::Size::Cells(_) => crate::project::Size::Cells(w),
            };
        }
        self.update_session_size(sid);
        true
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
            PromptKind::NewGroup(sid) => {
                if let Err(e) = self.new_group(cid, sid, &text) {
                    self.message(cid, e);
                }
            }
            PromptKind::RenameGroup(sid, gid) => self.rename_group(sid, gid, text),
            PromptKind::GroupTag(sid, gid) => {
                if let Err(e) = self.set_group_tag(sid, gid, &text) {
                    self.message(cid, e);
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
            PromptKind::DockAdd(id) => {
                if let Err(e) = self.add_dock_entry(id, &text) {
                    self.message(cid, e);
                }
            }
            PromptKind::ConfirmExport(id) => {
                let result = self.session_index(id).ok_or_else(|| "no session".to_string()).and_then(|si| self.export_project(si));
                match result {
                    Ok(p) => self.message(cid, format!("exported to {}", p.display())),
                    Err(e) => self.message(cid, e),
                }
            }
            PromptKind::DockRename(id) => self.rename_dock_entry(id, text),
            PromptKind::ConfirmDockClose(id) => {
                if let Some(si) = self.session_index(id) {
                    let active = self.sessions[si].dock.active;
                    self.remove_dock_entry(si, active);
                }
            }
            PromptKind::ConfirmRespawnPane(id) => {
                if let Err(e) = self.respawn_pane(id) {
                    self.message(cid, e);
                }
            }
            PromptKind::ConfirmRespawnWindow(id) => {
                let found = self.sessions.iter().enumerate().find_map(|(si, s)| {
                    s.windows.iter().position(|w| w.id == id).map(|wi| (si, wi))
                });
                if let Some((si, wi)) = found
                    && let Err(e) = self.respawn_window(si, wi)
                {
                    self.message(cid, e);
                }
            }
        }
        self.mark_dirty();
    }

    // ---- killing and reordering ----

    pub fn kill_window(&mut self, sid: SessionId, wi: usize) {
        let Some(s) = self.sessions.iter().find(|s| s.id == sid) else { return };
        let Some(w) = s.windows.get(wi) else { return };
        if w.pending {
            let si = self.session_index(sid).unwrap_or(0);
            self.drop_window(si, wi);
            self.mark_dirty();
            return;
        }
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
        // Windows that never started have no panes to close.
        if self.session_index(sid).is_some() {
            self.remove_session(sid);
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

    /// Switch the client to the window or session at sidebar row `idx`.
    /// Show the sidebar's item `idx`; `open` when it was chosen rather than just moved to.
    fn show_sidebar_item(&mut self, cid: ClientId, sid: SessionId, mode: SidebarMode, idx: usize, open: bool) {
        match mode {
            SidebarMode::Windows if open => self.open_window(sid, idx),
            SidebarMode::Windows => self.select_window(sid, idx),
            SidebarMode::Sessions => {
                if let Some(id) = self.sessions.get(idx).map(|s| s.id) {
                    if id != sid {
                        self.switch_client(cid, id);
                    }
                    if open {
                        self.focus_shown(id);
                    }
                }
            }
        }
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
            SidebarCmd::Down | SidebarCmd::Up => {
                let to = if cmd == SidebarCmd::Down { (sel + 1).min(count - 1) } else { sel.saturating_sub(1) };
                self.set_sidebar_focus(cid, Some(to));
                if self.cfg.sidebar.follow {
                    self.show_sidebar_item(cid, sid, mode, to, false);
                }
            }
            SidebarCmd::Leave => self.set_sidebar_focus(cid, None),
            SidebarCmd::Open => {
                self.set_sidebar_focus(cid, None);
                self.show_sidebar_item(cid, sid, mode, sel, true);
            }
            SidebarCmd::Kill => match mode {
                SidebarMode::Windows => {
                    let s = &self.sessions[si];
                    let tag = s.window_tag(sel).unwrap_or(' ');
                    let w = &s.windows[sel];
                    let (id, label) = (w.id, format!("kill window {tag} ({})? (y/n)", w.name()));
                    self.set_prompt(cid, PromptKind::ConfirmKillWindow(id), String::new(), label);
                }
                SidebarMode::Sessions => {
                    let s = &self.sessions[sel];
                    let (id, label) = (s.id, format!("kill session {} ({})? (y/n)", sel + 1, s.name));
                    self.set_prompt(cid, PromptKind::ConfirmKillSession(id), String::new(), label);
                }
            },
            SidebarCmd::Respawn => match mode {
                SidebarMode::Windows => {
                    let s = &self.sessions[si];
                    let tag = s.window_tag(sel).unwrap_or(' ');
                    let w = &s.windows[sel];
                    let (id, label) = (w.id, format!("respawn window {tag} ({})? (y/n)", w.name()));
                    self.set_prompt(cid, PromptKind::ConfirmRespawnWindow(id), String::new(), label);
                }
                SidebarMode::Sessions => return Err("respawn works on windows".into()),
            },
            SidebarCmd::Search => {
                if mode == SidebarMode::Sessions {
                    return Err("search works on windows".into());
                }
                return self.start_search(cid);
            }
            SidebarCmd::RenameGroup | SidebarCmd::GroupTag => {
                if mode == SidebarMode::Sessions {
                    return Err("groups hold windows".into());
                }
                let s = &self.sessions[si];
                let g = &s.groups[s.group_of(sel)];
                if cmd == SidebarCmd::RenameGroup {
                    let (id, text) = (g.id, g.name.clone());
                    self.set_prompt(cid, PromptKind::RenameGroup(sid, id), text, "rename group");
                } else {
                    let (id, label) = (g.id, format!("tag for group {} (now {})", g.name, g.tag));
                    self.set_prompt(cid, PromptKind::GroupTag(sid, id), String::new(), label);
                }
            }
            SidebarCmd::Export => {
                let target = if mode == SidebarMode::Sessions { sel } else { si };
                let path = Self::export_target(&self.snapshot_session(target))?;
                let id = self.sessions[target].id;
                let label = format!("export to {}? (y/n)", path.display());
                self.set_prompt(cid, PromptKind::ConfirmExport(id), String::new(), label);
            }
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
                let down = cmd == SidebarCmd::MoveDown;
                let to = match mode {
                    SidebarMode::Windows => self.move_window(sid, sel, down),
                    SidebarMode::Sessions => {
                        let to = if down { sel + 1 } else { sel.wrapping_sub(1) };
                        (to < count).then(|| self.sessions.swap(sel, to)).map(|_| to)
                    }
                };
                if let Some(to) = to {
                    self.set_sidebar_focus(cid, Some(to));
                    if self.cfg.sidebar.follow {
                        self.show_sidebar_item(cid, sid, mode, to, false);
                    }
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
