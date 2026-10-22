//! The dtm client: a thin relay between the local terminal and the server.

use std::io::IsTerminal;
use std::os::unix::process::CommandExt;
use std::process::Stdio;
use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::UnixStream;
use tokio::signal::unix::{signal, SignalKind};

use crate::config::Config;
use crate::project::{self, Plan};
use crate::protocol::{decode, encode, framed, AttachMode, ClientMsg, Conn, PaneDir, ServerMsg, SessionInfo, Snapshot};
use crate::{paths, tty};

/// Enter the alternate screen and disable autowrap (the server relies on it).
const ENTER: &[u8] = b"\x1b[?1049h\x1b[?7l\x1b[0m\x1b[2J\x1b[H";
/// Undo everything the server may have switched on.
const LEAVE: &[u8] = b"\x1b[0m\x1b[?1l\x1b[?2004l\x1b[?7h\x1b[?25h\x1b[?1049l";

fn spawn_server() -> Result<()> {
    let exe = std::env::current_exe().context("locating dtm executable")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("server").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    cmd.spawn().context("starting server")?;
    Ok(())
}

async fn connect(start_server: bool) -> Result<Conn> {
    let path = paths::socket_path();
    let first = match UnixStream::connect(&path).await {
        Ok(s) => return Ok(framed(s)),
        Err(e) => e,
    };
    if !start_server {
        bail!("no server running on {} ({first})", path.display());
    }
    // Refuse to start a server that would ignore a broken config silently.
    Config::load(&paths::config_path()).map_err(|e| anyhow!("config error: {e}"))?;
    spawn_server()?;
    for _ in 0..100 {
        tokio::time::sleep(Duration::from_millis(50)).await;
        if let Ok(s) = UnixStream::connect(&path).await {
            return Ok(framed(s));
        }
    }
    bail!("timed out waiting for the server to start")
}

async fn request(msg: ClientMsg) -> Result<ServerMsg> {
    let mut conn = connect(false).await?;
    conn.send(encode(&msg)).await?;
    let frame = conn.next().await.ok_or_else(|| anyhow!("server closed the connection"))??;
    decode(&frame)
}

pub async fn list() -> Result<Vec<SessionInfo>> {
    match request(ClientMsg::List).await? {
        ServerMsg::Sessions(v) => Ok(v),
        ServerMsg::Error(e) => bail!(e),
        other => bail!("unexpected reply: {other:?}"),
    }
}

pub async fn kill_server() -> Result<()> {
    let mut conn = connect(false).await?;
    conn.send(encode(&ClientMsg::KillServer)).await?;
    // The server answers with Exited and closes; either way we are done.
    let _ = conn.next().await;
    Ok(())
}

pub async fn reload() -> Result<()> {
    match request(ClientMsg::Reload).await? {
        ServerMsg::Ok => Ok(()),
        ServerMsg::Error(e) => bail!("config error: {e}"),
        other => bail!("unexpected reply: {other:?}"),
    }
}

/// The pane a command refers to: `-t`, else the one this process runs in.
fn target_pane(target: Option<u32>) -> Result<u32> {
    if let Some(t) = target {
        return Ok(t);
    }
    std::env::var("DTM_PANE")
        .ok()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| anyhow!("not inside a dtm pane (use -t PANE)"))
}

pub async fn select_pane(target: Option<u32>, dir: PaneDir) -> Result<()> {
    let pane = target_pane(target)?;
    match request(ClientMsg::SelectPane { pane, dir }).await? {
        ServerMsg::Ok => Ok(()),
        ServerMsg::Error(e) => bail!(e),
        other => bail!("unexpected reply: {other:?}"),
    }
}

pub async fn query(target: Option<u32>, format: String) -> Result<String> {
    let pane = target_pane(target)?;
    match request(ClientMsg::Query { pane, format }).await? {
        ServerMsg::Text(t) => Ok(t),
        ServerMsg::Error(e) => bail!(e),
        other => bail!("unexpected reply: {other:?}"),
    }
}

async fn snapshot(session: Option<String>) -> Result<Snapshot> {
    let pane = std::env::var("DTM_PANE").ok().and_then(|v| v.parse().ok());
    match request(ClientMsg::Snapshot { session, pane }).await? {
        ServerMsg::Snapshot(s) => Ok(s),
        ServerMsg::Error(e) => bail!(e),
        other => bail!("unexpected reply: {other:?}"),
    }
}

/// `dtm export`: write a running session to a project file. Returns the path written.
pub async fn export(arg: Option<String>, session: Option<String>) -> Result<std::path::PathBuf> {
    let snap = snapshot(session).await?;
    let cfg = paths::config_path();
    let cwd = std::env::current_dir().unwrap_or_default();
    let target = match arg {
        Some(a) => project::find(Some(&a), &cwd, &cfg).or_else(|e| {
            // An unknown name creates a new named project.
            if a.contains('/') || a.ends_with(".yaml") || a.ends_with(".yml") {
                Ok(cwd.join(&a))
            } else if project::valid_project_name(&a) && !cwd.join(&a).exists() {
                Ok(project::named_path(&cfg, &a))
            } else {
                Err(e)
            }
        }),
        None => match snap.source {
            Some(p) => Ok(p),
            None if snap.plan.root.as_os_str().is_empty() => Err("session has no directory: name a target".to_string()),
            None => Ok(snap.plan.root.join(project::PROJECT_FILE)),
        },
    }
    .map_err(|e| anyhow!(e))?;
    let existed = target.exists();
    project::export_to(&snap.plan, &target, &project::home_dir()).map_err(|e| anyhow!(e))?;
    if existed {
        eprintln!("dtm: updated {} (comments in it were not kept)", target.display());
    }
    Ok(target)
}

