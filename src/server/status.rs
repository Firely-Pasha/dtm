//! The status box under the sidebar: `[status] lines` are templates with `#{...}` variables,
//! expanded here; the values come from `Stats` (sampled each tick) and `GitInfo` (looked up in
//! the background), never from anything slow at draw time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Marks where a `#{fill}` goes; the character after it is what fills.
const FILL: char = '\u{1}';

/// Expand `template`: text, `##` (a `#`), `#{name}`, `#{name:arg}` and `#{?name,then,else}`.
/// Inside a branch `#,` and `#}` stand for a literal `,` and `}`. An unterminated `#{` stays as
/// it is. `fill` leaves a marker for `resolve_fills`; every other name asks `var(name, arg)`.
pub fn expand(template: &str, var: &dyn Fn(&str, Option<&str>) -> String) -> String {
    let chars: Vec<char> = template.chars().collect();
    let mut i = 0;
    parse(&chars, &mut i, var, false)
}

fn parse(chars: &[char], i: &mut usize, var: &dyn Fn(&str, Option<&str>) -> String, branch: bool) -> String {
    let mut out = String::new();
    while *i < chars.len() {
        let c = chars[*i];
        if branch && (c == ',' || c == '}') {
            break;
        }
        *i += 1;
        if c != '#' {
            out.push(c);
            continue;
        }
        match chars.get(*i) {
            Some('#') => {
                out.push('#');
                *i += 1;
            }
            Some(&e @ (',' | '}')) => {
                out.push(e);
                *i += 1;
            }
            Some('{') => {
                let start = *i;
                *i += 1;
                match parse_var(chars, i, var) {
                    Some(text) => out.push_str(&text),
                    None => {
                        // Not closed: keep the "#{" and go on after it.
                        *i = start + 1;
                        out.push_str("#{");
                    }
                }
            }
            _ => out.push('#'),
        }
    }
    out
}

/// After `#{`: a variable or a condition, up to and including its `}`.
fn parse_var(chars: &[char], i: &mut usize, var: &dyn Fn(&str, Option<&str>) -> String) -> Option<String> {
    if chars.get(*i) == Some(&'?') {
        *i += 1;
        let from = *i;
        while *i < chars.len() && chars[*i] != ',' && chars[*i] != '}' {
            *i += 1;
        }
        let name: String = chars[from..*i].iter().collect();
        let (mut then, mut other) = (String::new(), String::new());
        let truthy = {
            let v = if name == "fill" { String::new() } else { var(&name, None) };
            !v.is_empty() && v != "0"
        };
        if chars.get(*i) == Some(&',') {
            *i += 1;
            then = parse(chars, i, var, true);
            if chars.get(*i) == Some(&',') {
                *i += 1;
                other = parse(chars, i, var, true);
            }
        }
        if chars.get(*i) != Some(&'}') {
            return None;
        }
        *i += 1;
        return Some(if truthy { then } else { other });
    }
    let from = *i;
    while *i < chars.len() && chars[*i] != '}' {
        *i += 1;
    }
    if *i >= chars.len() {
        return None;
    }
    let body: String = chars[from..*i].iter().collect();
    *i += 1;
    let (name, arg) = match body.split_once(':') {
        Some((n, a)) => (n, Some(a)),
        None => (body.as_str(), None),
    };
    Some(if name == "fill" {
        format!("{FILL}{}", arg.and_then(|a| a.chars().next()).unwrap_or(' '))
    } else {
        var(name, arg)
    })
}

