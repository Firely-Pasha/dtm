//! `dtm debug-run`: one local shell through parser -> screen -> renderer,
//! without any client/server machinery.

use anyhow::Result;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::signal::unix::{signal, SignalKind};
use tokio::sync::mpsc;

use crate::pty::{default_shell, Pty, PtyEvent, SpawnOpts};
use crate::term::canvas::{canvas_from_screen, Canvas};
use crate::term::screen::Screen;
use crate::tty;

pub async fn run() -> Result<()> {
    let (cols, rows) = tty::size();
    let raw = tty::RawMode::enable()?;
    let mut screen = Screen::new(cols as usize, rows as usize, 1000);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let pty = Pty::spawn(
        SpawnOpts { shell: default_shell(), args: vec![], env: vec![], cwd: None, cols, rows },
        move |ev| {
            let _ = tx.send(ev);
        },
    )?;
    let mut out = tokio::io::stdout();
    out.write_all(b"\x1b[?1049h\x1b[?7l\x1b[2J").await?;
    let mut stdin = tokio::io::stdin();
    let mut winch = signal(SignalKind::window_change())?;
    let mut prev: Option<Canvas> = None;
    let mut buf = vec![0u8; 4096];
    'main: loop {
        let mut events = Vec::new();
        tokio::select! {
            ev = rx.recv() => match ev {
                Some(ev) => events.push(ev),
                None => break,
            },
            n = stdin.read(&mut buf) => {
                let n = n?;
                if n == 0 { break; }
                pty.write(buf[..n].to_vec());
            }
            _ = winch.recv() => {
                let (c, r) = tty::size();
                screen.resize(c as usize, r as usize);
                pty.resize(c, r);
                prev = None;
            }
        }
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        for ev in events {
            match ev {
                PtyEvent::Output(d) => screen.feed(&d),
                PtyEvent::Exited => break 'main,
            }
        }
        let replies = screen.take_replies();
        if !replies.is_empty() {
            pty.write(replies);
        }
        let canvas = canvas_from_screen(&screen);
        let bytes = canvas.diff(prev.as_ref());
        if !bytes.is_empty() {
            out.write_all(&bytes).await?;
            out.flush().await?;
        }
        prev = Some(canvas);
    }
    out.write_all(b"\x1b[0m\x1b[?1l\x1b[?2004l\x1b[?7h\x1b[?25h\x1b[?1049l").await?;
    out.flush().await?;
    drop(raw);
    std::process::exit(0);
}
