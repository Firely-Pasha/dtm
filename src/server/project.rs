//! Project sessions: lazily started windows, respawn and describing a session
//! as a project file.

use std::path::{Path, PathBuf};

use super::layout::{Axis, Node, PaneId, Rect};
use super::state::{ClientId, Dock, DockEntry, PaneSpec, Window};
use super::Server;
use crate::project::{self, Plan, PlanAxis, PlanChild, PlanDock, PlanDockPane, PlanNode, PlanWindow, Size};
use crate::protocol::{ServerMsg, Snapshot};

/// A pane to start: id, working directory, command and whether it takes focus.
type Leaf = (PaneId, PathBuf, Option<String>, bool);

fn axis_of(a: PlanAxis) -> Axis {
    match a {
        PlanAxis::Cols => Axis::Cols,
        PlanAxis::Rows => Axis::Rows,
    }
}

impl Server {
    // ---- opening ----

    pub(super) fn open_project_msg(
        &mut self,
        cid: ClientId,
        plan: Plan,
        source: PathBuf,
        cols: u16,
        rows: u16,
    ) {
        let Some(c) = self.clients.get_mut(&cid) else { return };
        if c.session.is_some() {
            return;
        }
        c.cols = cols.max(1);
        c.rows = rows.max(1);
        c.sidebar_visible = self.cfg.sidebar.visible;
        c.sidebar_mode = self.cfg.sidebar.default_mode;
        if let Some(v) = plan.sidebar_visible {
            c.sidebar_visible = v;
        }
        let running = self.sessions.iter().find(|s| s.name == plan.name).map(|s| s.id);
        let result = match running {
            Some(sid) => Ok(sid),
            None => self.create_project_session(cid, &plan, source),
        };
        match result {
            Ok(sid) => self.switch_client(cid, sid),
            Err(e) => self.send(cid, ServerMsg::Error(e)),
        }
    }

    fn default_label(&self, tree: &PlanNode) -> String {
        let first = tree.leaves().first().and_then(|l| l.1.and_then(|c| c.split_whitespace().next())).map(str::to_string);
        first.unwrap_or_else(|| {
            Path::new(&self.cfg.shell).file_name().map_or_else(|| "sh".into(), |n| n.to_string_lossy().into_owned())
        })
    }

    fn create_project_session(&mut self, cid: ClientId, plan: &Plan, source: PathBuf) -> Result<u32, String> {
        let size = self.content_size(cid);
        let windows: Vec<Window> = plan
            .windows
            .iter()
            .map(|w| Window {
                id: self.alloc_window_id(),
                pinned_name: w.name.clone(),
                auto_name: self.default_label(&w.tree),
                root: Node::Leaf(0),
                active: 0,
                last: None,
                zoomed: false,
                activity: false,
                bell: false,
                plan: Some(w.tree.clone()),
                pending: true,
            })
            .collect();
        let id = self.alloc_session_id();
        self.sessions.push(super::state::Session {
            id,
            name: plan.name.clone(),
            windows,
            active: plan.focus.min(plan.windows.len().saturating_sub(1)),
            size,
            owner: None,
            cwd: plan.root.clone(),
            project: Some(source),
            pane_command: plan.pane_command.clone(),
            dock: plan.dock.as_ref().map_or_else(Dock::default, |d| Dock {
                entries: d
                    .panes
                    .iter()
                    .map(|p| DockEntry {
                        name: p.name.clone(),
                        spec: PaneSpec { cwd: Some(p.cwd.clone()), command: p.command.clone() },
                        pane: None,
                    })
                    .collect(),
                active: 0,
                width: d.width,
                visible: d.visible,
                focused: false,
            }),
            dock_size: (1, 1),
            popup: None,
            popup_shown: false,
            sidebar_w: plan.sidebar_width,
            recent: Vec::new(),
            maximized: None,
            full_size: (0, 0),
        });
        self.had_session = true;
        let si = self.sessions.len() - 1;
        let eager: Vec<usize> =
            plan.windows.iter().enumerate().filter(|(_, w)| !w.lazy).map(|(i, _)| i).collect();
        for wi in eager {
            if let Err(e) = self.start_window(si, wi) {
                self.remove_session(id);
                return Err(e);
            }
        }
        Ok(id)
    }