/// The text of an expanded line without its fill markers.
fn plain(expanded: &str) -> String {
    let mut out = String::new();
    let mut chars = expanded.chars();
    while let Some(c) = chars.next() {
        if c == FILL {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

/// A line that is not empty to begin with but expands to nothing is left out.
pub fn visible(template: &str, expanded: &str) -> bool {
    template.is_empty() || !plain(expanded).trim().is_empty()
}

/// Turn the fill markers of an expanded line into `width` cells: the room left over is shared
/// evenly between them (the last ones get the odd cells); a line that does not fit is cut short.
pub fn resolve_fills(expanded: &str, width: usize) -> String {
    let mut parts: Vec<String> = vec![String::new()];
    let mut fills: Vec<char> = Vec::new();
    let mut chars = expanded.chars();
    while let Some(c) = chars.next() {
        if c == FILL {
            fills.push(chars.next().unwrap_or(' '));
            parts.push(String::new());
        } else {
            parts.last_mut().expect("a part").push(c);
        }
    }
    let used: usize = parts.iter().map(|p| p.width()).sum();
    let left = width.saturating_sub(used);
    let n = fills.len();
    let mut out = String::new();
    for (k, part) in parts.iter().enumerate() {
        out.push_str(part);
        if let Some(&f) = fills.get(k) {
            let share = left / n + usize::from(k >= n - left % n);
            let cw = f.width().unwrap_or(1).max(1);
            out.extend(std::iter::repeat_n(f, share / cw));
            out.extend(std::iter::repeat_n(' ', share % cw));
        }
    }
    out
}

// ---- values ----

/// What the machine reports, sampled once per tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub load: String,
    pub mem: String,
    pub mem_percent: String,
    pub battery: String,
    pub uptime: String,
}

impl Stats {
    pub fn sample() -> Stats {
        let mut s = Stats::default();
        let mut avg = [0f64; 1];
        // SAFETY: getloadavg writes at most one f64 into the array we pass with length 1.
        if unsafe { libc::getloadavg(avg.as_mut_ptr(), 1) } == 1 {
            s.load = format!("{:.2}", avg[0]);
        }
        #[cfg(target_os = "linux")]
        {
            if let Some((used, total)) = std::fs::read_to_string("/proc/meminfo").ok().as_deref().and_then(parse_meminfo) {
                s.mem = format!("{}/{}", gib(used), gib(total));
                s.mem_percent = format!("{}%", used * 100 / total.max(1));
            }
            s.battery = battery().unwrap_or_default();
            s.uptime = std::fs::read_to_string("/proc/uptime")
                .ok()
                .and_then(|t| t.split_whitespace().next()?.parse::<f64>().ok())
                .map(|secs| format_uptime(secs as u64))
                .unwrap_or_default();
        }
        s
    }
}

/// Used and total memory in KiB from the text of `/proc/meminfo`.
pub fn parse_meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| -> Option<u64> {
        text.lines().find_map(|l| l.strip_prefix(name)?.trim_start_matches(':').split_whitespace().next()?.parse().ok())
    };
    let total = field("MemTotal")?;
    let available = field("MemAvailable")?;
    (total > 0).then(|| (total.saturating_sub(available), total))
}

#[cfg(target_os = "linux")]
fn gib(kib: u64) -> String {
    format!("{:.1}G", kib as f64 / (1024.0 * 1024.0))
}

pub fn format_uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs / 3600 % 24, secs / 60 % 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

#[cfg(target_os = "linux")]
fn battery() -> Option<String> {
    for entry in std::fs::read_dir("/sys/class/power_supply").ok()?.flatten() {
        let dir = entry.path();
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok().map(|t| t.trim().to_string());
        if read("type").as_deref() != Some("Battery") {
            continue;
        }
        let capacity = read("capacity")?;
        let charging = matches!(read("status").as_deref(), Some("Charging"));
        return Some(format!("{capacity}%{}", if charging { "+" } else { "" }));
    }
    None
}

pub fn user() -> String {
    ["USER", "LOGNAME"].iter().find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty())).unwrap_or_default()
}

/// The machine's name up to the first dot.
pub fn host() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: gethostname writes at most buf.len() bytes into the buffer.
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return String::new();
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..end]);
    name.split('.').next().unwrap_or("").to_string()
}

/// The local time formatted with `strftime`.
pub fn time_format(format: &str) -> String {
    let Ok(fmt) = std::ffi::CString::new(format) else { return String::new() };
    // SAFETY: tm is plain data that localtime_r fills; strftime writes at most buf.len() bytes
    // and returns how many it wrote.
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut tm).is_null() {
            return String::new();
        }
        let mut buf = [0u8; 128];
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), fmt.as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

/// `dir` with the home directory written as `~`.
pub fn tilde(dir: &Path, home: &Path) -> String {
    if !home.as_os_str().is_empty()
        && let Ok(rest) = dir.strip_prefix(home)
    {
        return if rest.as_os_str().is_empty() { "~".into() } else { format!("~/{}", rest.display()) };
    }
    dir.display().to_string()
}

