//! Mouse handling: sidebar clicks, pane focus, wheel scrolling, and
//! forwarding to applications that asked for mouse reports.

use std::time::{Duration, Instant};

use super::copy::{word_at, CopyMode};
use super::layout::{PaneId, Rect};
use super::sidebar::{self, Line};
use super::state::{Click, ClientId, Drag};
use super::Server;
use crate::config::{Position, SidebarMode};
use crate::keys::{MouseEvent, MouseKind, ALT, CTRL, SHIFT};
use crate::term::screen::MouseTracking;

/// Clicks this close together (in time) on the same cell are a double or triple click.
const MULTI_CLICK: Duration = Duration::from_millis(400);

/// Lines scrolled per wheel notch.
const WHEEL_LINES: isize = 3;

/// Encode a mouse report for an application, using pane-relative 0-based coordinates.
pub fn encode_mouse(m: &MouseEvent, rx: usize, ry: usize, sgr: bool) -> Option<Vec<u8>> {
    let mods = (if m.mods & SHIFT != 0 { 4 } else { 0 })
        | (if m.mods & ALT != 0 { 8 } else { 0 })
        | (if m.mods & CTRL != 0 { 16 } else { 0 });
    let b = mods
        + match m.kind {
            MouseKind::Press => m.button as usize,
            MouseKind::Release => if sgr { m.button as usize } else { 3 },
            MouseKind::Drag => m.button as usize + 32,
            MouseKind::Move => 35,
            MouseKind::WheelUp => 64,
            MouseKind::WheelDown => 65,
        };
    if sgr {
        let end = if m.kind == MouseKind::Release { 'm' } else { 'M' };
        Some(format!("\x1b[<{};{};{}{}", b, rx + 1, ry + 1, end).into_bytes())
    } else if rx + 33 > 255 || ry + 33 > 255 || b + 32 > 255 {
        None
    } else {
        Some(vec![0x1b, b'[', b'M', (32 + b) as u8, (33 + rx) as u8, (33 + ry) as u8])
    }
}

/// The first screen row below a dock pane that starts under `top` header rows.
fn dock_pane_rows_end(pane_rows: usize, top: usize) -> usize {
    top + 1 + pane_rows
}