fn project_editor_command(path: &std::path::Path) -> std::process::Command {
    let editor = ["VISUAL", "EDITOR"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.trim().is_empty()))
        .unwrap_or_else(|| "vi".into());
    let mut c = std::process::Command::new("sh");
    c.arg("-c").arg(format!("{editor} \"$1\"")).arg("dtm-edit").arg(path);
    c
}

/// `dtm project edit NAME`: edit a named project, creating it from a template.
pub fn project_edit(name: &str) -> Result<()> {
    if !project::valid_project_name(name) {
        bail!("bad project name: {name:?}");
    }
    let path = project::named_path(&paths::config_path(), name);
    if !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")))?;
        let cwd = std::env::current_dir().unwrap_or_default();
        std::fs::write(&path, project::template(&cwd))?;
    }
    loop {
        let status = project_editor_command(&path).status().context("starting the editor")?;
        if !status.success() {
            bail!("editor failed");
        }
        match project::load(&path) {
            Ok(_) => return Ok(()),
            Err(e) => {
                eprintln!("dtm: {e}");
                eprint!("[e]dit again / [k]eep: ");
                let mut line = String::new();
                std::io::stdin().read_line(&mut line)?;
                if line.trim().eq_ignore_ascii_case("k") {
                    return Ok(());
                }
            }
        }
    }
}

pub fn project_rm(name: &str) -> Result<()> {
    let path = project::named_path(&paths::config_path(), name);
    if !project::valid_project_name(name) || !path.is_file() {
        bail!("no project {name:?}");
    }
    Ok(std::fs::remove_file(path)?)
}

enum Ending {
    Detached,
    Exited,
    Lost,
}

/// Attach to a session and relay until detached. Never returns on success.
pub async fn attach(mode: AttachMode, name: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_default();
    attach_with(mode != AttachMode::Existing, move |cols, rows| ClientMsg::Attach { mode, name, cols, rows, cwd }).await
}

/// Open a project (see `project::find` for what `arg` may be) and attach to its session.
pub async fn open_project(arg: Option<String>) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_default();
    let source = project::find(arg.as_deref(), &cwd, &paths::config_path()).map_err(|e| anyhow!(e))?;
    let plan: Plan = project::load_plan(&source).map_err(|e| anyhow!(e))?;
    attach_with(true, move |cols, rows| ClientMsg::OpenProject { plan, source, cols, rows }).await
}

async fn attach_with(start_server: bool, first: impl FnOnce(u16, u16) -> ClientMsg) -> Result<()> {
    if !std::io::stdin().is_terminal() {
        bail!("dtm needs a terminal (stdin is not a tty)");
    }
    let conn = connect(start_server).await?;
    let (mut sink, mut stream) = conn.split();
    let (cols, rows) = tty::size();
    sink.send(encode(&first(cols, rows))).await?;

    let raw = tty::RawMode::enable()?;
    let mut entered = false;
    let result = relay(&mut sink, &mut stream, &mut entered).await;
    let mut out = tokio::io::stdout();
    if entered {
        let _ = out.write_all(LEAVE).await;
        let _ = out.flush().await;
    }
    drop(raw);
    match result {
        Ok(Ending::Detached) => eprintln!("[detached]"),
        Ok(Ending::Exited) => eprintln!("[exited]"),
        Ok(Ending::Lost) => eprintln!("[lost connection to server]"),
        Err(e) => {
            eprintln!("dtm: {e:#}");
            std::process::exit(1);
        }
    }
    // A blocked stdin read would keep the runtime alive.
    std::process::exit(0);
}

async fn relay(
    sink: &mut (impl futures::Sink<bytes::Bytes, Error = std::io::Error> + Unpin),
    stream: &mut (impl futures::Stream<Item = std::io::Result<bytes::BytesMut>> + Unpin),
    entered: &mut bool,
) -> Result<Ending> {
    let mut out = tokio::io::stdout();
    let mut stdin = tokio::io::stdin();
    let mut winch = signal(SignalKind::window_change())?;
    let mut term = signal(SignalKind::terminate())?;
    let mut hup = signal(SignalKind::hangup())?;
    let mut buf = vec![0u8; 8192];
    loop {
        tokio::select! {
            n = stdin.read(&mut buf) => {
                let n = n?;
                if n == 0 {
                    return Ok(Ending::Lost);
                }
                sink.send(encode(&ClientMsg::Input(buf[..n].to_vec()))).await?;
            }
            _ = term.recv() => return Ok(Ending::Lost),
            _ = hup.recv() => return Ok(Ending::Lost),
            _ = winch.recv() => {
                let (cols, rows) = tty::size();
                sink.send(encode(&ClientMsg::Resize { cols, rows })).await?;
            }
            frame = stream.next() => {
                let Some(Ok(frame)) = frame else { return Ok(Ending::Lost) };
                match decode::<ServerMsg>(&frame)? {
                    ServerMsg::Output(data) => {
                        if !*entered {
                            out.write_all(ENTER).await?;
                            *entered = true;
                        }
                        out.write_all(&data).await?;
                        out.flush().await?;
                    }
                    ServerMsg::Detached => return Ok(Ending::Detached),
                    ServerMsg::Exited => return Ok(Ending::Exited),
                    ServerMsg::Error(e) => bail!("{e}"),
                    _ => {}
                }
            }
        }
    }
}
