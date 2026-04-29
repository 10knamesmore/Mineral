//! `mineral` 二进制入口。

use clap::Parser;
use mineral_cli::Args;

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;

    let args = Args::parse();
    match args.command {
        Some(command) => mineral_cli::run(command),
        None => mineral_tui::run(),
    }
}
