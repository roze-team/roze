mod generator;
mod parser;

use std::path::PathBuf;

use clap::{Parser, Subcommand, ValueEnum};
use roze_sqlx::SqlxDatabaseKind;

use generator::{DependencySource, GenerateMode, GenerateOptions, GeneratorCommand};

#[derive(Debug, Parser)]
#[command(name = "rozectl", version, about = "Roze service code generator")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum RozeSource {
    #[default]
    Git,
    Path,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum ModelFormat {
    #[default]
    Auto,
    Dsl,
    Sql,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum DbKind {
    #[default]
    Sqlite,
    Postgres,
    #[value(alias = "mysql")]
    MySql,
}

impl From<RozeSource> for DependencySource {
    fn from(value: RozeSource) -> Self {
        match value {
            RozeSource::Git => Self::Git,
            RozeSource::Path => Self::Path,
        }
    }
}

impl From<ModelFormat> for generator::model::ModelFormat {
    fn from(value: ModelFormat) -> Self {
        match value {
            ModelFormat::Auto => Self::Auto,
            ModelFormat::Dsl => Self::Dsl,
            ModelFormat::Sql => Self::Sql,
        }
    }
}

impl From<DbKind> for SqlxDatabaseKind {
    fn from(value: DbKind) -> Self {
        match value {
            DbKind::Sqlite => Self::Sqlite,
            DbKind::Postgres => Self::Postgres,
            DbKind::MySql => Self::MySql,
        }
    }
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
    Model {
        #[command(subcommand)]
        command: ModelCommands,
    },
    #[command(hide = true)]
    Generate {
        api: PathBuf,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
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
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
    New {
        name: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        force: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
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
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
    New {
        name: String,
        #[arg(long)]
        out: Option<PathBuf>,
        #[arg(long)]
        force: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
}

#[derive(Debug, Subcommand)]
enum ModelCommands {
    Generate {
        schema: PathBuf,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
        #[arg(long, value_enum, default_value_t)]
        format: ModelFormat,
    },
    Inspect {
        table: String,
        #[arg(long)]
        db_url: String,
        #[arg(long, value_enum, default_value_t)]
        db_kind: DbKind,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let registry = generator::registry();

    match cli.command {
        Commands::Generate {
            api,
            out,
            force,
            update,
            roze_source,
        } => registry.dispatch(GeneratorCommand::ApiGenerate {
            api,
            out,
            options: options(force, update, roze_source),
        })?,
        Commands::Api { command } => match command {
            ApiCommands::Generate {
                api,
                out,
                force,
                update,
                roze_source,
            } => registry.dispatch(GeneratorCommand::ApiGenerate {
                api,
                out,
                options: options(force, update, roze_source),
            })?,
            ApiCommands::New {
                name,
                out,
                force,
                roze_source,
            } => {
                let out = resolve_new_out(&name, out);
                registry.dispatch(GeneratorCommand::ApiNew {
                    name,
                    out,
                    options: GenerateOptions::new(
                        if force {
                            GenerateMode::Force
                        } else {
                            GenerateMode::Create
                        },
                        roze_source.into(),
                    ),
                })?;
            }
        },
        Commands::Rpc { command } => match command {
            RpcCommands::Generate {
                api,
                out,
                force,
                update,
                roze_source,
            } => registry.dispatch(GeneratorCommand::RpcGenerate {
                api,
                out,
                options: options(force, update, roze_source),
            })?,
            RpcCommands::New {
                name,
                out,
                force,
                roze_source,
            } => {
                let out = resolve_new_out(&name, out);
                registry.dispatch(GeneratorCommand::RpcNew {
                    name,
                    out,
                    options: GenerateOptions::new(
                        if force {
                            GenerateMode::Force
                        } else {
                            GenerateMode::Create
                        },
                        roze_source.into(),
                    ),
                })?;
            }
        },
        Commands::Model { command } => match command {
            ModelCommands::Generate {
                schema,
                out,
                force,
                update,
                roze_source,
                format,
            } => registry.dispatch(GeneratorCommand::ModelGenerate {
                schema,
                out,
                options: GenerateOptions::new(
                    if force {
                        GenerateMode::Force
                    } else if update {
                        GenerateMode::Update
                    } else {
                        GenerateMode::Create
                    },
                    roze_source.into(),
                ),
                format: format.into(),
            })?,
            ModelCommands::Inspect {
                table,
                db_url,
                db_kind,
                out,
                force,
                update,
                roze_source,
            } => registry.dispatch(GeneratorCommand::ModelInspect {
                table,
                db_url,
                db_kind: db_kind.into(),
                out,
                options: GenerateOptions::new(
                    if force {
                        GenerateMode::Force
                    } else if update {
                        GenerateMode::Update
                    } else {
                        GenerateMode::Create
                    },
                    roze_source.into(),
                ),
            })?,
        },
    }

    Ok(())
}

fn options(force: bool, update: bool, roze_source: RozeSource) -> GenerateOptions {
    let mode = if force {
        GenerateMode::Force
    } else if update {
        GenerateMode::Update
    } else {
        GenerateMode::Create
    };
    GenerateOptions::new(mode, roze_source.into())
}

fn resolve_new_out(name: &str, out: Option<PathBuf>) -> PathBuf {
    out.unwrap_or_else(|| PathBuf::from(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[test]
    fn new_project_defaults_to_current_directory() {
        assert_eq!(resolve_new_out("user", None), PathBuf::from("user"));
    }

    #[test]
    fn new_project_honors_explicit_output() {
        assert_eq!(
            resolve_new_out("user", Some(PathBuf::from("services/user"))),
            PathBuf::from("services/user")
        );
    }

    #[test]
    fn model_inspect_accepts_schema_qualified_tables() {
        let cli = Cli::try_parse_from([
            "rozectl",
            "model",
            "inspect",
            "public.users",
            "--db-kind",
            "postgres",
            "--db-url",
            "postgres://postgres:postgres@localhost:5432/roze",
        ])
        .expect("parse postgres inspect");

        match cli.command {
            Commands::Model {
                command:
                    ModelCommands::Inspect {
                        table,
                        db_url,
                        db_kind,
                        ..
                    },
            } => {
                assert_eq!(table, "public.users");
                assert_eq!(db_url, "postgres://postgres:postgres@localhost:5432/roze");
                assert!(matches!(db_kind, DbKind::Postgres));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "rozectl",
            "model",
            "inspect",
            "db.users",
            "--db-kind",
            "mysql",
            "--db-url",
            "mysql://root:root@localhost:3306/roze",
        ])
        .expect("parse mysql inspect");

        match cli.command {
            Commands::Model {
                command:
                    ModelCommands::Inspect {
                        table,
                        db_url,
                        db_kind,
                        ..
                    },
            } => {
                assert_eq!(table, "db.users");
                assert_eq!(db_url, "mysql://root:root@localhost:3306/roze");
                assert!(matches!(db_kind, DbKind::MySql));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
