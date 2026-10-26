use std::path::PathBuf;
use std::time::Instant;

use tokio::sync::mpsc::UnboundedSender;

use super::layout::{Node, PaneId, Rect};
use crate::config::SidebarMode;
use crate::project::{PlanNode, Size, DEFAULT_DOCK_WIDTH};
use crate::protocol::ServerMsg;
use crate::pty::Pty;
use crate::term::canvas::Canvas;
use crate::term::screen::Screen;

pub type WindowId = u32;
pub type SessionId = u32;
pub type ClientId = u32;
pub type GroupId = u32;

pub use crate::project::DEFAULT_GROUP;

/// The tag of the window at `pos` in its group: 1-9, then a-z.
pub fn window_tag(pos: usize) -> Option<char> {
    match pos {
        0..9 => char::from_digit(pos as u32 + 1, 10),
        9..35 => Some((b'a' + (pos - 9) as u8) as char),
        _ => None,
    }
}

/// The position in its group of the window tagged `tag`.
pub fn tag_pos(tag: char) -> Option<usize> {
    match tag {
        '1'..='9' => Some(tag as usize - '1' as usize),
        'a'..='z' => Some(tag as usize - 'a' as usize + 9),
        _ => None,
    }
}

/// Windows that belong together; a group's windows sit next to each other in `Session::windows`.
pub struct Group {
    pub id: GroupId,
    pub name: String,
    /// Typed after the prefix to pick one of the group's windows by its tag.
    pub tag: char,
}

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
    pub group: GroupId,
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

    pub fn rects(&self, size: (u16, u16), sp: super::layout::Spacing) -> Vec<(PaneId, Rect)> {
        let full = Rect::new(0, 0, size.0, size.1);
        if self.zoomed {
            return vec![(self.active, full)];
        }
        let mut v = Vec::new();
        self.root.layout(full, sp, &mut v);
        v
    }
}

/// A session-wide pane in the dock; started when its tab is first shown.
pub struct DockEntry {
    pub name: String,
    pub spec: PaneSpec,
    pub pane: Option<PaneId>,
}

pub struct Dock {
    pub entries: Vec<DockEntry>,
    pub active: usize,
    pub width: Size,
    pub visible: bool,
    /// Keys go to the dock pane instead of the window.
    pub focused: bool,
}

impl Default for Dock {
    fn default() -> Dock {
        Dock { entries: Vec::new(), active: 0, width: DEFAULT_DOCK_WIDTH, visible: false, focused: false }
    }
}

/// A pane that belongs to the session but not to a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Extra {
    Popup,
    Dock(usize),
}

pub struct Session {
    pub id: SessionId,
    pub name: String,
    /// Grouped together, in the order of `groups`.
    pub windows: Vec<Window>,
    pub groups: Vec<Group>,
    pub active: usize,
    /// Size of the pane area, set by the owning client.
    pub size: (u16, u16),
    pub owner: Option<ClientId>,
    pub cwd: PathBuf,
    /// The project file this session was opened from.
    pub project: Option<PathBuf>,
    /// Typed into every new pane before its own command.
    pub pane_command: Option<String>,
    pub dock: Dock,
    /// Size of a dock pane (the dock minus its tab row and separator).
    pub dock_size: (u16, u16),
    pub popup: Option<PaneId>,
    pub popup_shown: bool,
    /// Sidebar width set by resizing (shared by every client); the configured width when unset.
    pub sidebar_w: Option<u16>,
    /// Windows in the order they were last opened, most recent first.
    pub recent: Vec<WindowId>,
    /// The pane filling the whole terminal (toggle-maximize), if any.
    pub maximized: Option<PaneId>,
    /// The owner client's whole terminal, which a maximized pane gets.
    pub full_size: (u16, u16),
}

impl Session {
    /// The popup, while it is on screen.
    pub fn shown_popup(&self) -> Option<PaneId> {
        self.popup.filter(|_| self.popup_shown)
    }

