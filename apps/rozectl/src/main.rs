mod generator;
mod parser;

use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "rozectl", version, about = "Roze service code generator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Api {
        #[command(subcommand)]
        command: ApiCommands,
    },
    Rpc {
        #[command(subcommand)]
        command: RpcCommands,
    },
    #[command(hide = true)]
    Generate {
        api: PathBuf,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ApiCommands {
    Generate {
        api: PathBuf,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
    },
    New {
        name: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum RpcCommands {
    Generate {
        api: PathBuf,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
    },
    New {
        name: String,
        #[arg(long)]
        out: Option<PathBuf>,
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
        Commands::Api { command } => match command {
            ApiCommands::Generate { api, out, force } => {
                let source = std::fs::read_to_string(&api)
                    .with_context(|| format!("failed to read {}", api.display()))?;
                let spec = parser::parse_api(&source)?;
                generator::generate_project(&spec, &out, force)
                    .with_context(|| format!("failed to generate project at {}", out.display()))?;
            }
            ApiCommands::New { name, out, force } => {
                let out = resolve_new_out(&name, out);
                generator::create_api_project(&name, &out, force)
                    .with_context(|| format!("failed to create api project at {}", out.display()))?;
            }
        },
        Commands::Rpc { command } => match command {
            RpcCommands::Generate { api, out, force } => {
                let source = std::fs::read_to_string(&api)
                    .with_context(|| format!("failed to read {}", api.display()))?;
                let spec = parser::parse_api(&source)?;
                generator::generate_project(&spec, &out, force)
                    .with_context(|| format!("failed to generate project at {}", out.display()))?;
            }
            RpcCommands::New { name, out, force } => {
                let out = resolve_new_out(&name, out);
                generator::create_rpc_project(&name, &out, force)
                    .with_context(|| format!("failed to create rpc project at {}", out.display()))?;
            }
        },
    }

    Ok(())
}

fn resolve_new_out(name: &str, out: Option<PathBuf>) -> PathBuf {
    out.unwrap_or_else(|| PathBuf::from("apps").join(name))
}
