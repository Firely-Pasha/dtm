//! Looking at what is running in a pane, to decide who gets navigation keys.

/// One process from `/proc/<pid>/stat`.
#[derive(Debug, PartialEq, Eq)]
pub struct Proc {
    pub comm: String,
    pub state: char,
    pub pgrp: i32,
}

/// Parse a `/proc/<pid>/stat` line. The command name sits in parentheses and
/// may itself contain spaces and parentheses, so find the *last* `)`.
pub fn parse_stat(line: &str) -> Option<Proc> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close < open {
        return None;
    }
    let comm = line[open + 1..close].to_string();
    // After the name: state ppid pgrp ...
    let mut rest = line[close + 1..].split_whitespace();
    let state = rest.next()?.chars().next()?;
    let _ppid = rest.next()?;
    let pgrp = rest.next()?.parse().ok()?;
    Some(Proc { comm, state, pgrp })
}

/// Command names of the live (not stopped, dead or zombie) members of a process group.
pub fn group_commands(pgrp: i32) -> Vec<String> {
    let Ok(dir) = std::fs::read_dir("/proc") else { return Vec::new() };
    dir.flatten()
        .filter(|e| e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()))
        .filter_map(|e| std::fs::read_to_string(e.path().join("stat")).ok())
        .filter_map(|s| parse_stat(&s))
        .filter(|p| p.pgrp == pgrp && !matches!(p.state, 'T' | 't' | 'X' | 'x' | 'Z'))
        .map(|p| p.comm)
        .collect()
}

/// Is this program one that handles Ctrl-h/j/k/l navigation itself?
///
/// This is the pattern vim-tmux-navigator uses,
/// `g?\.?(view|l?n?vim?x?|fzf)(diff)?(-wrapped)?`, plus any `extra` names.
pub fn is_editor(comm: &str, extra: &[String]) -> bool {
    let name = comm.rsplit('/').next().unwrap_or(comm);
    if extra.iter().any(|e| e == name) {
        return true;
    }
    let mut s = name;
    if let Some(r) = s.strip_prefix('g') {
        s = r;
    }
    if let Some(r) = s.strip_prefix('.') {
        s = r;
    }
    if let Some(r) = s.strip_suffix("-wrapped") {
        s = r;
    }
    if let Some(r) = s.strip_suffix("diff") {
        s = r;
    }
    if s == "view" || s == "fzf" {
        return true;
    }
    // l?n?vim?x?
    let s = s.strip_prefix('l').unwrap_or(s);
    let s = s.strip_prefix('n').unwrap_or(s);
    let Some(s) = s.strip_prefix("vi") else { return false };
    let s = s.strip_prefix('m').unwrap_or(s);
    let s = s.strip_prefix('x').unwrap_or(s);
    s.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_lines() {
        let p = parse_stat("1234 (nvim) S 1200 1234 1200 34816 1234 4194304 ...").unwrap();
        assert_eq!(p, Proc { comm: "nvim".into(), state: 'S', pgrp: 1234 });
        // the name may contain spaces and parentheses
        let p = parse_stat("99 (a b) c) T 1 77 1 0").unwrap();
        assert_eq!(p, Proc { comm: "a b) c".into(), state: 'T', pgrp: 77 });
        assert!(parse_stat("garbage").is_none());
        assert!(parse_stat("1 (x) S 2").is_none());
    }

    #[test]
    fn recognizes_editors() {
        for name in [
            "vim", "nvim", "vi", "view", "vimdiff", "nvimdiff", "gvim", "lvim", "vimx", "nvimx", ".nvim-wrapped",
            ".vim-wrapped", "fzf", "/usr/bin/nvim", "gview", "lnvim",
        ] {
            assert!(is_editor(name, &[]), "{name}");
        }
        for name in ["bash", "sh", "vimperator", "nano", "less", "emacs", "vimm", "", "nvimdiffx", "evim", "cat"] {
            assert!(!is_editor(name, &[]), "{name}");
        }
    }

    #[test]
    fn extra_names_are_editors_too() {
        let extra = vec!["helix".to_string(), "hx".to_string()];
        assert!(is_editor("hx", &extra));
        assert!(is_editor("helix", &extra));
        assert!(!is_editor("hx", &[]));
    }

    #[test]
    fn finds_this_process_group() {
        // SAFETY: plain syscall.
        let pgrp = unsafe { libc::getpgrp() };
        let me = std::fs::read_to_string("/proc/self/comm").unwrap();
        assert!(group_commands(pgrp).iter().any(|c| *c == me.trim()), "{me}");
        assert!(group_commands(i32::MAX).is_empty());
    }
}
