//! Test harness: run a program on a pty and watch its output through an emulator.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use dtm::pty::{Pty, PtyEvent, SpawnOpts};
use dtm::term::screen::Screen;
use tokio::sync::mpsc;

pub struct Harness {
    pty: Pty,
    pub screen: Screen,
    rx: mpsc::UnboundedReceiver<PtyEvent>,
    pub exited: bool,
}

impl Harness {
    pub fn spawn(program: &str, args: &[&str], env: &[(&str, &str)], cols: u16, rows: u16) -> Harness {
        Harness::spawn_in(program, args, env, cols, rows, None)
    }

    pub fn spawn_in(
        program: &str,
        args: &[&str],
        env: &[(&str, &str)],
        cols: u16,
        rows: u16,
        cwd: Option<&std::path::Path>,
    ) -> Harness {
        let (tx, rx) = mpsc::unbounded_channel();
        let pty = Pty::spawn(
            SpawnOpts {
                shell: program.to_string(),
                args: args.iter().map(|s| s.to_string()).collect(),
                env: env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
                cwd: cwd.map(|p| p.to_path_buf()),
                cols,
                rows,
            },
            move |ev| {
                let _ = tx.send(ev);
            },
        )
        .expect("spawn");
        Harness { pty, screen: Screen::new(cols as usize, rows as usize, 100), rx, exited: false }
    }

    pub fn send(&self, s: &str) {
        self.pty.write(s.as_bytes().to_vec());
    }

    pub fn text(&self) -> String {
        self.screen.text().join("\n")
    }

    pub fn resize(&mut self, cols: u16, rows: u16) {
        self.screen.resize(cols as usize, rows as usize);
        self.pty.resize(cols, rows);
    }

    /// Pump output until `pred` holds for the screen text or the timeout hits.
    pub async fn wait_for(&mut self, what: &str, pred: impl Fn(&str) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if pred(&self.text()) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                panic!("timed out waiting for {what}; screen:\n{}", self.text());
            }
            match tokio::time::timeout(left, self.rx.recv()).await {
                Ok(Some(PtyEvent::Output(d))) => {
                    self.screen.feed(&d);
                    let r = self.screen.take_replies();
                    if !r.is_empty() {
                        self.pty.write(r);
                    }
                }
                Ok(Some(PtyEvent::Exited)) => {
                    self.exited = true;
                    if pred(&self.text()) {
                        return;
                    }
                }
                Ok(None) => panic!("channel closed waiting for {what}; screen:\n{}", self.text()),
                Err(_) => {}
            }
        }
    }

    /// Pump output until `pred` holds for the emulated screen (cells and their attributes).
    pub async fn wait_screen(&mut self, what: &str, pred: impl Fn(&Screen) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !pred(&self.screen) {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {what}; screen:\n{}", self.text());
            if let Ok(Some(PtyEvent::Output(d))) = tokio::time::timeout(left, self.rx.recv()).await {
                self.screen.feed(&d);
                let r = self.screen.take_replies();
                if !r.is_empty() {
                    self.pty.write(r);
                }
            }
        }
    }

    /// Wait until the cell at (x, y) holds `ch`.
    pub async fn wait_cell(&mut self, x: usize, y: usize, ch: char) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self.screen.cell(x, y).ch != ch {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for {ch:?} at {x},{y}; screen:\n{}", self.text());
            if let Ok(Some(PtyEvent::Output(d))) = tokio::time::timeout(left, self.rx.recv()).await {
                self.screen.feed(&d);
            }
        }
    }

    /// Wait until the terminal cursor (which follows the focused pane) satisfies `pred(x, y)`.
    pub async fn wait_cursor(&mut self, what: &str, pred: impl Fn(usize, usize) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (x, y) = self.screen.cursor_pos();
            if pred(x, y) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for cursor: {what}; screen:\n{}", self.text());
            if let Ok(Some(PtyEvent::Output(d))) = tokio::time::timeout(left, self.rx.recv()).await {
                self.screen.feed(&d);
            }
        }
    }

    pub async fn wait_text(&mut self, needle: &str) {
        let n = needle.to_string();
        self.wait_for(&format!("{needle:?}"), move |t| t.contains(&n)).await;
    }

    pub async fn wait_exit(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !self.exited {
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting for exit; screen:\n{}", self.text());
            if let Ok(Some(ev)) = tokio::time::timeout(left, self.rx.recv()).await {
                match ev {
                    PtyEvent::Output(d) => self.screen.feed(&d),
                    PtyEvent::Exited => self.exited = true,
                }
            }
        }
    }
}