impl Server {
    pub fn handle_mouse(&mut self, cid: ClientId, m: MouseEvent) {
        let Some(c) = self.clients.get(&cid) else { return };
        let Some(si) = c.session.and_then(|sid| self.sessions.iter().position(|s| s.id == sid)) else { return };
        if c.sidebar_max && c.sidebar_focus.is_some() {
            return self.sidebar_click(cid, si, m);
        }
        if let Some(p) = self.sessions[si].maximized.filter(|p| self.panes.contains_key(p)) {
            self.pane_mouse(p, &m, m.x as usize, m.y as usize);
            self.mark_dirty();
            return;
        }
        // A drag in progress gets its motion and release wherever the pointer is.
        if c.drag.is_some() {
            match m.kind {
                MouseKind::Drag | MouseKind::Release => return self.drag_mouse(cid, si, m),
                MouseKind::Press => {
                    if let Some(c) = self.clients.get_mut(&cid) {
                        c.drag = None;
                    }
                }
                _ => {}
            }
        }
        let Some(c) = self.clients.get(&cid) else { return };
        let left_press = m.kind == MouseKind::Press && m.button == 0;
        let sw = self.sidebar_width(c) as usize;
        let dw = self.dock_width(c, &self.sessions[si]) as usize;
        let cols = c.cols as usize;
        let (content_x, sidebar_x, dock_x) = match self.cfg.sidebar.position {
            Position::Left => (sw, 0, cols - dw),
            Position::Right => (dw, cols - sw, 0),
        };
        let cw = cols - sw - dw;
        let (x, y) = (m.x as usize, m.y as usize);
        let left_side = self.cfg.sidebar.position == Position::Left;
        if sw > 0 && x >= sidebar_x && x < sidebar_x + sw {
            // The edge facing the panes drags the sidebar's width.
            let edge = if left_side { sidebar_x + sw - 1 } else { sidebar_x };
            if left_press && x == edge {
                return self.start_drag(cid, Drag::SidebarEdge);
            }
            return self.sidebar_click(cid, si, m);
        }
        if self.sessions[si].shown_popup().is_some() {
            return self.popup_mouse(si, content_x, m);
        }
        if dw > 0 && x >= dock_x && x < dock_x + dw {
            let edge = if left_side { dock_x } else { dock_x + dw - 1 };
            if left_press && x == edge {
                return self.start_drag(cid, Drag::DockEdge);
            }
            return self.dock_mouse(si, dock_x, dw, m);
        }
        if x < content_x || x >= content_x + cw {
            return;
        }
        let (cx, cy) = (x - content_x, y);
        let s = &self.sessions[si];
        if cx >= s.size.0 as usize || cy >= s.size.1 as usize {
            return;
        }
        if left_press && !s.active_window().zoomed {
            let whole = Rect::new(0, 0, s.size.0, s.size.1);
            if let Some(b) = s.active_window().root.hit_border(whole, self.spacing(), cx as u16, cy as u16) {
                return self.start_drag(cid, Drag::Border { path: b.path, axis: b.axis, rect: b.rect });
            }
        }
        let hit = s.active_window().rects(s.size, self.spacing()).into_iter().find(|(_, r)| {
            r.w > 0
                && r.h > 0
                && (r.x as usize..(r.x + r.w) as usize).contains(&cx)
                && (r.y as usize..(r.y + r.h) as usize).contains(&cy)
        });
        let Some((pane_id, r)) = hit else { return };
        let inner = self.pane_area(r);
        if m.kind == MouseKind::Press {
            self.sessions[si].dock.focused = false;
            if self.sessions[si].active_window().active != pane_id {
                self.sessions[si].active_window_mut().focus(pane_id);
                let wi = self.sessions[si].active;
                self.refresh_name(si, wi);
            }
        }
        // Over a box border (islands style) there is nothing to forward to the program.
        let (ix, iy) = (inner.x as usize, inner.y as usize);
        if cx >= ix && cy >= iy && cx < ix + inner.w as usize && cy < iy + inner.h as usize {
            self.pane_mouse(pane_id, &m, cx - ix, cy - iy);
            if left_press {
                let area = (content_x + ix, iy, inner.w as usize, inner.h as usize);
                self.begin_select(cid, pane_id, area, (cx - ix, cy - iy), &m);
            }
        }
        self.mark_dirty();
    }

    fn start_drag(&mut self, cid: ClientId, drag: Drag) {
        if let Some(c) = self.clients.get_mut(&cid) {
            c.drag = Some(drag);
        }
        self.mark_dirty();
    }

    /// A left press in a window pane: it may turn into a text selection (when the program in
    /// the pane does not take the mouse itself), and a double or triple click selects a word or line.
    fn begin_select(&mut self, cid: ClientId, pane_id: PaneId, area: (usize, usize, usize, usize), (rx, ry): (usize, usize), m: &MouseEvent) {
        let Some(p) = self.panes.get(&pane_id) else { return };
        if p.screen.modes().mouse != MouseTracking::Off && p.copy.is_none() {
            return;
        }
        let top = p.copy.as_ref().map_or(p.screen.history_len(), |c| c.top(&p.screen));
        let origin = (top + ry, rx);
        let now = Instant::now();
        let Some(c) = self.clients.get_mut(&cid) else { return };
        let count = match c.last_click {
            Some(k) if now.duration_since(k.at) < MULTI_CLICK && (k.x, k.y) == (m.x, m.y) => k.count % 3 + 1,
            _ => 1,
        };
        c.last_click = Some(Click { at: now, x: m.x, y: m.y, count });
        c.drag = Some(Drag::Select { pane: pane_id, origin, area });
        if count >= 2 {
            self.select_unit(pane_id, origin, count);
        }
    }

    /// Select the word (`count` 2) or the line (3) at `origin`, in copy mode.
    fn select_unit(&mut self, pane_id: PaneId, origin: (usize, usize), count: u8) {
        let Some(p) = self.panes.get_mut(&pane_id) else { return };
        let screen = &p.screen;
        let copy = p.copy.get_or_insert_with(|| CopyMode::new(screen));
        copy.rect = false;
        if count == 2 {
            let (first, last) = word_at(screen, origin.0, origin.1);
            copy.anchor = Some((origin.0, first));
            copy.line_mode = false;
            copy.reveal(screen, origin.0, last, false);
        } else {
            copy.anchor = Some((origin.0, 0));
            copy.line_mode = true;
            copy.reveal(screen, origin.0, origin.1, false);
        }
    }