    // ---- starting windows ----

    /// Start the pending window every attached client is looking at.
    pub fn start_visible_windows(&mut self) {
        self.start_visible_dock();
        let mut jobs: Vec<(usize, usize)> = Vec::new();
        for (si, s) in self.sessions.iter().enumerate() {
            if s.windows.is_empty() || !self.clients.values().any(|c| c.session == Some(s.id)) {
                continue;
            }
            if s.active_window().pending {
                jobs.push((si, s.active));
            }
        }
        for (si, wi) in jobs {
            if let Err(e) = self.start_window(si, wi) {
                // Give up on the plan, or this would be retried on every frame.
                let w = &mut self.sessions[si].windows[wi];
                w.plan = None;
                w.pending = false;
                let sid = self.sessions[si].id;
                let viewers: Vec<ClientId> =
                    self.clients.values().filter(|c| c.session == Some(sid)).map(|c| c.id).collect();
                for cid in viewers {
                    self.message(cid, e.clone());
                }
            }
        }
    }

    fn build_tree(&mut self, node: &PlanNode, size: (u16, u16), leaves: &mut Vec<Leaf>) -> Node {
        match node {
            PlanNode::Pane { cwd, command, focus } => {
                let id = self.alloc_pane_id();
                leaves.push((id, cwd.clone(), command.clone(), *focus));
                Node::Leaf(id)
            }
            PlanNode::Split { axis, children } => {
                let axis = axis_of(*axis);
                let total = if axis == Axis::Cols { size.0 } else { size.1 };
                let sizes: Vec<Option<Size>> = children.iter().map(|c| c.size).collect();
                let fr = project::fractions(&sizes, total);
                self.fold(children, &fr, axis, size, leaves)
            }
        }
    }

    /// n children as nested binary splits, each taking its share of what is left.
    fn fold(
        &mut self,
        children: &[PlanChild],
        fr: &[f32],
        axis: Axis,
        size: (u16, u16),
        leaves: &mut Vec<Leaf>,
    ) -> Node {
        if children.len() == 1 {
            return self.build_tree(&children[0].node, size, leaves);
        }
        let ratio = (fr[0] / fr.iter().sum::<f32>()).clamp(0.02, 0.98);
        let total = if axis == Axis::Cols { size.0 } else { size.1 };
        let avail = total.saturating_sub(1);
        let alen = ((f32::from(avail) * ratio).round() as u16).min(avail);
        let (asize, bsize) = match axis {
            Axis::Cols => ((alen, size.1), (avail - alen, size.1)),
            Axis::Rows => ((size.0, alen), (size.0, avail - alen)),
        };
        let a = self.build_tree(&children[0].node, asize, leaves);
        let b = self.fold(&children[1..], &fr[1..], axis, bsize, leaves);
        Node::Split { axis, ratio, a: Box::new(a), b: Box::new(b) }
    }

    /// Spawn the panes of a pending window and type their commands.
    pub fn start_window(&mut self, si: usize, wi: usize) -> Result<(), String> {
        let w = &self.sessions[si].windows[wi];
        if !w.pending {
            return Ok(());
        }
        let plan = w.plan.clone().ok_or("window has no layout")?;
        let size = self.sessions[si].size;
        let mut leaves = Vec::new();
        let tree = self.build_tree(&plan, size, &mut leaves);
        let mut rects = Vec::new();
        tree.layout(Rect::new(0, 0, size.0, size.1), &mut rects);
        let mut started: Vec<PaneId> = Vec::new();
        for (id, cwd, _, _) in &leaves {
            let r = rects.iter().find(|(p, _)| p == id).map(|(_, r)| *r).unwrap_or(Rect::new(0, 0, 1, 1));
            if let Err(e) = self.spawn_pane(*id, Some(cwd.clone()), (r.w, r.h)) {
                for p in started {
                    self.panes.remove(&p);
                }
                return Err(format!("{}: {e}", cwd.display()));
            }
            started.push(*id);
        }
        let active = leaves.iter().find(|l| l.3).or(leaves.first()).map(|l| l.0).unwrap_or(0);
        let w = &mut self.sessions[si].windows[wi];
        w.root = tree;
        w.active = active;
        w.last = None;
        w.zoomed = false;
        w.pending = false;
        let sid = self.sessions[si].id;
        for (id, _, command, _) in leaves {
            self.setup_pane(sid, id, command);
        }
        self.relayout(sid);
        self.refresh_name(si, wi);
        self.mark_dirty();
        Ok(())
    }