    /// The dock's pane that is on screen (started and visible).
    pub fn shown_dock_pane(&self) -> Option<PaneId> {
        if self.dock.visible { self.dock.entries.get(self.dock.active).and_then(|e| e.pane) } else { None }
    }

    pub fn active_window(&self) -> &Window {
        &self.windows[self.active]
    }

    pub fn active_window_mut(&mut self) -> &mut Window {
        &mut self.windows[self.active]
    }

    pub fn group_index(&self, id: GroupId) -> Option<usize> {
        self.groups.iter().position(|g| g.id == id)
    }

    /// The index in `groups` of window `wi`'s group.
    pub fn group_of(&self, wi: usize) -> usize {
        self.group_index(self.windows[wi].group).unwrap_or(0)
    }

    /// Indices of the windows in group `gi`.
    pub fn group_windows(&self, gi: usize) -> std::ops::Range<usize> {
        let Some(id) = self.groups.get(gi).map(|g| g.id) else { return 0..0 };
        let start = self.windows.iter().position(|w| w.group == id).unwrap_or(self.windows.len());
        let len = self.windows[start..].iter().take_while(|w| w.group == id).count();
        start..start + len
    }

    /// Window `wi`'s tag in its group.
    pub fn window_tag(&self, wi: usize) -> Option<char> {
        window_tag(wi - self.group_windows(self.group_of(wi)).start)
    }

    /// The window tagged `tag` in group `gi`.
    pub fn tagged_window(&self, gi: usize, tag: char) -> Option<usize> {
        let r = self.group_windows(gi);
        tag_pos(tag).map(|p| r.start + p).filter(|wi| r.contains(wi))
    }

    pub fn add_group(&mut self, name: String, tag: char) -> GroupId {
        let id = self.groups.iter().map(|g| g.id + 1).max().unwrap_or(1);
        self.groups.push(Group { id, name, tag });
        id
    }

    /// Move window `from` one place down (or up). Past the end of its group it joins the
    /// next group; a window alone in its group takes the whole group past the next one.
    /// Returns where the window ended up.
    pub fn move_window(&mut self, from: usize, down: bool) -> Option<usize> {
        let id = self.windows.get(from)?.id;
        let gi = self.group_of(from);
        let range = self.group_windows(gi);
        let inside = if down { from + 1 < range.end } else { from > range.start };
        if inside {
            let to = if down { from + 1 } else { from - 1 };
            self.windows.swap(from, to);
            if self.active == from {
                self.active = to;
            } else if self.active == to {
                self.active = from;
            }
            return Some(to);
        }
        let next = if down { gi + 1 } else { gi.checked_sub(1)? };
        if next >= self.groups.len() {
            return None;
        }
        if range.len() == 1 {
            self.groups.swap(gi, next);
        } else {
            self.windows[from].group = self.groups[next].id;
        }
        self.normalize_groups();
        self.windows.iter().position(|w| w.id == id)
    }