    /// The pointer moved with the button held.
    fn drag_mouse(&mut self, cid: ClientId, si: usize, m: MouseEvent) {
        let Some(drag) = self.clients.get(&cid).and_then(|c| c.drag.clone()) else { return };
        let release = m.kind == MouseKind::Release;
        if release && let Some(c) = self.clients.get_mut(&cid) {
            c.drag = None;
        }
        match drag {
            Drag::Select { pane, origin, area } => {
                if !release {
                    self.drag_select(pane, origin, area, &m);
                } else if self.panes.get(&pane).and_then(|p| p.copy.as_ref()).is_some_and(|c| c.anchor.is_some()) {
                    self.message(cid, "y copies the selection");
                }
            }
            Drag::Border { path, axis, rect } if !release => {
                let Some(c) = self.clients.get(&cid) else { return };
                let content_x = self.content_x(c, &self.sessions[si]);
                let (cx, cy) = ((m.x as usize).saturating_sub(content_x) as u16, m.y);
                let first_len = match axis {
                    super::layout::Axis::Cols => cx.saturating_sub(rect.x),
                    super::layout::Axis::Rows => cy.saturating_sub(rect.y),
                };
                let (sp, size, sid) = (self.spacing(), self.sessions[si].size, self.sessions[si].id);
                let whole = Rect::new(0, 0, size.0, size.1);
                if self.sessions[si].active_window_mut().root.set_first_len(&path, first_len, whole, sp) {
                    self.relayout(sid);
                }
            }
            Drag::SidebarEdge if !release => {
                let Some(c) = self.clients.get(&cid) else { return };
                let w = if self.cfg.sidebar.position == Position::Left { m.x + 1 } else { c.cols.saturating_sub(m.x) };
                self.set_sidebar_width(cid, si, w);
            }
            Drag::DockEdge if !release => {
                let Some(c) = self.clients.get(&cid) else { return };
                let w = if self.cfg.sidebar.position == Position::Left { c.cols.saturating_sub(m.x) } else { m.x + 1 };
                self.set_dock_width(cid, si, w);
            }
            Drag::Reorder { at } if !release => self.drag_reorder(cid, si, at, m.y as usize),
            _ => {}
        }
        self.mark_dirty();
    }

    /// Extend the selection of a drag that began at `origin` to the pointer, scrolling the pane
    /// while the pointer is above or below its text.
    fn drag_select(&mut self, pane_id: PaneId, origin: (usize, usize), area: (usize, usize, usize, usize), m: &MouseEvent) {
        let Some(p) = self.panes.get_mut(&pane_id) else { return };
        let screen = &p.screen;
        let copy = p.copy.get_or_insert_with(|| CopyMode::new(screen));
        let (ax, ay, aw, ah) = area;
        let (x, y) = (m.x as usize, m.y as usize);
        if y < ay {
            copy.scroll(1, screen);
        } else if y >= ay + ah {
            copy.scroll(-1, screen);
        }
        copy.anchor = Some(origin);
        copy.line_mode = false;
        copy.rect = false;
        copy.cx = x.saturating_sub(ax).min(aw.saturating_sub(1));
        copy.cy = y.saturating_sub(ay).min(ah.saturating_sub(1));
        copy.clamp(screen);
    }

    /// The terminal column where the session's panes start.
    fn content_x(&self, c: &super::state::Client, s: &super::state::Session) -> usize {
        match self.cfg.sidebar.position {
            Position::Left => self.sidebar_width(c) as usize,
            Position::Right => self.dock_width(c, s) as usize,
        }
    }