// ---- git ----

/// The state of the repository around a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitInfo {
    pub branch: String,
    pub dirty: bool,
}

/// Read `git status --porcelain=v2 --branch` output.
pub fn parse_git_status(text: &str) -> Option<GitInfo> {
    let (mut head, mut oid, mut dirty) = (None, None, false);
    for line in text.lines() {
        if let Some(h) = line.strip_prefix("# branch.head ") {
            head = Some(h.trim().to_string());
        } else if let Some(o) = line.strip_prefix("# branch.oid ") {
            oid = Some(o.trim().to_string());
        } else if !line.starts_with('#') && !line.is_empty() {
            dirty = true;
        }
    }
    let branch = match head?.as_str() {
        "(detached)" => oid?.chars().take(7).collect(),
        name => name.to_string(),
    };
    Some(GitInfo { branch, dirty })
}

/// Git lookups: one at a time per directory, at most every few seconds.
#[derive(Default)]
pub struct GitCache {
    info: HashMap<PathBuf, Option<GitInfo>>,
    running: HashMap<PathBuf, Instant>,
    checked: HashMap<PathBuf, Instant>,
}

const GIT_EVERY: std::time::Duration = std::time::Duration::from_secs(3);
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

impl GitCache {
    pub fn get(&self, dir: &Path) -> Option<&GitInfo> {
        self.info.get(dir)?.as_ref()
    }

    /// Is a new lookup of `dir` due? Marks it started when it is.
    pub fn due(&mut self, dir: &Path) -> bool {
        let now = Instant::now();
        if self.running.get(dir).is_some_and(|t| now.duration_since(*t) < GIT_TIMEOUT * 2) {
            return false;
        }
        if self.checked.get(dir).is_some_and(|t| now.duration_since(*t) < GIT_EVERY) {
            return false;
        }
        self.running.insert(dir.to_path_buf(), now);
        true
    }

    /// Store a lookup's result; true when it changed what is shown.
    pub fn finish(&mut self, dir: PathBuf, info: Option<GitInfo>) -> bool {
        self.running.remove(&dir);
        self.checked.insert(dir.clone(), Instant::now());
        self.info.insert(dir, info.clone()) != Some(info)
    }

    /// Forget directories no session uses any more.
    pub fn retain(&mut self, dirs: &[PathBuf]) {
        self.info.retain(|d, _| dirs.contains(d));
        self.checked.retain(|d, _| dirs.contains(d));
    }
}

/// Ask git about `dir` (in the background; `None` outside a repository or without git).
pub async fn git_lookup(dir: PathBuf) -> Option<GitInfo> {
    let run = tokio::process::Command::new("git")
        .arg("-C")
        .arg(&dir)
        .args(["status", "--porcelain=v2", "--branch", "--untracked-files=no"])
        .env("GIT_OPTIONAL_LOCKS", "0")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = tokio::time::timeout(GIT_TIMEOUT, run).await.ok()?.ok()?;
    if !out.status.success() {
        return None;
    }
    parse_git_status(&String::from_utf8_lossy(&out.stdout))
}

// ---- the server side ----

use super::state::{Client, Session};
use super::Server;

