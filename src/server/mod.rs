//! The dtm server: owns all sessions, panes and attached clients.
//!
//! Everything runs in a single actor loop; PTY readers and client
//! connections feed it through one event channel, so no locking is needed.

pub mod actions;
pub mod copy;
pub mod extra;
pub mod groups;
pub mod input;
pub mod ipc;
pub mod mouse;
pub mod project;
pub mod layout;
pub mod render;
pub mod search;
pub mod sidebar;
pub mod state;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use futures::{SinkExt, StreamExt};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

use crate::config::{Config, Position, SidebarMode};
use crate::fuzzy;
use crate::paths;
use crate::protocol::{decode, encode, framed, AttachMode, ClientMsg, ServerMsg, SessionInfo};
use crate::pty::PtyEvent;
use crate::term::canvas::Canvas;
use crate::term::cell::{Attrs, Cell, Color};
use layout::PaneId;
use sidebar::{Footer, Item, Line, SidebarView};
use state::{Client, ClientId, Pane, Session, SessionId};

/// Content narrower than this hides the sidebar.
const MIN_CONTENT: u16 = 20;
/// The dock (separator included) is never narrower than this.
const MIN_DOCK: u16 = 12;
/// Delay between a state change and the frame being sent.
const RENDER_DELAY: Duration = Duration::from_millis(8);
const MESSAGE_TTL: Duration = Duration::from_secs(3);

pub enum Event {
    Connected { id: ClientId, tx: UnboundedSender<ServerMsg> },
    Msg { id: ClientId, msg: ClientMsg },
    Gone { id: ClientId },
    Pane { id: PaneId, ev: PtyEvent },
    KeyFlush { id: ClientId },
    /// Draw again (an indicator timed out).
    Redraw,
}

pub struct Server {
    pub cfg: Config,
    pub cfg_path: Option<PathBuf>,
    pub sessions: Vec<Session>,
    pub panes: HashMap<PaneId, Pane>,
    pub clients: BTreeMap<ClientId, Client>,
    pub events: UnboundedSender<Event>,
    /// Exported to panes so the `dtm` CLI and editor plugins find the server.
    pub socket: PathBuf,
    pub exe: String,
    pub render_at: Option<Instant>,
    pub shutdown: bool,
    /// Text from the last copy-mode yank.
    pub paste_text: String,
    had_session: bool,
    next_pane: u32,
    next_window: u32,
    next_session: u32,
}