    /// Move the dragged window (or session) one place towards the row the pointer is on.
    fn drag_reorder(&mut self, cid: ClientId, si: usize, at: usize, y: usize) {
        let Some(c) = self.clients.get(&cid) else { return };
        if c.search.is_some() {
            return;
        }
        let mode = c.sidebar_mode;
        let Some((_, target)) = self.sidebar_target(cid, si, y) else { return };
        if target == at {
            return;
        }
        let sid = self.sessions[si].id;
        // One place at a time until the entry is where the pointer is. A window that joins
        // another group keeps its index for a step, so allow for that.
        let mut at = at;
        for _ in 0..2 * self.sessions[si].windows.len().max(self.sessions.len()) + 2 {
            if at == target {
                break;
            }
            let down = target > at;
            let moved = match mode {
                SidebarMode::Windows => self.move_window(sid, at, down),
                SidebarMode::Sessions => {
                    let to = if down { at + 1 } else { at.wrapping_sub(1) };
                    (to < self.sessions.len()).then(|| self.sessions.swap(at, to)).map(|_| to)
                }
            };
            match moved {
                Some(now) => at = now,
                None => break,
            }
        }
        if let Some(c) = self.clients.get_mut(&cid) {
            c.drag = Some(Drag::Reorder { at });
            if c.sidebar_focus.is_some() {
                c.sidebar_focus = Some(at);
            }
        }
    }

    /// The sidebar row, and the window or session index it lists, drawn on screen row `y`.
    fn sidebar_target(&self, cid: ClientId, si: usize, y: usize) -> Option<(usize, usize)> {
        let c = self.clients.get(&cid)?;
        let s = &self.sessions[si];
        let rows = self.sidebar_rows(c, s);
        let lines = self.sidebar_lines(c, s);
        let active_row = match c.sidebar_mode {
            SidebarMode::Windows => rows.iter().position(|&i| i == s.active).unwrap_or(0),
            SidebarMode::Sessions => si,
        };
        let active = lines.iter().position(|&l| l == Line::Row(active_row)).unwrap_or(0);
        let has_footer = self.footer_visible(c);
        let at = sidebar::item_at(lines.len(), active, c.rows as usize, has_footer, self.cfg.islands, y)?;
        let row = match lines[at] {
            Line::Row(row) => row,
            Line::Gap => return None,
            // A group's heading stands for its first listed window.
            Line::Group(gi) => rows.iter().position(|&i| s.group_of(i) == gi)?,
        };
        Some((row, *rows.get(row)?))
    }

    /// Events over the popup go to it; everything else is ignored while it is up.
    fn popup_mouse(&mut self, si: usize, content_x: usize, m: MouseEvent) {
        let s = &self.sessions[si];
        let Some(pane) = s.shown_popup() else { return };
        let inner = self.popup_inner(s.size);
        let (x, y) = (m.x as usize, m.y as usize);
        let (ix, iy) = (content_x + inner.x as usize, inner.y as usize);
        if x >= ix && y >= iy && x < ix + inner.w as usize && y < iy + inner.h as usize {
            self.pane_mouse(pane, &m, x - ix, y - iy);
            self.mark_dirty();
        }
    }

    fn dock_mouse(&mut self, si: usize, dock_x: usize, dw: usize, m: MouseEvent) {
        let sep_left = self.cfg.sidebar.position == Position::Left;
        let islands = self.cfg.islands;
        let mut px = if sep_left { dock_x + 1 } else { dock_x };
        let mut pw = dw - 1;
        // In islands style the box's border surrounds the tab row and the pane.
        let top = usize::from(islands);
        if islands {
            px += 1;
            pw = pw.saturating_sub(2);
        }
        let (x, y) = (m.x as usize, m.y as usize);
        if x < px || x >= px + pw {
            return;
        }
        let dock = &self.sessions[si].dock;
        if y < top {
            return;
        }
        if y == top {
            if m.kind == MouseKind::Press && m.button == 0 {
                let names: Vec<&str> = dock.entries.iter().map(|e| e.name.as_str()).collect();
                let hit = super::render::dock_tabs(&names, dock.active, pw)
                    .into_iter()
                    .find(|&(_, x0, x1)| (px + x0..px + x1).contains(&x))
                    .map(|t| t.0);
                if let Some(i) = hit {
                    let d = &mut self.sessions[si].dock;
                    d.active = i;
                    d.focused = true;
                    self.mark_dirty();
                }
            }
            return;
        }
        let Some(pane) = self.sessions[si].shown_dock_pane() else { return };
        if m.kind == MouseKind::Press {
            self.sessions[si].dock.focused = true;
        }
        if y >= dock_pane_rows_end(self.sessions[si].dock_size.1 as usize, top) {
            return;
        }
        self.pane_mouse(pane, &m, x - px, y - top - 1);
        self.mark_dirty();
    }