impl Server {
    /// The lines of the status box for `c` in `s`, expanded and with the empty ones left out.
    /// Fills are still markers: they depend on the width the sidebar is drawn at.
    pub fn status_lines(&self, c: &Client, s: &Session) -> Vec<String> {
        if !self.cfg.status.visible {
            return Vec::new();
        }
        let pane = self.focused_pane_in(s).and_then(|id| self.panes.get(&id));
        let git = if s.cwd.as_os_str().is_empty() { None } else { self.git.get(&s.cwd) };
        let st = &self.stats;
        let var = |name: &str, arg: Option<&str>| -> String {
            match name {
                "user" => user(),
                "host" => host(),
                "time" => time_format(arg.unwrap_or("%H:%M")),
                "date" => time_format(arg.unwrap_or("%Y-%m-%d")),
                "session" => s.name.clone(),
                "window" => {
                    let w = s.active_window();
                    w.pinned_name.clone().unwrap_or_else(|| w.auto_name.clone())
                }
                "group" => s.groups.get(s.group_of(s.active)).map(|g| g.name.clone()).unwrap_or_default(),
                "pane_cwd" => pane.and_then(|p| p.pty.cwd()).map(|d| tilde(&d, &crate::project::home_dir())).unwrap_or_default(),
                "pane_command" => pane.and_then(|p| p.pty.process_name()).unwrap_or_default(),
                "sessions" => self.sessions.len().to_string(),
                "windows" => s.windows.len().to_string(),
                "clients" => self.clients.values().filter(|k| k.session == c.session).count().to_string(),
                "load" => st.load.clone(),
                "mem" => st.mem.clone(),
                "mem_percent" => st.mem_percent.clone(),
                "battery" => st.battery.clone(),
                "uptime" => st.uptime.clone(),
                "git" => if git.is_some() { "1".into() } else { String::new() },
                "git_branch" => git.map(|g| g.branch.clone()).unwrap_or_default(),
                "git_dirty" => if git.is_some_and(|g| g.dirty) { "*".into() } else { String::new() },
                _ => String::new(),
            }
        };
        self.cfg
            .status
            .lines
            .iter()
            .map(|t| (t, expand(t, &var)))
            .filter(|(t, e)| visible(t, e))
            .map(|(_, e)| e)
            .collect()
    }

