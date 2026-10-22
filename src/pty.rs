//! Pseudo-terminal spawning and async I/O.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use nix::fcntl::{fcntl, FcntlArg, FdFlag, OFlag};
use nix::pty::{openpty, Winsize};
use tokio::io::unix::AsyncFd;
use tokio::process::Command;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

pub enum PtyEvent {
    Output(Vec<u8>),
    Exited,
}

pub struct SpawnOpts {
    pub shell: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    pub cols: u16,
    pub rows: u16,
}

pub struct Pty {
    master: Arc<AsyncFd<OwnedFd>>,
    pid: u32,
    input: mpsc::UnboundedSender<Vec<u8>>,
    tasks: Vec<JoinHandle<()>>,
}

fn winsize(cols: u16, rows: u16) -> Winsize {
    Winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 }
}

impl Pty {
    /// Spawn `opts.shell` on a new pty. `on_event` is called from tasks for
    /// every chunk of output and once when the child exits.
    pub fn spawn(opts: SpawnOpts, on_event: impl Fn(PtyEvent) + Send + Sync + 'static) -> Result<Pty> {
        let pty = openpty(Some(&winsize(opts.cols, opts.rows)), None).context("openpty")?;
        let (master, slave) = (pty.master, pty.slave);
        fcntl(&master, FcntlArg::F_SETFL(OFlag::O_NONBLOCK))?;
        fcntl(&master, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;
        fcntl(&slave, FcntlArg::F_SETFD(FdFlag::FD_CLOEXEC))?;

        let mut cmd = Command::new(&opts.shell);
        cmd.args(&opts.args).envs(opts.env.iter().map(|(k, v)| (k, v)));
        cmd.stdin(slave.try_clone()?)
            .stdout(slave.try_clone()?)
            .stderr(slave)
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor")
            .env_remove("TERM_PROGRAM");
        if let Some(cwd) = &opts.cwd {
            if cwd.is_dir() {
                cmd.current_dir(cwd);
            }
        }
        // SAFETY: only async-signal-safe libc calls between fork and exec.
        unsafe {
            cmd.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = cmd.spawn().with_context(|| format!("spawning {}", opts.shell))?;
        let pid = child.id().unwrap_or(0);
        drop(cmd);

        let master = Arc::new(AsyncFd::new(master)?);
        let on_event: Arc<dyn Fn(PtyEvent) + Send + Sync> = Arc::new(on_event);
        let mut tasks = Vec::new();

        let (m, ev) = (master.clone(), on_event.clone());
        tasks.push(tokio::spawn(async move {
            let mut buf = vec![0u8; 65536];
            loop {
                let Ok(mut guard) = m.readable().await else { break };
                match guard.try_io(|fd| nix::unistd::read(fd.get_ref(), &mut buf).map_err(io::Error::from)) {
                    Ok(Ok(0)) | Ok(Err(_)) => break,
                    Ok(Ok(n)) => ev(PtyEvent::Output(buf[..n].to_vec())),
                    Err(_would_block) => continue,
                }
            }
        }));

        let (tx, mut rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let m = master.clone();
        tasks.push(tokio::spawn(async move {
            while let Some(data) = rx.recv().await {
                let mut off = 0;
                while off < data.len() {
                    let Ok(mut guard) = m.writable().await else { return };
                    match guard.try_io(|fd| nix::unistd::write(fd.get_ref(), &data[off..]).map_err(io::Error::from)) {
                        Ok(Ok(n)) => off += n,
                        Ok(Err(_)) => return,
                        Err(_would_block) => continue,
                    }
                }
            }
        }));

        let ev = on_event;
        tasks.push(tokio::spawn(async move {
            let _ = child.wait().await;
            ev(PtyEvent::Exited);
        }));

        Ok(Pty { master, pid, input: tx, tasks })
    }

    pub fn write(&self, data: Vec<u8>) {
        let _ = self.input.send(data);
    }

    pub fn resize(&self, cols: u16, rows: u16) {
        let ws = libc::winsize { ws_row: rows, ws_col: cols, ws_xpixel: 0, ws_ypixel: 0 };
        // SAFETY: valid fd and pointer to a winsize.
        unsafe {
            libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &ws);
        }
    }

    fn foreground_pgrp(&self) -> Option<i32> {
        // SAFETY: plain syscall on a valid fd.
        let pg = unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) };
        (pg > 0).then_some(pg)
    }

    /// Working directory of the foreground process (falling back to the shell).
    pub fn cwd(&self) -> Option<PathBuf> {
        let from = |pid: i32| std::fs::read_link(format!("/proc/{pid}/cwd")).ok();
        self.foreground_pgrp().and_then(from).or_else(|| from(self.pid as i32))
    }

    /// Name of the foreground process.
    pub fn process_name(&self) -> Option<String> {
        let pg = self.foreground_pgrp().unwrap_or(self.pid as i32);
        let s = std::fs::read_to_string(format!("/proc/{pg}/comm")).ok()?;
        Some(s.trim().to_string())
    }
}

impl Drop for Pty {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

pub fn default_shell() -> String {
    std::env::var("SHELL").ok().filter(|s| Path::new(s).exists()).unwrap_or_else(|| "/bin/sh".into())
}
