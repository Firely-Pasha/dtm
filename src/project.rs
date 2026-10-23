//! Project files (`project.dtm.yaml`): a session's windows, pane layout, paths
//! and start commands, plus converting a running session back into one.

use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const PROJECT_FILE: &str = "project.dtm.yaml";

// ---- the file as written by people ----

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProjectFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lazy: Option<bool>,
    /// Typed into every pane's shell before the pane's own command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dock: Option<DockSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sidebar: Option<SidebarSpec>,
    pub windows: Vec<WindowSpec>,
}

/// Sidebar settings for this project's session.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SidebarSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
}

/// The side panel of session-wide panes.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DockSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<SizeSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub visible: Option<bool>,
    pub panes: Vec<DockPaneSpec>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DockPaneSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WindowSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lazy: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub layout: Option<NodeSpec>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SplitKind {
    /// Side by side.
    #[serde(alias = "cols")]
    Columns,
    /// Stacked.
    Rows,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum SizeSpec {
    Cells(u16),
    Text(String),
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct NodeSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub split: Option<SplitKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub panes: Option<Vec<NodeSpec>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<SizeSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<bool>,
}

// ---- the resolved plan sent to the server ----

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq)]
pub enum Size {
    Percent(f32),
    Cells(u16),
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanAxis {
    Cols,
    Rows,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PlanChild {
    pub size: Option<Size>,
    pub node: PlanNode,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum PlanNode {
    Pane { cwd: PathBuf, command: Option<String>, focus: bool },
    Split { axis: PlanAxis, children: Vec<PlanChild> },
}

impl PlanNode {
    /// The panes, in layout order.
    pub fn leaves(&self) -> Vec<(&Path, Option<&str>, bool)> {
        match self {
            PlanNode::Pane { cwd, command, focus } => vec![(cwd.as_path(), command.as_deref(), *focus)],
            PlanNode::Split { children, .. } => children.iter().flat_map(|c| c.node.leaves()).collect(),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PlanWindow {
    pub name: Option<String>,
    pub lazy: bool,
    pub tree: PlanNode,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PlanDockPane {
    pub name: String,
    pub cwd: PathBuf,
    pub command: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct PlanDock {
    pub width: Size,
    pub visible: bool,
    pub panes: Vec<PlanDockPane>,
}

/// Width of a dock that does not say.
pub const DEFAULT_DOCK_WIDTH: Size = Size::Percent(35.0);

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Plan {
    pub name: String,
    pub root: PathBuf,
    pub windows: Vec<PlanWindow>,
    /// Index of the window shown first.
    pub focus: usize,
    pub pane_command: Option<String>,
    pub dock: Option<PlanDock>,
    pub sidebar_width: Option<u16>,
    pub sidebar_visible: Option<bool>,
}

/// The name a pane gets when it has none: its command's first word, or the shell.
pub fn default_pane_name(command: Option<&str>, shell: &str) -> String {
    command
        .and_then(|c| c.split_whitespace().next())
        .map(|w| w.rsplit('/').next().unwrap_or(w).to_string())
        .unwrap_or_else(|| shell.rsplit('/').next().unwrap_or("sh").to_string())
}

// ---- paths ----

/// Drop `.` components and resolve `..` lexically.
pub fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// Absolute form of `path`: `~` is `home`, relative paths are under `base`.
pub fn expand(path: &str, base: &Path, home: &Path) -> PathBuf {
    let p = if path == "~" {
        home.to_path_buf()
    } else if let Some(rest) = path.strip_prefix("~/") {
        home.join(rest)
    } else if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        base.join(path)
    };
    clean(&p)
}

/// The shortest way to write `path` in a file that lives in `base`.
/// `None` means "the same as `base`".
pub fn collapse(path: &Path, base: &Path, home: &Path) -> Option<String> {
    let path = clean(path);
    if path == base {
        return None;
    }
    if let Ok(rel) = path.strip_prefix(base) {
        return Some(rel.display().to_string());
    }
    if let Ok(rel) = path.strip_prefix(home)
        && !home.as_os_str().is_empty()
    {
        return Some(if rel.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rel.display()) });
    }
    Some(path.display().to_string())
}

pub fn home_dir() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

// ---- loading and validating ----

/// `size` in cells out of `total`.
pub fn cells(size: Size, total: u16) -> u16 {
    match size {
        Size::Percent(p) => (f32::from(total) * p / 100.0).round() as u16,
        Size::Cells(n) => n,
    }
}

pub fn parse_size(s: &SizeSpec) -> Result<Size, String> {
    match s {
        SizeSpec::Cells(0) => Err("size must be above 0".into()),
        SizeSpec::Cells(n) => Ok(Size::Cells(*n)),
        SizeSpec::Text(t) => {
            let t = t.trim();
            if let Some(p) = t.strip_suffix('%') {
                let v: f32 = p.trim().parse().map_err(|_| format!("bad size {t:?}"))?;
                if v > 0.0 && v < 100.0 { Ok(Size::Percent(v)) } else { Err(format!("size {t:?} must be between 0% and 100%")) }
            } else {
                match t.parse::<u16>() {
                    Ok(n) if n > 0 => Ok(Size::Cells(n)),
                    _ => Err(format!("bad size {t:?} (use 30% or a number of cells)")),
                }
            }
        }
    }
}

pub fn parse(text: &str) -> Result<ProjectFile, String> {
    let file: ProjectFile = serde_norway::from_str(text).map_err(|e| e.to_string())?;
    validate(&file)?;
    Ok(file)
}

pub fn load(path: &Path) -> Result<ProjectFile, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn validate_node(n: &NodeSpec, at: &str) -> Result<(), String> {
    if let Some(s) = &n.size {
        parse_size(s).map_err(|e| format!("{at}: {e}"))?;
    }
    match (&n.split, &n.panes) {
        (None, None) => Ok(()),
        (Some(_), Some(panes)) => {
            if n.command.is_some() {
                return Err(format!("{at}: a split cannot have a command (put it on a pane)"));
            }
            if panes.len() < 2 {
                return Err(format!("{at}: split needs at least 2 panes"));
            }
            for (i, p) in panes.iter().enumerate() {
                validate_node(p, &format!("{at}.panes[{i}]"))?;
            }
            Ok(())
        }
        (Some(_), None) => Err(format!("{at}: split needs a `panes` list")),
        (None, Some(_)) => Err(format!("{at}: `panes` needs `split: columns` or `split: rows`")),
    }
}

pub fn validate(file: &ProjectFile) -> Result<(), String> {
    if file.windows.is_empty() {
        return Err("no windows".into());
    }
    if let Some(w) = file.sidebar.as_ref().and_then(|s| s.width)
        && !(4..=200).contains(&w)
    {
        return Err("sidebar.width must be between 4 and 200".into());
    }
    if let Some(d) = &file.dock {
        if d.panes.is_empty() {
            return Err("dock: needs at least one pane".into());
        }
        if let Some(w) = &d.width {
            parse_size(w).map_err(|e| format!("dock.width: {e}"))?;
        }
    }
    for (i, w) in file.windows.iter().enumerate() {
        let at = format!("windows[{i}]");
        if let Some(l) = &w.layout {
            if w.command.is_some() {
                return Err(format!("{at}: use either `command` or `layout`, not both"));
            }
            validate_node(l, &format!("{at}.layout"))?;
        }
    }
    Ok(())
}

fn node_to_plan(n: &NodeSpec, inherited: &Path, base_home: (&Path, &Path)) -> PlanNode {
    let (_, home) = base_home;
    let cwd = match &n.path {
        Some(p) => expand(p, inherited, home),
        None => inherited.to_path_buf(),
    };
    match (&n.split, &n.panes) {
        (Some(kind), Some(panes)) => PlanNode::Split {
            axis: match kind {
                SplitKind::Columns => PlanAxis::Cols,
                SplitKind::Rows => PlanAxis::Rows,
            },
            children: panes
                .iter()
                .map(|p| PlanChild {
                    size: p.size.as_ref().and_then(|s| parse_size(s).ok()),
                    node: node_to_plan(p, &cwd, base_home),
                })
                .collect(),
        },
        _ => PlanNode::Pane { cwd, command: n.command.clone(), focus: n.focus.unwrap_or(false) },
    }
}

/// Turn a validated file into a plan. `file_dir` is where the file lives.
pub fn resolve(file: &ProjectFile, file_dir: &Path, home: &Path) -> Plan {
    let root = match &file.root {
        Some(r) => expand(r, file_dir, home),
        None => clean(file_dir),
    };
    let name = file
        .name
        .clone()
        .or_else(|| root.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "project".into());
    let default_lazy = file.lazy.unwrap_or(true);
    let windows: Vec<PlanWindow> = file
        .windows
        .iter()
        .map(|w| {
            let wpath = match &w.path {
                Some(p) => expand(p, &root, home),
                None => root.clone(),
            };
            let tree = match &w.layout {
                Some(l) => node_to_plan(l, &wpath, (&root, home)),
                None => PlanNode::Pane { cwd: wpath, command: w.command.clone(), focus: false },
            };
            PlanWindow { name: w.name.clone(), lazy: w.lazy.unwrap_or(default_lazy), tree }
        })
        .collect();
    let focus = file.windows.iter().position(|w| w.focus == Some(true)).unwrap_or(0);
    let dock = file.dock.as_ref().map(|d| PlanDock {
        width: d.width.as_ref().and_then(|w| parse_size(w).ok()).unwrap_or(DEFAULT_DOCK_WIDTH),
        visible: d.visible.unwrap_or(false),
        panes: d
            .panes
            .iter()
            .map(|p| PlanDockPane {
                name: p.name.clone().unwrap_or_else(|| default_pane_name(p.command.as_deref(), "shell")),
                cwd: p.path.as_ref().map_or_else(|| root.clone(), |x| expand(x, &root, home)),
                command: p.command.clone(),
            })
            .collect(),
    });
    let sidebar = file.sidebar.clone().unwrap_or_default();
    Plan {
        name,
        root,
        windows,
        focus,
        pane_command: file.pane_command.clone(),
        dock,
        sidebar_width: sidebar.width,
        sidebar_visible: sidebar.visible,
    }
}

/// Load and resolve a project file.
pub fn load_plan(path: &Path) -> Result<Plan, String> {
    let file = load(path)?;
    let dir = path.parent().map(|p| if p.as_os_str().is_empty() { Path::new(".") } else { p });
    let dir = match dir {
        Some(d) => std::path::absolute(d).map_err(|e| e.to_string())?,
        None => std::env::current_dir().map_err(|e| e.to_string())?,
    };
    Ok(resolve(&file, &dir, &home_dir()))
}

/// Fractions (summing to 1) of `total` cells for children with the given sizes.
/// Children without a size share what is left equally.
pub fn fractions(sizes: &[Option<Size>], total: u16) -> Vec<f32> {
    let total = f32::from(total.max(1));
    let mut fr: Vec<Option<f32>> = sizes
        .iter()
        .map(|s| {
            s.map(|s| match s {
                Size::Percent(p) => p / 100.0,
                Size::Cells(c) => f32::from(c) / total,
            })
        })
        .collect();
    let used: f32 = fr.iter().flatten().sum();
    let free = fr.iter().filter(|f| f.is_none()).count();
    if free > 0 {
        let each = ((1.0 - used) / free as f32).max(0.05);
        for f in fr.iter_mut().filter(|f| f.is_none()) {
            *f = Some(each);
        }
    }
    let sum: f32 = fr.iter().flatten().sum();
    fr.into_iter().map(|f| f.unwrap_or(0.0) / sum).collect()
}

// ---- finding project files ----

/// Directory of named projects.
pub fn named_dir(config_path: &Path) -> PathBuf {
    config_path.parent().unwrap_or(Path::new(".")).join("projects")
}

pub fn named_path(config_path: &Path, name: &str) -> PathBuf {
    named_dir(config_path).join(format!("{name}.yaml"))
}

pub fn valid_project_name(name: &str) -> bool {
    !name.is_empty() && !name.starts_with('.') && !name.contains(['/', '\\', '\0'])
}

/// Names of the saved projects.
pub fn list_named(config_path: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(named_dir(config_path))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.path().file_stem().map(|s| s.to_string_lossy().into_owned()).filter(|_| e.path().extension().is_some_and(|x| x == "yaml")))
        .collect();
    names.sort();
    names
}

/// Where `arg` points: nothing means `./project.dtm.yaml`; otherwise a file, a
/// directory holding one, or the name of a saved project.
pub fn find(arg: Option<&str>, cwd: &Path, config_path: &Path) -> Result<PathBuf, String> {
    let Some(arg) = arg else {
        let p = cwd.join(PROJECT_FILE);
        return if p.is_file() { Ok(p) } else { Err(format!("no {PROJECT_FILE} in {}", cwd.display())) };
    };
    let given = cwd.join(arg);
    if given.is_file() {
        return Ok(given);
    }
    if given.is_dir() {
        let p = given.join(PROJECT_FILE);
        return if p.is_file() { Ok(p) } else { Err(format!("no {PROJECT_FILE} in {}", given.display())) };
    }
    let named = named_path(config_path, arg);
    if valid_project_name(arg) && named.is_file() {
        return Ok(named);
    }
    Err(format!("no project {arg:?}: not a file or directory, and {} does not exist", named.display()))
}

// ---- exporting ----

/// Quote one command-line word for a POSIX shell, only when needed.
pub fn shell_quote(word: &str) -> String {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c);
    if !word.is_empty() && word.chars().all(safe) {
        word.to_string()
    } else {
        format!("'{}'", word.replace('\'', r"'\''"))
    }
}

pub fn quote_args(args: &[String]) -> String {
    args.iter().map(|a| shell_quote(a)).collect::<Vec<_>>().join(" ")
}

fn spec_of(node: &PlanNode, inherited: &Path, base: &Path, home: &Path, single_leaf: bool) -> NodeSpec {
    let rel = |cwd: &Path| -> Option<String> {
        if clean(cwd) == clean(inherited) { None } else { collapse(cwd, base, home).or_else(|| Some(".".into())) }
    };
    match node {
        PlanNode::Pane { cwd, command, focus } => NodeSpec {
            path: rel(cwd),
            command: command.clone(),
            focus: (*focus && !single_leaf).then_some(true),
            ..Default::default()
        },
        PlanNode::Split { axis, children } => NodeSpec {
            split: Some(match axis {
                PlanAxis::Cols => SplitKind::Columns,
                PlanAxis::Rows => SplitKind::Rows,
            }),
            panes: Some(
                children
                    .iter()
                    .map(|c| {
                        let mut s = spec_of(&c.node, inherited, base, home, single_leaf);
                        s.size = c.size.map(|z| match z {
                            Size::Percent(p) => SizeSpec::Text(format!("{}%", (p.round() as i64).max(1))),
                            Size::Cells(n) => SizeSpec::Cells(n),
                        });
                        s
                    })
                    .collect(),
            ),
            ..Default::default()
        },
    }
}

/// Describe a running session as a project file that lives in `base`.
/// `root` is never written: paths are relative to the file's own directory.
pub fn from_plan(plan: &Plan, base: &Path, home: &Path) -> ProjectFile {
    let base = clean(base);
    let windows = plan
        .windows
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let leaves = w.tree.leaves();
            let first = leaves[0].0.to_path_buf();
            let shared = leaves.iter().all(|(c, ..)| clean(c) == clean(&first));
            let (path, inherited) = if shared {
                (collapse(&first, &base, home), first.clone())
            } else {
                (None, base.clone())
            };
            let mut spec = WindowSpec {
                name: w.name.clone(),
                path,
                focus: (i == plan.focus && plan.windows.len() > 1 && i != 0).then_some(true),
                ..Default::default()
            };
            match &w.tree {
                PlanNode::Pane { command, .. } => spec.command = command.clone(),
                tree => spec.layout = Some(spec_of(tree, &inherited, &base, home, leaves.len() == 1)),
            }
            spec
        })
        .collect();
    let dock = plan.dock.as_ref().filter(|d| !d.panes.is_empty()).map(|d| DockSpec {
        width: Some(match d.width {
            Size::Percent(p) => SizeSpec::Text(format!("{}%", (p.round() as i64).max(1))),
            Size::Cells(n) => SizeSpec::Cells(n),
        }),
        visible: d.visible.then_some(true),
        panes: d
            .panes
            .iter()
            .map(|p| DockPaneSpec {
                name: (p.name != default_pane_name(p.command.as_deref(), "shell")).then(|| p.name.clone()),
                path: collapse(&p.cwd, &base, home),
                command: p.command.clone(),
            })
            .collect(),
    });
    ProjectFile {
        name: Some(plan.name.clone()),
        root: None,
        lazy: None,
        pane_command: plan.pane_command.clone(),
        dock,
        sidebar: (plan.sidebar_width.is_some() || plan.sidebar_visible.is_some())
            .then_some(SidebarSpec { width: plan.sidebar_width, visible: plan.sidebar_visible }),
        windows,
    }
}

/// Keep what a person wrote at the top of an existing file (name, root, lazy
/// flags, pane command, dock size) and take the windows from the export.
pub fn merge(existing: &ProjectFile, mut exported: ProjectFile) -> ProjectFile {
    if existing.name.is_some() {
        exported.name = existing.name.clone();
    }
    exported.root = existing.root.clone();
    exported.lazy = existing.lazy;
    if existing.pane_command.is_some() {
        exported.pane_command = existing.pane_command.clone();
    }
    exported.sidebar = match (&existing.sidebar, exported.sidebar.take()) {
        (Some(old), Some(new)) => Some(SidebarSpec { width: new.width.or(old.width), visible: old.visible }),
        (Some(old), None) => Some(old.clone()),
        (None, new) => new,
    };
    if let (Some(old), Some(new)) = (&existing.dock, &mut exported.dock) {
        new.width = old.width.clone().or(new.width.take());
        new.visible = old.visible;
    }
    for w in &mut exported.windows {
        if let Some(old) = existing.windows.iter().find(|o| o.name.is_some() && o.name == w.name) {
            w.lazy = old.lazy;
        }
    }
    exported
}

/// Write `plan` to `target`, merging with the file already there.
pub fn export_to(plan: &Plan, target: &Path, home: &Path) -> Result<(), String> {
    let target = std::path::absolute(target).map_err(|e| e.to_string())?;
    let dir = target.parent().ok_or("bad target path")?;
    let existing = if target.is_file() { Some(load(&target)?) } else { None };
    // Paths are written relative to the file's `root` when it has one.
    let base = match existing.as_ref().and_then(|e| e.root.as_deref()) {
        Some(r) => expand(r, dir, home),
        None => clean(dir),
    };
    let mut file = from_plan(plan, &base, home);
    if let Some(old) = &existing {
        file = merge(old, file);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let text = serde_norway::to_string(&file).map_err(|e| e.to_string())?;
    std::fs::write(&target, text).map_err(|e| format!("{}: {e}", target.display()))
}

/// A starting point for `dtm project edit` (named projects need an explicit root).
pub fn template(root: &Path) -> String {
    format!(
        "\
# dtm project. Open it with `dtm open NAME`.
root: {root}
# lazy: true              # windows start when first shown (false: all at once)
windows:
  - name: shell
  # - name: edit
  #   layout:
  #     split: columns    # columns = side by side, rows = stacked
  #     panes:
  #       - command: nvim
  #         size: 65%
  #       - command: cargo watch -x check
",
        root = root.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/u";

    fn plan(text: &str, dir: &str) -> Plan {
        resolve(&parse(text).unwrap(), Path::new(dir), Path::new(HOME))
    }

    #[test]
    fn parses_the_documented_example() {
        let p = plan(
            "name: dtm\nroot: ~/code\nwindows:\n  - name: edit\n    path: src\n    focus: true\n    layout:\n      split: columns\n      panes:\n        - command: nvim\n          size: 65%\n          focus: true\n        - split: rows\n          panes:\n            - command: cargo watch\n            - path: /tmp\n  - name: logs\n    command: journalctl -f\n    lazy: false\n",
            "/proj",
        );
        assert_eq!(p.name, "dtm");
        assert_eq!(p.root, Path::new("/home/u/code"));
        assert_eq!(p.focus, 0);
        assert!(p.windows[0].lazy);
        assert!(!p.windows[1].lazy);
        let leaves = p.windows[0].tree.leaves();
        assert_eq!(leaves.len(), 3);
        assert_eq!(leaves[0], (Path::new("/home/u/code/src"), Some("nvim"), true));
        assert_eq!(leaves[2].0, Path::new("/tmp"));
        assert_eq!(p.windows[1].tree.leaves()[0], (Path::new("/home/u/code"), Some("journalctl -f"), false));
    }

    #[test]
    fn the_shipped_example_is_valid() {
        let p = plan(include_str!("../examples/project.dtm.yaml"), "/code/app");
        assert_eq!(p.windows.len(), 3);
        assert!(!p.windows[1].lazy);
        assert_eq!(p.windows[0].tree.leaves().len(), 3);
    }

    #[test]
    fn root_defaults_to_the_file_directory_and_names_the_session() {
        let p = plan("windows:\n  - {}\n", "/work/app");
        assert_eq!(p.root, Path::new("/work/app"));
        assert_eq!(p.name, "app");
    }

    #[test]
    fn rejects_bad_files() {
        let err = |t: &str| parse(t).unwrap_err();
        assert!(err("windows: []").contains("no windows"));
        assert!(err("windows:\n  - bogus: 1\n").contains("bogus"));
        assert!(
            err("windows:\n  - command: a\n    layout: {command: b}\n").contains("not both")
        );
        let e = err("windows:\n  - layout:\n      split: rows\n      panes:\n        - command: a\n");
        assert!(e.starts_with("windows[0].layout: split needs at least 2 panes"), "{e}");
        let e = err("windows:\n  - layout:\n      split: rows\n      panes:\n        - command: a\n        - size: 150%\n");
        assert!(e.contains("windows[0].layout.panes[1]"), "{e}");
        assert!(err("windows:\n  - layout:\n      panes: [{}, {}]\n").contains("split"));
        assert!(err("windows:\n  - layout:\n      split: rows\n      command: x\n      panes: [{}, {}]\n").contains("command"));
    }

    #[test]
    fn nested_paths_resolve_against_the_nearest_parent() {
        let p = plan(
            "windows:\n  - path: a\n    layout:\n      split: rows\n      path: b\n      panes:\n        - {}\n        - path: ../c\n        - path: ~/d\n",
            "/r",
        );
        let cwds: Vec<_> = p.windows[0].tree.leaves().iter().map(|l| l.0.to_path_buf()).collect();
        assert_eq!(cwds, vec![PathBuf::from("/r/a/b"), PathBuf::from("/r/a/c"), PathBuf::from("/home/u/d")]);
    }

    #[test]
    fn sizes_become_fractions() {
        let f = fractions(&[Some(Size::Percent(50.0)), None, None], 100);
        assert_eq!(f.len(), 3);
        assert!((f[0] - 0.5).abs() < 1e-5 && (f[1] - 0.25).abs() < 1e-5 && (f[2] - 0.25).abs() < 1e-5);
        let f = fractions(&[Some(Size::Cells(20)), None], 80);
        assert!((f[0] - 0.25).abs() < 1e-5 && (f[1] - 0.75).abs() < 1e-5);
        let f = fractions(&[None, None], 10);
        assert!((f[0] - 0.5).abs() < 1e-5);
        // Sizes that overshoot are normalised.
        let f = fractions(&[Some(Size::Percent(90.0)), Some(Size::Percent(90.0))], 10);
        assert!((f.iter().sum::<f32>() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn size_syntax() {
        assert_eq!(parse_size(&SizeSpec::Text("30%".into())), Ok(Size::Percent(30.0)));
        assert_eq!(parse_size(&SizeSpec::Cells(12)), Ok(Size::Cells(12)));
        assert_eq!(parse_size(&SizeSpec::Text("12".into())), Ok(Size::Cells(12)));
        assert!(parse_size(&SizeSpec::Text("0%".into())).is_err());
        assert!(parse_size(&SizeSpec::Text("abc".into())).is_err());
    }

    #[test]
    fn expand_and_collapse() {
        let (base, home) = (Path::new("/p"), Path::new(HOME));
        assert_eq!(expand("x/../y", base, home), Path::new("/p/y"));
        assert_eq!(expand("~", base, home), Path::new(HOME));
        assert_eq!(collapse(Path::new("/p"), base, home), None);
        assert_eq!(collapse(Path::new("/p/src/a"), base, home).as_deref(), Some("src/a"));
        assert_eq!(collapse(Path::new("/home/u/x"), base, home).as_deref(), Some("~/x"));
        assert_eq!(collapse(Path::new("/etc"), base, home).as_deref(), Some("/etc"));
    }

    #[test]
    fn export_has_no_root_and_relative_paths() {
        let text = "root: /elsewhere\nwindows:\n  - name: edit\n    layout:\n      split: columns\n      panes:\n        - {command: nvim, size: 60%}\n        - {path: sub}\n";
        let p = plan(text, "/proj");
        let file = from_plan(&p, Path::new("/elsewhere"), Path::new(HOME));
        assert_eq!(file.root, None);
        let yaml = serde_norway::to_string(&file).unwrap();
        assert!(!yaml.contains("root"), "{yaml}");
        assert!(yaml.contains("path: sub"), "{yaml}");
        assert!(yaml.contains("command: nvim"), "{yaml}");
        assert!(yaml.contains("60%"), "{yaml}");
        // Re-reading the export gives the same plan.
        let again = resolve(&parse(&yaml).unwrap(), Path::new("/elsewhere"), Path::new(HOME));
        assert_eq!(again.windows, p.windows);
    }

    #[test]
    fn export_of_a_single_pane_window_uses_the_shorthand() {
        let p = Plan {
            name: "n".into(),
            root: "/r".into(),
            focus: 1,
            pane_command: None,
            dock: None,
            sidebar_width: None,
            sidebar_visible: None,
            windows: vec![
                PlanWindow {
                    name: Some("a".into()),
                    lazy: true,
                    tree: PlanNode::Pane { cwd: "/r/x".into(), command: Some("vim".into()), focus: true },
                },
                PlanWindow {
                    name: None,
                    lazy: true,
                    tree: PlanNode::Pane { cwd: "/r".into(), command: None, focus: true },
                },
            ],
        };
        let f = from_plan(&p, Path::new("/r"), Path::new(HOME));
        assert_eq!(f.windows[0].path.as_deref(), Some("x"));
        assert_eq!(f.windows[0].command.as_deref(), Some("vim"));
        assert!(f.windows[0].layout.is_none());
        assert_eq!(f.windows[1].focus, Some(true));
        assert_eq!(f.windows[1].path, None);
    }

    #[test]
    fn merge_keeps_name_root_and_lazy() {
        let old = parse("name: mine\nroot: /x\nlazy: false\nwindows:\n  - {name: a, lazy: true}\n").unwrap();
        let new = ProjectFile {
            name: Some("session".into()),
            root: None,
            lazy: None,
            pane_command: None,
            dock: None,
            sidebar: None,
            windows: vec![WindowSpec { name: Some("a".into()), ..Default::default() }, WindowSpec::default()],
        };
        let m = merge(&old, new);
        assert_eq!(m.name.as_deref(), Some("mine"));
        assert_eq!(m.lazy, Some(false));
        assert_eq!(m.root.as_deref(), Some("/x"));
        assert_eq!(m.windows[0].lazy, Some(true));
        assert_eq!(m.windows[1].lazy, None);
    }

    #[test]
    fn export_keeps_an_existing_root_and_writes_paths_relative_to_it() {
        let tmp = std::env::temp_dir().join(format!("dtm-root-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();
        let target = tmp.join("p.yaml");
        std::fs::write(&target, "root: /code/app\npane_command: echo hi\nwindows:\n  - {name: a}\n").unwrap();
        let p = Plan {
            name: "app".into(),
            root: "/code/app".into(),
            focus: 0,
            pane_command: None,
            dock: None,
            sidebar_width: None,
            sidebar_visible: None,
            windows: vec![PlanWindow {
                name: Some("a".into()),
                lazy: true,
                tree: PlanNode::Pane { cwd: "/code/app/src".into(), command: None, focus: true },
            }],
        };
        export_to(&p, &target, Path::new(HOME)).unwrap();
        let f = load(&target).unwrap();
        assert_eq!(f.root.as_deref(), Some("/code/app"));
        assert_eq!(f.pane_command.as_deref(), Some("echo hi"));
        assert_eq!(f.windows[0].path.as_deref(), Some("src"));
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn pane_command_and_dock_parse_resolve_and_export() {
        let text = "pane_command: source env\ndock:\n  width: 40%\n  visible: true\n  panes:\n    - command: lazygit\n    - {name: ai, command: claude, path: sub}\nwindows:\n  - {}\n";
        let p = plan(text, "/proj");
        assert_eq!(p.pane_command.as_deref(), Some("source env"));
        let d = p.dock.clone().unwrap();
        assert_eq!(d.width, Size::Percent(40.0));
        assert!(d.visible);
        assert_eq!(d.panes[0].name, "lazygit");
        assert_eq!(d.panes[1].cwd, Path::new("/proj/sub"));
        let f = from_plan(&p, Path::new("/proj"), Path::new(HOME));
        let again = resolve(&parse(&serde_norway::to_string(&f).unwrap()).unwrap(), Path::new("/proj"), Path::new(HOME));
        assert_eq!(again.dock, p.dock);
        assert_eq!(again.pane_command, p.pane_command);
        assert!(parse("dock: {panes: []}\nwindows: [{}]\n").is_err());
        assert!(parse("dock: {width: x, panes: [{}]}\nwindows: [{}]\n").is_err());
    }

    #[test]
    fn sidebar_settings_parse_resolve_export_and_merge() {
        let p = plan("sidebar: {width: 30, visible: false}\nwindows: [{}]\n", "/proj");
        assert_eq!((p.sidebar_width, p.sidebar_visible), (Some(30), Some(false)));
        assert!(parse("sidebar: {width: 2}\nwindows: [{}]\n").is_err());
        let mut live = p.clone();
        live.sidebar_width = Some(40);
        live.sidebar_visible = None;
        let old = parse("sidebar: {width: 30, visible: false}\nwindows: [{}]\n").unwrap();
        let m = merge(&old, from_plan(&live, Path::new("/proj"), Path::new(HOME)));
        assert_eq!(m.sidebar, Some(SidebarSpec { width: Some(40), visible: Some(false) }));
        // Nothing resized: the file's own settings stay.
        live.sidebar_width = None;
        let m = merge(&old, from_plan(&live, Path::new("/proj"), Path::new(HOME)));
        assert_eq!(m.sidebar, old.sidebar);
    }

    #[test]
    fn quoting() {
        assert_eq!(shell_quote("abc-1.txt"), "abc-1.txt");
        assert_eq!(shell_quote("a b"), "'a b'");
        assert_eq!(shell_quote("it's"), r"'it'\''s'");
        assert_eq!(shell_quote(""), "''");
        assert_eq!(quote_args(&["sleep".into(), "1000".into()]), "sleep 1000");
    }

    #[test]
    fn lookup_order() {
        let tmp = std::env::temp_dir().join(format!("dtm-proj-{}", std::process::id()));
        let cfg = tmp.join("cfg/config.toml");
        std::fs::create_dir_all(tmp.join("cfg/projects")).unwrap();
        std::fs::create_dir_all(tmp.join("work/dir")).unwrap();
        std::fs::write(tmp.join("work").join(PROJECT_FILE), "windows: [{}]\n").unwrap();
        std::fs::write(tmp.join("work/dir").join(PROJECT_FILE), "windows: [{}]\n").unwrap();
        std::fs::write(tmp.join("work/other.yaml"), "windows: [{}]\n").unwrap();
        std::fs::write(tmp.join("cfg/projects/mine.yaml"), "windows: [{}]\n").unwrap();
        let work = tmp.join("work");
        assert_eq!(find(None, &work, &cfg).unwrap(), work.join(PROJECT_FILE));
        assert_eq!(find(Some("dir"), &work, &cfg).unwrap(), work.join("dir").join(PROJECT_FILE));
        assert_eq!(find(Some("other.yaml"), &work, &cfg).unwrap(), work.join("other.yaml"));
        assert_eq!(find(Some("mine"), &work, &cfg).unwrap(), tmp.join("cfg/projects/mine.yaml"));
        assert!(find(Some("nope"), &work, &cfg).is_err());
        assert!(find(None, &tmp, &cfg).is_err());
        assert_eq!(list_named(&cfg), vec!["mine".to_string()]);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