    /// Read the machine's numbers and start due git lookups; true when something shown changed.
    pub fn refresh_status(&mut self) -> bool {
        if !self.cfg.status.visible {
            return false;
        }
        let stats = Stats::sample();
        let mut dirty = std::mem::replace(&mut self.stats, stats) != self.stats;
        if self.cfg.status.lines.iter().any(|l| l.contains("git")) {
            let dirs: Vec<PathBuf> =
                self.sessions.iter().map(|s| s.cwd.clone()).filter(|d| !d.as_os_str().is_empty()).collect();
            self.git.retain(&dirs);
            for dir in dirs {
                if self.git.due(&dir) {
                    let tx = self.events.clone();
                    tokio::spawn(async move {
                        let info = git_lookup(dir.clone()).await;
                        let _ = tx.send(super::Event::Git { dir, info });
                    });
                }
            }
        }
        // The clock and the like: redraw only when a client's text differs from what it last got.
        let texts: Vec<(super::state::ClientId, String)> = self
            .clients
            .values()
            .filter_map(|c| {
                let s = self.sessions.iter().find(|s| Some(s.id) == c.session)?;
                Some((c.id, self.status_lines(c, s).join("\n")))
            })
            .collect();
        for (id, text) in texts {
            if let Some(c) = self.clients.get_mut(&id)
                && c.status_seen != text
            {
                c.status_seen = text;
                dirty = true;
            }
        }
        dirty
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(name: &str, arg: Option<&str>) -> String {
        match name {
            "a" => "A".into(),
            "zero" => "0".into(),
            "time" => format!("T{}", arg.unwrap_or("")),
            _ => String::new(),
        }
    }

    fn ex(t: &str) -> String {
        expand(t, &vars)
    }

    #[test]
    fn expands_variables_arguments_and_escapes() {
        assert_eq!(ex("x #{a} y"), "x A y");
        assert_eq!(ex("#{time:%H:%M}"), "T%H:%M", "only the first colon splits");
        assert_eq!(ex("##{a}"), "#{a}");
        assert_eq!(ex("a#b"), "a#b");
        assert_eq!(ex("#{nope}|"), "|", "unknown names expand to nothing");
        assert_eq!(ex("#{unterminated"), "#{unterminated");
        assert_eq!(ex("#{a} #{unterminated"), "A #{unterminated");
    }

    #[test]
    fn conditions_pick_a_branch() {
        assert_eq!(ex("#{?a,yes,no}"), "yes");
        assert_eq!(ex("#{?nope,yes,no}"), "no");
        assert_eq!(ex("#{?zero,yes,no}"), "no", "0 is false");
        assert_eq!(ex("[#{?nope,yes}]"), "[]", "the else branch is optional");
        assert_eq!(ex("#{?a,#{a}#{a},}"), "AA", "branches expand variables");
        assert_eq!(ex("#{?a,#{?nope,x,y},z}"), "y", "conditions nest");
        assert_eq!(ex("#{?a,1#,2#},}"), "1,2}", "escapes keep a comma and a brace in a branch");
        assert_eq!(ex("#{?a,yes"), "#{?a,yes", "an unclosed condition stays literal");
    }

    #[test]
    fn lines_that_expand_to_nothing_are_dropped_but_blank_ones_stay() {
        assert!(visible("", ""));
        assert!(visible("x #{a}", &ex("x #{a}")));
        assert!(!visible("#{nope}", &ex("#{nope}")));
        assert!(!visible("#{?nope,x,}", &ex("#{?nope,x,}")));
        assert!(!visible("#{fill}#{nope}", &ex("#{fill}#{nope}")), "fill alone is nothing");
    }

    #[test]
    fn fills_share_the_room_that_is_left() {
        let line = |t: &str, w: usize| resolve_fills(&ex(t), w);
        assert_eq!(line("ab#{fill}cd", 8), "ab    cd");
        assert_eq!(line("#{fill}x#{fill}", 5), "  x  ");
        assert_eq!(line("#{fill}x#{fill}", 6), "  x   ", "the odd cell goes to the last fill");
        assert_eq!(line("a#{fill:·}b", 5), "a···b");
        assert_eq!(line("a#{fill:─}b#{fill}c", 7), "a──b  c");
        assert_eq!(line("abcdef#{fill}gh", 4), "abcdefgh", "no room: no fill (the caller cuts it)");
        assert_eq!(line("plain", 9), "plain");
        assert_eq!(line("a#{fill:世}b", 5), "a世 b", "a wide filler leaves a blank for the odd cell");
    }

    #[test]
    fn meminfo_used_is_total_minus_available() {
        let text = "MemTotal:       16000000 kB\nMemFree:  100 kB\nMemAvailable:   10000000 kB\n";
        assert_eq!(parse_meminfo(text), Some((6_000_000, 16_000_000)));
        assert_eq!(parse_meminfo("nothing"), None);
    }

    #[test]
    fn uptime_is_short() {
        assert_eq!(format_uptime(5 * 60 + 3), "5m");
        assert_eq!(format_uptime(3 * 3600 + 120), "3h 2m");
        assert_eq!(format_uptime(3 * 86_400 + 4 * 3600), "3d 4h");
    }

    #[test]
    fn home_becomes_a_tilde() {
        let home = Path::new("/home/me");
        assert_eq!(tilde(Path::new("/home/me"), home), "~");
        assert_eq!(tilde(Path::new("/home/me/src/x"), home), "~/src/x");
        assert_eq!(tilde(Path::new("/home/mexico"), home), "/home/mexico");
        assert_eq!(tilde(Path::new("/tmp"), Path::new("")), "/tmp");
    }

    #[test]
    fn git_status_gives_branch_and_dirtiness() {
        let clean = "# branch.oid 0123456789abcdef\n# branch.head main\n# branch.upstream origin/main\n";
        assert_eq!(parse_git_status(clean), Some(GitInfo { branch: "main".into(), dirty: false }));
        let dirty = "# branch.oid 0123456789abcdef\n# branch.head feat/x\n1 .M N... 100644 100644 100644 a b src/x.rs\n";
        assert_eq!(parse_git_status(dirty), Some(GitInfo { branch: "feat/x".into(), dirty: true }));
        let detached = "# branch.oid 0123456789abcdef\n# branch.head (detached)\n";
        assert_eq!(parse_git_status(detached), Some(GitInfo { branch: "0123456".into(), dirty: false }));
        assert_eq!(parse_git_status(""), None);
    }

    #[test]
    fn git_lookups_are_throttled() {
        let mut g = GitCache::default();
        let dir = Path::new("/x");
        assert!(g.due(dir));
        assert!(!g.due(dir), "one at a time");
        assert!(g.finish(dir.to_path_buf(), Some(GitInfo { branch: "m".into(), dirty: false })));
        assert!(!g.due(dir), "not again right away");
        assert!(!g.finish(dir.to_path_buf(), Some(GitInfo { branch: "m".into(), dirty: false })), "same result: nothing changed");
        assert_eq!(g.get(dir).map(|i| i.branch.as_str()), Some("m"));
    }
}
