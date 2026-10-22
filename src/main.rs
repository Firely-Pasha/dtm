use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "dtm", about = "A terminal multiplexer")]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run one local shell through the emulator (no server).
    #[command(hide = true)]
    DebugRun,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Some(Cmd::DebugRun) | None => dtm::debug::run().await,
    }
}
