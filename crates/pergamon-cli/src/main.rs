//! # pergamon CLI
//!
//! Command-line interface for pergamon — unified personal information
//! system. Combines RSS reader, read-later, bookmark manager, and
//! knowledge retention engine into a single CLI + ratatui TUI.

use clap::Parser;

/// pergamon — unified personal information system.
#[derive(Debug, Parser)]
#[command(name = "pergamon", version, about)]
struct Cli {
    /// Print version information.
    #[arg(long)]
    info: bool,
}

fn main() {
    let cli = Cli::parse();

    if cli.info {
        println!("pergamon-core {}", pergamon_core::VERSION);
    }
}