/// Run the server until its last session ends (or it is told to quit).
pub async fn run(socket: PathBuf, cfg: Config, cfg_path: Option<PathBuf>) -> Result<()> {
    paths::prepare_socket_dir(&socket).context("creating socket directory")?;
    if socket.exists() {
        if std::os::unix::net::UnixStream::connect(&socket).is_ok() {
            bail!("a server is already running on {}", socket.display());
        }
        std::fs::remove_file(&socket)?;
    }
    let listener = UnixListener::bind(&socket).with_context(|| format!("binding {}", socket.display()))?;
    tracing::info!("listening on {}", socket.display());

    let (tx, mut rx) = unbounded_channel();
    let accept_tx = tx.clone();
    let accept = tokio::spawn(async move {
        let mut next: ClientId = 1;
        while let Ok((stream, _)) = listener.accept().await {
            tokio::spawn(connection(next, stream, accept_tx.clone()));
            next += 1;
        }
    });

    let mut server = Server::new(cfg, cfg_path, socket.clone(), tx);
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    loop {
        let at = server.render_at;
        let render_wait = async {
            match at {
                Some(t) => tokio::time::sleep_until(t.into()).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            ev = rx.recv() => match ev {
                Some(ev) => server.handle(ev),
                None => break,
            },
            _ = render_wait => server.flush(),
            _ = tick.tick() => server.on_tick(),
        }
        if server.should_exit() {
            break;
        }
    }
    tracing::info!("server exiting");
    accept.abort();
    drop(server);
    // Let writer tasks deliver final messages (Exited) before the process ends.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let _ = std::fs::remove_file(&socket);
    Ok(())
}

async fn connection(id: ClientId, stream: UnixStream, events: UnboundedSender<Event>) {
    let (out_tx, mut out_rx) = unbounded_channel::<ServerMsg>();
    let (mut sink, mut stream) = framed(stream).split();
    if events.send(Event::Connected { id, tx: out_tx }).is_err() {
        return;
    }
    tokio::spawn(async move {
        while let Some(msg) = out_rx.recv().await {
            if sink.send(encode(&msg)).await.is_err() {
                break;
            }
        }
    });
    while let Some(Ok(frame)) = stream.next().await {
        match decode::<ClientMsg>(&frame) {
            Ok(msg) => {
                if events.send(Event::Msg { id, msg }).is_err() {
                    return;
                }
            }
            Err(_) => break,
        }
    }
    let _ = events.send(Event::Gone { id });
}

impl Server {
    pub fn new(cfg: Config, cfg_path: Option<PathBuf>, socket: PathBuf, events: UnboundedSender<Event>) -> Server {
        Server {
            cfg,
            cfg_path,
            sessions: Vec::new(),
            panes: HashMap::new(),
            clients: BTreeMap::new(),
            events,
            socket,
            exe: std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "dtm".into()),
            render_at: None,
            shutdown: false,
            paste_text: String::new(),
            had_session: false,
            next_pane: 1,
            next_window: 1,
            next_session: 1,
        }
    }

    pub fn should_exit(&self) -> bool {
        self.shutdown || (self.had_session && self.sessions.is_empty())
    }

    pub fn mark_dirty(&mut self) {
        if self.render_at.is_none() {
            self.render_at = Some(Instant::now() + RENDER_DELAY);
        }
    }

    pub fn alloc_pane_id(&mut self) -> PaneId {
        self.next_pane += 1;
        self.next_pane - 1
    }

    pub fn alloc_window_id(&mut self) -> u32 {
        self.next_window += 1;
        self.next_window - 1
    }

    pub fn alloc_session_id(&mut self) -> SessionId {
        self.next_session += 1;
        self.next_session - 1
    }

    pub fn handle(&mut self, ev: Event) {
        self.handle_event(ev);
        // A window that just became visible starts before the next event sees it.
        self.start_visible_windows();
    }

    fn handle_event(&mut self, ev: Event) {
        match ev {
            Event::Connected { id, tx } => {
                self.clients.insert(id, Client::new(id, tx));
            }
            Event::Gone { id } => self.remove_client(id),
            Event::Msg { id, msg } => self.client_msg(id, msg),
            Event::Pane { id, ev: PtyEvent::Output(data) } => self.pane_output(id, data),
            Event::Pane { id, ev: PtyEvent::Exited } => self.close_pane(id),
            Event::Redraw => self.mark_dirty(),
            Event::KeyFlush { id } => {
                if let Some(c) = self.clients.get_mut(&id) {
                    c.flush_scheduled = false;
                }
                self.process_keys(id, true);
            }
        }
    }

    fn client_msg(&mut self, cid: ClientId, msg: ClientMsg) {
        match msg {
            ClientMsg::Attach { mode, name, cols, rows, cwd } => self.attach_msg(cid, mode, name, cols, rows, cwd),
            ClientMsg::OpenProject { plan, source, cols, rows } => self.open_project_msg(cid, plan, source, cols, rows),
            ClientMsg::Snapshot { session, pane } => match self.snapshot(session.as_deref(), pane) {
                Ok(snap) => self.send(cid, ServerMsg::Snapshot(snap)),
                Err(e) => self.send(cid, ServerMsg::Error(e)),
            },
            ClientMsg::Input(data) => self.handle_input(cid, data),
            ClientMsg::Resize { cols, rows } => self.client_resize(cid, cols, rows),
            ClientMsg::Detach => self.detach(cid),
            ClientMsg::List => {
                let list = self
                    .sessions
                    .iter()
                    .map(|s| SessionInfo {
                        name: s.name.clone(),
                        windows: s.windows.len(),
                        attached: self.clients.values().filter(|c| c.session == Some(s.id)).count(),
                    })
                    .collect();
                self.send(cid, ServerMsg::Sessions(list));
            }
            ClientMsg::KillServer => {
                for c in self.clients.values() {
                    c.send(ServerMsg::Exited);
                }
                self.shutdown = true;
            }
            ClientMsg::Reload => match self.reload() {
                Ok(()) => self.send(cid, ServerMsg::Ok),
                Err(e) => self.send(cid, ServerMsg::Error(e)),
            },
            ClientMsg::SelectPane { pane, dir } => match self.select_pane_ipc(pane, dir) {
                Ok(()) => self.send(cid, ServerMsg::Ok),
                Err(e) => self.send(cid, ServerMsg::Error(e)),
            },
            ClientMsg::Query { pane, format } => match self.query_ipc(pane, &format) {
                Ok(text) => self.send(cid, ServerMsg::Text(text)),
                Err(e) => self.send(cid, ServerMsg::Error(e)),
            },
        }
    }

    pub fn send(&self, cid: ClientId, msg: ServerMsg) {
        if let Some(c) = self.clients.get(&cid) {
            c.send(msg);
        }
    }

    pub fn message(&mut self, cid: ClientId, text: impl Into<String>) {
        if let Some(c) = self.clients.get_mut(&cid) {
            c.message = Some((text.into(), Instant::now()));
        }
        self.mark_dirty();
    }

    // ---- attaching ----

    fn find_session(&self, name: Option<&str>) -> Option<SessionId> {
        match name {
            Some(n) => self.sessions.iter().find(|s| s.name == n).map(|s| s.id),
            None => self.sessions.first().map(|s| s.id),
        }
    }

    fn attach_msg(
        &mut self,
        cid: ClientId,
        mode: AttachMode,
        name: Option<String>,
        cols: u16,
        rows: u16,
        cwd: PathBuf,
    ) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        if c.session.is_some() {
            return;
        }
        c.cols = cols.max(1);
        c.rows = rows.max(1);
        c.sidebar_visible = self.cfg.sidebar.visible;
        c.sidebar_mode = self.cfg.sidebar.default_mode;
        let existing = self.find_session(name.as_deref());
        let result = match (mode, existing) {
            (AttachMode::New, Some(_)) if name.is_some() => {
                Err(format!("duplicate session: {}", name.as_deref().unwrap_or("")))
            }
            (AttachMode::Existing, None) => Err(match &name {
                Some(n) => format!("no such session: {n}"),
                None => "no sessions".to_string(),
            }),
            (AttachMode::Default | AttachMode::Existing, Some(sid)) => Ok(sid),
            _ => {
                let size = self.content_size(cid);
                self.create_session(name, Some(cwd), size)
            }
        };
        match result {
            Ok(sid) => {
                self.switch_client(cid, sid);
                self.focus_shown(sid);
            }
            Err(e) => {
                self.send(cid, ServerMsg::Error(e));
                if self.should_exit() {
                }
            }
        }
    }

    pub fn detach(&mut self, cid: ClientId) {
        if let Some(c) = self.clients.remove(&cid) {
            c.send(ServerMsg::Detached);
            if let Some(sid) = c.session {
                self.client_left(sid, cid);
            }
        }
    }

    fn remove_client(&mut self, cid: ClientId) {
        if let Some(c) = self.clients.remove(&cid)
            && let Some(sid) = c.session {
                self.client_left(sid, cid);
            }
    }

    /// Point a client at a session, making it the size owner.
    pub fn switch_client(&mut self, cid: ClientId, sid: SessionId) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let old = c.session.replace(sid);
        c.prev = None;
        c.prompt = None;
        c.search = None;
        c.prefix = None;
        if let Some(old) = old.filter(|&o| o != sid) {
            self.client_left(old, cid);
        }
        if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
            s.owner = Some(cid);
        }
        self.update_session_size(sid);
        self.mark_dirty();
    }

    /// A client stopped viewing `sid`: hand the size over to the most recent other one.
    pub fn client_left(&mut self, sid: SessionId, cid: ClientId) {
        let next = self
            .clients
            .values()
            .filter(|c| c.session == Some(sid) && c.id != cid)
            .max_by_key(|c| c.last_active)
            .map(|c| c.id);
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) else { return };
        if s.owner == Some(cid) {
            s.owner = next;
        }
        self.update_session_size(sid);
        self.mark_dirty();
    }

    fn client_resize(&mut self, cid: ClientId, cols: u16, rows: u16) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        c.cols = cols.max(1);
        c.rows = rows.max(1);
        c.prev = None;
        if let Some(sid) = c.session {
            self.touch(cid);
            self.update_session_size(sid);
        }
        self.mark_dirty();
    }

    /// Record activity; the most recently active client decides the size.
    pub fn touch(&mut self, cid: ClientId) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        c.last_active = Instant::now();
        let Some(sid) = c.session else { return };
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) else { return };
        if s.owner != Some(cid) {
            s.owner = Some(cid);
            self.update_session_size(sid);
            self.mark_dirty();
        }
    }

    // ---- sizing ----

    pub fn sidebar_width(&self, c: &Client) -> u16 {
        let set = c.session.and_then(|id| self.sessions.iter().find(|s| s.id == id)).and_then(|s| s.sidebar_w);
        let w = set.unwrap_or(self.cfg.sidebar.width);
        if c.sidebar_visible && c.cols >= w + MIN_CONTENT { w } else { 0 }
    }

    /// Width of the dock for this client, 0 when hidden, empty or there is no room.
    pub fn dock_width(&self, c: &Client, s: &Session) -> u16 {
        if !s.dock.visible {
            return 0;
        }
        let avail = c.cols - self.sidebar_width(c);
        if avail < MIN_CONTENT + MIN_DOCK {
            return 0;
        }
        crate::project::cells(s.dock.width, c.cols).clamp(MIN_DOCK, avail - MIN_CONTENT)
    }

    fn client_dock_width(&self, c: &Client) -> u16 {
        c.session
            .and_then(|id| self.sessions.iter().find(|s| s.id == id))
            .map_or(0, |s| self.dock_width(c, s))
    }

    pub fn content_size(&self, cid: ClientId) -> (u16, u16) {
        match self.clients.get(&cid) {
            Some(c) => (c.cols - self.sidebar_width(c) - self.client_dock_width(c), c.rows),
            None => (80, 24),
        }
    }

    pub fn update_session_size(&mut self, sid: SessionId) {
        let Some(owner) = self.sessions.iter().find(|s| s.id == sid).and_then(|s| s.owner) else { return };
        let Some(c) = self.clients.get(&owner) else { return };
        let size = self.content_size(owner);
        let dw = self.client_dock_width(c);
        let rows = c.rows;
        let full = (c.cols, c.rows);
        let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) else { return };
        s.size = size;
        s.full_size = full;
        if dw > 0 {
            s.dock_size = ((dw - 1).max(1), rows.saturating_sub(1).max(1));
        }
        self.relayout(sid);
    }

    /// Resize every pane of a session to match its layout rect.
    pub fn relayout(&mut self, sid: SessionId) {
        let Some(s) = self.sessions.iter().find(|s| s.id == sid) else { return };
        let mut jobs: Vec<(PaneId, u16, u16)> = s
            .windows
            .iter()
            .flat_map(|w| w.rects(s.size))
            .filter(|(_, r)| r.w > 0 && r.h > 0)
            .map(|(id, r)| (id, r.w, r.h))
            .collect();
        if s.dock.visible {
            jobs.extend(s.dock.entries.iter().filter_map(|e| e.pane).map(|p| (p, s.dock_size.0, s.dock_size.1)));
        }
        if let Some(p) = s.popup {
            let r = self.popup_inner(s.size);
            jobs.push((p, r.w, r.h));
        }
        if let Some(p) = s.maximized.filter(|_| s.full_size.0 > 0 && s.full_size.1 > 0) {
            // Applied last, so it wins over the pane's normal size.
            jobs.push((p, s.full_size.0, s.full_size.1));
        }
        for (id, w, h) in jobs {
            if let Some(p) = self.panes.get_mut(&id) {
                p.resize(w, h);
            }
        }
    }

    // ---- pane events ----

    fn locate(&self, pane: PaneId) -> Option<(usize, usize)> {
        self.sessions.iter().enumerate().find_map(|(si, s)| {
            s.windows.iter().position(|w| w.root.contains(pane)).map(|wi| (si, wi))
        })
    }

    fn pane_output(&mut self, id: PaneId, data: Vec<u8>) {
        let Some(p) = self.panes.get_mut(&id) else { return };
        let history = p.screen.history_len();
        p.screen.feed(&data);
        if let Some(copy) = p.copy.as_mut() {
            copy.history_grew(p.screen.history_len().saturating_sub(history));
            copy.clamp(&p.screen);
        }
        let replies = p.screen.take_replies();
        if !replies.is_empty() {
            p.pty.write(replies);
        }
        let bell = p.screen.take_bell();
        let title_changed = p.screen.title != p.last_title;
        if title_changed {
            p.last_title = p.screen.title.clone();
        }
        let Some((si, wi)) = self.locate(id) else {
            if self.extra_of(id).is_some() {
                self.mark_dirty();
            }
            return;
        };
        let visible = self.sessions[si].active == wi;
        let w = &mut self.sessions[si].windows[wi];
        let mut dirty = visible;
        if !visible && !w.activity {
            w.activity = true;
            dirty = true;
        }
        if bell && !visible && !w.bell {
            w.bell = true;
            dirty = true;
        }
        if title_changed && w.active == id {
            self.refresh_name(si, wi);
            dirty = true;
        }
        if dirty {
            self.mark_dirty();
        }
    }

    pub fn refresh_name(&mut self, si: usize, wi: usize) -> bool {
        let w = &self.sessions[si].windows[wi];
        if w.pending {
            return false;
        }
        let name = match self.panes.get(&w.active) {
            Some(p) if !p.screen.title.is_empty() => p.screen.title.clone(),
            Some(p) => p.pty.process_name().unwrap_or_else(|| "sh".into()),
            None => "sh".into(),
        };
        let w = &mut self.sessions[si].windows[wi];
        let changed = w.auto_name != name;
        w.auto_name = name;
        changed
    }

    fn on_tick(&mut self) {
        let mut dirty = false;
        for si in 0..self.sessions.len() {
            for wi in 0..self.sessions[si].windows.len() {
                dirty |= self.refresh_name(si, wi);
            }
        }
        for c in self.clients.values_mut() {
            if c.message.as_ref().is_some_and(|(_, t)| t.elapsed() >= MESSAGE_TTL) {
                c.message = None;
                dirty = true;
            }
        }
        if dirty {
            self.mark_dirty();
        }
    }

    pub fn reload(&mut self) -> Result<(), String> {
        let cfg = match &self.cfg_path {
            Some(p) => Config::load(p)?,
            None => Config::default(),
        };
        self.cfg = cfg;
        let ids: Vec<_> = self.sessions.iter().map(|s| s.id).collect();
        for sid in ids {
            self.update_session_size(sid);
        }
        self.mark_dirty();
        Ok(())
    }

    // ---- rendering ----

    /// Does the sidebar draw a footer line for this client? (Not for prompts.)
    pub fn footer_visible(&self, c: &Client) -> bool {
        c.message.is_some() || c.sidebar_focus.is_some()
    }

    /// Keep each client's sidebar selection inside its list.
    /// A maximized pane or sidebar ends when focus moves away from it.
    fn sync_maximized(&mut self) {
        let stale: Vec<SessionId> = self
            .sessions
            .iter()
            .filter(|s| s.maximized.is_some() && s.maximized != self.focused_pane_in(s))
            .map(|s| s.id)
            .collect();
        for sid in stale {
            if let Some(s) = self.sessions.iter_mut().find(|s| s.id == sid) {
                s.maximized = None;
            }
            self.relayout(sid);
        }
        for c in self.clients.values_mut().filter(|c| c.sidebar_focus.is_none()) {
            c.sidebar_max = false;
        }
    }

    fn clamp_sidebar_focus(&mut self) {
        let counts: Vec<(ClientId, usize)> = self
            .clients
            .values()
            .filter(|c| c.sidebar_focus.is_some())
            .map(|c| {
                let n = match c.session.and_then(|id| self.sessions.iter().find(|s| s.id == id)) {
                    Some(s) => self.sidebar_rows(c, s).len(),
                    None => 0,
                };
                // While searching, an empty result list keeps the sidebar focused.
                (c.id, if c.search.is_some() { n.max(1) } else { n })
            })
            .collect();
        for (id, n) in counts {
            if let Some(c) = self.clients.get_mut(&id) {
                c.sidebar_focus = if n == 0 { None } else { c.sidebar_focus.map(|f| f.min(n - 1)) };
            }
        }
    }

    pub fn flush(&mut self) {
        self.render_at = None;
        self.start_visible_windows();
        self.clamp_sidebar_focus();
        self.sync_maximized();
        let mut contents: HashMap<SessionId, Canvas> = HashMap::new();
        let mut frames: Vec<(ClientId, Canvas)> = Vec::new();
        for c in self.clients.values() {
            let Some(s) = c.session.and_then(|sid| self.sessions.iter().find(|s| s.id == sid)) else { continue };
            let content = contents
                .entry(s.id)
                .or_insert_with(|| render::render_window(s.active_window(), &self.panes, s.size));
            frames.push((c.id, self.compose(c, s, content)));
        }
        for (cid, canvas) in frames {
            let Some(c) = self.clients.get_mut(&cid) else { continue };
            let bytes = canvas.diff(c.prev.as_ref());
            c.prev = Some(canvas);
            if !bytes.is_empty() {
                c.send(ServerMsg::Output(bytes));
            }
        }
    }

    /// A window's sidebar label: its tag (after its group's, while searching several groups) and name.
    fn window_label(s: &Session, wi: usize, with_group: bool) -> String {
        let tag = s.window_tag(wi).unwrap_or(' ');
        let name = s.windows[wi].name();
        if with_group {
            format!("{}{tag} {name}", s.groups[s.group_of(wi)].tag)
        } else {
            format!("{tag} {name}")
        }
    }

    /// The window or session indices the sidebar lists, in row order.
    pub fn sidebar_rows(&self, c: &Client, s: &Session) -> Vec<usize> {
        match (c.sidebar_mode, &c.search) {
            (SidebarMode::Windows, Some(q)) => {
                // Typing a group's name finds its windows too.
                let labels: Vec<String> = (0..s.windows.len())
                    .map(|i| format!("{} {}", Self::window_label(s, i, true), s.groups[s.group_of(i)].name))
                    .collect();
                let mut rows = fuzzy::rank(&q.query, labels.iter().map(String::as_str));
                // Results are listed group by group, best match first within each.
                rows.sort_by_key(|&i| s.group_of(i));
                rows
            }
            (SidebarMode::Windows, None) => (0..s.windows.len()).collect(),
            (SidebarMode::Sessions, _) => (0..self.sessions.len()).collect(),
        }
    }

    /// What each sidebar line shows: the rows of `sidebar_rows`, with a heading before each
    /// group's windows (and an empty line between groups) when there are several groups.
    pub fn sidebar_lines(&self, c: &Client, s: &Session) -> Vec<Line> {
        let rows = self.sidebar_rows(c, s);
        let headings = c.sidebar_mode == SidebarMode::Windows && s.groups.len() > 1;
        let mut lines = Vec::with_capacity(rows.len() + 2 * s.groups.len());
        let mut last = None;
        for (row, &i) in rows.iter().enumerate() {
            if headings && last != Some(s.group_of(i)) {
                if last.is_some() {
                    lines.push(Line::Gap);
                }
                lines.push(Line::Group(s.group_of(i)));
            }
            last = Some(s.group_of(i));
            lines.push(Line::Row(row));
        }
        lines
    }

    fn sidebar_view(&self, c: &Client, s: &Session) -> SidebarView {
        let mut subtag = None;
        let (header, subheader, items) = match c.sidebar_mode {
            SidebarMode::Windows => {
                let rows = self.sidebar_rows(c, s);
                let mut items: Vec<Item> = self
                    .sidebar_lines(c, s)
                    .into_iter()
                    .map(|line| {
                        let row = match line {
                            Line::Row(row) => row,
                            Line::Group(gi) => {
                                let g = &s.groups[gi];
                                return Item::heading(g.name.clone(), g.tag);
                            }
                            Line::Gap => return Item::gap(),
                        };
                        let i = rows[row];
                        let w = &s.windows[i];
                        let mut marks = String::new();
                        if w.zoomed {
                            marks.push('Z');
                        }
                        if w.bell {
                            marks.push('!');
                        }
                        if w.activity {
                            marks.push('●');
                        }
                        if w.pending {
                            marks.push('…');
                        }
                        let label = Self::window_label(s, i, false);
                        Item { label, marks, active: i == s.active, selected: c.sidebar_focus == Some(row), heading: false }
                    })
                    .collect();
                let sub = match &c.search {
                    Some(q) => {
                        if items.is_empty() {
                            items.push(Item { label: "no match".into(), marks: String::new(), active: false, selected: false, heading: false });
                        }
                        format!("/{}", q.query)
                    }
                    // One group: its title is the subheader. Several: each has a heading in the list.
                    None if s.groups.len() == 1 => {
                        subtag = Some(s.groups[0].tag);
                        s.groups[0].name.clone()
                    }
                    None => String::new(),
                };
                (s.name.clone(), sub, items)
            }
            SidebarMode::Sessions => {
                let items = self
                    .sessions
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        let elsewhere = self.clients.values().any(|k| k.id != c.id && k.session == Some(o.id));
                        Item {
                            label: format!("{} {}", i + 1, o.name),
                            marks: format!("{}w{}", o.windows.len(), if elsewhere { "*" } else { "" }),
                            active: o.id == s.id,
                            selected: c.sidebar_focus == Some(i),
                            heading: false,
                        }
                    })
                    .collect();
                ("dtm".to_string(), "sessions".to_string(), items)
            }
        };
        let footer = match &c.message {
            Some((m, _)) => Some(Footer { text: m.clone(), prompt: false }),
            None if c.search.is_some() => Some(Footer { text: "⏎ open  esc back".to_string(), prompt: false }),
            None if c.sidebar_focus.is_some() => {
                Some(Footer { text: "j/k ⏎ x r g t n / e q".to_string(), prompt: false })
            }
            None => None,
        };
        SidebarView { header, subheader, subtag, items, footer, focused: c.sidebar_focus.is_some() }
    }

    fn compose(&self, c: &Client, s: &Session, content: &Canvas) -> Canvas {
        let (cols, rows) = (c.cols as usize, c.rows as usize);
        if let Some(p) = s.maximized.and_then(|p| self.panes.get(&p)) {
            return self.compose_maximized(c, p);
        }
        let sw = self.sidebar_width(c) as usize;
        let dw = self.dock_width(c, s) as usize;
        let mut canvas = Canvas::new(cols, rows);
        // Left sidebar: [sidebar][content][dock]; right sidebar: [dock][content][sidebar].
        let (content_x, sidebar_x, dock_x) = match self.cfg.sidebar.position {
            Position::Left => (sw, 0, cols - dw),
            Position::Right => (dw, cols - sw, 0),
        };
        let cw = cols - sw - dw;
        canvas.blit_canvas(content, content_x, 0, cw, rows);
        canvas.app_cursor = content.app_cursor;
        canvas.bracketed_paste = content.bracketed_paste;
        canvas.cursor_style = content.cursor_style;
        canvas.mouse = self.cfg.mouse;
        canvas.cursor = content.cursor.filter(|&(x, y)| x < cw && y < rows).map(|(x, y)| (x + content_x, y));
        if dw > 0 {
            let focused = s.dock.focused && s.shown_popup().is_none();
            let cur = render::draw_dock(&mut canvas, (dock_x, dw), self.cfg.sidebar.position == Position::Left, &s.dock, &self.panes, focused);
            if focused {
                canvas.cursor = cur;
            }
        }
        if s.shown_popup().is_some() {
            let pane = s.popup.and_then(|p| self.panes.get(&p));
            canvas.cursor = render::draw_popup(&mut canvas, content_x, self.popup_outer(s.size), pane);
        }
        if self.extra_focused(s)
            && let Some(p) = self.focused_pane_in(s).and_then(|id| self.panes.get(&id))
        {
            let m = p.screen.modes();
            canvas.app_cursor = m.app_cursor;
            canvas.bracketed_paste = m.bracketed_paste;
            canvas.cursor_style = m.cursor_style;
        }
        let view = self.sidebar_view(c, s);
        if sw > 0 {
            sidebar::draw(&mut canvas, sidebar_x, sw, self.cfg.sidebar.position, &view);
        } else if let Some(f) = &view.footer {
            // No sidebar: show messages on the bottom row.
            let attrs = Attrs { fg: Color::Idx(8), bg: Color::Default, flags: 0 };
            canvas.fill(0, rows - 1, cols, 1, Cell::blank(attrs));
            canvas.put_str(0, rows - 1, &f.text, attrs, cols);
        }
        if c.sidebar_focus.is_some() {
            canvas.cursor = None;
        }
        if c.resize_until.is_some_and(|t| Instant::now() < t) && s.shown_popup().is_none() {
            // While resizing, say how big the thing being resized is.
            let w = s.active_window();
            let badge = if c.sidebar_focus.is_some() {
                Some((format!("{sw} cols"), sidebar_x, 0, sw, rows))
            } else if s.dock.focused && s.shown_dock_pane().is_some() {
                Some((format!("{dw} cols"), dock_x, 0, dw, rows))
            } else {
                w.rects(s.size).into_iter().find(|(id, _)| *id == w.active).map(|(_, r)| {
                    (format!("{}×{}", r.w, r.h), content_x + r.x as usize, r.y as usize, r.w as usize, r.h as usize)
                })
            };
            if let Some((text, x, y, bw, bh)) = badge {
                render::draw_badge(&mut canvas, (x, y, bw, bh), &text);
            }
        }
        if c.sidebar_max && c.sidebar_focus.is_some() {
            sidebar::draw(&mut canvas, 0, cols, Position::Left, &view);
        }
        self.draw_prompt_box(c, &mut canvas);
        canvas
    }

    /// Prompts are a box in the middle of the screen, over everything.
    fn draw_prompt_box(&self, c: &Client, canvas: &mut Canvas) {
        if let Some(p) = &c.prompt {
            canvas.cursor = render::draw_prompt(canvas, &p.label, &p.text, p.kind.is_confirm());
        }
    }

    /// One pane over the whole terminal.
    fn compose_maximized(&self, c: &Client, pane: &Pane) -> Canvas {
        let (cols, rows) = (c.cols as usize, c.rows as usize);
        let mut canvas = Canvas::new(cols, rows);
        canvas.cursor = render::draw_pane_view(&mut canvas, pane, (0, 0, cols, rows), true);
        let m = pane.screen.modes();
        canvas.app_cursor = m.app_cursor;
        canvas.bracketed_paste = m.bracketed_paste;
        canvas.cursor_style = m.cursor_style;
        canvas.mouse = self.cfg.mouse;
        if let Some((text, _)) = &c.message {
            let attrs = Attrs { fg: Color::Idx(8), bg: Color::Default, flags: 0 };
            canvas.fill(0, rows - 1, cols, 1, Cell::blank(attrs));
            canvas.put_str(0, rows - 1, text, attrs, cols);
        }
        self.draw_prompt_box(c, &mut canvas);
        canvas
    }
}
