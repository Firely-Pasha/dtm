use anyhow::Result;
use clap::{Parser, Subcommand};
use dtm::protocol::{AttachMode, PaneDir};
use dtm::{client, config::Config, paths, server};

#[derive(Parser)]
#[command(name = "dtm", version, about = "A terminal multiplexer")]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Start a new session.
    New {
        /// Name of the new session.
        #[arg(short = 's', long)]
        session: Option<String>,
    },
    /// Attach to an existing session.
    Attach {
        /// Session to attach to (default: the first one).
        #[arg(short = 't', long)]
        target: Option<String>,
    },
    /// List sessions.
    Ls,
    /// Terminate the server and every session.
    KillServer,
    /// Re-read the configuration file.
    Reload,
    /// Move pane focus (for editor plugins): relative to the pane given by -t or $DTM_PANE.
    SelectPane {
        /// Pane id (default: $DTM_PANE).
        #[arg(short = 't', long)]
        target: Option<u32>,
        #[arg(short = 'L', conflicts_with_all = ["right", "up", "down", "last"])]
        left: bool,
        #[arg(short = 'R', conflicts_with_all = ["up", "down", "last"])]
        right: bool,
        #[arg(short = 'U', conflicts_with_all = ["down", "last"])]
        up: bool,
        #[arg(short = 'D', conflicts_with = "last")]
        down: bool,
        /// The previously focused pane.
        #[arg(short = 'l')]
        last: bool,
    },
    /// Print a format such as '#{window_zoomed_flag}' or '#{pane_at_left}' for a pane.
    Query {
        #[arg(short = 't', long)]
        target: Option<u32>,
        format: String,
    },
    #[command(hide = true)]
    Server,
    /// Run one local shell through the emulator (no server).
    #[command(hide = true)]
    DebugRun,
}

fn init_logging() {
    let path = paths::log_path();
    let (Some(dir), Some(file)) = (path.parent(), path.file_name()) else { return };
    if std::fs::create_dir_all(dir).is_err() {
        return;
    }
    let appender = tracing_appender::rolling::never(dir, file);
    let _ = tracing_subscriber::fmt().with_writer(appender).with_ansi(false).try_init();
}

async fn run_server() -> Result<()> {
    init_logging();
    let cfg_path = paths::config_path();
    let cfg = Config::load(&cfg_path).unwrap_or_else(|e| {
        tracing::error!("config error, using defaults: {e}");
        Config::default()
    });
    server::run(paths::socket_path(), cfg, Some(cfg_path)).await
}

async fn real_main(cli: Cli) -> Result<()> {
    match cli.command {
        None => client::attach(AttachMode::Default, None).await,
        Some(Cmd::New { session }) => client::attach(AttachMode::New, session).await,
        Some(Cmd::Attach { target }) => client::attach(AttachMode::Existing, target).await,
        Some(Cmd::Ls) => {
            let sessions = client::list().await?;
            if sessions.is_empty() {
                println!("no sessions");
            }
            for s in sessions {
                let plural = if s.windows == 1 { "window" } else { "windows" };
                let attached = if s.attached > 0 { " (attached)" } else { "" };
                println!("{}: {} {plural}{attached}", s.name, s.windows);
            }
            Ok(())
        }
        Some(Cmd::KillServer) => client::kill_server().await,
        Some(Cmd::Reload) => {
            client::reload().await?;
            println!("config reloaded");
            Ok(())
        }
        Some(Cmd::SelectPane { target, left, right, up, down, last }) => {
            let dir = match (left, right, up, down, last) {
                (true, ..) => PaneDir::Left,
                (_, true, ..) => PaneDir::Right,
                (_, _, true, ..) => PaneDir::Up,
                (_, _, _, true, _) => PaneDir::Down,
                (.., true) => PaneDir::Last,
                _ => anyhow::bail!("select-pane needs one of -L -R -U -D -l"),
            };
            client::select_pane(target, dir).await
        }
        Some(Cmd::Query { target, format }) => {
            println!("{}", client::query(target, format).await?);
            Ok(())
        }
        Some(Cmd::Server) => run_server().await,
        Some(Cmd::DebugRun) => dtm::debug::run().await,
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    if let Err(e) = real_main(cli).await {
        eprintln!("dtm: {e:#}");
        std::process::exit(1);
    }
}