    // ---- respawn ----

    /// Replace a pane with a fresh process started the same way.
    pub fn respawn_pane(&mut self, pane: PaneId) -> Result<(), String> {
        let old = self.panes.get(&pane).ok_or("no such pane")?;
        let (spec, size) = (old.spec.clone(), (old.screen.cols() as u16, old.screen.rows() as u16));
        if let Some((si, extra)) = self.extra_of(pane) {
            let new = self.alloc_pane_id();
            self.spawn_pane(new, spec.cwd.clone(), size)?;
            let s = &mut self.sessions[si];
            match extra {
                super::state::Extra::Popup => s.popup = Some(new),
                super::state::Extra::Dock(i) => s.dock.entries[i].pane = Some(new),
            }
            self.panes.remove(&pane);
            let sid = self.sessions[si].id;
            self.setup_pane(sid, new, spec.command);
            self.mark_dirty();
            return Ok(());
        }
        let (si, wi) = self.locate_pane(pane).ok_or("no such pane")?;
        let new = self.alloc_pane_id();
        self.spawn_pane(new, spec.cwd.clone(), size)?;
        let w = &mut self.sessions[si].windows[wi];
        w.root.replace(pane, new);
        if w.active == pane {
            w.active = new;
        }
        if w.last == Some(pane) {
            w.last = Some(new);
        }
        // Dropping the old pane hangs up its terminal; its exit event is then ignored.
        self.panes.remove(&pane);
        let sid = self.sessions[si].id;
        self.setup_pane(sid, new, spec.command);
        self.refresh_name(si, wi);
        self.mark_dirty();
        Ok(())
    }

    /// Restart a window: from its project layout, or pane by pane.
    pub fn respawn_window(&mut self, si: usize, wi: usize) -> Result<(), String> {
        let w = &self.sessions[si].windows[wi];
        if w.pending {
            return Ok(());
        }
        if w.plan.is_none() {
            for p in w.root.leaves() {
                self.respawn_pane(p)?;
            }
            return Ok(());
        }
        for p in w.root.leaves() {
            self.panes.remove(&p);
        }
        let w = &mut self.sessions[si].windows[wi];
        w.root = Node::Leaf(0);
        w.active = 0;
        w.last = None;
        w.pending = true;
        self.start_window(si, wi)
    }

    // ---- describing a session ----

    fn plan_of(&self, node: &Node, active: PaneId, root: &Path) -> PlanNode {
        match node {
            Node::Leaf(id) => {
                let pane = self.panes.get(id);
                let cwd = pane
                    .and_then(|p| p.pty.cwd().or_else(|| p.spec.cwd.clone()))
                    .unwrap_or_else(|| root.to_path_buf());
                let command = pane.and_then(|p| p.spec.command.clone().or_else(|| p.pty.foreground_cmdline()));
                PlanNode::Pane { cwd, command, focus: *id == active }
            }
            Node::Split { axis, .. } => {
                let mut parts: Vec<(f32, &Node)> = Vec::new();
                collect_chain(node, *axis, 1.0, &mut parts);
                PlanNode::Split {
                    axis: match axis {
                        Axis::Cols => PlanAxis::Cols,
                        Axis::Rows => PlanAxis::Rows,
                    },
                    children: parts
                        .into_iter()
                        .map(|(f, n)| PlanChild {
                            size: Some(Size::Percent(f * 100.0)),
                            node: self.plan_of(n, active, root),
                        })
                        .collect(),
                }
            }
        }
    }