    fn pane_mouse(&mut self, pane_id: PaneId, m: &MouseEvent, rx: usize, ry: usize) {
        let Some(p) = self.panes.get_mut(&pane_id) else { return };
        let modes = p.screen.modes();
        let wheel = matches!(m.kind, MouseKind::WheelUp | MouseKind::WheelDown);
        let up = m.kind == MouseKind::WheelUp;
        if let Some(copy) = p.copy.as_mut() {
            if wheel {
                let live = copy.scroll(if up { WHEEL_LINES } else { -WHEEL_LINES }, &p.screen);
                if live && !up {
                    p.copy = None;
                }
            } else if m.kind == MouseKind::Press && m.button == 0 {
                copy.cx = rx;
                copy.cy = ry;
                copy.clamp(&p.screen);
            }
            return;
        }
        let wanted = match m.kind {
            MouseKind::Move => modes.mouse == MouseTracking::Any,
            MouseKind::Drag => matches!(modes.mouse, MouseTracking::Drag | MouseTracking::Any),
            _ => modes.mouse != MouseTracking::Off,
        };
        if wanted {
            if let Some(bytes) = encode_mouse(m, rx, ry, modes.mouse_sgr) {
                p.pty.write(bytes);
            }
        } else if wheel && modes.mouse == MouseTracking::Off {
            if modes.alt_screen {
                // Full-screen apps without mouse support: scroll with arrow keys.
                let key: &[u8] = match (up, modes.app_cursor) {
                    (true, true) => b"\x1bOA",
                    (true, false) => b"\x1b[A",
                    (false, true) => b"\x1bOB",
                    (false, false) => b"\x1b[B",
                };
                p.pty.write(key.repeat(WHEEL_LINES as usize));
            } else if up {
                let mut copy = CopyMode::new(&p.screen);
                copy.scroll(WHEEL_LINES, &p.screen);
                p.copy = Some(copy);
            }
        }
    }

    fn sidebar_click(&mut self, cid: ClientId, si: usize, m: MouseEvent) {
        if m.kind != MouseKind::Press || m.button != 0 {
            return;
        }
        let Some((row, idx)) = self.sidebar_target(cid, si, m.y as usize) else { return };
        let Some(mode) = self.clients.get(&cid).map(|c| c.sidebar_mode) else { return };
        if let Some(c) = self.clients.get_mut(&cid)
            && c.sidebar_focus.is_some() {
                c.sidebar_focus = Some(row);
            }
        match mode {
            SidebarMode::Windows => {
                let sid = self.sessions[si].id;
                self.open_window(sid, idx);
            }
            SidebarMode::Sessions => {
                let id = self.sessions[idx].id;
                self.switch_client(cid, id);
                self.focus_shown(id);
            }
        }
        // Dragging on from the press moves the entry (not while searching).
        if let Some(c) = self.clients.get_mut(&cid)
            && c.search.is_none()
        {
            c.drag = Some(Drag::Reorder { at: idx });
        }
        self.mark_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(kind: MouseKind, button: u8, mods: u8) -> MouseEvent {
        MouseEvent { kind, button, x: 0, y: 0, mods }
    }

    #[test]
    fn encodes_sgr() {
        assert_eq!(encode_mouse(&ev(MouseKind::Press, 0, 0), 4, 9, true).unwrap(), b"\x1b[<0;5;10M");
        assert_eq!(encode_mouse(&ev(MouseKind::Release, 2, 0), 0, 0, true).unwrap(), b"\x1b[<2;1;1m");
        assert_eq!(encode_mouse(&ev(MouseKind::WheelUp, 0, 0), 1, 1, true).unwrap(), b"\x1b[<64;2;2M");
        assert_eq!(encode_mouse(&ev(MouseKind::Drag, 0, CTRL), 0, 0, true).unwrap(), b"\x1b[<48;1;1M");
    }

    #[test]
    fn encodes_legacy_x10() {
        assert_eq!(encode_mouse(&ev(MouseKind::Press, 0, 0), 0, 0, false).unwrap(), b"\x1b[M !!");
        assert_eq!(encode_mouse(&ev(MouseKind::Release, 1, 0), 2, 3, false).unwrap(), b"\x1b[M##$");
        assert!(encode_mouse(&ev(MouseKind::Press, 0, 0), 300, 0, false).is_none());
    }
}
