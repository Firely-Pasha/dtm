//! Local terminal helpers: raw mode and size.

use std::io;
use std::os::fd::AsRawFd;

use anyhow::{Context, Result};
use nix::sys::termios::{cfmakeraw, tcgetattr, tcsetattr, SetArg, Termios};

pub struct RawMode {
    orig: Termios,
}

impl RawMode {
    pub fn enable() -> Result<RawMode> {
        let stdin = io::stdin();
        let orig = tcgetattr(&stdin).context("stdin is not a terminal")?;
        let mut raw = orig.clone();
        cfmakeraw(&mut raw);
        tcsetattr(&stdin, SetArg::TCSANOW, &raw)?;
        Ok(RawMode { orig })
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = tcsetattr(io::stdin(), SetArg::TCSANOW, &self.orig);
    }
}

/// (cols, rows) of the controlling terminal.
pub fn size() -> (u16, u16) {
    for fd in [io::stdout().as_raw_fd(), io::stdin().as_raw_fd()] {
        let mut ws: libc::winsize = unsafe { std::mem::zeroed() };
        // SAFETY: valid fd and pointer to a winsize.
        if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ, &mut ws) } == 0 && ws.ws_col > 0 && ws.ws_row > 0 {
            return (ws.ws_col, ws.ws_row);
        }
    }
    (80, 24)
}
