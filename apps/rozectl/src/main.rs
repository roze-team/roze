mod generator;
mod parser;

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use clap::{Parser, Subcommand, ValueEnum};
use roze_sqlx::SqlxDatabaseKind;

use generator::{
    goctl::{DockerOptions, HelmOptions, KubeDeployOptions, OpenApiOutputFormat},
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
    Diff {
        #[command(subcommand)]
        command: DiffCommands,
    },
    Contract {
        #[command(subcommand)]
        command: ContractCommands,
    },
    Mock {
        #[command(subcommand)]
        command: MockCommands,
    },
    Test {
        #[command(subcommand)]
        command: TestCommands,
    },
    Dev {
        #[command(subcommand)]
        command: DevCommands,
    },
    Doctor {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        port: Vec<u16>,
        #[arg(long)]
        tcp: Vec<String>,
        #[arg(long)]
        tool: Vec<String>,
    },
    Doc {
        #[command(subcommand)]
        command: DocCommands,
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
    Helm {
        #[command(subcommand)]
        command: HelmCommands,
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
        #[arg(long)]
        service_account: bool,
        #[arg(long)]
        pdb: bool,
        #[arg(long, default_value = "1")]
        min_available: String,
        #[arg(long)]
        network_policy: bool,
        #[arg(long, default_value = "deploy/kubernetes.yaml")]
        out: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum HelmCommands {
    Chart {
        #[arg(long)]
        name: String,
        #[arg(long)]
        image: String,
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
        config_map: Option<String>,
        #[arg(long, default_value = "0.1.0")]
        chart_version: String,
        #[arg(long, default_value = "0.1.0")]
        app_version: String,
        #[arg(long, default_value = "deploy/chart")]
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
enum DiffCommands {
    Api {
        api: PathBuf,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
    Rpc {
        api: PathBuf,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
    Model {
        schema: PathBuf,
        #[arg(long, default_value = ".")]
        out: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
        #[arg(long, value_enum, default_value_t)]
        format: ModelFormat,
        #[arg(long, value_enum, default_value_t)]
        orm: ModelOrm,
    },
}

#[derive(Debug, Subcommand)]
enum ContractCommands {
    Check {
        #[arg(long)]
        old: PathBuf,
        #[arg(long)]
        new: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum MockCommands {
    Gen {
        #[arg(short = 'a', long = "api")]
        api: PathBuf,
        #[arg(long, default_value = "mock-server")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum TestCommands {
    Gen {
        #[arg(short = 'a', long = "api")]
        api: PathBuf,
        #[arg(long, default_value = "contract-tests")]
        out: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:3000")]
        base_url: String,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum DevCommands {
    Up {
        #[arg(
            short = 'f',
            long = "file",
            default_value = "docker-compose.integration.yml"
        )]
        file: PathBuf,
        #[arg(long)]
        profile: Vec<String>,
        #[arg(long)]
        detach: bool,
    },
    Down {
        #[arg(
            short = 'f',
            long = "file",
            default_value = "docker-compose.integration.yml"
        )]
        file: PathBuf,
        #[arg(long)]
        profile: Vec<String>,
        #[arg(short = 'v', long)]
        volumes: bool,
    },
    Status {
        #[arg(
            short = 'f',
            long = "file",
            default_value = "docker-compose.integration.yml"
        )]
        file: PathBuf,
        #[arg(long)]
        profile: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
enum DocCommands {
    Service {
        #[arg(short = 'a', long = "api")]
        api: PathBuf,
        #[arg(long, default_value = "SERVICE.md")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
    },
    AiContext {
        #[arg(short = 'a', long = "api")]
        api: PathBuf,
        #[arg(long, default_value = "AI_CONTEXT.md")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
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
        Commands::Diff { command } => run_diff(command, &registry)?,
        Commands::Contract { command } => run_contract(command)?,
        Commands::Mock { command } => match command {
            MockCommands::Gen { api, out, force } => {
                generator::write_mock_server_project(&api, &out, force)?;
            }
        },
        Commands::Test { command } => match command {
            TestCommands::Gen {
                api,
                out,
                base_url,
                force,
            } => {
                generator::write_http_smoke_test_project(&api, &out, &base_url, force)?;
            }
        },
        Commands::Dev { command } => run_dev(command)?,
        Commands::Doctor {
            config,
            port,
            tcp,
            tool,
        } => run_doctor(config, port, tcp, tool)?,
        Commands::Doc { command } => match command {
            DocCommands::Service { api, out, force } => {
                generator::write_service_markdown_doc(&api, &out, force)?;
            }
            DocCommands::AiContext { api, out, force } => {
                generator::write_ai_context_markdown_doc(&api, &out, force)?;
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
        Commands::Helm { command } => match command {
            HelmCommands::Chart {
                name,
                image,
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
                config_map,
                chart_version,
                app_version,
                out,
            } => generator::goctl::write_helm_chart(HelmOptions {
                deploy: KubeDeployOptions {
                    name,
                    image,
                    namespace: "default".to_string(),
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
                    env_file: None,
                    config_map,
                    out,
                },
                chart_version,
                app_version,
            })?,
        },
    }

    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContractIssue {
    kind: ContractIssueKind,
    detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContractIssueKind {
    Breaking,
}

fn run_contract(command: ContractCommands) -> anyhow::Result<()> {
    match command {
        ContractCommands::Check { old, new } => {
            let old_spec = read_api_spec(&old)?;
            let new_spec = read_api_spec(&new)?;
            let issues = check_contract_compatibility(&old_spec, &new_spec);
            if issues.is_empty() {
                println!("contract check passed: no breaking changes detected");
                return Ok(());
            }

            eprintln!("contract check failed: {} breaking change(s)", issues.len());
            for issue in issues {
                eprintln!("- {}", issue.detail);
            }
            anyhow::bail!("contract check failed")
        }
    }
}

fn read_api_spec(path: &Path) -> anyhow::Result<parser::ApiSpec> {
    let source = fs::read_to_string(path)
        .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", path.display()))?;
    parser::parse_api(&source)
        .map_err(|err| anyhow::anyhow!("failed to parse {}: {err}", path.display()))
}

fn check_contract_compatibility(
    old_spec: &parser::ApiSpec,
    new_spec: &parser::ApiSpec,
) -> Vec<ContractIssue> {
    let mut issues = Vec::new();
    check_rest_routes(old_spec, new_spec, &mut issues);
    check_rpc_methods(old_spec, new_spec, &mut issues);
    check_types(old_spec, new_spec, &mut issues);
    issues
}

fn check_rest_routes(
    old_spec: &parser::ApiSpec,
    new_spec: &parser::ApiSpec,
    issues: &mut Vec<ContractIssue>,
) {
    let new_routes = new_spec
        .rest_routes
        .iter()
        .map(|route| (rest_route_key(new_spec, route), route))
        .collect::<BTreeMap<_, _>>();

    for old_route in &old_spec.rest_routes {
        let key = rest_route_key(old_spec, old_route);
        let Some(new_route) = new_routes.get(&key) else {
            issues.push(breaking(format!("REST route removed or changed: {key}")));
            continue;
        };
        if old_route.request != new_route.request {
            issues.push(breaking(format!(
                "REST route {key} request type changed: {} -> {}",
                old_route.request, new_route.request
            )));
        }
        if old_route.response != new_route.response {
            issues.push(breaking(format!(
                "REST route {key} response type changed: {} -> {}",
                old_route.response, new_route.response
            )));
        }
    }
}

fn check_rpc_methods(
    old_spec: &parser::ApiSpec,
    new_spec: &parser::ApiSpec,
    issues: &mut Vec<ContractIssue>,
) {
    let new_methods = new_spec
        .rpc_methods
        .iter()
        .map(|method| (method.name.as_str(), method))
        .collect::<BTreeMap<_, _>>();

    for old_method in &old_spec.rpc_methods {
        let Some(new_method) = new_methods.get(old_method.name.as_str()) else {
            issues.push(breaking(format!("RPC method removed: {}", old_method.name)));
            continue;
        };
        if old_method.request != new_method.request {
            issues.push(breaking(format!(
                "RPC method {} request type changed: {} -> {}",
                old_method.name, old_method.request, new_method.request
            )));
        }
        if old_method.response != new_method.response {
            issues.push(breaking(format!(
                "RPC method {} response type changed: {} -> {}",
                old_method.name, old_method.response, new_method.response
            )));
        }
    }
}

fn check_types(
    old_spec: &parser::ApiSpec,
    new_spec: &parser::ApiSpec,
    issues: &mut Vec<ContractIssue>,
) {
    let new_types = new_spec
        .types
        .iter()
        .map(|ty| (ty.name.as_str(), ty))
        .collect::<BTreeMap<_, _>>();

    for old_ty in &old_spec.types {
        let Some(new_ty) = new_types.get(old_ty.name.as_str()) else {
            issues.push(breaking(format!("type removed: {}", old_ty.name)));
            continue;
        };
        let new_fields = new_ty
            .fields
            .iter()
            .map(|field| (contract_field_key(field), field))
            .collect::<BTreeMap<_, _>>();
        let old_fields = old_ty
            .fields
            .iter()
            .map(|field| (contract_field_key(field), field))
            .collect::<BTreeMap<_, _>>();

        for old_field in &old_ty.fields {
            let key = contract_field_key(old_field);
            let Some(new_field) = new_fields.get(&key) else {
                issues.push(breaking(format!(
                    "field removed: {}.{}",
                    old_ty.name, old_field.name
                )));
                continue;
            };
            if old_field.ty != new_field.ty {
                issues.push(breaking(format!(
                    "field type changed: {}.{} {} -> {}",
                    old_ty.name, old_field.name, old_field.ty, new_field.ty
                )));
            }
            if old_field.source != new_field.source {
                issues.push(breaking(format!(
                    "field source changed: {}.{} {:?} -> {:?}",
                    old_ty.name, old_field.name, old_field.source, new_field.source
                )));
            }
        }

        for new_field in &new_ty.fields {
            let key = contract_field_key(new_field);
            if !old_fields.contains_key(&key) && !is_optional_field(new_field) {
                issues.push(breaking(format!(
                    "required field added: {}.{}",
                    new_ty.name, new_field.name
                )));
            }
        }
    }
}

fn rest_route_key(spec: &parser::ApiSpec, route: &parser::RestRoute) -> String {
    format!(
        "{} {}",
        http_method_name(&route.method),
        generator::rest::full_route_path_for_route(spec, route)
    )
}

fn http_method_name(method: &parser::HttpMethod) -> &'static str {
    match method {
        parser::HttpMethod::Get => "GET",
        parser::HttpMethod::Post => "POST",
        parser::HttpMethod::Put => "PUT",
        parser::HttpMethod::Patch => "PATCH",
        parser::HttpMethod::Delete => "DELETE",
    }
}

fn contract_field_key(field: &parser::Field) -> String {
    field
        .wire_name
        .as_deref()
        .or(field.json_name.as_deref())
        .unwrap_or(&field.name)
        .to_string()
}

fn is_optional_field(field: &parser::Field) -> bool {
    field.validate.as_deref().is_some_and(|rules| {
        has_contract_rule(rules, "optional") || has_contract_rule(rules, "omitempty")
    })
}

fn has_contract_rule(rules: &str, name: &str) -> bool {
    rules.split(',').map(str::trim).any(|rule| {
        rule == name
            || rule
                .strip_prefix(name)
                .is_some_and(|rest| rest.starts_with('='))
    })
}

fn breaking(detail: impl Into<String>) -> ContractIssue {
    ContractIssue {
        kind: ContractIssueKind::Breaking,
        detail: detail.into(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DoctorStatus {
    Ok,
    Warn,
    Fail,
}

impl DoctorStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Ok => "OK",
            Self::Warn => "WARN",
            Self::Fail => "FAIL",
        }
    }
}

#[derive(Debug)]
struct DoctorCheck {
    status: DoctorStatus,
    name: String,
    detail: String,
}

impl DoctorCheck {
    fn ok(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Ok,
            name: name.into(),
            detail: detail.into(),
        }
    }

    fn warn(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Warn,
            name: name.into(),
            detail: detail.into(),
        }
    }

    fn fail(name: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            status: DoctorStatus::Fail,
            name: name.into(),
            detail: detail.into(),
        }
    }
}

fn run_doctor(
    config: Option<PathBuf>,
    ports: Vec<u16>,
    tcp_targets: Vec<String>,
    extra_tools: Vec<String>,
) -> anyhow::Result<()> {
    let mut checks = Vec::new();
    for tool in doctor_tools(extra_tools) {
        checks.push(check_tool(&tool));
    }
    if let Some(config) = config {
        checks.push(check_config(&config));
    }
    for port in ports {
        checks.push(check_port(port));
    }
    for target in tcp_targets {
        checks.push(check_tcp(&target));
    }

    let has_failures = checks
        .iter()
        .any(|check| matches!(check.status, DoctorStatus::Fail));
    for check in checks {
        println!(
            "{} {:<18} {}",
            check.status.label(),
            check.name,
            check.detail
        );
    }
    if has_failures {
        anyhow::bail!("doctor found failing checks");
    }
    Ok(())
}

fn doctor_tools(extra_tools: Vec<String>) -> Vec<String> {
    let mut tools = vec![
        "rustc".to_string(),
        "cargo".to_string(),
        "docker".to_string(),
        "kubectl".to_string(),
    ];
    for tool in extra_tools {
        if !tools.iter().any(|existing| existing == &tool) {
            tools.push(tool);
        }
    }
    tools
}

fn check_tool(tool: &str) -> DoctorCheck {
    let output = tool_version_command(tool)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    match output {
        Ok(output) if output.status.success() => {
            let version = String::from_utf8_lossy(&output.stdout);
            let version = version.lines().next().unwrap_or("available").trim();
            DoctorCheck::ok(format!("tool:{tool}"), version)
        }
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let detail = stderr
                .lines()
                .next()
                .unwrap_or("version check failed")
                .trim();
            DoctorCheck::warn(format!("tool:{tool}"), detail)
        }
        Err(err) => DoctorCheck::warn(format!("tool:{tool}"), format!("not available: {err}")),
    }
}

fn tool_version_command(tool: &str) -> Command {
    let mut command = Command::new(tool);
    match tool {
        "kubectl" => {
            command.args(["version", "--client"]);
        }
        _ => {
            command.arg("--version");
        }
    }
    command
}

fn check_config(path: &Path) -> DoctorCheck {
    if path.is_file() {
        DoctorCheck::ok("config", format!("{} exists", path.display()))
    } else if path.exists() {
        DoctorCheck::fail("config", format!("{} is not a file", path.display()))
    } else {
        DoctorCheck::fail("config", format!("{} does not exist", path.display()))
    }
}

fn check_port(port: u16) -> DoctorCheck {
    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(listener) => {
            drop(listener);
            DoctorCheck::ok(format!("port:{port}"), "available")
        }
        Err(err) => DoctorCheck::fail(format!("port:{port}"), format!("unavailable: {err}")),
    }
}

fn check_tcp(target: &str) -> DoctorCheck {
    let mut addrs = match target.to_socket_addrs() {
        Ok(addrs) => addrs,
        Err(err) => {
            return DoctorCheck::fail(format!("tcp:{target}"), format!("invalid target: {err}"));
        }
    };
    let Some(addr) = addrs.next() else {
        return DoctorCheck::fail(format!("tcp:{target}"), "no socket address resolved");
    };
    match TcpStream::connect_timeout(&addr, Duration::from_secs(2)) {
        Ok(stream) => {
            drop(stream);
            DoctorCheck::ok(format!("tcp:{target}"), "reachable")
        }
        Err(err) => DoctorCheck::fail(format!("tcp:{target}"), format!("unreachable: {err}")),
    }
}

fn run_dev(command: DevCommands) -> anyhow::Result<()> {
    let args = dev_compose_args(&command)?;
    let status = Command::new("docker")
        .args(args)
        .status()
        .map_err(|err| anyhow::anyhow!("failed to run docker compose: {err}"))?;
    if !status.success() {
        anyhow::bail!("docker compose exited with {status}");
    }
    Ok(())
}

fn dev_compose_args(command: &DevCommands) -> anyhow::Result<Vec<OsString>> {
    let (file, profiles, action_args): (&Path, &[String], Vec<OsString>) = match command {
        DevCommands::Up {
            file,
            profile,
            detach,
        } => {
            let mut action_args = vec![OsString::from("up")];
            if *detach {
                action_args.push(OsString::from("-d"));
            }
            (file.as_path(), profile.as_slice(), action_args)
        }
        DevCommands::Down {
            file,
            profile,
            volumes,
        } => {
            let mut action_args = vec![OsString::from("down")];
            if *volumes {
                action_args.push(OsString::from("-v"));
            }
            (file.as_path(), profile.as_slice(), action_args)
        }
        DevCommands::Status { file, profile } => (
            file.as_path(),
            profile.as_slice(),
            vec![OsString::from("ps")],
        ),
    };

    if !file.is_file() {
        anyhow::bail!("compose file {} does not exist", file.display());
    }

    let mut args = vec![
        OsString::from("compose"),
        OsString::from("-f"),
        file.as_os_str().to_os_string(),
    ];
    for profile in profiles {
        args.push(OsString::from("--profile"));
        args.push(OsString::from(profile));
    }
    args.extend(action_args);
    Ok(args)
}

fn run_diff(command: DiffCommands, registry: &generator::GeneratorRegistry) -> anyhow::Result<()> {
    let (out, generator_command) = match command {
        DiffCommands::Api {
            api,
            out,
            roze_source,
        } => {
            let mode = diff_mode(&out);
            let command = GeneratorCommand::ApiGenerate {
                api,
                out: PathBuf::new(),
                options: GenerateOptions::new(mode, roze_source.into()),
            };
            (out, command)
        }
        DiffCommands::Rpc {
            api,
            out,
            roze_source,
        } => {
            let mode = diff_mode(&out);
            let command = GeneratorCommand::RpcGenerate {
                api,
                out: PathBuf::new(),
                options: GenerateOptions::new(mode, roze_source.into()),
            };
            (out, command)
        }
        DiffCommands::Model {
            schema,
            out,
            roze_source,
            format,
            orm,
        } => {
            let mode = diff_mode(&out);
            let command = GeneratorCommand::ModelGenerate {
                schema,
                out: PathBuf::new(),
                options: GenerateOptions::new(mode, roze_source.into()),
                format: format.into(),
                orm: orm.into(),
            };
            (out, command)
        }
    };

    let report = generator::diff_project(&out, generator_command, registry)?;
    if report.is_empty() {
        println!("No changes.");
    } else {
        print!("{report}");
    }
    Ok(())
}

fn diff_mode(out: &Path) -> GenerateMode {
    if out.exists() {
        GenerateMode::Update
    } else {
        GenerateMode::Create
    }
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
    fn doctor_tcp_check_accepts_reachable_endpoint() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("bind test listener");
        let addr = listener.local_addr().expect("listener addr");
        let check = check_tcp(&addr.to_string());
        assert_eq!(check.status, DoctorStatus::Ok);
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

        let diff = Cli::try_parse_from(["rozectl", "diff", "api", "user.api", "--out", "out"])
            .expect("parse diff api");
        assert!(matches!(
            diff.command,
            Commands::Diff {
                command: DiffCommands::Api { .. }
            }
        ));

        let contract = Cli::try_parse_from([
            "rozectl", "contract", "check", "--old", "old.api", "--new", "new.api",
        ])
        .expect("parse contract check");
        assert!(matches!(
            contract.command,
            Commands::Contract {
                command: ContractCommands::Check { .. }
            }
        ));

        let mock = Cli::try_parse_from([
            "rozectl",
            "mock",
            "gen",
            "--api",
            "user.api",
            "--out",
            "mock-server",
            "--force",
        ])
        .expect("parse mock gen");
        assert!(matches!(
            mock.command,
            Commands::Mock {
                command: MockCommands::Gen { force: true, .. }
            }
        ));

        let test_gen = Cli::try_parse_from([
            "rozectl",
            "test",
            "gen",
            "--api",
            "user.api",
            "--out",
            "contract-tests",
            "--base-url",
            "http://127.0.0.1:3000",
            "--force",
        ])
        .expect("parse test gen");
        assert!(matches!(
            test_gen.command,
            Commands::Test {
                command: TestCommands::Gen { force: true, .. }
            }
        ));

        let doctor = Cli::try_parse_from([
            "rozectl",
            "doctor",
            "--config",
            "config.yaml",
            "--port",
            "3000",
            "--tcp",
            "127.0.0.1:6379",
            "--tool",
            "helm",
        ])
        .expect("parse doctor");
        assert!(matches!(doctor.command, Commands::Doctor { .. }));

        let doc_service = Cli::try_parse_from([
            "rozectl",
            "doc",
            "service",
            "--api",
            "user.api",
            "--out",
            "SERVICE.md",
            "--force",
        ])
        .expect("parse doc service");
        assert!(matches!(
            doc_service.command,
            Commands::Doc {
                command: DocCommands::Service { force: true, .. }
            }
        ));

        let doc_ai_context = Cli::try_parse_from([
            "rozectl",
            "doc",
            "ai-context",
            "--api",
            "user.api",
            "--out",
            "AI_CONTEXT.md",
            "--force",
        ])
        .expect("parse doc ai-context");
        assert!(matches!(
            doc_ai_context.command,
            Commands::Doc {
                command: DocCommands::AiContext { force: true, .. }
            }
        ));

        let dev = Cli::try_parse_from([
            "rozectl",
            "dev",
            "up",
            "--file",
            "docker-compose.integration.yml",
            "--profile",
            "mq",
            "--detach",
        ])
        .expect("parse dev up");
        assert!(matches!(
            dev.command,
            Commands::Dev {
                command: DevCommands::Up { detach: true, .. }
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

        let helm = Cli::try_parse_from([
            "rozectl",
            "helm",
            "chart",
            "--name",
            "user",
            "--image",
            "registry.example.com/user:1.2.3",
            "--env",
            "RUST_LOG=info",
            "--config-map",
            "user-config",
            "--out",
            "deploy/user-chart",
        ])
        .expect("parse helm chart");
        assert!(matches!(
            helm.command,
            Commands::Helm {
                command: HelmCommands::Chart { .. }
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
    fn dev_compose_args_include_file_profiles_and_action() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-dev-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create dev test root");
        let file = root.join("compose.yml");
        fs::write(&file, "services: {}\n").expect("write compose file");

        let args = dev_compose_args(&DevCommands::Up {
            file: file.clone(),
            profile: vec!["mq".to_string(), "db".to_string()],
            detach: true,
        })
        .expect("compose args");

        assert_eq!(
            args,
            vec![
                OsString::from("compose"),
                OsString::from("-f"),
                file.as_os_str().to_os_string(),
                OsString::from("--profile"),
                OsString::from("mq"),
                OsString::from("--profile"),
                OsString::from("db"),
                OsString::from("up"),
                OsString::from("-d"),
            ]
        );

        fs::remove_dir_all(root).expect("remove dev test root");
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

    #[test]
    fn contract_check_allows_optional_field_addition() {
        let old_spec = parser::parse_api(
            r#"
            service user {
                get /users/:id (GetUserReq) returns (GetUserResp)
                rpc Ping (PingReq) returns (PingResp)
            }
            type GetUserReq {
                id string `path:"id"`
            }
            type GetUserResp {
                id string `json:"id"`
            }
            type PingReq {
                requestId string `json:"requestId"`
            }
            type PingResp {
                ok bool `json:"ok"`
            }
            "#,
        )
        .expect("parse old spec");
        let new_spec = parser::parse_api(
            r#"
            service user {
                get /users/:id (GetUserReq) returns (GetUserResp)
                rpc Ping (PingReq) returns (PingResp)
            }
            type GetUserReq {
                id string `path:"id"`
                traceId string `json:"traceId,optional" validate:"optional"`
            }
            type GetUserResp {
                id string `json:"id"`
                nickname string `json:"nickname,optional" validate:"omitempty"`
            }
            type PingReq {
                requestId string `json:"requestId"`
            }
            type PingResp {
                ok bool `json:"ok"`
            }
            "#,
        )
        .expect("parse new spec");

        let issues = check_contract_compatibility(&old_spec, &new_spec);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn contract_check_reports_breaking_changes() {
        let old_spec = parser::parse_api(
            r#"
            service user {
                get /users/:id (GetUserReq) returns (GetUserResp)
                delete /users/:id (DeleteUserReq) returns (DeleteUserResp)
                rpc Ping (PingReq) returns (PingResp)
            }
            type GetUserReq {
                id string `path:"id"`
            }
            type GetUserResp {
                id string `json:"id"`
                name string `json:"name"`
            }
            type DeleteUserReq {
                id string `path:"id"`
            }
            type DeleteUserResp {
                ok bool `json:"ok"`
            }
            type PingReq {
                requestId string `json:"requestId"`
            }
            type PingResp {
                ok bool `json:"ok"`
            }
            "#,
        )
        .expect("parse old spec");
        let new_spec = parser::parse_api(
            r#"
            service user {
                get /members/:id (GetUserReq) returns (GetUserResp)
                rpc Ping (PingReqV2) returns (PingResp)
            }
            type GetUserReq {
                id uint64 `path:"id"`
                tenantId string `json:"tenantId"`
            }
            type GetUserResp {
                id string `json:"id"`
            }
            type PingReqV2 {
                requestId string `json:"requestId"`
            }
            type PingResp {
                ok bool `json:"ok"`
            }
            "#,
        )
        .expect("parse new spec");

        let issues = check_contract_compatibility(&old_spec, &new_spec);
        let report = issues
            .iter()
            .map(|issue| issue.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(report.contains("REST route removed or changed: GET /users/:id"));
        assert!(report.contains("REST route removed or changed: DELETE /users/:id"));
        assert!(report.contains("RPC method Ping request type changed"));
        assert!(report.contains("field type changed: GetUserReq.id string -> uint64"));
        assert!(report.contains("field removed: GetUserResp.name"));
        assert!(report.contains("required field added: GetUserReq.tenantId"));
    }
}
