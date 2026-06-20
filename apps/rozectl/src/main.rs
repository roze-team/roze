mod generator;
mod parser;

use std::{ffi::OsString, path::PathBuf};

use clap::{Parser, Subcommand, ValueEnum};
use roze_sqlx::SqlxDatabaseKind;

use generator::{
    goctl::{DockerOptions, KubeDeployOptions, OpenApiOutputFormat},
    DependencySource, GenerateMode, GenerateOptions, GeneratorCommand,
};

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
enum ModelOrm {
    #[value(name = "sea-orm")]
    SeaOrm,
    #[default]
    Toasty,
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

impl From<ModelOrm> for generator::model::ModelOrm {
    fn from(value: ModelOrm) -> Self {
        match value {
            ModelOrm::SeaOrm => Self::SeaOrm,
            ModelOrm::Toasty => Self::Toasty,
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
    Docker {
        #[arg(short = 'g', long = "go")]
        go: PathBuf,
        #[arg(long, default_value = "Dockerfile")]
        out: PathBuf,
        #[arg(long, default_value = "rust:1-bookworm")]
        builder_image: String,
        #[arg(long, default_value = "debian:bookworm-slim")]
        base_image: String,
        #[arg(long, default_value_t = 3000)]
        port: u16,
        #[arg(long, default_value = "UTC")]
        timezone: String,
        #[arg(long)]
        binary: Option<String>,
    },
    Kube {
        #[command(subcommand)]
        command: KubeCommands,
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
    Go {
        #[arg(short = 'a', long = "api")]
        api: PathBuf,
        #[arg(short = 'd', long = "dir", default_value = ".")]
        dir: PathBuf,
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
    Swagger {
        #[arg(short = 'a', long = "api")]
        api: PathBuf,
        #[arg(short = 'd', long = "dir", default_value = ".")]
        dir: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        format: SwaggerFormat,
    },
    Doc {
        #[arg(short = 'd', long = "dir", default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'o', long = "o", default_value = "doc")]
        out: PathBuf,
        #[arg(short = 'a', long = "api")]
        api: Option<PathBuf>,
    },
    Ts {
        #[arg(short = 'a', long = "api")]
        api: PathBuf,
        #[arg(short = 'd', long = "dir", default_value = ".")]
        dir: PathBuf,
    },
    Dart {
        #[arg(short = 'a', long = "api")]
        api: PathBuf,
        #[arg(short = 'd', long = "dir", default_value = ".")]
        dir: PathBuf,
    },
    Plugin {
        #[arg(short = 'p', long = "p")]
        plugin: String,
        #[arg(short = 'a', long = "api")]
        api: PathBuf,
        #[arg(short = 'd', long = "dir", default_value = ".")]
        dir: PathBuf,
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
        proto: PathBuf,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long = "zrpc_out")]
        zrpc_out: Option<PathBuf>,
        #[arg(long = "go_out")]
        go_out: Option<PathBuf>,
        #[arg(long = "go-grpc_out")]
        go_grpc_out: Option<PathBuf>,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum SwaggerFormat {
    #[default]
    Json,
    Yaml,
}

impl From<SwaggerFormat> for OpenApiOutputFormat {
    fn from(value: SwaggerFormat) -> Self {
        match value {
            SwaggerFormat::Json => Self::Json,
            SwaggerFormat::Yaml => Self::Yaml,
        }
    }
}

#[derive(Debug, Subcommand)]
enum KubeCommands {
    Deploy {
        #[arg(long)]
        name: String,
        #[arg(long)]
        image: String,
        #[arg(long, default_value = "default")]
        namespace: String,
        #[arg(long, default_value_t = 2)]
        replicas: u32,
        #[arg(long, default_value_t = 3000)]
        port: u16,
        #[arg(long, alias = "request-cpu", default_value = "100m")]
        cpu_request: String,
        #[arg(long, alias = "limit-cpu", default_value = "500m")]
        cpu_limit: String,
        #[arg(long, alias = "request-memory", default_value = "128Mi")]
        memory_request: String,
        #[arg(long, alias = "limit-memory", default_value = "512Mi")]
        memory_limit: String,
        #[arg(long, default_value_t = 1)]
        min_replicas: u32,
        #[arg(long, default_value_t = 5)]
        max_replicas: u32,
        #[arg(long, default_value_t = 70)]
        target_cpu: u32,
        #[arg(long)]
        env: Vec<String>,
        #[arg(long)]
        env_file: Option<PathBuf>,
        #[arg(long)]
        config_map: Option<String>,
        #[arg(long, default_value = "deploy/kubernetes.yaml")]
        out: PathBuf,
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
        #[arg(long, value_enum, default_value_t)]
        orm: ModelOrm,
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
        #[arg(long, value_enum, default_value_t)]
        orm: ModelOrm,
    },
    Mysql {
        #[command(subcommand)]
        command: MysqlModelCommands,
    },
    Pg {
        #[command(subcommand)]
        command: PgModelCommands,
    },
    Mongo {
        #[arg(long = "type")]
        ty: String,
        #[arg(short = 'd', long = "dir", default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
}

#[derive(Debug, Subcommand)]
enum MysqlModelCommands {
    Ddl {
        #[arg(short = 's', long = "src")]
        src: PathBuf,
        #[arg(short = 'd', long = "dir", default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
        #[arg(long, value_enum, default_value_t)]
        orm: ModelOrm,
    },
    Datasource {
        #[arg(short = 'u', long = "url")]
        url: String,
        #[arg(short = 't', long = "table")]
        table: String,
        #[arg(short = 'd', long = "dir", default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
        #[arg(long, value_enum, default_value_t)]
        orm: ModelOrm,
    },
}

#[derive(Debug, Subcommand)]
enum PgModelCommands {
    Datasource {
        #[arg(short = 'u', long = "url")]
        url: String,
        #[arg(short = 't', long = "table")]
        table: String,
        #[arg(short = 'd', long = "dir", default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        schema: Option<String>,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
        #[arg(long, value_enum, default_value_t)]
        orm: ModelOrm,
    },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse_from(normalize_goctl_args(std::env::args_os()));
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
            ApiCommands::Go {
                api,
                dir,
                force,
                update,
                roze_source,
            } => registry.dispatch(GeneratorCommand::ApiGenerate {
                api,
                out: dir,
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
            ApiCommands::Swagger { api, dir, format } => {
                generator::goctl::write_swagger(&api, &dir, format.into())?;
            }
            ApiCommands::Doc { dir, out, api } => {
                let out = if out.is_absolute() {
                    out
                } else {
                    dir.join(out)
                };
                generator::goctl::write_api_markdown_doc(api.as_deref(), &dir, &out)?;
            }
            ApiCommands::Ts { api, dir } => {
                generator::write_ts_client(&api, &dir.join("client.ts"))?;
            }
            ApiCommands::Dart { api, dir } => {
                generator::write_dart_client(&api, &dir.join("client.dart"))?;
            }
            ApiCommands::Plugin { plugin, api, dir } => {
                generator::goctl::run_api_plugin(&plugin, &api, &dir)?;
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
            RpcCommands::Protoc {
                proto,
                out,
                zrpc_out,
                go_out: _,
                go_grpc_out: _,
                force,
                update,
                roze_source,
            } => {
                let out = zrpc_out.unwrap_or(out);
                generator::goctl::generate_rpc_from_proto(
                    &proto,
                    &out,
                    options(force, update, roze_source),
                )?;
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
                orm,
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
                orm: orm.into(),
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
                orm,
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
                orm: orm.into(),
            })?,
            ModelCommands::Mysql { command } => match command {
                MysqlModelCommands::Ddl {
                    src,
                    dir,
                    force,
                    update,
                    roze_source,
                    orm,
                } => registry.dispatch(GeneratorCommand::ModelGenerate {
                    schema: src,
                    out: dir,
                    options: options(force, update, roze_source),
                    format: generator::model::ModelFormat::Sql,
                    orm: orm.into(),
                })?,
                MysqlModelCommands::Datasource {
                    url,
                    table,
                    dir,
                    force,
                    update,
                    roze_source,
                    orm,
                } => registry.dispatch(GeneratorCommand::ModelInspect {
                    table,
                    schema: None,
                    db_url: url,
                    db_kind: SqlxDatabaseKind::MySql,
                    out: dir,
                    options: options(force, update, roze_source),
                    orm: orm.into(),
                })?,
            },
            ModelCommands::Pg { command } => match command {
                PgModelCommands::Datasource {
                    url,
                    table,
                    dir,
                    schema,
                    force,
                    update,
                    roze_source,
                    orm,
                } => registry.dispatch(GeneratorCommand::ModelInspect {
                    table,
                    schema,
                    db_url: url,
                    db_kind: SqlxDatabaseKind::Postgres,
                    out: dir,
                    options: options(force, update, roze_source),
                    orm: orm.into(),
                })?,
            },
            ModelCommands::Mongo {
                ty,
                dir,
                force,
                update,
                roze_source,
            } => {
                generator::goctl::generate_mongo_model_type(
                    &ty,
                    &dir,
                    options(force, update, roze_source),
                )?;
            }
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
        Commands::Docker {
            go,
            out,
            builder_image,
            base_image,
            port,
            timezone,
            binary,
        } => generator::goctl::write_dockerfile(DockerOptions {
            main: go,
            out,
            builder_image,
            base_image,
            port,
            timezone,
            binary,
        })?,
        Commands::Kube { command } => match command {
            KubeCommands::Deploy {
                name,
                image,
                namespace,
                replicas,
                port,
                cpu_request,
                cpu_limit,
                memory_request,
                memory_limit,
                min_replicas,
                max_replicas,
                target_cpu,
                env,
                env_file,
                config_map,
                out,
            } => generator::goctl::write_kube_deploy(KubeDeployOptions {
                name,
                image,
                namespace,
                replicas,
                port,
                cpu_request,
                cpu_limit,
                memory_request,
                memory_limit,
                min_replicas,
                max_replicas,
                target_cpu,
                env,
                env_file,
                config_map,
                out,
            })?,
        },
    }

    Ok(())
}

fn normalize_goctl_args<I>(args: I) -> Vec<OsString>
where
    I: IntoIterator,
    I::Item: Into<OsString>,
{
    args.into_iter()
        .map(Into::into)
        .map(|arg| match arg.to_str() {
            Some("-api") => OsString::from("--api"),
            Some("-dir") => OsString::from("--dir"),
            Some("-src") => OsString::from("--src"),
            Some("-url") => OsString::from("--url"),
            Some("-table") => OsString::from("--table"),
            Some("-schema") => OsString::from("--schema"),
            Some("-go") => OsString::from("--go"),
            Some("-o") => OsString::from("--o"),
            _ => arg,
        })
        .collect()
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

    fn parse(args: impl IntoIterator<Item = &'static str>) -> Cli {
        Cli::try_parse_from(normalize_goctl_args(args)).expect("parse cli")
    }

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

        let toasty = Cli::try_parse_from([
            "rozectl", "model", "generate", "user.sql", "--format", "sql", "--orm", "toasty",
        ])
        .expect("parse toasty model generate");
        assert!(matches!(
            toasty.command,
            Commands::Model {
                command: ModelCommands::Generate {
                    format: ModelFormat::Sql,
                    orm: ModelOrm::Toasty,
                    ..
                }
            }
        ));

        let default_orm = Cli::try_parse_from(["rozectl", "model", "generate", "user.sql"])
            .expect("parse default model orm");
        assert!(matches!(
            default_orm.command,
            Commands::Model {
                command: ModelCommands::Generate {
                    orm: ModelOrm::Toasty,
                    ..
                }
            }
        ));

        let api_go = parse(["rozectl", "api", "go", "-api", "user.api", "-dir", "out"]);
        assert!(matches!(
            api_go.command,
            Commands::Api {
                command: ApiCommands::Go { .. }
            }
        ));

        let swagger = Cli::try_parse_from([
            "rozectl", "api", "swagger", "--api", "user.api", "--dir", "docs", "--format", "yaml",
        ])
        .expect("parse api swagger");
        assert!(matches!(
            swagger.command,
            Commands::Api {
                command: ApiCommands::Swagger {
                    format: SwaggerFormat::Yaml,
                    ..
                }
            }
        ));

        let doc = Cli::try_parse_from(["rozectl", "api", "doc", "--dir", ".", "--o", "doc"])
            .expect("parse api doc");
        assert!(matches!(
            doc.command,
            Commands::Api {
                command: ApiCommands::Doc { .. }
            }
        ));

        let ts = Cli::try_parse_from(["rozectl", "api", "ts", "--api", "user.api", "--dir", "sdk"])
            .expect("parse api ts");
        assert!(matches!(
            ts.command,
            Commands::Api {
                command: ApiCommands::Ts { .. }
            }
        ));

        let dart = Cli::try_parse_from([
            "rozectl", "api", "dart", "--api", "user.api", "--dir", "sdk",
        ])
        .expect("parse api dart");
        assert!(matches!(
            dart.command,
            Commands::Api {
                command: ApiCommands::Dart { .. }
            }
        ));

        let plugin = Cli::try_parse_from([
            "rozectl", "api", "plugin", "-p", "cat", "--api", "user.api", "--dir", "out",
        ])
        .expect("parse api plugin");
        assert!(matches!(
            plugin.command,
            Commands::Api {
                command: ApiCommands::Plugin { .. }
            }
        ));

        let docker = parse(["rozectl", "docker", "-go", "main.go"]);
        assert!(matches!(docker.command, Commands::Docker { .. }));

        let kube = Cli::try_parse_from([
            "rozectl",
            "kube",
            "deploy",
            "--name",
            "user",
            "--image",
            "user:latest",
            "--env-file",
            ".env",
            "--config-map",
            "user-config",
        ])
        .expect("parse kube deploy");
        assert!(matches!(
            kube.command,
            Commands::Kube {
                command: KubeCommands::Deploy { .. }
            }
        ));

        let mysql_ddl = parse([
            "rozectl", "model", "mysql", "ddl", "-src", "user.sql", "-dir", "out",
        ]);
        assert!(matches!(
            mysql_ddl.command,
            Commands::Model {
                command: ModelCommands::Mysql {
                    command: MysqlModelCommands::Ddl { .. }
                }
            }
        ));

        let mysql_ddl_toasty = parse([
            "rozectl", "model", "mysql", "ddl", "-src", "user.sql", "-dir", "out", "--orm",
            "toasty",
        ]);
        assert!(matches!(
            mysql_ddl_toasty.command,
            Commands::Model {
                command: ModelCommands::Mysql {
                    command: MysqlModelCommands::Ddl {
                        orm: ModelOrm::Toasty,
                        ..
                    }
                }
            }
        ));

        let mysql_datasource = parse([
            "rozectl",
            "model",
            "mysql",
            "datasource",
            "-url",
            "mysql://root@localhost/db",
            "-table",
            "users",
            "-dir",
            "out",
        ]);
        assert!(matches!(
            mysql_datasource.command,
            Commands::Model {
                command: ModelCommands::Mysql {
                    command: MysqlModelCommands::Datasource { .. }
                }
            }
        ));

        let pg_datasource = parse([
            "rozectl",
            "model",
            "pg",
            "datasource",
            "-url",
            "postgres://postgres@localhost/db",
            "-table",
            "users",
            "--schema",
            "public",
            "-dir",
            "out",
        ]);
        assert!(matches!(
            pg_datasource.command,
            Commands::Model {
                command: ModelCommands::Pg {
                    command: PgModelCommands::Datasource { .. }
                }
            }
        ));

        let mongo_type = Cli::try_parse_from([
            "rozectl", "model", "mongo", "--type", "User", "--dir", "out",
        ])
        .expect("parse mongo type");
        assert!(matches!(
            mongo_type.command,
            Commands::Model {
                command: ModelCommands::Mongo { .. }
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
