mod common;
use common::Harness;

#[tokio::test]
async fn shell_and_pager_render_correctly() {
    let dir = env!("CARGO_TARGET_TMPDIR");
    let file = format!("{dir}/lines.txt");
    let content: String = (1..=200).map(|i| format!("line-{i}\n")).collect();
    std::fs::write(&file, content).unwrap();

    let mut h = Harness::spawn(
        env!("CARGO_BIN_EXE_dtm"),
        &["debug-run"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
        80,
        24,
    );
    h.send("echo hi$((1+2))\r");
    h.wait_text("hi3").await;

    h.send(&format!("less {file}\r"));
    h.wait_text("line-1\n").await;
    h.send(" ");
    h.wait_text("line-30").await;
    assert!(!h.text().contains("line-1\n"));
    h.send("q");
    h.wait_for("shell prompt after less", |t| !t.contains("line-30")).await;

    h.send("exit\r");
    h.wait_exit().await;
}

#[tokio::test]
async fn vim_edits_and_quits() {
    if std::process::Command::new("vim").arg("--version").output().is_err() {
        return;
    }
    let mut h = Harness::spawn(
        env!("CARGO_BIN_EXE_dtm"),
        &["debug-run"],
        &[("SHELL", "/bin/sh"), ("PS1", "$ ")],
        80,
        24,
    );
    h.send("vim -u NONE -N\r");
    h.wait_for("vim splash", |t| t.contains("VIM")).await;
    h.send("ihello vim\x1b");
    h.wait_text("hello vim").await;
    h.send(":q!\r");
    h.wait_for("back at shell", |t| !t.contains("hello vim")).await;
    h.send("exit\r");
    h.wait_exit().await;
}
