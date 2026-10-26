//! Wire protocol between client and server: length-prefixed bincode frames.

use std::path::PathBuf;

use anyhow::Result;
use bytes::Bytes;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::net::UnixStream;
use crate::project::Plan;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachMode {
    /// Attach to the named (or first) session, creating it if missing.
    Default,
    /// Always create a new session.
    New,
    /// Attach to an existing session only.
    Existing,
}

/// Where `select-pane` should move focus.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneDir {
    Left,
    Right,
    Up,
    Down,
    /// The previously focused pane.
    Last,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ClientMsg {
    Attach { mode: AttachMode, name: Option<String>, cols: u16, rows: u16, cwd: PathBuf },
    /// Attach to the project's session, creating it from the plan if it is not running.
    OpenProject { plan: Plan, source: PathBuf, cols: u16, rows: u16 },
    /// Describe a session as a project (for export).
    Snapshot { session: Option<String>, pane: Option<u32> },
    Input(Vec<u8>),
    Resize { cols: u16, rows: u16 },
    Detach,
    List,
    KillServer,
    Reload,
    /// Move focus relative to `pane` (used by editor plugins).
    SelectPane { pane: u32, dir: PaneDir },
    /// Expand a `#{...}` format for `pane`.
    Query { pane: u32, format: String },
    /// Run a command line (see `crate::command`) in a session: the named one, else the one
    /// `pane` is in, else the only one.
    Command { session: Option<String>, pane: Option<u32>, line: String },
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct SessionInfo {
    pub name: String,
    pub windows: usize,
    pub attached: usize,
}

/// A running session described as a project.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Snapshot {
    pub plan: Plan,
    /// The project file the session was opened from, if any.
    pub source: Option<PathBuf>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum ServerMsg {
    Output(Vec<u8>),
    Detached,
    Exited,
    Error(String),
    Sessions(Vec<SessionInfo>),
    Text(String),
    Snapshot(Snapshot),
    Ok,
}

pub type Conn = Framed<UnixStream, LengthDelimitedCodec>;

pub fn framed(stream: UnixStream) -> Conn {
    LengthDelimitedCodec::builder().max_frame_length(64 << 20).new_framed(stream)
}

pub fn encode<T: Serialize>(msg: &T) -> Bytes {
    Bytes::from(bincode::serialize(msg).expect("serializable message"))
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T> {
    Ok(bincode::deserialize(bytes)?)
}