    pub fn snapshot_session(&self, si: usize) -> Snapshot {
        let s = &self.sessions[si];
        let mut root = s.cwd.clone();
        if root.as_os_str().is_empty() {
            root = self.panes.get(&s.active_window().active).and_then(|p| p.pty.cwd()).unwrap_or_default();
        }
        let windows = s
            .windows
            .iter()
            .map(|w| PlanWindow {
                name: w.pinned_name.clone(),
                lazy: true,
                tree: match (&w.plan, w.pending) {
                    (Some(plan), true) => plan.clone(),
                    _ => self.plan_of(&w.root, w.active, &root),
                },
            })
            .collect();
        let dock = (!s.dock.entries.is_empty()).then(|| PlanDock {
            width: s.dock.width,
            visible: s.dock.visible,
            panes: s
                .dock
                .entries
                .iter()
                .map(|e| {
                    let pane = e.pane.and_then(|p| self.panes.get(&p));
                    PlanDockPane {
                        name: e.name.clone(),
                        cwd: pane
                            .and_then(|p| p.pty.cwd())
                            .or_else(|| e.spec.cwd.clone())
                            .unwrap_or_else(|| root.clone()),
                        command: e.spec.command.clone().or_else(|| pane.and_then(|p| p.pty.foreground_cmdline())),
                    }
                })
                .collect(),
        });
        Snapshot {
            plan: Plan { name: s.name.clone(), root, windows, focus: s.active, pane_command: s.pane_command.clone(), dock, sidebar_width: s.sidebar_w, sidebar_visible: None },
            source: s.project.clone(),
        }
    }

    /// `dtm export`: pick the session by name, by pane, or the only one.
    pub fn snapshot(&self, session: Option<&str>, pane: Option<u32>) -> Result<Snapshot, String> {
        let si = match (session, pane) {
            (Some(n), _) => self.sessions.iter().position(|s| s.name == n).ok_or(format!("no such session: {n}"))?,
            (None, Some(p)) if self.locate_pane(p).is_some() => self.locate_pane(p).map(|l| l.0).unwrap_or(0),
            _ if self.sessions.len() == 1 => 0,
            _ if self.sessions.is_empty() => return Err("no sessions".into()),
            _ => return Err("several sessions are running: pick one with -t".into()),
        };
        Ok(self.snapshot_session(si))
    }

    /// The `export-project` action: write the client's session to its project file.
    /// The file an export of this snapshot writes.
    pub fn export_target(snap: &Snapshot) -> Result<PathBuf, String> {
        match &snap.source {
            Some(p) => Ok(p.clone()),
            None if snap.plan.root.as_os_str().is_empty() => Err("session has no directory to export to".into()),
            None => Ok(snap.plan.root.join(project::PROJECT_FILE)),
        }
    }

    pub fn export_project(&mut self, si: usize) -> Result<PathBuf, String> {
        let snap = self.snapshot_session(si);
        let target = Self::export_target(&snap)?;
        project::export_to(&snap.plan, &target, &project::home_dir())?;
        self.sessions[si].project = Some(target.clone());
        Ok(target)
    }
}

/// Collect the children of a chain of splits on the same axis, with the share each takes.
fn collect_chain<'a>(node: &'a Node, axis: Axis, share: f32, out: &mut Vec<(f32, &'a Node)>) {
    match node {
        Node::Split { axis: a, ratio, a: left, b: right } if *a == axis => {
            collect_chain(left, axis, share * ratio, out);
            collect_chain(right, axis, share * (1.0 - ratio), out);
        }
        other => out.push((share, other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chains_flatten_with_shares() {
        let t = Node::Split {
            axis: Axis::Cols,
            ratio: 0.5,
            a: Box::new(Node::Leaf(1)),
            b: Box::new(Node::Split {
                axis: Axis::Cols,
                ratio: 0.5,
                a: Box::new(Node::Leaf(2)),
                b: Box::new(Node::Split {
                    axis: Axis::Rows,
                    ratio: 0.5,
                    a: Box::new(Node::Leaf(3)),
                    b: Box::new(Node::Leaf(4)),
                }),
            }),
        };
        let mut out = Vec::new();
        collect_chain(&t, Axis::Cols, 1.0, &mut out);
        let shares: Vec<f32> = out.iter().map(|o| o.0).collect();
        assert_eq!(shares, vec![0.5, 0.25, 0.25]);
        assert!(matches!(out[2].1, Node::Split { axis: Axis::Rows, .. }));
    }
}
