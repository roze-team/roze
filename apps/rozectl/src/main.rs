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
    Mongo,
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
            ModelFormat::Mongo => Self::Mongo,
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
    Template {
        #[command(subcommand)]
        command: TemplateCommands,
    },
    Openapi {
        #[command(subcommand)]
        command: OpenApiCommands,
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
    Goctl {
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
    Client {
        #[command(subcommand)]
        command: ClientCommands,
    },
}

#[derive(Debug, Subcommand)]
enum ClientCommands {
    Ts {
        api: PathBuf,
        #[arg(long, default_value = "client.ts")]
        out: PathBuf,
    },
    Js {
        api: PathBuf,
        #[arg(long, default_value = "client.js")]
        out: PathBuf,
    },
    Dart {
        api: PathBuf,
        #[arg(long, default_value = "client.dart")]
        out: PathBuf,
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
    Protoc {
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
enum TemplateCommands {
    List,
    Show {
        name: String,
    },
    Init {
        #[arg(long, default_value = "templates")]
        out: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum OpenApiCommands {
    Generate {
        api: PathBuf,
        #[arg(long, default_value = "openapi.json")]
        out: PathBuf,
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
        schema: Option<String>,
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
            ApiCommands::Goctl {
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
            ApiCommands::Client { command } => match command {
                ClientCommands::Ts { api, out } => {
                    generator::write_ts_client(&api, &out)?;
                }
                ClientCommands::Js { api, out } => {
                    generator::write_js_client(&api, &out)?;
                }
                ClientCommands::Dart { api, out } => {
                    generator::write_dart_client(&api, &out)?;
                }
            },
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
            RpcCommands::Protoc {
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
                schema,
                db_url,
                db_kind,
                out,
                force,
                update,
                roze_source,
            } => registry.dispatch(GeneratorCommand::ModelInspect {
                table,
                schema,
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
        Commands::Template { command } => match command {
            TemplateCommands::List => {
                println!("api\nrpc\nmodel");
            }
            TemplateCommands::Show { name } => {
                println!("{}", generator::template(&name)?);
            }
            TemplateCommands::Init { out } => {
                generator::init_templates(&out)?;
            }
        },
        Commands::Openapi { command } => match command {
            OpenApiCommands::Generate { api, out } => {
                generator::write_openapi_json(&api, &out)?;
            }
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
    fn parses_compatibility_commands() {
        let api = Cli::try_parse_from(["rozectl", "api", "goctl", "user.api", "--out", "out"])
            .expect("parse api goctl");
        assert!(matches!(
            api.command,
            Commands::Api {
                command: ApiCommands::Goctl { .. }
            }
        ));

        let rpc = Cli::try_parse_from(["rozectl", "rpc", "protoc", "user.api"])
            .expect("parse rpc protoc");
        assert!(matches!(
            rpc.command,
            Commands::Rpc {
                command: RpcCommands::Protoc { .. }
            }
        ));

        let template =
            Cli::try_parse_from(["rozectl", "template", "show", "api"]).expect("parse template");
        assert!(matches!(
            template.command,
            Commands::Template {
                command: TemplateCommands::Show { .. }
            }
        ));

        let openapi = Cli::try_parse_from([
            "rozectl",
            "openapi",
            "generate",
            "user.api",
            "--out",
            "openapi.json",
        ])
        .expect("parse openapi");
        assert!(matches!(
            openapi.command,
            Commands::Openapi {
                command: OpenApiCommands::Generate { .. }
            }
        ));

        let client = Cli::try_parse_from([
            "rozectl",
            "api",
            "client",
            "ts",
            "user.api",
            "--out",
            "sdk/user.ts",
        ])
        .expect("parse api client ts");
        assert!(matches!(
            client.command,
            Commands::Api {
                command: ApiCommands::Client {
                    command: ClientCommands::Ts { .. }
                }
            }
        ));

        let js_client = Cli::try_parse_from([
            "rozectl",
            "api",
            "client",
            "js",
            "user.api",
            "--out",
            "sdk/user.js",
        ])
        .expect("parse api client js");
        assert!(matches!(
            js_client.command,
            Commands::Api {
                command: ApiCommands::Client {
                    command: ClientCommands::Js { .. }
                }
            }
        ));

        let dart_client = Cli::try_parse_from([
            "rozectl",
            "api",
            "client",
            "dart",
            "user.api",
            "--out",
            "sdk/user.dart",
        ])
        .expect("parse api client dart");
        assert!(matches!(
            dart_client.command,
            Commands::Api {
                command: ApiCommands::Client {
                    command: ClientCommands::Dart { .. }
                }
            }
        ));

        let mongo = Cli::try_parse_from([
            "rozectl",
            "model",
            "generate",
            "user.model",
            "--format",
            "mongo",
        ])
        .expect("parse mongo model generate");
        assert!(matches!(
            mongo.command,
            Commands::Model {
                command: ModelCommands::Generate {
                    format: ModelFormat::Mongo,
                    ..
                }
            }
        ));
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
                        schema,
                        db_url,
                        db_kind,
                        ..
                    },
            } => {
                assert_eq!(table, "public.users");
                assert!(schema.is_none());
                assert_eq!(db_url, "postgres://postgres:postgres@localhost:5432/roze");
                assert!(matches!(db_kind, DbKind::Postgres));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "rozectl",
            "model",
            "inspect",
            "users",
            "--schema",
            "db",
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
                        schema,
                        db_url,
                        db_kind,
                        ..
                    },
            } => {
                assert_eq!(table, "users");
                assert_eq!(schema.as_deref(), Some("db"));
                assert_eq!(db_url, "mysql://root:root@localhost:3306/roze");
                assert!(matches!(db_kind, DbKind::MySql));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "rozectl",
            "model",
            "inspect",
            "public.users",
            "--schema",
            "public",
            "--db-kind",
            "postgres",
            "--db-url",
            "postgres://postgres:postgres@localhost:5432/roze",
        ])
        .expect("parse postgres inspect with schema");

        match cli.command {
            Commands::Model {
                command:
                    ModelCommands::Inspect {
                        table,
                        schema,
                        db_kind,
                        ..
                    },
            } => {
                assert_eq!(table, "public.users");
                assert_eq!(schema.as_deref(), Some("public"));
                assert!(matches!(db_kind, DbKind::Postgres));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        let cli = Cli::try_parse_from([
            "rozectl",
            "model",
            "inspect",
            "db.users",
            "--schema",
            "db",
            "--db-kind",
            "mysql",
            "--db-url",
            "mysql://root:root@localhost:3306/roze",
        ])
        .expect("parse mysql inspect with schema");

        match cli.command {
            Commands::Model {
                command:
                    ModelCommands::Inspect {
                        table,
                        schema,
                        db_kind,
                        ..
                    },
            } => {
                assert_eq!(table, "db.users");
                assert_eq!(schema.as_deref(), Some("db"));
                assert!(matches!(db_kind, DbKind::MySql));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}
