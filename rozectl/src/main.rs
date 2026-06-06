mod generator;
mod parser;

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "rozectl", about = "Roze service code generator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Generate {
        api: PathBuf,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Generate { api, out, force } => {
            let source = std::fs::read_to_string(&api)
                .with_context(|| format!("failed to read {}", api.display()))?;
            let spec = parser::parse_api(&source)?;
            generator::generate_project(&spec, &out, force)
                .with_context(|| format!("failed to generate project at {}", out.display()))?;
        }
    }

    Ok(())
}
