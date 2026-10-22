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
        let vars = self.vars();
        let pairs: Vec<(&str, &str)> = vars.iter().map(|(k, v)| (*k, v.as_str())).collect();
        Harness::spawn(env!("CARGO_BIN_EXE_dtm"), args, &pairs, cols, rows)
    }

    fn run(&self, args: &[&str]) -> (bool, String) {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_dtm"));
        cmd.args(args);
        for (k, v) in self.vars() {
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
    h.send(&format!("{CTRL_A}h"));
    h.send("echo left$((1+1))\r");
    h.wait_text("left2").await;
    h.send(&format!("{CTRL_A}l"));
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
