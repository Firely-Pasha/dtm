//! End-to-end tests: the real `dtm` binary on a pty, watched through an emulator.

mod common;

use std::path::PathBuf;
use std::process::Command;

use common::Harness;

const CTRL_A: &str = "\x01";

struct Env {
    dir: PathBuf,
    sock: PathBuf,
    config: PathBuf,
}

impl Env {
    fn new(name: &str, config: &str) -> Env {
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("e2e-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let env = Env { sock: dir.join("s.sock"), config: dir.join("config.toml"), dir };
        std::fs::write(&env.config, config).unwrap();
        env
    }

    fn vars(&self) -> Vec<(&'static str, String)> {
        vec![
            ("DTM_SOCKET", self.sock.display().to_string()),
            ("DTM_CONFIG", self.config.display().to_string()),
            ("XDG_STATE_HOME", self.dir.join("state").display().to_string()),
            ("SHELL", "/bin/sh".to_string()),
            ("PS1", "$ ".to_string()),
            ("HOME", self.dir.display().to_string()),
        ]
    }

    fn client(&self, args: &[&str], cols: u16, rows: u16) -> Harness {
        self.client_in(None, args, cols, rows)
    }

    fn client_in(&self, cwd: Option<&std::path::Path>, args: &[&str], cols: u16, rows: u16) -> Harness {
        let vars = self.vars();
        let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        Harness::spawn_in(env!("CARGO_BIN_EXE_dtm"), args, &pairs, cols, rows, cwd)
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        self.run_with(args, &[])
    }

    fn run_with(&self, args: &[&str], extra: &[(&str, &str)]) -> (bool, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dtm"));
        cmd.args(args).env_remove("DTM_PANE");
        for (k, v) in self.vars() {
            cmd.env(k, v);
        }
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let out = cmd.output().unwrap();
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        (out.status.success(), text)
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = self.run(&["kill-server"]);
    }
}

#[tokio::test]
async fn attach_split_detach_reattach() {
    let env = Env::new("basic", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send("echo hi$((1+2))\r");
    h.wait_text("hi3").await;
    // sidebar on the left: header, mode label and a separator column
    let text = h.text();
    assert!(text.lines().next().unwrap().contains("0"), "{text}");
    assert!(text.contains("windows"));
    assert_eq!(h.screen.cell(23, 5).ch, '│');

    h.send(&format!("{CTRL_A}|"));
    h.send("echo second$((2+2))\r");
    h.wait_text("second4").await;
    assert!(h.text().contains("hi3"), "first pane still visible:\n{}", h.text());

    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
    let (ok, out) = env.run(&["ls"]);
    assert!(ok && out.contains("0: 1 window"), "{out}");

    let mut h2 = env.client(&["attach"], 100, 30);
    h2.wait_text("second4").await;
    assert!(h2.text().contains("hi3"));
    h2.send(&format!("{CTRL_A}d"));
    h2.wait_exit().await;
}

#[tokio::test]
async fn windows_sidebar_modes_and_rename() {
    let env = Env::new("windows", "");
    let mut h = env.client(&["new", "-s", "work"], 100, 30);
    h.wait_text("work").await;
    h.send(&format!("{CTRL_A}c"));
    h.wait_text("2 sh").await;
    assert!(h.text().contains("1 sh"));

    h.send(&format!("{CTRL_A},"));
    h.wait_text("rename window: sh").await;
    h.send("\x7f\x7f");
    h.send("editor\r");
    h.wait_text("2 editor").await;

    // switch back to window 1 with the digit key, then toggle the list
    h.send(&format!("{CTRL_A}1"));
    h.send(&format!("{CTRL_A}s"));
    h.wait_text("sessions").await;
    assert!(h.text().contains("1 work"), "{}", h.text());
    h.send(&format!("{CTRL_A}s"));
    h.wait_text("2 editor").await;

    h.send(&format!("{CTRL_A}$"));
    h.wait_text("rename session: work").await;
    h.send("\x1b");
    h.wait_for("prompt cancelled", |t| !t.contains("rename session")).await;

    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
    let (_, out) = env.run(&["ls"]);
    assert!(out.contains("work: 2 windows"), "{out}");
}

#[tokio::test]
async fn sidebar_on_the_right_and_reload() {
    let env = Env::new("right", "[sidebar]\nposition = \"right\"\nwidth = 20\n");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    // separator is the first column of the 20-wide sidebar at x = 80
    assert_eq!(h.screen.cell(80, 5).ch, '│');
    assert_ne!(h.screen.cell(20, 5).ch, '│');

    std::fs::write(&env.config, "[sidebar]\nposition = \"left\"\nwidth = 20\n").unwrap();
    let (ok, out) = env.run(&["reload"]);
    assert!(ok, "{out}");
    h.wait_cell(19, 5, '│').await;
    assert_ne!(h.screen.cell(80, 5).ch, '│');
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn broken_config_is_rejected_on_reload() {
    let env = Env::new("badreload", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    std::fs::write(&env.config, "[keys.prefix]\n\"x\" = \"bogus\"\n").unwrap();
    let (ok, out) = env.run(&["reload"]);
    assert!(!ok && out.contains("bogus"), "{out}");
    // the running server still works
    h.send("echo alive$((1+1))\r");
    h.wait_text("alive2").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn custom_prefix_and_bindings() {
    let env = Env::new(
        "keys",
        "[options]\nprefix = \"C-b\"\n[keys.prefix]\n\"w\" = \"new-window\"\n\"c\" = \"none\"\n[keys.root]\n\"M-1\" = \"select-window 1\"\n",
    );
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send("\x02w");
    h.wait_text("2 sh").await;
    // the old prefix is now plain input (Ctrl-A moves to line start in the shell: no effect on screen)
    h.send(&format!("{CTRL_A}c\x15"));
    // C-b c is unbound: nothing happens
    h.send("\x02c");
    h.send("echo marker$((3+4))\r");
    h.wait_text("marker7").await;
    assert!(!h.text().contains("3 sh"), "{}", h.text());
    // root binding: Alt-1 goes to window 1 where marker7 is not visible
    h.send("\x1b1");
    h.wait_for("window 1 shown", |t| !t.contains("marker7")).await;
    h.send("\x02d");
    h.wait_exit().await;
}

#[tokio::test]
async fn send_prefix_and_zoom_and_kill_pane() {
    let env = Env::new("zoom", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}-"));
    h.send("echo bottom$((1+1))\r");
    h.wait_text("bottom2").await;
    let rows: Vec<String> = h.screen.text();
    assert!(rows.iter().any(|r| r.contains('─')), "horizontal border drawn:\n{}", h.text());

    h.send(&format!("{CTRL_A}z"));
    h.wait_for("zoomed", |t| !t.contains('─')).await;
    assert!(h.text().contains("Z"), "zoom marker in the sidebar");
    h.send(&format!("{CTRL_A}z"));
    h.wait_for("unzoomed", |t| t.contains('─')).await;

    h.send(&format!("{CTRL_A}x"));
    h.wait_for("pane closed", |t| !t.contains('─')).await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn exiting_every_shell_ends_the_server() {
    let env = Env::new("exit", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}c"));
    h.wait_text("2 sh").await;
    h.send("exit\r");
    h.wait_for("one window left", |t| !t.contains("2 sh")).await;
    h.send("exit\r");
    h.wait_exit().await;
    assert!(h.text().contains("exited") || h.exited);
    for _ in 0..50 {
        if !env.run(&["ls"]).0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    panic!("server still running after the last shell exited");
}

#[tokio::test]
async fn latest_active_client_decides_the_size() {
    let env = Env::new("sizes", "");
    let mut a = env.client(&[], 100, 30);
    a.wait_text("1 sh").await;
    a.send("stty size\r");
    a.wait_text("30 76").await;

    let mut b = env.client(&["attach"], 124, 40);
    b.wait_text("1 sh").await;
    b.send("stty size\r");
    b.wait_text("40 100").await;
    a.send("stty size\r");
    a.wait_text("30 76\n").await;
    a.send(&format!("{CTRL_A}d"));
    a.wait_exit().await;
    b.send(&format!("{CTRL_A}d"));
    b.wait_exit().await;
}

#[tokio::test]
async fn named_sessions_and_errors() {
    let env = Env::new("named", "");
    let mut h = env.client(&["new", "-s", "alpha"], 100, 30);
    h.wait_text("alpha").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;

    let (ok, out) = env.run(&["attach", "-t", "nope"]);
    assert!(!ok);
    let _ = out;
    // `attach` without a tty fails cleanly rather than hanging
    let (ok, out) = env.run(&["new", "-s", "beta"]);
    assert!(!ok && out.contains("terminal"), "{out}");

    let mut h2 = env.client(&["new", "-s", "alpha"], 100, 30);
    h2.wait_exit().await;
    assert!(h2.text().contains("duplicate session"), "{}", h2.text());

    let mut h3 = env.client(&["new", "-s", "beta"], 100, 30);
    h3.wait_text("beta").await;
    h3.send(&format!("{CTRL_A}s"));
    h3.wait_text("2 beta").await;
    assert!(h3.text().contains("1 alpha"));
    h3.send(&format!("{CTRL_A}("));
    h3.wait_text("alpha").await;
    h3.send(&format!("{CTRL_A}d"));
    h3.wait_exit().await;
}

/// Parse the last `stty size` result ("ROWS COLS") visible in a pane area.
fn last_size(text: &str, rows: usize) -> Option<usize> {
    text.lines()
        .filter_map(|l| l.rsplit('│').next())
        .filter_map(|l| {
            let toks: Vec<&str> = l.split_whitespace().collect();
            let [.., r, c] = toks[..] else { return None };
            (r.parse::<usize>().ok()? == rows).then(|| c.parse().ok())?
        })
        .next_back()
}

#[tokio::test]
async fn focus_and_resize_panes() {
    let env = Env::new("resize", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|"));
    h.send("stty size\r");
    h.wait_for("size of new pane", |t| last_size(t, 30).is_some()).await;
    let right = last_size(&h.text(), 30).unwrap();
    assert_eq!(right, 37);

    // moving the border left grows the right pane
    h.send(&format!("{CTRL_A}H"));
    h.send("stty size\r");
    h.wait_for("wider pane", |t| last_size(t, 30).is_some_and(|c| c > right)).await;

    // focus the left pane and type there, then back right and with focus-next
    h.send(&format!("{CTRL_A}\x1b[D"));
    h.send("echo left$((1+1))\r");
    h.wait_text("left2").await;
    h.send(&format!("{CTRL_A}\x1b[C"));
    h.send("echo right$((1+2))\r");
    h.wait_text("right3").await;
    h.send(&format!("{CTRL_A}o"));
    h.send("echo next$((2+2))\r");
    h.wait_text("next4").await;
    // the text landed in different panes: the left pane shows left2 and next4
    let text = h.text();
    let line = text.lines().find(|l| l.contains("next4")).unwrap();
    let before_border = line.split('│').nth(1).unwrap_or("");
    assert!(before_border.contains("next4"), "{text}");
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn fullscreen_apps_work_inside_dtm() {
    let env = Env::new("apps", "");
    let file = env.dir.join("lines.txt");
    let content: String = (1..=200).map(|i| format!("line-{i}\n")).collect();
    std::fs::write(&file, content).unwrap();

    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("less {}\r", file.display()));
    h.wait_for("first page", |t| t.lines().any(|l| l.ends_with("line-1"))).await;
    // application cursor mode is mirrored to the outer terminal
    h.send("\x1bOB");
    h.wait_for("scrolled one line", |t| !t.lines().any(|l| l.ends_with("line-1"))).await;
    h.send(" ");
    h.wait_for("next page", |t| t.contains("line-40")).await;
    h.send("q");
    h.wait_for("back at the prompt", |t| !t.contains("line-40")).await;

    if Command::new("vim").arg("--version").output().is_ok() {
        h.send("vim -u NONE -N\r");
        h.wait_for("vim", |t| t.contains("VIM")).await;
        h.send("ihello vim\x1b");
        h.wait_text("hello vim").await;
        h.send(":q!\r");
        h.wait_for("vim closed", |t| !t.contains("hello vim")).await;
    }
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn copy_mode_scrolls_yanks_and_pastes() {
    let env = Env::new("copy", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send("seq 1 100\r");
    h.wait_text("100").await;

    h.send(&format!("{CTRL_A}["));
    h.wait_text("[copy 0/").await;
    h.send("g");
    h.wait_for("top of history", |t| t.contains("seq 1 100") && t.contains("[copy")).await;
    h.send("y");
    h.wait_text("copied 11 characters").await;
    h.wait_for("left copy mode", |t| !t.contains("[copy")).await;

    h.send(&format!("{CTRL_A}]"));
    h.wait_text("$ $ seq 1 100").await;
    h.send("\x15");

    // selection across lines: select two lines near the top and yank
    h.send(&format!("{CTRL_A}[g"));
    h.send("vjy");
    h.wait_text("copied").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn mouse_clicks_wheel_and_forwarding() {
    let env = Env::new("mouse", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    // the outer terminal is asked for SGR mouse reports
    h.wait_for("mouse enabled", |_| true).await;
    assert!(h.screen.modes().mouse != dtm::term::screen::MouseTracking::Off && h.screen.modes().mouse_sgr);

    // sidebar: second window, then click back and forth
    h.send(&format!("{CTRL_A}c"));
    h.wait_text("2 sh").await;
    h.send("echo w2marker$((1+1))\r");
    h.wait_text("w2marker2").await;
    h.send("\x1b[<0;5;3M\x1b[<0;5;3m"); // click "1 sh" (row 3, 1-based)
    h.wait_for("window 1 shown", |t| !t.contains("w2marker2")).await;
    h.send("\x1b[<0;5;4M\x1b[<0;5;4m"); // click "2 sh"
    h.wait_text("w2marker2").await;

    // clicking a pane focuses it
    h.send(&format!("{CTRL_A}|"));
    h.send("\x1b[<0;30;6M\x1b[<0;30;6m"); // left pane
    h.send("echo clicked$((1+1))\r");
    h.wait_text("clicked2").await;
    let text = h.text();
    let line = text.lines().find(|l| l.contains("clicked2")).unwrap();
    let parts: Vec<&str> = line.split('│').collect();
    assert!(parts.len() >= 3 && parts[1].contains("clicked2"), "{text}");

    // wheel up with plain output enters copy mode; wheel down returns
    h.send("seq 1 200\r");
    h.wait_text("200").await;
    h.send("\x1b[<64;30;10M");
    h.wait_text("[copy").await;
    h.send("\x1b[<65;30;10M".repeat(80).as_str());
    h.wait_for("left copy mode", |t| !t.contains("[copy")).await;

    // applications that enable mouse reporting get pane-relative events
    h.send("printf '\\033[?1000h\\033[?1006h'; cat -v\r");
    h.wait_text("2 cat").await;
    h.send("\x1b[<0;34;7M");
    h.wait_text("^[[<0;").await;
    h.send("\x03");
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn mouse_can_be_disabled() {
    let env = Env::new("nomouse", "[options]\nmouse = false\n");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send("echo ready$((1+1))\r");
    h.wait_text("ready2").await;
    assert_eq!(h.screen.modes().mouse, dtm::term::screen::MouseTracking::Off);
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

async fn rename_window(h: &mut Harness, new: &str, old_len: usize) {
    h.send(&format!("{CTRL_A},"));
    h.wait_text("rename window").await;
    h.send(&"\x7f".repeat(old_len));
    h.send(&format!("{new}\r"));
    h.wait_for("prompt closed", |t| !t.contains("rename window")).await;
}

#[tokio::test]
async fn prompts_span_the_whole_bottom_row() {
    let env = Env::new("promptbar", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A},"));
    h.wait_text("rename window: sh").await;
    let row = h.screen.row_text(29);
    assert!(row.starts_with(" rename window: sh"), "{row:?}");
    // the bar covers the sidebar column and the pane area, edge to edge
    for x in [0, 10, 23, 24, 60, 99] {
        assert!(h.screen.cell(x, 29).attrs.has(dtm::term::cell::flags::REVERSE), "column {x}");
    }
    assert_eq!(h.screen.cursor_pos().1, 29);
    h.send("\x1b");
    h.wait_for("closed", |t| !t.contains("rename window")).await;
    // with the sidebar hidden it is the same bar
    h.send(&format!("{CTRL_A}b{CTRL_A},"));
    h.wait_text("rename window").await;
    assert!(h.screen.row_text(29).starts_with(" rename window"));
    h.send("\x1b");
    h.wait_for("closed again", |t| !t.contains("rename window")).await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn plain_keys_keep_resizing_for_a_moment() {
    let env = Env::new("repeat", "[options]\nrepeat_time = 400\n");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|"));
    h.send("stty size\r");
    h.wait_for("initial size", |t| last_size(t, 30).is_some()).await;
    let start = last_size(&h.text(), 30).unwrap();

    // prefix + H once, then plain h h (1 cell each): 5 + 1 + 1 cells wider
    h.send(&format!("{CTRL_A}Hhh"));
    h.send("stty size\r");
    h.wait_for("wider", |t| last_size(t, 30).is_some_and(|c| c >= start + 6)).await;
    let wide = last_size(&h.text(), 30).unwrap();
    assert!(wide <= start + 8, "{wide} vs {start}");

    // a non-resize key ends the repeat; and after the time has passed h is plain input
    h.send(&format!("{CTRL_A}H"));
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    h.send("hello\r");
    h.wait_text("hello: not found").await;
    h.send("stty size\r");
    h.wait_for("size after expiry", |t| last_size(t, 30).is_some_and(|c| c > wide)).await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn repeat_can_be_disabled() {
    let env = Env::new("norepeat", "[options]\nrepeat_time = 0\n");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|{CTRL_A}Hhello\r"));
    h.wait_text("hello: not found").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn shift_v_selects_whole_lines() {
    let env = Env::new("vline", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send("seq 1 100\r");
    h.wait_text("100").await;
    h.send(&format!("{CTRL_A}[g"));
    h.wait_text("[copy").await;
    // the first two history lines are "$ seq 1 100" and "1"
    h.send("lllVjy");
    h.wait_text("copied 13 characters").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

const SIDEBAR_HINT: &str = "j/k ⏎ x r n / e q";

#[tokio::test]
async fn sidebar_takes_focus_from_the_panes() {
    let env = Env::new("sbfocus", "[keys.root]\n\"C-h\" = \"focus-left\"\n\"C-l\" = \"focus-right\"\n");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;

    // directional focus with nothing to the left goes into the sidebar...
    h.send("\x08");
    h.wait_text(SIDEBAR_HINT).await;
    assert_eq!(h.screen.cell(0, 2).ch, '›');
    // ...and the opposite direction returns to the pane
    h.send("\x0c");
    h.wait_for("left sidebar", |t| !t.contains(SIDEBAR_HINT)).await;

    // the prefix keys do the same
    h.send(&format!("{CTRL_A}\x1b[D"));
    h.wait_text(SIDEBAR_HINT).await;
    h.send(&format!("{CTRL_A}\x1b[C"));
    h.wait_for("back in the pane", |t| !t.contains(SIDEBAR_HINT)).await;

    // with two panes, focus-left from the right one reaches the left pane first
    h.send(&format!("{CTRL_A}|"));
    h.send(&format!("{CTRL_A}\x1b[D"));
    h.send("echo left$((2+3))\r");
    h.wait_text("left5").await;
    assert!(!h.text().contains(SIDEBAR_HINT));
    h.send(&format!("{CTRL_A}\x1b[D"));
    h.wait_text(SIDEBAR_HINT).await;
    // keys the sidebar does not know are swallowed instead of reaching the shell
    h.send("z");
    h.send("q");
    h.wait_for("left", |t| !t.contains(SIDEBAR_HINT)).await;
    assert!(!h.text().contains("$ z"));
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn sidebar_manages_windows() {
    let env = Env::new("sbwin", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    rename_window(&mut h, "A", 2).await;
    h.wait_text("1 A").await;
    h.send(&format!("{CTRL_A}c"));
    h.wait_text("2 sh").await;
    rename_window(&mut h, "B", 2).await;
    h.send(&format!("{CTRL_A}c"));
    h.wait_text("3 sh").await;
    rename_window(&mut h, "C", 2).await;
    h.wait_for("three named windows", |t| t.contains("1 A") && t.contains("2 B") && t.contains("3 C")).await;
    h.send("echo marker$((6*7))\r");
    h.wait_text("marker42").await;

    // reorder: move C up one place
    h.send(&format!("{CTRL_A}w"));
    h.wait_text(SIDEBAR_HINT).await;
    h.send("K");
    h.wait_for("reordered", |t| t.contains("2 C") && t.contains("3 B")).await;
    assert!(h.text().contains("marker42"), "the active window followed the move");

    // kill with confirmation: n cancels, y kills
    h.send("x");
    h.wait_text("kill window 2 (C)? (y/n)").await;
    assert!(h.screen.row_text(29).starts_with(" kill window 2 (C)?"));
    h.send("n");
    h.wait_for("cancelled", |t| !t.contains("(y/n)")).await;
    assert!(h.text().contains("2 C"));
    h.send("jx");
    h.wait_text("kill window 3 (B)? (y/n)").await;
    h.send("y");
    h.wait_for("B gone", |t| !t.contains("3 B")).await;
    assert!(h.text().contains("1 A") && h.text().contains("2 C"));

    // moving the selection shows that window right away (here: window 1)
    h.send("k");
    h.wait_for("window 1 shown", |t| !t.contains("marker42")).await;
    h.send("r");
    h.wait_text("rename window: A").await;
    h.send("\x7fZ\r");
    h.wait_for("renamed", |t| t.contains("1 Z") && t.contains("2 C")).await;

    // open the selected window and leave the sidebar
    h.send("\r");
    h.wait_for("left sidebar", |t| !t.contains(SIDEBAR_HINT)).await;
    assert!(!h.text().contains("marker42"), "still on window 1");

    // new window from the sidebar
    h.send(&format!("{CTRL_A}w"));
    h.send("n");
    h.wait_text("3 sh").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn sidebar_manages_sessions() {
    let env = Env::new("sbsess", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}s"));
    h.wait_text("sessions").await;
    h.send(&format!("{CTRL_A}S"));
    h.wait_text("2 1").await;

    // select the first session and kill it
    h.send(&format!("{CTRL_A}wk"));
    h.send("x");
    h.wait_text("kill session 1 (0)? (y/n)").await;
    h.send("y");
    h.wait_for("session 0 gone", |t| t.contains("1 1") && !t.contains("2 1")).await;
    let (ok, out) = env.run(&["ls"]);
    assert!(ok && out.contains("1: 1 window") && !out.contains("0: "), "{out}");

    // sessions can be created, renamed and reordered from the sidebar
    h.send("n");
    h.wait_text("2 0").await;
    h.send("r");
    h.wait_text("rename session: 0").await;
    h.send("\x7fwork\r");
    h.wait_text("2 work").await;
    h.send("K");
    h.wait_for("moved up", |t| t.contains("1 work") && t.contains("2 1")).await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn sidebar_selection_shows_the_window_as_it_is_highlighted() {
    let env = Env::new("follow", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send("echo one$((1+0))\r");
    h.wait_text("one1").await;
    h.send(&format!("{CTRL_A}c"));
    h.wait_text("2 sh").await;
    h.send("echo two$((1+1))\r");
    h.wait_text("two2").await;

    h.send(&format!("{CTRL_A}w"));
    h.wait_text(SIDEBAR_HINT).await;
    h.send("k");
    h.wait_for("window 1 shown", |t| t.contains("one1") && !t.contains("two2")).await;
    h.send("j");
    h.wait_for("window 2 shown", |t| t.contains("two2") && !t.contains("one1")).await;
    // leaving keeps what is shown
    h.send("k");
    h.wait_text("one1").await;
    h.send("q");
    h.wait_for("left", |t| !t.contains(SIDEBAR_HINT)).await;
    assert!(h.text().contains("one1"));
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn sidebar_follow_can_be_turned_off() {
    let env = Env::new("nofollow", "[sidebar]\nfollow = false\n");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send("echo one$((1+0))\r");
    h.wait_text("one1").await;
    h.send(&format!("{CTRL_A}c"));
    h.wait_text("2 sh").await;
    h.send("echo two$((1+1))\r");
    h.wait_text("two2").await;
    h.send(&format!("{CTRL_A}wk"));
    h.wait_text(SIDEBAR_HINT).await;
    // the selection moved but the window did not change until Enter
    h.wait_cell(0, 2, '›').await;
    assert!(h.text().contains("two2") && !h.text().contains("one1"));
    h.send("\r");
    h.wait_text("one1").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

/// The text of the last pane column: everything after the last vertical line on each row.
fn right_pane_text(h: &Harness) -> String {
    h.text().lines().map(|l| l.rsplit('│').next().unwrap_or("")).collect::<Vec<_>>().join("\n")
}

#[tokio::test]
async fn panes_get_their_dtm_environment_and_cli() {
    let env = Env::new("paneenv", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send("echo ENV:$DTM_PANE:$(basename \"$DTM_SOCKET\"):$($DTM_BIN query '#{window_zoomed_flag}#{pane_at_left}#{pane_at_right}')\r");
    h.wait_text("ENV:1:s.sock:011").await;

    h.send(&format!("{CTRL_A}|"));
    h.send("echo P:$DTM_PANE:$($DTM_BIN query '#{pane_at_left}#{pane_at_right}#{pane_id}')\r");
    h.wait_text("P:2:012").await;
    // the pane can move focus with the CLI, as an editor plugin does
    h.send("$DTM_BIN select-pane -L; echo SYNC$((10+1))\r");
    h.wait_text("SYNC11").await;
    h.send("echo LEFT$((1+1))\r");
    h.wait_text("LEFT2").await;
    assert!(!right_pane_text(&h).contains("LEFT2"), "typed into the left pane:\n{}", h.text());
    // and an explicit target works from anywhere
    h.send("$DTM_BIN select-pane -t 1 -R; echo SYNC$((10+2))\r");
    h.wait_text("SYNC12").await;
    h.send("echo RIGHT$((1+2))\r");
    h.wait_text("RIGHT3").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;

    let (ok, out) = env.run(&["select-pane", "-L"]);
    assert!(!ok && out.contains("not inside a dtm pane"), "{out}");
    let (ok, out) = env.run(&["query", "-t", "99", "#{pane_id}"]);
    assert!(!ok && out.contains("no such pane"), "{out}");
}

#[tokio::test]
async fn ctrl_hjkl_move_between_panes_and_into_the_sidebar() {
    let env = Env::new("ctrlnav", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|"));
    h.send("\x08");
    h.send("echo L$((1+1))\r");
    h.wait_text("L2").await;
    assert!(!right_pane_text(&h).contains("L2"), "Ctrl-h went to the left pane");
    h.send("\x0c");
    h.send("echo R$((1+2))\r");
    h.wait_text("R3").await;
    assert!(right_pane_text(&h).contains("R3"), "Ctrl-l went back to the right pane");

    // Ctrl-h twice: left pane, then the sidebar; Ctrl-l comes back out
    h.send("\x08\x08");
    h.wait_text(SIDEBAR_HINT).await;
    h.send("\x0c");
    h.wait_for("left sidebar", |t| !t.contains(SIDEBAR_HINT)).await;

    // vertical: Ctrl-k / Ctrl-j between stacked panes
    h.send(&format!("{CTRL_A}c"));
    h.wait_text("2 sh").await;
    h.send(&format!("{CTRL_A}-"));
    h.send("\x0b");
    h.send("echo UP$((2+2))\r");
    h.wait_text("UP4").await;
    h.send("\x0a");
    h.send("echo DOWN$((2+3))\r");
    h.wait_text("DOWN5").await;
    let text = h.text();
    let row = |needle: &str| text.lines().position(|l| l.contains(needle)).unwrap();
    assert!(row("UP4") < row("DOWN5"), "{text}");

    // <prefix> Ctrl-l sends a real Ctrl-l to the pane
    h.send("cat -v\r");
    h.send(&format!("{CTRL_A}\x0c\r"));
    h.wait_text("^L").await;
    h.send("\x03");
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn focus_last_toggles_between_two_panes() {
    let env = Env::new("focuslast", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|"));
    h.send(&format!("{CTRL_A};"));
    h.send("echo A$((1+1))\r");
    h.wait_text("A2").await;
    assert!(!right_pane_text(&h).contains("A2"));
    h.send(&format!("{CTRL_A};"));
    h.send("echo B$((1+2))\r");
    h.wait_text("B3").await;
    assert!(right_pane_text(&h).contains("B3"));
    h.send("$DTM_BIN select-pane -l; echo SYNC$((10+3))\r");
    h.wait_text("SYNC13").await;
    h.send("echo C$((2+2))\r");
    h.wait_text("C4").await;
    assert!(!right_pane_text(&h).contains("C4"));
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn zoom_is_visible_to_queries() {
    let env = Env::new("zoomq", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|{CTRL_A}z"));
    h.send("echo Z=$($DTM_BIN query '#{window_zoomed_flag}')\r");
    h.wait_text("Z=1").await;
    // moving focus from a zoomed pane unzooms the window
    h.send("$DTM_BIN select-pane -L; echo SYNC$((10+4))\r");
    h.wait_text("SYNC14").await;
    h.send("echo Z=$($DTM_BIN query '#{window_zoomed_flag}')\r");
    h.wait_text("Z=0").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

fn has_nvim() -> bool {
    Command::new("nvim").arg("--version").output().is_ok()
}

const PLUGIN_DIR: &str = env!("CARGO_MANIFEST_DIR");

#[tokio::test]
async fn neovim_plugin_hands_over_at_its_edge() {
    if !has_nvim() {
        return;
    }
    let env = Env::new("nvimnav", "");
    let mut h = env.client(&[], 120, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|{CTRL_A}\x1b[D"));
    h.send(&format!("nvim -u NORC --cmd 'set rtp+={PLUGIN_DIR}'\r"));
    h.wait_for("nvim", |t| t.matches('~').count() > 5).await;
    // 120 columns: sidebar 24, left pane 48, border, right pane from column 73
    const RIGHT: usize = 73;

    // Ctrl-l reaches nvim (it is in the foreground); nvim has no window to the
    // right and so asks dtm: focus moves to the right-hand shell pane.
    h.send("\x0c");
    h.wait_cursor("right pane", |x, _| x >= RIGHT).await;
    h.send("echo R$((1+1))\r");
    h.wait_text("R2").await;
    assert!(right_pane_text(&h).contains("R2"));

    // Ctrl-h from a plain shell pane is handled by dtm directly
    h.send("\x08");
    h.wait_cursor("left pane", |x, _| x < RIGHT).await;
    // inside nvim: split, then Ctrl-l moves between nvim's windows only
    h.send(":vsplit\r");
    h.wait_for("split", |t| t.lines().filter(|l| l.matches('│').count() >= 3).count() > 10).await;
    h.send("\x0c");
    h.send("ihere\x1b");
    h.wait_text("here").await;
    h.wait_cursor("still in nvim", |x, _| x < RIGHT).await;
    assert!(!right_pane_text(&h).contains("here"), "stayed in nvim's right window");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    // now at nvim's right edge: the next Ctrl-l crosses into the dtm pane
    h.send("\x0c");
    h.wait_cursor("right pane again", |x, _| x >= RIGHT).await;
    h.send("echo R$((2+3))\r");
    h.wait_text("R5").await;
    assert!(right_pane_text(&h).contains("R5"));

    // back to nvim and quit it
    h.send("\x08");
    h.wait_cursor("left pane again", |x, _| x < RIGHT).await;
    h.send(":qa!\r");
    h.wait_for("nvim closed", |t| !t.contains("here")).await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn neovim_plugin_commands_work_without_a_key_press() {
    if !has_nvim() {
        return;
    }
    let env = Env::new("nvimcmd", "");
    let mut h = env.client(&[], 120, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|{CTRL_A}\x1b[D"));
    // headless nvim in the left pane asks dtm to move right, then exits
    h.send(&format!(
        "nvim --headless -u NORC --cmd 'set rtp+={PLUGIN_DIR}' -c DtmNavigateRight -c 'qa!'; echo DONE$((10+1))\r"
    ));
    h.wait_text("DONE11").await;
    h.wait_cursor("right pane", |x, _| x >= 73).await;
    h.send("echo H$((4+4))\r");
    h.wait_text("H8").await;
    assert!(right_pane_text(&h).contains("H8"), "{}", h.text());
    // the plugin does nothing harmful outside dtm
    h.send(&format!(
        "env -u DTM_PANE nvim --headless -u NORC --cmd 'set rtp+={PLUGIN_DIR}' -c DtmNavigateLeft -c 'qa!'; echo OUT$((1+1))\r"
    ));
    h.wait_text("OUT2").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn prefix_hjkl_resize_while_arrows_move_focus() {
    let env = Env::new("prefixhjkl", "[options]\nrepeat_time = 0\n");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|"));
    h.send("stty size\r");
    h.wait_for("size", |t| last_size(t, 30).is_some()).await;
    let start = last_size(&h.text(), 30).unwrap();

    // prefix h moves the border left: the focused (right) pane grows, and keeps the focus
    h.send(&format!("{CTRL_A}h"));
    h.send("stty size\r");
    h.wait_for("wider", |t| last_size(t, 30).is_some_and(|c| c > start)).await;
    assert!(right_pane_text(&h).matches("30 ").count() >= 2, "still typing in the right pane");

    // the arrow keys move focus
    h.send(&format!("{CTRL_A}\x1b[D"));
    h.send("echo left$((1+1))\r");
    h.wait_text("left2").await;
    assert!(!right_pane_text(&h).contains("left2"));
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

// ---- projects ----

fn write(path: &std::path::Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

fn lines(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path).map(|t| t.lines().count()).unwrap_or(0)
}

/// Poll a condition that is not visible on the screen.
async fn eventually(what: &str, cond: impl Fn() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !cond() {
        assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
}

/// Processes whose command line is exactly `args`.
fn count_procs(args: &[&str]) -> usize {
    let want = format!("{}\0", args.join("\0"));
    std::fs::read_dir("/proc")
        .unwrap()
        .flatten()
        .filter_map(|e| std::fs::read(e.path().join("cmdline")).ok())
        .filter(|c| c == want.as_bytes())
        .count()
}

#[tokio::test]
async fn project_runs_commands_in_their_paths_and_starts_windows_lazily() {
    let env = Env::new("proj-open", "");
    let proj = env.dir.join("proj");
    std::fs::create_dir_all(proj.join("sub")).unwrap();
    write(
        &proj.join("project.dtm.yaml"),
        "name: demo\nwindows:\n  - name: edit\n    layout:\n      split: columns\n      panes:\n        - command: echo A$((1+1)); pwd\n        - path: sub\n          command: echo B$((2+2)); pwd\n  - name: second\n    command: touch started\n",
    );
    let mut h = env.client(&["open", proj.to_str().unwrap()], 120, 30);
    h.wait_text("A2").await;
    h.wait_text("B4").await;
    let text = h.text();
    assert!(text.contains("1 edit") && text.contains("2 second"), "{text}");
    assert!(text.lines().any(|l| l.ends_with("proj") || l.contains("proj ")), "root pane cwd:\n{text}");
    assert!(text.contains("proj/sub"), "sub pane cwd:\n{text}");
    assert!(!proj.join("started").exists(), "window 2 must not have started yet");

    h.send(&format!("{CTRL_A}2"));
    let started = proj.join("started");
    eventually("lazy window started", || started.exists()).await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
}

#[tokio::test]
async fn project_lazy_false_starts_everything_at_once() {
    let env = Env::new("proj-eager", "");
    let proj = env.dir.join("proj");
    write(&proj.join("project.dtm.yaml"), "lazy: false\nwindows:\n  - command: echo one\n  - command: touch eager\n");
    let mut h = env.client(&["open", proj.to_str().unwrap()], 100, 30);
    h.wait_text("one").await;
    let eager = proj.join("eager");
    eventually("eager window started", || eager.exists()).await;
}

#[tokio::test]
async fn plain_dtm_opens_the_project_here_and_reattaches() {
    let env = Env::new("proj-plain", "");
    let proj = env.dir.join("proj");
    write(&proj.join("project.dtm.yaml"), "name: again\nwindows:\n  - name: w\n    command: echo X >> ran.txt\n");
    let mut h = env.client_in(Some(&proj), &[], 100, 30);
    h.wait_text("1 w").await;
    let ran = proj.join("ran.txt");
    eventually("command ran", || lines(&ran) == 1).await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
    let (_, out) = env.run(&["ls"]);
    assert!(out.contains("again: 1 window"), "{out}");

    let mut h2 = env.client_in(Some(&proj), &[], 100, 30);
    h2.wait_text("1 w").await;
    h2.send("echo SYNC$((10+1))\r");
    h2.wait_text("SYNC11").await;
    assert_eq!(lines(&ran), 1, "an existing session must not run its commands again");
    let (_, out) = env.run(&["ls"]);
    assert!(out.contains("again: 1 window") && !out.contains("1: "), "{out}");
}

#[tokio::test]
async fn invalid_project_is_reported_before_anything_starts() {
    let env = Env::new("proj-bad", "");
    let proj = env.dir.join("proj");
    write(&proj.join("project.dtm.yaml"), "windows:\n  - layout:\n      split: rows\n      panes:\n        - {}\n");
    let (ok, out) = env.run_with(&["open", proj.to_str().unwrap()], &[]);
    assert!(!ok && out.contains("split needs at least 2 panes"), "{out}");
}

#[tokio::test]
async fn export_writes_the_layout_and_reopens() {
    let env = Env::new("proj-export", "");
    // A duration no other run's processes share, so counting them is reliable.
    let secs = (20000 + std::process::id() % 10000).to_string();
    let mut h = env.client_in(Some(&env.dir), &["new", "-s", "ex"], 120, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|"));
    h.send(&format!("echo M$((1+1)); sleep {secs}\r"));
    h.wait_text("M2").await;

    let out = env.dir.join("out/project.dtm.yaml");
    let target = out.to_str().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let yaml = loop {
        let (ok, msg) = env.run(&["export", target, "-t", "ex"]);
        assert!(ok, "{msg}");
        let y = std::fs::read_to_string(&out).unwrap();
        if y.contains(&format!("sleep {secs}")) {
            break y;
        }
        assert!(std::time::Instant::now() < deadline, "no sleep in export:\n{y}");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    };
    assert!(yaml.contains("split: columns") && yaml.contains(&format!("command: sleep {secs}")), "{yaml}");
    assert!(!yaml.contains("root"), "{yaml}");

    // The exported file opens as a second session with the same two panes.
    write(&out, &yaml.replace("name: ex", "name: ex2"));
    let mut h2 = env.client(&["open", target], 120, 30);
    h2.wait_text("1 sh").await;
    h2.wait_for("both panes", |t| t.lines().any(|l| l.matches('│').count() >= 2)).await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while count_procs(&["sleep", &secs]) != 2 {
        assert!(std::time::Instant::now() < deadline, "second session did not run the command: {}\n{}\n{}", count_procs(&["sleep", &secs]), h2.text(), env.run(&["ls"]).1);
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn export_updates_the_opened_file_and_keeps_its_name() {
    let env = Env::new("proj-update", "");
    let proj = env.dir.join("proj");
    let file = proj.join("project.dtm.yaml");
    write(&file, "# my comment\nname: upd\nroot: .\nwindows:\n  - name: a\n    lazy: true\n  - name: b\n");
    let mut h = env.client(&["open", proj.to_str().unwrap()], 100, 30);
    h.wait_text("1 a").await;
    let (ok, msg) = env.run(&["export", "-t", "upd"]);
    assert!(ok, "{msg}");
    let yaml = std::fs::read_to_string(&file).unwrap();
    assert!(yaml.contains("name: upd") && yaml.contains("name: a") && yaml.contains("name: b"), "{yaml}");
    assert!(yaml.contains("root: ."), "an existing root is kept:\n{yaml}");
    assert!(!yaml.contains("my comment"), "{yaml}");
    assert!(msg.contains("comments"), "{msg}");
}

#[tokio::test]
async fn respawn_pane_restarts_it_with_a_new_id() {
    let env = Env::new("respawn-pane", "");
    let proj = env.dir.join("proj");
    let secs = (20000 + std::process::id() % 10000).to_string();
    write(&proj.join("project.dtm.yaml"), &format!("name: rp\nwindows:\n  - command: echo Z >> runs.txt; exec sleep {secs}\n"));
    let mut h = env.client(&["open", proj.to_str().unwrap()], 100, 30);
    let runs = proj.join("runs.txt");
    eventually("first run", || lines(&runs) == 1).await;
    eventually("sleep up", || count_procs(&["sleep", &secs]) == 1).await;
    let (ok, out) = env.run(&["query", "-t", "1", "#{pane_id}"]);
    assert!(ok && out.trim() == "1", "{out}");

    h.send(&format!("{CTRL_A}R"));
    h.wait_text("respawn pane? (y/n)").await;
    h.send("y");
    eventually("second run", || lines(&runs) == 2).await;
    eventually("old process gone", || count_procs(&["sleep", &secs]) == 1).await;
    let (ok, out) = env.run(&["query", "-t", "2", "#{pane_id}"]);
    assert!(ok && out.trim() == "2", "{out}");
    let (ok, _) = env.run(&["query", "-t", "1", "#{pane_id}"]);
    assert!(!ok, "the old pane id must be gone");
    // Answering no changes nothing.
    h.send(&format!("{CTRL_A}R"));
    h.wait_text("respawn pane? (y/n)").await;
    h.send("n");
    h.wait_for("prompt closed", |t| !t.contains("respawn pane?")).await;
    assert_eq!(lines(&runs), 2);
}

#[tokio::test]
async fn respawn_window_restores_the_project_layout() {
    let env = Env::new("respawn-window", "");
    let proj = env.dir.join("proj");
    write(
        &proj.join("project.dtm.yaml"),
        "name: rw\nwindows:\n  - layout:\n      split: columns\n      panes:\n        - command: echo W >> w.txt\n        - command: echo W >> w.txt\n",
    );
    let mut h = env.client(&["open", proj.to_str().unwrap()], 120, 30);
    let w = proj.join("w.txt");
    eventually("both ran", || lines(&w) == 2).await;
    h.send(&format!("{CTRL_A}-"));
    h.wait_for("three panes", |t| t.lines().any(|l| l.matches('─').count() > 3)).await;

    h.send(&format!("{CTRL_A}\x1br"));
    h.wait_text("respawn window").await;
    h.send("y");
    eventually("commands ran again", || lines(&w) == 4).await;
    h.wait_for("extra pane gone", |t| !t.lines().any(|l| l.matches('─').count() > 3)).await;
    h.send("echo SYNC$((10+1))\r");
    h.wait_text("SYNC11").await;
    assert_eq!(lines(&w), 4);
}

#[tokio::test]
async fn respawn_of_a_plain_pane_keeps_its_directory() {
    let env = Env::new("respawn-plain", "");
    let sub = env.dir.join("work/sub");
    std::fs::create_dir_all(&sub).unwrap();
    let mut h = env.client_in(Some(&sub), &["new"], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}|"));
    h.send("cd /tmp\r");
    h.send(&format!("{CTRL_A}R"));
    h.wait_text("respawn pane? (y/n)").await;
    h.send("y");
    h.send("echo P$((1+1)); pwd\r");
    h.wait_text("P2").await;
    assert!(h.text().lines().any(|l| l.contains("work/sub")), "{}", h.text());
}

#[tokio::test]
async fn named_projects_are_edited_listed_opened_and_removed() {
    let env = Env::new("proj-named", "");
    let editor = env.dir.join("editor.sh");
    write(
        &editor,
        &format!(
            "#!/bin/sh\ncat > \"$1\" <<'EOF'\nroot: {}\nwindows:\n  - name: named\n    command: echo N$((3+3))\nEOF\n",
            env.dir.display()
        ),
    );
    std::fs::set_permissions(&editor, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let (ok, out) = env.run_with(&["project", "edit", "foo"], &[("EDITOR", editor.to_str().unwrap()), ("VISUAL", "")]);
    assert!(ok, "{out}");
    assert!(env.dir.join("projects/foo.yaml").is_file());
    let (_, out) = env.run(&["project", "ls"]);
    assert_eq!(out.trim(), "foo");

    let mut h = env.client(&["open", "foo"], 100, 30);
    h.wait_text("N6").await;
    assert!(h.text().contains("1 named"), "{}", h.text());
    drop(h);

    let (ok, out) = env.run(&["project", "rm", "foo"]);
    assert!(ok, "{out}");
    let (_, out) = env.run(&["project", "ls"]);
    assert_eq!(out.trim(), "");
    let (ok, out) = env.run(&["open", "foo"]);
    assert!(!ok && out.contains("no project"), "{out}");
}

// ---- export from the sidebar, search, pane_command, popup, dock ----

#[tokio::test]
async fn sidebar_export_updates_the_named_project_and_keeps_its_root() {
    let env = Env::new("sb-export", "");
    let work = env.dir.join("work");
    std::fs::create_dir_all(work.join("src")).unwrap();
    let file = env.dir.join("projects/np.yaml");
    write(&file, &format!("name: np\nroot: {}\nwindows:\n  - name: one\n    path: src\n", work.display()));
    let mut h = env.client(&["open", "np"], 100, 30);
    h.wait_text("1 one").await;
    h.send(&format!("{CTRL_A}w"));
    h.wait_text(SIDEBAR_HINT).await;
    let before = std::fs::read_to_string(&file).unwrap();
    h.send("e");
    h.wait_text("export to").await;
    h.send("n");
    h.wait_for("declined", |t| !t.contains("(y/n)")).await;
    assert_eq!(std::fs::read_to_string(&file).unwrap(), before, "declining must not write");
    h.send("e");
    h.wait_text("(y/n)").await;
    h.send("y");
    h.wait_text("exported to").await;
    let yaml = std::fs::read_to_string(&file).unwrap();
    assert!(yaml.contains(&format!("root: {}", work.display())), "{yaml}");
    assert!(yaml.contains("path: src"), "paths stay relative to the root:\n{yaml}");
    assert!(!work.join("project.dtm.yaml").exists(), "a named project must not be exported to its root");
}

#[tokio::test]
async fn search_filters_windows_fuzzily() {
    let env = Env::new("search", "");
    let proj = env.dir.join("proj");
    write(
        &proj.join("project.dtm.yaml"),
        "name: sr\nwindows:\n  - name: alpha\n    command: echo AA$((1+1))\n  - name: server\n    command: echo SRV$((3+4))\n  - name: shell\n",
    );
    let mut h = env.client(&["open", proj.to_str().unwrap()], 100, 30);
    h.wait_text("AA2").await;

    h.send(&format!("{CTRL_A}f"));
    h.wait_text("/").await;
    h.send("srv");
    h.wait_for("filtered to the server window", |t| t.contains("2 server") && !t.contains("1 alpha") && !t.contains("3 shell")).await;
    h.wait_text("SRV7").await;
    h.send("\r");
    h.wait_for("search closed", |t| !t.contains("⏎ open") && t.contains("1 alpha") && t.contains("SRV7")).await;

    // Escape goes back to the window the search started from.
    h.send(&format!("{CTRL_A}f"));
    h.send("alp");
    h.wait_for("alpha shown", |t| t.contains("AA2") && !t.contains("SRV7")).await;
    h.send("\x1b");
    h.wait_for("back on the server", |t| t.contains("SRV7") && t.contains("3 shell")).await;

    h.send(&format!("{CTRL_A}f"));
    h.send("zzz");
    h.wait_text("no match").await;
    h.send("\x7f\x7f\x7f\x7f");
    h.wait_for("search cancelled", |t| !t.contains("no match") && t.contains("2 server")).await;
}

#[tokio::test]
async fn pane_command_runs_before_every_command_including_later_splits() {
    let env = Env::new("pane-command", "");
    let proj = env.dir.join("proj");
    write(
        &proj.join("project.dtm.yaml"),
        "name: pc\npane_command: export X=$((40+2))\nwindows:\n  - name: w\n    command: echo V$X\n",
    );
    let mut h = env.client(&["open", proj.to_str().unwrap()], 100, 30);
    h.wait_text("V42").await;
    h.send(&format!("{CTRL_A}|"));
    h.send("echo W$X\r");
    h.wait_text("W42").await;
}

#[tokio::test]
async fn popup_opens_in_the_cwd_hides_and_dies_with_the_prefix_x() {
    let env = Env::new("popup", "");
    let secs = (30000 + std::process::id() % 10000).to_string();
    let mut h = env.client_in(Some(&env.dir), &[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}P"));
    h.wait_text("popup").await;
    h.send("pwd; echo PP$((20+2))\r");
    h.wait_text("PP22").await;
    assert!(h.text().contains("e2e-popup"), "the popup starts in the pane's directory:\n{}", h.text());

    h.send(&format!("{CTRL_A}P"));
    h.wait_for("popup hidden", |t| !t.contains("PP22") && !t.contains("popup")).await;
    h.send(&format!("{CTRL_A}P"));
    h.wait_for("same shell back", |t| t.contains("PP22")).await;

    h.send(&format!("exec sleep {secs}\r"));
    eventually("popup process", || count_procs(&["sleep", &secs]) == 1).await;
    h.send(&format!("{CTRL_A}x"));
    h.wait_for("popup killed", |t| !t.contains("popup")).await;
    eventually("popup process gone", || count_procs(&["sleep", &secs]) == 0).await;

    // The window underneath is untouched and has the keyboard again.
    h.send("echo AFTER$((1+1))\r");
    h.wait_text("AFTER2").await;
}

/// The dock's part of a screen row (after the last separator).
fn dock_tabs(row: &str) -> &str {
    row.rsplit('│').next().unwrap_or(row)
}

fn column_of(text: &str, needle: &str) -> Option<usize> {
    text.lines().find(|l| l.contains(needle)).map(|l| l.split(needle).next().unwrap().chars().count())
}

#[tokio::test]
async fn dock_panes_start_lazily_take_focus_and_can_be_added_closed_and_exported() {
    let env = Env::new("dock", "");
    let proj = env.dir.join("proj");
    let file = proj.join("project.dtm.yaml");
    write(
        &file,
        "name: dk\ndock:\n  visible: true\n  width: 40%\n  panes:\n    - name: one\n      command: echo D$((1+1)); touch d1\n    - name: two\n      command: touch d2\nwindows:\n  - name: main\n",
    );
    let mut h = env.client(&["open", proj.to_str().unwrap()], 120, 30);
    h.wait_text("D2").await;
    let text = h.text();
    assert!(h.screen.row_text(0).contains(" one ") && h.screen.row_text(0).contains(" two"), "tabs:\n{text}");
    assert!(column_of(&text, "D2").unwrap() > 60, "the dock is on the right:\n{text}");
    eventually("first dock pane started", || proj.join("d1").exists()).await;
    assert!(!proj.join("d2").exists(), "the second dock pane starts only when shown");

    h.send(&format!("{CTRL_A}}}"));
    eventually("second dock pane started", || proj.join("d2").exists()).await;

    // Ctrl-l from the only window pane reaches the dock; Ctrl-h leads back.
    h.send("\x0c");
    h.send("echo T$((5+5))\r");
    h.wait_text("T10").await;
    assert!(column_of(&h.text(), "T10").unwrap() > 60, "typed into the dock:\n{}", h.text());
    h.send("\x08");
    h.send("echo W$((2+2))\r");
    h.wait_text("W4").await;
    assert!(column_of(&h.text(), "W4").unwrap() < 60, "typed into the window:\n{}", h.text());

    // Add a pane with a prompt, then close it again.
    h.send(&format!("{CTRL_A}A"));
    h.wait_text("dock command").await;
    h.send("echo ADDED$((7+7))\r");
    h.wait_text("ADDED14").await;
    assert!(dock_tabs(&h.screen.row_text(0)).contains(" echo"), "{}", h.text());
    h.send(&format!("{CTRL_A}X"));
    h.wait_text("close dock pane echo? (y/n)").await;
    h.send("y");
    h.wait_for("tab closed", |t| t.lines().next().is_some_and(|l| !dock_tabs(l).contains(" echo") && l.contains(" two"))).await;

    // The dock is part of the exported project.
    let (ok, msg) = env.run(&["export", "-t", "dk"]);
    assert!(ok, "{msg}");
    let yaml = std::fs::read_to_string(&file).unwrap();
    assert!(yaml.contains("dock:") && yaml.contains("name: one") && yaml.contains("width: 40%"), "{yaml}");
    assert!(yaml.contains("touch d2"), "{yaml}");
    assert!(!yaml.contains("ADDED"), "{yaml}");
}

#[tokio::test]
async fn an_empty_dock_can_be_opened_to_add_the_first_pane() {
    let env = Env::new("dock-empty", "");
    let mut h = env.client(&[], 120, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}D"));
    h.wait_text("no panes").await;
    h.send(&format!("{CTRL_A}A"));
    h.wait_text("dock command").await;
    h.send("echo FIRST$((1+1))\r");
    h.wait_text("FIRST2").await;
    assert!(column_of(&h.text(), "FIRST2").unwrap() > 60, "{}", h.text());
    h.send(&format!("{CTRL_A}D"));
    h.wait_for("dock hidden", |t| !t.contains("FIRST2")).await;
}

#[tokio::test]
async fn prefix_h_l_resize_the_focused_sidebar_and_dock() {
    let env = Env::new("panel-resize", "");
    let proj = env.dir.join("proj");
    write(
        &proj.join("project.dtm.yaml"),
        "name: pr\ndock:\n  visible: true\n  width: 40%\n  panes:\n    - command: echo DK\nwindows:\n  - name: main\n",
    );
    let mut h = env.client(&["open", proj.to_str().unwrap()], 120, 30);
    h.wait_text("DK").await;
    h.wait_cell(23, 5, '│').await; // sidebar separator, width 24
    h.wait_cell(72, 5, '│').await; // dock separator, width 48

    // Sidebar on the left: l grows it, h shrinks it.
    h.send(&format!("{CTRL_A}w"));
    h.wait_text(SIDEBAR_HINT).await;
    h.send(&format!("{CTRL_A}l"));
    h.wait_cell(28, 5, '│').await;
    // Like the dock, plain h/l keep resizing by one cell for a moment.
    h.send("l");
    h.wait_cell(29, 5, '│').await;
    h.send("h");
    h.wait_cell(28, 5, '│').await;
    h.send(&format!("{CTRL_A}h"));
    h.wait_cell(23, 5, '│').await;
    h.send("q");
    h.wait_for("sidebar left", |t| !t.contains(SIDEBAR_HINT)).await;

    // Dock on the right: h grows it, l shrinks it.
    h.send("\x0c");
    h.wait_cursor("in the dock", |x, _| x > 72).await;
    h.send(&format!("{CTRL_A}h"));
    h.wait_cell(67, 5, '│').await;
    h.send(&format!("{CTRL_A}l"));
    h.wait_cell(72, 5, '│').await;
}

#[tokio::test]
async fn sidebar_width_is_kept_by_the_session() {
    let env = Env::new("sidebar-width", "");
    let mut h = env.client(&[], 100, 30);
    h.wait_text("1 sh").await;
    h.send(&format!("{CTRL_A}w"));
    h.wait_text(SIDEBAR_HINT).await;
    h.send(&format!("{CTRL_A}l"));
    h.wait_cell(28, 5, '│').await;
    h.send("n");
    h.wait_text("2 sh").await;
    h.send(&format!("{CTRL_A}d"));
    h.wait_exit().await;
    let mut h2 = env.client(&[], 100, 30);
    h2.wait_text("2 sh").await;
    h2.wait_cell(28, 5, '│').await;
}

#[tokio::test]
async fn project_sidebar_width_is_applied_and_exported_and_o_opens() {
    let env = Env::new("proj-sidebar", "");
    let proj = env.dir.join("proj");
    let file = proj.join("project.dtm.yaml");
    write(&file, "name: sbp\nsidebar:\n  width: 30\nwindows:\n  - name: a\n  - name: b\n");
    let mut h = env.client(&["open", proj.to_str().unwrap()], 100, 30);
    h.wait_text("1 a").await;
    h.wait_cell(29, 5, '│').await;
    h.send(&format!("{CTRL_A}w"));
    h.wait_text(SIDEBAR_HINT).await;
    h.send(&format!("{CTRL_A}l"));
    h.wait_cell(34, 5, '│').await;
    h.send("j");
    h.wait_for("window 2 shown", |t| t.contains("2 b")).await;
    h.send("o");
    h.wait_for("o left the sidebar", |t| !t.contains(SIDEBAR_HINT)).await;
    let (ok, msg) = env.run(&["export", "-t", "sbp"]);
    assert!(ok, "{msg}");
    let yaml = std::fs::read_to_string(&file).unwrap();
    assert!(yaml.contains("sidebar:") && yaml.contains("width: 35"), "{yaml}");
}