    /// Put the windows back in group order (keeping each group's own order) and drop empty groups.
    pub fn normalize_groups(&mut self) {
        let shown = self.windows.get(self.active).map(|w| w.id);
        let order: Vec<GroupId> = self.groups.iter().map(|g| g.id).collect();
        let rank = |w: &Window| order.iter().position(|&g| g == w.group).unwrap_or(order.len());
        self.windows.sort_by_key(rank);
        let windows = &self.windows;
        self.groups.retain(|g| windows.iter().any(|w| w.group == g.id));
        if let Some(i) = shown.and_then(|id| self.windows.iter().position(|w| w.id == id)) {
            self.active = i;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PromptKind {
    RenameWindow(WindowId),
    RenameSession(SessionId),
    NewGroup(SessionId),
    RenameGroup(SessionId, GroupId),
    GroupTag(SessionId, GroupId),
    /// A copy-mode search in a pane (`true`: forward).
    CopySearch(PaneId, bool),
    /// A command line for the shown session (`<prefix> :`).
    Command,
    ConfirmKillWindow(WindowId),
    ConfirmKillSession(SessionId),
    ConfirmRespawnPane(PaneId),
    ConfirmRespawnWindow(WindowId),
    DockAdd(SessionId),
    DockRename(SessionId),
    ConfirmDockClose(SessionId),
    ConfirmExport(SessionId),
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
                | PromptKind::ConfirmDockClose(_)
                | PromptKind::ConfirmExport(_)
        )
    }
}

pub struct Prompt {
    pub kind: PromptKind,
    pub text: String,
    pub label: String,
    /// The history entry shown, when stepping through it with Up/Down.
    pub history: Option<usize>,
    /// Tab completion in progress.
    pub tab: Option<TabState>,
}

/// Cycling through the names a Tab press completes.
pub struct TabState {
    pub candidates: Vec<String>,
    /// The candidate shown (`None` until the second press).
    pub shown: Option<usize>,
}

/// The key list overlay.
pub struct HelpState {
    pub scroll: usize,
    /// Only lines containing this are listed (any case).
    pub query: String,
    /// Keys are being typed into the query.
    pub typing: bool,
}

/// Incremental window search in the sidebar.
pub struct Search {
    pub query: String,
    /// The window that was shown when the search began.
    pub origin: usize,
}

/// Ctrl-Tab stepping through recently opened windows.
pub struct Cycle {
    /// The order when the cycle began; `list[0]` is the window it started from.
    pub list: Vec<WindowId>,
    pub idx: usize,
}

/// A mouse drag in progress, begun by a press with the left button.
#[derive(Clone, Debug)]
pub enum Drag {
    /// Selecting text in a window pane: where the press was (absolute line, column) and the
    /// pane's text area on the terminal (x, y, width, height).
    Select { pane: PaneId, origin: (usize, usize), area: (usize, usize, usize, usize) },
    /// Moving the border of a split.
    Border { path: Vec<bool>, axis: super::layout::Axis, rect: Rect },
    SidebarEdge,
    DockEdge,
    /// Moving a window (or session, in the sessions list) in the sidebar; `at` is where it is.
    Reorder { at: usize },
}

/// The last left click, to tell double and triple clicks.
#[derive(Clone, Copy, Debug)]
pub struct Click {
    pub at: Instant,
    pub x: u16,
    pub y: u16,
    pub count: u8,
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
    pub search: Option<Search>,
    /// A group's tag was typed after the prefix: the next key is a window tag in this group.
    pub pending_group: Option<GroupId>,
    /// The recent-window cycle in progress; the shown window counts as opened once it ends.
    pub cycle: Option<Cycle>,
    /// Further recent-window presses until this moment go on with the cycle.
    pub cycle_until: Option<Instant>,
    /// The focused sidebar fills the whole terminal.
    pub sidebar_max: bool,
    /// Plain resize keys are accepted until this moment.
    pub resize_until: Option<Instant>,
    pub prev: Option<Canvas>,
    pub last_active: Instant,
    pub message: Option<(String, Instant)>,
    /// The key list overlay, while it is open.
    pub help: Option<HelpState>,
    pub drag: Option<Drag>,
    pub last_click: Option<Click>,
}

impl Client {
    /// A prompt or the help list is on screen and has the keyboard.
    pub fn overlay(&self) -> bool {
        self.prompt.is_some() || self.help.is_some()
    }

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
            search: None,
            pending_group: None,
            cycle: None,
            cycle_until: None,
            sidebar_max: false,
            resize_until: None,
            prev: None,
            last_active: Instant::now(),
            message: None,
            help: None,
            drag: None,
            last_click: None,
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
            group: 1,
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

    #[test]
    fn window_tags_are_digits_then_letters() {
        assert_eq!(window_tag(0), Some('1'));
        assert_eq!(window_tag(8), Some('9'));
        assert_eq!(window_tag(9), Some('a'));
        assert_eq!(window_tag(34), Some('z'));
        assert_eq!(window_tag(35), None);
        for pos in 0..35 {
            assert_eq!(tag_pos(window_tag(pos).unwrap()), Some(pos));
        }
        assert_eq!(tag_pos('0'), None);
        assert_eq!(tag_pos('A'), None);
    }

