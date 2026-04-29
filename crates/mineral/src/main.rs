//! Thin entrypoint for the `mineral` command.

use clap::Parser;
use mineral_cli::Args;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let args = Args::parse();
    match args.command {
        Some(command) => mineral_cli::run(command).map_err(|e| color_eyre::eyre::eyre!(e)),
        None => mineral_tui::run(),
    }
}