    pub(crate) fn session(groups: &[GroupId]) -> Session {
        let windows = groups
            .iter()
            .enumerate()
            .map(|(i, &group)| Window { id: i as WindowId + 1, group, ..window() })
            .collect();
        Session {
            id: 1,
            name: "s".into(),
            windows,
            groups: vec![
                Group { id: 1, name: "a".into(), tag: 'g' },
                Group { id: 2, name: "b".into(), tag: 'i' },
                Group { id: 3, name: "c".into(), tag: 'q' },
            ],
            active: 0,
            size: (80, 24),
            owner: None,
            cwd: PathBuf::new(),
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
        }
    }

    #[test]
    fn windows_are_tagged_by_their_place_in_the_group() {
        let s = session(&[1, 1, 2, 2, 2, 3]);
        assert_eq!(s.group_windows(0), 0..2);
        assert_eq!(s.group_windows(1), 2..5);
        assert_eq!(s.group_windows(2), 5..6);
        assert_eq!(s.group_windows(3), 0..0);
        assert_eq!((s.window_tag(1), s.window_tag(2), s.window_tag(4)), (Some('2'), Some('1'), Some('3')));
        assert_eq!(s.tagged_window(1, '3'), Some(4));
        assert_eq!(s.tagged_window(1, '4'), None);
        assert_eq!(s.tagged_window(2, '1'), Some(5));
    }

    #[test]
    fn normalizing_regroups_windows_and_drops_empty_groups() {
        let mut s = session(&[2, 1, 2, 1]);
        s.active = 2;
        s.normalize_groups();
        let order: Vec<(WindowId, GroupId)> = s.windows.iter().map(|w| (w.id, w.group)).collect();
        assert_eq!(order, vec![(2, 1), (4, 1), (1, 2), (3, 2)]);
        assert_eq!(s.windows[s.active].id, 3, "the shown window stays shown");
        assert_eq!(s.groups.iter().map(|g| g.id).collect::<Vec<_>>(), vec![1, 2]);
        assert_eq!(s.add_group("d".into(), 'u'), 3);
    }

    fn layout(s: &Session) -> Vec<(WindowId, GroupId)> {
        s.windows.iter().map(|w| (w.id, w.group)).collect()
    }

    #[test]
    fn moving_a_window_inside_its_group_swaps_it_with_the_neighbour() {
        let mut s = session(&[1, 1, 2]);
        s.active = 0;
        assert_eq!(s.move_window(0, true), Some(1));
        assert_eq!(layout(&s), vec![(2, 1), (1, 1), (3, 2)]);
        assert_eq!(s.active, 1, "the shown window moved with it");
        assert_eq!(s.move_window(1, false), Some(0));
        assert_eq!(s.move_window(0, false), None, "nothing above the first group");
    }

    #[test]
    fn moving_past_a_groups_edge_joins_the_next_group() {
        let mut s = session(&[1, 1, 2, 2]);
        assert_eq!(s.move_window(1, true), Some(1));
        assert_eq!(layout(&s), vec![(1, 1), (2, 2), (3, 2), (4, 2)]);
        assert_eq!(s.move_window(1, false), Some(1));
        assert_eq!(layout(&s), vec![(1, 1), (2, 1), (3, 2), (4, 2)]);
        assert_eq!(s.move_window(3, true), None, "nothing below the last group");
    }

    #[test]
    fn a_window_alone_in_its_group_moves_the_whole_group() {
        let mut s = session(&[1, 2, 2]);
        assert_eq!(s.move_window(0, true), Some(2));
        assert_eq!(layout(&s), vec![(2, 2), (3, 2), (1, 1)]);
        assert_eq!(s.groups.iter().map(|g| g.id).collect::<Vec<_>>(), vec![2, 1]);
    }

    #[test]
    fn an_emptied_group_goes_away() {
        let mut s = session(&[1, 2]);
        s.windows[1].group = 1;
        s.normalize_groups();
        assert_eq!(s.groups.len(), 1);
    }
}
