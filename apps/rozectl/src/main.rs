mod generator;
mod parser;

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{OsStr, OsString},
    fs,
    net::{TcpListener, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Duration,
};

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};

use generator::{
    native::{DockerOptions, HelmOptions, KubeDeployOptions},
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
    #[value(name = "mysql")]
    MySql,
    Mongo,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum SearchEngine {
    #[default]
    Elasticsearch,
    Opensearch,
    Meilisearch,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum CompletionShell {
    #[default]
    Bash,
    Zsh,
    Fish,
    Powershell,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum QuickstartKind {
    #[default]
    Api,
    Rpc,
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

impl From<DbKind> for generator::model::InspectDatabaseKind {
    fn from(value: DbKind) -> Self {
        match value {
            DbKind::Sqlite => Self::Sqlite,
            DbKind::Postgres => Self::Postgres,
            DbKind::MySql => Self::MySql,
            DbKind::Mongo => Self::Mongo,
        }
    }
}

impl From<SearchEngine> for generator::search::SearchEngine {
    fn from(value: SearchEngine) -> Self {
        match value {
            SearchEngine::Elasticsearch => Self::Elasticsearch,
            SearchEngine::Opensearch => Self::Opensearch,
            SearchEngine::Meilisearch => Self::Meilisearch,
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
    Search {
        #[command(subcommand)]
        command: SearchCommands,
    },
    Stream {
        #[command(subcommand)]
        command: StreamCommands,
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
        binary: String,
    },
    Kube {
        #[command(subcommand)]
        command: KubeCommands,
    },
    Helm {
        #[command(subcommand)]
        command: HelmCommands,
    },
    Completion {
        #[arg(value_enum, default_value_t)]
        shell: CompletionShell,
    },
    Env,
    Upgrade {
        #[arg(long, default_value = "https://github.com/roze-team/roze.git")]
        repo: String,
        #[arg(long, conflicts_with = "rev")]
        branch: Option<String>,
        #[arg(long, conflicts_with = "branch")]
        rev: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    Quickstart {
        #[arg(default_value = "hello")]
        name: String,
        #[arg(long, value_enum, default_value_t)]
        kind: QuickstartKind,
        #[arg(short = 'o', long, alias = "o")]
        out: Option<PathBuf>,
        #[arg(long)]
        force: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
}

#[derive(Debug, Subcommand)]
enum ApiCommands {
    #[command(alias = "gen")]
    Generate {
        api: Option<PathBuf>,
        #[arg(short = 'a', long = "api")]
        api_file: Option<PathBuf>,
        #[arg(short = 'o', long, alias = "o", default_value = ".")]
        out: PathBuf,
        #[arg(short = 'd', long = "dir")]
        dir: Option<PathBuf>,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
    Go {
        #[arg(short = 'a', long)]
        api: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
        #[arg(long)]
        style: Option<String>,
    },
    Swagger {
        #[arg(short = 'a', long)]
        api: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        yaml: bool,
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
    Client {
        #[command(subcommand)]
        command: ClientCommands,
    },
    Ts {
        #[arg(short = 'a', long)]
        api: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
    Js {
        #[arg(short = 'a', long)]
        api: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
    Dart {
        #[arg(short = 'a', long)]
        api: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
    Java {
        #[arg(short = 'a', long)]
        api: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
    Kotlin {
        #[arg(short = 'a', long)]
        api: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
    Swift {
        #[arg(short = 'a', long)]
        api: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
    Ios {
        #[arg(short = 'a', long)]
        api: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
    Android {
        #[arg(short = 'a', long)]
        api: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'o', long)]
        out: Option<PathBuf>,
    },
    Doc {
        #[arg(long, default_value = ".")]
        dir: PathBuf,
        #[arg(short = 'o', long = "out", alias = "o", default_value = "doc")]
        out: PathBuf,
        #[arg(long)]
        api: Option<PathBuf>,
    },
    Plugin {
        #[arg(long)]
        plugin: String,
        #[arg(long)]
        api: PathBuf,
        #[arg(long, default_value = ".")]
        dir: PathBuf,
    },
    Validate {
        api: Option<PathBuf>,
        #[arg(short = 'a', long = "api")]
        api_file: Option<PathBuf>,
    },
    Format {
        api: Option<PathBuf>,
        #[arg(short = 'a', long = "api")]
        api_file: Option<PathBuf>,
        #[arg(long)]
        write: bool,
        #[arg(long, conflicts_with = "write")]
        check: bool,
    },
}

#[derive(Debug, Subcommand)]
enum ClientCommands {
    Ts {
        api: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = "client.ts")]
        out: PathBuf,
    },
    Js {
        api: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = "client.js")]
        out: PathBuf,
    },
    Dart {
        api: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = "client.dart")]
        out: PathBuf,
    },
    Java {
        api: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = "RozeApiClient.java")]
        out: PathBuf,
    },
    Kotlin {
        api: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = "RozeApiClient.kt")]
        out: PathBuf,
    },
    Swift {
        api: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = "RozeApiClient.swift")]
        out: PathBuf,
    },
    Ios {
        api: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = "RozeApiClient.swift")]
        out: PathBuf,
    },
    Android {
        api: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = "RozeApiClient.kt")]
        out: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum RpcCommands {
    #[command(alias = "gen")]
    Generate {
        api: Option<PathBuf>,
        #[arg(short = 'a', long = "api")]
        api_file: Option<PathBuf>,
        #[arg(short = 'o', long, alias = "o", default_value = ".")]
        out: PathBuf,
        #[arg(short = 'd', long = "dir")]
        dir: Option<PathBuf>,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
        #[arg(short = 'm', long = "multiple")]
        multiple: bool,
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
        #[arg(short = 'o', long, alias = "o", default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
        #[arg(short = 'm', long = "multiple")]
        multiple: bool,
    },
    Template {
        #[arg(short = 'o', long = "out", alias = "o")]
        out: Option<PathBuf>,
        #[arg(long)]
        force: bool,
    },
}

#[allow(clippy::large_enum_variant)]
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
        #[arg(long, default_value = "1")]
        min_available: String,
        #[arg(long, default_value = "deploy/kubernetes.yaml")]
        out: PathBuf,
    },
    Validate {
        #[arg(long, default_value = "deploy/kubernetes.yaml")]
        file: PathBuf,
    },
}

#[allow(clippy::large_enum_variant)]
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
        #[arg(long, default_value = "1")]
        min_available: String,
        #[arg(long, default_value = "0.1.0")]
        chart_version: String,
        #[arg(long, default_value = "0.1.0")]
        app_version: String,
        #[arg(long, default_value = "deploy/chart")]
        out: PathBuf,
    },
    Validate {
        #[arg(long, default_value = "deploy/chart")]
        chart: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum TemplateCommands {
    List {
        #[arg(long, alias = "home")]
        dir: Option<PathBuf>,
    },
    Show {
        name: String,
        #[arg(long, alias = "home")]
        dir: Option<PathBuf>,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long)]
        branch: Option<String>,
    },
    Init {
        #[arg(long, alias = "home", default_value = "templates")]
        out: PathBuf,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long)]
        branch: Option<String>,
    },
    Diff {
        name: String,
        #[arg(long, alias = "home", default_value = "templates")]
        dir: PathBuf,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long)]
        branch: Option<String>,
    },
    Update {
        name: String,
        #[arg(long, alias = "home", default_value = "templates")]
        dir: PathBuf,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        force: bool,
    },
    Revert {
        name: String,
        #[arg(long, alias = "home", default_value = "templates")]
        dir: PathBuf,
        #[arg(long)]
        remote: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        no_backup: bool,
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
        #[arg(short = 'o', long, alias = "o", default_value = "mock-server")]
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
        #[arg(short = 'o', long, alias = "o", default_value = "contract-tests")]
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
        #[arg(short = 'o', long, alias = "o", default_value = "SERVICE.md")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
    },
    AiContext {
        #[arg(short = 'a', long = "api")]
        api: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = "AI_CONTEXT.md")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
    },
}

#[derive(Debug, Subcommand)]
enum OpenApiCommands {
    #[command(alias = "gen")]
    Generate {
        api: Option<PathBuf>,
        #[arg(short = 'a', long = "api")]
        api_file: Option<PathBuf>,
        #[arg(short = 'o', long, alias = "o", default_value = "openapi.json")]
        out: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
enum ModelCommands {
    #[command(alias = "gen")]
    Generate {
        schema: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = ".")]
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
        #[arg(long, default_value_t = 100)]
        sample_size: u64,
        #[arg(short = 'o', long, alias = "o", default_value = ".")]
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
        command: ModelSqlCompatCommands,
    },
    Pg {
        #[command(subcommand)]
        command: ModelSqlCompatCommands,
    },
    Mongo {
        #[arg(long)]
        schema: Option<PathBuf>,
        #[arg(long)]
        collection: Option<String>,
        #[arg(long)]
        db_url: Option<String>,
        #[arg(long, default_value_t = 100)]
        sample_size: u64,
        #[arg(short = 'd', long, default_value = ".")]
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
enum ModelSqlCompatCommands {
    Datasource {
        #[arg(short = 'u', long = "url", alias = "db-url")]
        db_url: String,
        #[arg(short = 't', long = "table")]
        table: String,
        #[arg(long)]
        schema: Option<String>,
        #[arg(short = 'd', long, default_value = ".")]
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
    Ddl {
        #[arg(short = 's', long = "src")]
        src: PathBuf,
        #[arg(short = 'd', long, default_value = ".")]
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
enum SearchCommands {
    #[command(alias = "gen")]
    Generate {
        schema: PathBuf,
        #[arg(long, value_enum, default_value_t)]
        engine: SearchEngine,
        #[arg(short = 'o', long, alias = "o", default_value = ".")]
        out: PathBuf,
        #[arg(long)]
        force: bool,
        #[arg(long, conflicts_with = "force")]
        update: bool,
        #[arg(long, value_enum, default_value_t)]
        roze_source: RozeSource,
    },
    Inspect {
        index: String,
        #[arg(long, value_enum)]
        engine: SearchEngine,
        #[arg(long)]
        url: String,
        #[arg(long)]
        api_key: Option<String>,
        #[arg(long, default_value_t = 100)]
        sample_size: u64,
        #[arg(short = 'o', long, alias = "o", default_value = ".")]
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
enum StreamCommands {
    #[command(alias = "generate")]
    Gen {
        #[arg(long)]
        api: PathBuf,
        #[arg(short = 'o', long, alias = "o", default_value = "stream")]
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
    let cli = parse_cli_from(std::env::args_os());
    let registry = generator::registry();

    match cli.command {
        Commands::Api { command } => match command {
            ApiCommands::Generate {
                api,
                api_file,
                out,
                dir,
                force,
                update,
                roze_source,
            } => {
                let api = resolve_input_path(api, api_file, "api")?;
                let out = dir.unwrap_or(out);
                validate_api_for_generation(&api)?;
                registry.dispatch(GeneratorCommand::ApiGenerate {
                    api,
                    out,
                    options: options(force, update, roze_source),
                })?
            }
            ApiCommands::Go {
                api,
                dir,
                force,
                update,
                roze_source,
                style: _,
            } => {
                validate_api_for_generation(&api)?;
                registry.dispatch(GeneratorCommand::ApiGenerate {
                    api,
                    out: dir,
                    options: options(force, update, roze_source),
                })?
            }
            ApiCommands::Swagger { api, dir, yaml } => {
                validate_api_for_generation(&api)?;
                if yaml {
                    generator::write_openapi_yaml(&api, &dir.join("swagger.yaml"))?;
                } else {
                    generator::write_openapi_json(&api, &dir.join("swagger.json"))?;
                }
            }
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
            ApiCommands::Client { command } => match command {
                ClientCommands::Ts { api, out } => {
                    validate_api_for_generation(&api)?;
                    generator::write_ts_client(&api, &out)?;
                }
                ClientCommands::Js { api, out } => {
                    validate_api_for_generation(&api)?;
                    generator::write_js_client(&api, &out)?;
                }
                ClientCommands::Dart { api, out } => {
                    validate_api_for_generation(&api)?;
                    generator::write_dart_client(&api, &out)?;
                }
                ClientCommands::Java { api, out } => {
                    validate_api_for_generation(&api)?;
                    generator::write_java_client(&api, &out)?;
                }
                ClientCommands::Kotlin { api, out } => {
                    validate_api_for_generation(&api)?;
                    generator::write_kotlin_client(&api, &out)?;
                }
                ClientCommands::Swift { api, out } | ClientCommands::Ios { api, out } => {
                    validate_api_for_generation(&api)?;
                    generator::write_swift_client(&api, &out)?;
                }
                ClientCommands::Android { api, out } => {
                    validate_api_for_generation(&api)?;
                    generator::write_kotlin_client(&api, &out)?;
                }
            },
            ApiCommands::Ts { api, dir, out } => {
                validate_api_for_generation(&api)?;
                generator::write_ts_client(&api, &resolve_client_out(dir, out, "client.ts"))?;
            }
            ApiCommands::Js { api, dir, out } => {
                validate_api_for_generation(&api)?;
                generator::write_js_client(&api, &resolve_client_out(dir, out, "client.js"))?;
            }
            ApiCommands::Dart { api, dir, out } => {
                validate_api_for_generation(&api)?;
                generator::write_dart_client(&api, &resolve_client_out(dir, out, "client.dart"))?;
            }
            ApiCommands::Java { api, dir, out } => {
                validate_api_for_generation(&api)?;
                generator::write_java_client(
                    &api,
                    &resolve_client_out(dir, out, "RozeApiClient.java"),
                )?;
            }
            ApiCommands::Kotlin { api, dir, out } => {
                validate_api_for_generation(&api)?;
                generator::write_kotlin_client(
                    &api,
                    &resolve_client_out(dir, out, "RozeApiClient.kt"),
                )?;
            }
            ApiCommands::Swift { api, dir, out } | ApiCommands::Ios { api, dir, out } => {
                validate_api_for_generation(&api)?;
                generator::write_swift_client(
                    &api,
                    &resolve_client_out(dir, out, "RozeApiClient.swift"),
                )?;
            }
            ApiCommands::Android { api, dir, out } => {
                validate_api_for_generation(&api)?;
                generator::write_kotlin_client(
                    &api,
                    &resolve_client_out(dir, out, "RozeApiClient.kt"),
                )?;
            }
            ApiCommands::Doc { dir, out, api } => {
                validate_optional_api_for_generation(api.as_deref())?;
                let out = if out.is_absolute() {
                    out
                } else {
                    dir.join(out)
                };
                generator::native::write_api_markdown_doc(api.as_deref(), &dir, &out)?;
            }
            ApiCommands::Plugin { plugin, api, dir } => {
                validate_api_for_generation(&api)?;
                generator::native::run_api_plugin(&plugin, &api, &dir)?;
            }
            ApiCommands::Validate { api, api_file } => {
                let api = resolve_input_path(api, api_file, "api")?;
                run_api_validate(&api)?
            }
            ApiCommands::Format {
                api,
                api_file,
                write,
                check,
            } => {
                let api = resolve_input_path(api, api_file, "api")?;
                run_api_format(&api, write, check)?
            }
        },
        Commands::Rpc { command } => match command {
            RpcCommands::Generate {
                api,
                api_file,
                out,
                dir,
                force,
                update,
                roze_source,
                multiple: _,
            } => {
                let api = resolve_input_path(api, api_file, "api")?;
                let out = dir.unwrap_or(out);
                validate_api_for_generation(&api)?;
                registry.dispatch(GeneratorCommand::RpcGenerate {
                    api,
                    out,
                    options: options(force, update, roze_source),
                })?
            }
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
                force,
                update,
                roze_source,
                multiple: _,
            } => {
                generator::native::generate_rpc_from_proto(
                    &proto,
                    &out,
                    options(force, update, roze_source),
                )?;
            }
            RpcCommands::Template { out, force } => {
                let template = generator::template("rpc")?;
                if let Some(out) = out {
                    if out.exists() && !force {
                        anyhow::bail!(
                            "{} already exists; pass --force to overwrite it",
                            out.display()
                        );
                    }
                    if let Some(parent) =
                        out.parent().filter(|parent| !parent.as_os_str().is_empty())
                    {
                        fs::create_dir_all(parent).map_err(|err| {
                            anyhow::anyhow!("failed to create {}: {err}", parent.display())
                        })?;
                    }
                    fs::write(&out, template).map_err(|err| {
                        anyhow::anyhow!("failed to write {}: {err}", out.display())
                    })?;
                } else {
                    println!("{template}");
                }
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
                sample_size,
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
                sample_size,
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
            ModelCommands::Mysql { command } => {
                run_sql_model_compat(command, DbKind::MySql, &registry)?
            }
            ModelCommands::Pg { command } => {
                run_sql_model_compat(command, DbKind::Postgres, &registry)?
            }
            ModelCommands::Mongo {
                schema,
                collection,
                db_url,
                sample_size,
                dir,
                force,
                update,
                roze_source,
            } => match (schema, collection, db_url) {
                (Some(schema), None, None) => registry.dispatch(GeneratorCommand::ModelGenerate {
                    schema,
                    out: dir,
                    options: options(force, update, roze_source),
                    format: generator::model::ModelFormat::Mongo,
                    orm: generator::model::ModelOrm::Toasty,
                })?,
                (None, Some(table), Some(db_url)) => {
                    registry.dispatch(GeneratorCommand::ModelInspect {
                        table,
                        schema: None,
                        db_url,
                        db_kind: generator::model::InspectDatabaseKind::Mongo,
                        sample_size,
                        out: dir,
                        options: options(force, update, roze_source),
                        orm: generator::model::ModelOrm::Toasty,
                    })?
                }
                _ => anyhow::bail!(
                    "model mongo expects either --schema <file> or --collection <name> --db-url <url>"
                ),
            },
        },
        Commands::Search { command } => match command {
            SearchCommands::Generate {
                schema,
                engine,
                out,
                force,
                update,
                roze_source,
            } => generator::search::generate_search_project(
                &schema,
                engine.into(),
                &out,
                options(force, update, roze_source),
            )?,
            SearchCommands::Inspect {
                index,
                engine,
                url,
                api_key,
                sample_size,
                out,
                force,
                update,
                roze_source,
            } => {
                let rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()
                    .context("failed to create async runtime for search inspection")?;
                rt.block_on(generator::search::inspect_search_index(
                    &index,
                    engine.into(),
                    &url,
                    api_key.as_deref(),
                    sample_size,
                    &out,
                    options(force, update, roze_source),
                ))?;
            }
        },
        Commands::Stream { command } => match command {
            StreamCommands::Gen {
                api,
                out,
                force,
                update,
                roze_source,
            } => {
                validate_api_for_generation(&api)?;
                generator::write_stream_worker_project(
                    &api,
                    &out,
                    options(force, update, roze_source),
                )?
            }
        },
        Commands::Template { command } => match command {
            TemplateCommands::List { dir } => {
                run_template_list(dir.as_deref())?;
            }
            TemplateCommands::Show {
                name,
                dir,
                remote,
                branch,
            } => {
                println!(
                    "{}",
                    template_source(&name, dir.as_deref(), remote.as_deref(), branch.as_deref())?
                );
            }
            TemplateCommands::Init {
                out,
                remote,
                branch,
            } => {
                run_template_init(&out, remote.as_deref(), branch.as_deref())?;
            }
            TemplateCommands::Diff {
                name,
                dir,
                remote,
                branch,
            } => {
                run_template_diff(&name, &dir, remote.as_deref(), branch.as_deref())?;
            }
            TemplateCommands::Update {
                name,
                dir,
                remote,
                branch,
                force,
            } => {
                run_template_update(&name, &dir, remote.as_deref(), branch.as_deref(), force)?;
            }
            TemplateCommands::Revert {
                name,
                dir,
                remote,
                branch,
                no_backup,
            } => {
                run_template_revert(&name, &dir, remote.as_deref(), branch.as_deref(), !no_backup)?;
            }
        },
        Commands::Diff { command } => run_diff(command, &registry)?,
        Commands::Contract { command } => run_contract(command)?,
        Commands::Mock { command } => match command {
            MockCommands::Gen { api, out, force } => {
                validate_api_for_generation(&api)?;
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
                validate_api_for_generation(&api)?;
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
                validate_api_for_generation(&api)?;
                generator::write_service_markdown_doc(&api, &out, force)?;
            }
            DocCommands::AiContext { api, out, force } => {
                validate_api_for_generation(&api)?;
                generator::write_ai_context_markdown_doc(&api, &out, force)?;
            }
        },
        Commands::Openapi { command } => match command {
            OpenApiCommands::Generate { api, api_file, out } => {
                let api = resolve_input_path(api, api_file, "api")?;
                validate_api_for_generation(&api)?;
                generator::write_openapi_json(&api, &out)?;
            }
        },
        Commands::Docker {
            out,
            builder_image,
            base_image,
            port,
            timezone,
            binary,
        } => {
            let validate_file = out.clone();
            generator::native::write_dockerfile(DockerOptions {
                out,
                builder_image,
                base_image,
                port,
                timezone,
                binary,
            })?;
            run_dockerfile_validate(&validate_file)?;
        }
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
                min_available,
                out,
            } => {
                let validate_file = out.clone();
                generator::native::write_kube_deploy(KubeDeployOptions {
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
                    min_available,
                    out,
                })?;
                run_kube_validate(&validate_file)?;
            }
            KubeCommands::Validate { file } => run_kube_validate(&file)?,
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
                min_available,
                chart_version,
                app_version,
                out,
            } => {
                let validate_chart = out.clone();
                generator::native::write_helm_chart(HelmOptions {
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
                        min_available,
                        out,
                    },
                    chart_version,
                    app_version,
                })?;
                run_helm_validate(&validate_chart)?;
            }
            HelmCommands::Validate { chart } => run_helm_validate(&chart)?,
        },
        Commands::Completion { shell } => {
            print!("{}", render_completion(shell));
        }
        Commands::Env => run_env()?,
        Commands::Upgrade {
            repo,
            branch,
            rev,
            dry_run,
        } => run_upgrade(&repo, branch.as_deref(), rev.as_deref(), dry_run)?,
        Commands::Quickstart {
            name,
            kind,
            out,
            force,
            roze_source,
        } => {
            let out = resolve_new_out(&name, out);
            let mode = if force {
                GenerateMode::Force
            } else {
                GenerateMode::Create
            };
            let options = GenerateOptions::new(mode, roze_source.into());
            match kind {
                QuickstartKind::Api => registry.dispatch(GeneratorCommand::ApiNew {
                    name,
                    out,
                    options,
                })?,
                QuickstartKind::Rpc => registry.dispatch(GeneratorCommand::RpcNew {
                    name,
                    out,
                    options,
                })?,
            }
        }
    }

    Ok(())
}

fn parse_cli_from<I, T>(args: I) -> Cli
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    Cli::parse_from(normalize_go_style_flags(args))
}

#[cfg(test)]
fn try_parse_cli_from<I, T>(args: I) -> Result<Cli, clap::Error>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    Cli::try_parse_from(normalize_go_style_flags(args))
}

fn normalize_go_style_flags<I, T>(args: I) -> Vec<OsString>
where
    I: IntoIterator<Item = T>,
    T: Into<OsString>,
{
    args.into_iter()
        .map(|arg| normalize_go_style_flag(arg.into()))
        .collect()
}

fn normalize_go_style_flag(arg: OsString) -> OsString {
    let text = arg.to_string_lossy();
    let Some(rest) = text.strip_prefix('-') else {
        return arg;
    };
    if rest.starts_with('-') || rest.len() <= 1 {
        return arg;
    }
    let name = rest.split_once('=').map_or(rest, |(name, _)| name);
    if GO_STYLE_LONG_FLAGS.contains(&name) {
        OsString::from(format!("--{rest}"))
    } else {
        arg
    }
}

const GO_STYLE_LONG_FLAGS: &[&str] = &[
    "api",
    "api-key",
    "branch",
    "chart",
    "check",
    "collection",
    "db-kind",
    "db-url",
    "dir",
    "dry-run",
    "engine",
    "env",
    "file",
    "force",
    "format",
    "home",
    "image",
    "kind",
    "multiple",
    "namespace",
    "no-backup",
    "orm",
    "out",
    "plugin",
    "port",
    "remote",
    "replicas",
    "repo",
    "rev",
    "roze-source",
    "sample-size",
    "schema",
    "src",
    "style",
    "table",
    "update",
    "url",
    "write",
    "yaml",
];

#[derive(Debug, Clone, PartialEq, Eq)]
struct ContractIssue {
    kind: ContractIssueKind,
    detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ApiValidationIssue {
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
            let issues = check_contract_breaking_changes(&old_spec, &new_spec);
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

fn run_api_validate(path: &Path) -> anyhow::Result<()> {
    let spec = read_api_spec(path)?;
    let issues = validate_api_spec(&spec);
    if issues.is_empty() {
        println!("api validate passed: {}", path.display());
        return Ok(());
    }

    eprintln!(
        "api validate failed: {} issue(s) in {}",
        issues.len(),
        path.display()
    );
    for issue in issues {
        eprintln!("- {}", issue.detail);
    }
    anyhow::bail!("api validate failed")
}

fn validate_api_for_generation(path: &Path) -> anyhow::Result<()> {
    let spec = read_api_spec(path)?;
    let issues = validate_api_spec(&spec);
    if issues.is_empty() {
        return Ok(());
    }

    let details = issues
        .iter()
        .map(|issue| format!("- {}", issue.detail))
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::bail!(
        "api validation failed before generation for {}:\n{details}",
        path.display()
    )
}

fn validate_optional_api_for_generation(path: Option<&Path>) -> anyhow::Result<()> {
    if let Some(path) = path {
        validate_api_for_generation(path)?;
    }
    Ok(())
}

fn run_api_format(path: &Path, write: bool, check: bool) -> anyhow::Result<()> {
    let source = fs::read_to_string(path)
        .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", path.display()))?;
    let spec = parser::parse_api(&source)
        .map_err(|err| anyhow::anyhow!("failed to parse {}: {err}", path.display()))?;
    let formatted = format_api_spec(&spec);

    if check {
        if normalize_line_endings(&source) == formatted {
            println!("api format check passed: {}", path.display());
            return Ok(());
        }
        anyhow::bail!("api format check failed: {}", path.display());
    }

    if write {
        fs::write(path, formatted)
            .map_err(|err| anyhow::anyhow!("failed to write {}: {err}", path.display()))?;
        println!("api formatted: {}", path.display());
        return Ok(());
    }

    print!("{formatted}");
    Ok(())
}

fn run_template_list(dir: Option<&Path>) -> anyhow::Result<()> {
    if let Some(dir) = dir {
        let mut names = Vec::new();
        for name in ["api", "rpc", "model"] {
            if dir.join(template_file_name(name)?).exists() {
                names.push(name);
            }
        }
        if names.is_empty() {
            println!("api\nrpc\nmodel");
        } else {
            println!("{}", names.join("\n"));
        }
        return Ok(());
    }

    println!("api\nrpc\nmodel");
    Ok(())
}

fn run_template_init(out: &Path, remote: Option<&str>, branch: Option<&str>) -> anyhow::Result<()> {
    if let Some(remote) = remote {
        let home = RemoteTemplateHome::clone(remote, branch)?;
        copy_template_home(home.path(), out)?;
        return Ok(());
    }
    generator::init_templates(out)
}

fn run_template_diff(
    name: &str,
    dir: &Path,
    remote: Option<&str>,
    branch: Option<&str>,
) -> anyhow::Result<()> {
    let path = dir.join(template_file_name(name)?);
    let expected = template_source(name, None, remote, branch)?;
    if !path.exists() {
        println!("A {}", path.display());
        println!(
            "{}",
            render_unified_diff("", &expected, "/dev/null", &path.display().to_string())
        );
        return Ok(());
    }

    let current = fs::read_to_string(&path)
        .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", path.display()))?;
    if normalize_line_endings(&current) == normalize_line_endings(&expected) {
        println!("template {name} is up to date: {}", path.display());
        return Ok(());
    }

    println!("M {}", path.display());
    println!(
        "{}",
        render_unified_diff(
            &normalize_line_endings(&current),
            &normalize_line_endings(&expected),
            &path.display().to_string(),
            &format!("builtin:{name}")
        )
    );
    Ok(())
}

fn run_template_update(
    name: &str,
    dir: &Path,
    remote: Option<&str>,
    branch: Option<&str>,
    force: bool,
) -> anyhow::Result<()> {
    let path = dir.join(template_file_name(name)?);
    let expected = template_source(name, None, remote, branch)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| anyhow::anyhow!("failed to create {}: {err}", parent.display()))?;
    }

    if path.exists() {
        let current = fs::read_to_string(&path)
            .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", path.display()))?;
        if normalize_line_endings(&current) == normalize_line_endings(&expected) {
            println!("template {name} is already up to date: {}", path.display());
            return Ok(());
        }
        if !force {
            anyhow::bail!(
                "template {} differs from built-in template; run `rozectl template diff {name} --dir {}` first or pass --force",
                path.display(),
                dir.display()
            );
        }
    }

    fs::write(&path, expected)
        .map_err(|err| anyhow::anyhow!("failed to write {}: {err}", path.display()))?;
    println!("template {name} updated: {}", path.display());
    Ok(())
}

fn run_template_revert(
    name: &str,
    dir: &Path,
    remote: Option<&str>,
    branch: Option<&str>,
    backup: bool,
) -> anyhow::Result<()> {
    let path = dir.join(template_file_name(name)?);
    let expected = template_source(name, None, remote, branch)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|err| anyhow::anyhow!("failed to create {}: {err}", parent.display()))?;
    }

    if path.exists() {
        let current = fs::read_to_string(&path)
            .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", path.display()))?;
        if normalize_line_endings(&current) == normalize_line_endings(&expected) {
            println!("template {name} is already built-in: {}", path.display());
            return Ok(());
        }
        if backup {
            let backup_path = template_backup_path(&path);
            fs::write(&backup_path, current).map_err(|err| {
                anyhow::anyhow!("failed to write backup {}: {err}", backup_path.display())
            })?;
            println!("template {name} backup written: {}", backup_path.display());
        }
    }

    fs::write(&path, expected)
        .map_err(|err| anyhow::anyhow!("failed to write {}: {err}", path.display()))?;
    println!("template {name} reverted: {}", path.display());
    Ok(())
}

fn template_source(
    name: &str,
    dir: Option<&Path>,
    remote: Option<&str>,
    branch: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(dir) = dir {
        let path = dir.join(template_file_name(name)?);
        return fs::read_to_string(&path)
            .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", path.display()));
    }

    if let Some(remote) = remote {
        let home = RemoteTemplateHome::clone(remote, branch)?;
        return read_template_from_home(home.path(), name);
    }

    generator::template(name)
}

fn read_template_from_home(home: &Path, name: &str) -> anyhow::Result<String> {
    let path = home.join(template_file_name(name)?);
    fs::read_to_string(&path)
        .map_err(|err| anyhow::anyhow!("failed to read template {}: {err}", path.display()))
}

fn copy_template_home(from: &Path, to: &Path) -> anyhow::Result<()> {
    fs::create_dir_all(to)
        .map_err(|err| anyhow::anyhow!("failed to create {}: {err}", to.display()))?;
    for name in ["api", "rpc", "model"] {
        let file_name = template_file_name(name)?;
        let source = from.join(file_name);
        if source.exists() {
            fs::copy(&source, to.join(file_name)).map_err(|err| {
                anyhow::anyhow!(
                    "failed to copy template {} to {}: {err}",
                    source.display(),
                    to.join(file_name).display()
                )
            })?;
        }
    }
    Ok(())
}

struct RemoteTemplateHome {
    path: PathBuf,
}

impl RemoteTemplateHome {
    fn clone(remote: &str, branch: Option<&str>) -> anyhow::Result<Self> {
        let path = std::env::temp_dir().join(format!(
            "rozectl-template-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|duration| duration.as_nanos())
                .unwrap_or(0)
        ));

        let mut command = Command::new("git");
        command.arg("clone").arg("--depth").arg("1");
        if let Some(branch) = branch {
            command.arg("--branch").arg(branch);
        }
        command.arg(remote).arg(&path);
        let status = command
            .status()
            .with_context(|| format!("failed to clone template remote {remote}"))?;
        if !status.success() {
            anyhow::bail!("template remote clone failed with status {status}");
        }

        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RemoteTemplateHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn template_backup_path(path: &Path) -> PathBuf {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| format!("{extension}.bak"))
        .unwrap_or_else(|| "bak".to_string());
    path.with_extension(extension)
}

fn template_file_name(name: &str) -> anyhow::Result<&'static str> {
    match name {
        "api" => Ok("api.api"),
        "rpc" => Ok("rpc.api"),
        "model" => Ok("model.model"),
        other => anyhow::bail!("unknown template `{other}`; expected api, rpc or model"),
    }
}

fn render_unified_diff(old: &str, new: &str, old_name: &str, new_name: &str) -> String {
    let old_lines = old.lines().collect::<Vec<_>>();
    let new_lines = new.lines().collect::<Vec<_>>();
    let mut out = String::new();
    out.push_str(&format!("--- {old_name}\n"));
    out.push_str(&format!("+++ {new_name}\n"));
    out.push_str("@@\n");

    let max_len = old_lines.len().max(new_lines.len());
    for idx in 0..max_len {
        match (old_lines.get(idx), new_lines.get(idx)) {
            (Some(old_line), Some(new_line)) if old_line == new_line => {
                out.push_str(&format!(" {old_line}\n"));
            }
            (Some(old_line), Some(new_line)) => {
                out.push_str(&format!("-{old_line}\n"));
                out.push_str(&format!("+{new_line}\n"));
            }
            (Some(old_line), None) => out.push_str(&format!("-{old_line}\n")),
            (None, Some(new_line)) => out.push_str(&format!("+{new_line}\n")),
            (None, None) => {}
        }
    }
    out
}

fn format_api_spec(spec: &parser::ApiSpec) -> String {
    let mut out = String::new();
    out.push_str("syntax = \"v1\"\n\n");

    if !spec.info.is_empty() {
        out.push_str("info (\n");
        for pair in &spec.info {
            out.push_str(&format!("    {}: \"{}\"\n", pair.key, pair.value));
        }
        out.push_str(")\n\n");
    }

    if spec.rest_routes.iter().all(|route| route.server.is_none()) {
        if let Some(server) = &spec.server {
            render_api_server_block(&mut out, server, "");
            out.push('\n');
        }
    }

    out.push_str(&format!("service {} {{\n", spec.service));
    for route in &spec.rest_routes {
        if let Some(server) = &route.server {
            render_api_server_block(&mut out, server, "    ");
        }
        if let Some(doc) = &route.doc {
            out.push_str(&format!("    @doc \"{}\"\n", doc));
        }
        for middleware in &route.middlewares {
            out.push_str(&format!("    @middleware {}\n", middleware));
        }
        if let Some(handler) = &route.handler {
            out.push_str(&format!("    @handler {}\n", handler));
        }
        out.push_str(&format!(
            "    {} {} ({}) returns ({})\n",
            format_api_http_method(&route.method),
            route.path,
            route.request,
            route.response
        ));
    }
    for method in &spec.rpc_methods {
        out.push_str(&format!(
            "    rpc {} ({}) returns ({})\n",
            method.name, method.request, method.response
        ));
    }
    out.push_str("}\n");

    if !spec.types.is_empty() {
        out.push('\n');
        out.push_str("type (\n");
        for ty in &spec.types {
            out.push_str(&format!("    {} {{\n", ty.name));
            for field in &ty.fields {
                if field.embedded {
                    out.push_str(&format!(
                        "        {}{}\n",
                        field.ty,
                        format_api_field_tags(field)
                    ));
                } else {
                    out.push_str(&format!(
                        "        {} {}{}\n",
                        field.name,
                        field.ty,
                        format_api_field_tags(field)
                    ));
                }
            }
            out.push_str("    }\n");
        }
        out.push_str(")\n");
    }

    out
}

fn render_api_server_block(out: &mut String, server: &parser::ServerSpec, indent: &str) {
    out.push_str(&format!("{indent}@server (\n"));
    if let Some(prefix) = &server.prefix {
        out.push_str(&format!("{indent}    prefix: {prefix}\n"));
    }
    if let Some(group) = &server.group {
        out.push_str(&format!("{indent}    group: {group}\n"));
    }
    if let Some(jwt) = &server.jwt {
        out.push_str(&format!("{indent}    jwt: {jwt}\n"));
    }
    if !server.middlewares.is_empty() {
        out.push_str(&format!(
            "{indent}    middleware: {}\n",
            server.middlewares.join(", ")
        ));
    }
    out.push_str(&format!("{indent})\n"));
}

fn format_api_http_method(method: &parser::HttpMethod) -> &'static str {
    match method {
        parser::HttpMethod::Get => "get",
        parser::HttpMethod::Head => "head",
        parser::HttpMethod::Post => "post",
        parser::HttpMethod::Put => "put",
        parser::HttpMethod::Patch => "patch",
        parser::HttpMethod::Delete => "delete",
    }
}

fn format_api_field_tags(field: &parser::Field) -> String {
    let mut tags = Vec::new();
    match field.source {
        parser::FieldSource::Auto => {}
        parser::FieldSource::Json => push_api_field_tag(&mut tags, "json", field),
        parser::FieldSource::Query => push_api_field_tag(&mut tags, "query", field),
        parser::FieldSource::Form => push_api_field_tag(&mut tags, "form", field),
        parser::FieldSource::Path => push_api_field_tag(&mut tags, "path", field),
        parser::FieldSource::Header => push_api_field_tag(&mut tags, "header", field),
    }
    if let Some(validate) = &field.validate {
        tags.push(format!("validate:\"{validate}\""));
    }

    if tags.is_empty() {
        String::new()
    } else {
        format!(" `{}`", tags.join(" "))
    }
}

fn push_api_field_tag(tags: &mut Vec<String>, name: &str, field: &parser::Field) {
    let wire_name = field
        .wire_name
        .as_deref()
        .or(field.json_name.as_deref())
        .unwrap_or(field.name.as_str());
    tags.push(format!("{name}:\"{wire_name}\""));
}

fn normalize_line_endings(source: &str) -> String {
    source.replace("\r\n", "\n")
}

fn validate_api_spec(spec: &parser::ApiSpec) -> Vec<ApiValidationIssue> {
    let mut issues = Vec::new();
    validate_unique_types(spec, &mut issues);
    validate_unique_type_fields(spec, &mut issues);
    validate_reserved_empty_types(spec, &mut issues);
    validate_generated_service_identifiers(spec, &mut issues);
    validate_generated_type_names(spec, &mut issues);
    validate_unique_generated_type_fields(spec, &mut issues);
    validate_unique_rest_routes(spec, &mut issues);
    validate_unique_rpc_methods(spec, &mut issues);
    validate_unique_generated_names(spec, &mut issues);
    validate_generated_rest_rpc_identifiers(spec, &mut issues);
    validate_generated_middleware_identifiers(spec, &mut issues);
    validate_referenced_types(spec, &mut issues);
    validate_route_path_params(spec, &mut issues);
    issues
}

fn validate_unique_types(spec: &parser::ApiSpec, issues: &mut Vec<ApiValidationIssue>) {
    let mut seen = BTreeSet::new();
    for ty in &spec.types {
        if !seen.insert(ty.name.as_str()) {
            issues.push(api_validation_issue(format!("duplicate type: {}", ty.name)));
        }
    }
}

fn validate_unique_type_fields(spec: &parser::ApiSpec, issues: &mut Vec<ApiValidationIssue>) {
    for ty in &spec.types {
        let mut seen_names = BTreeSet::new();
        let mut seen_wire_names = BTreeSet::new();
        for field in &ty.fields {
            if !seen_names.insert(field.name.as_str()) {
                issues.push(api_validation_issue(format!(
                    "duplicate field: {}.{}",
                    ty.name, field.name
                )));
            }
            let wire_name = field
                .wire_name
                .as_deref()
                .or(field.json_name.as_deref())
                .unwrap_or(field.name.as_str());
            if !seen_wire_names.insert((field.source, wire_name)) {
                issues.push(api_validation_issue(format!(
                    "duplicate wire field: {} {:?} `{}`",
                    ty.name, field.source, wire_name
                )));
            }
        }
    }
}

fn validate_reserved_empty_types(spec: &parser::ApiSpec, issues: &mut Vec<ApiValidationIssue>) {
    let uses_empty_req = spec
        .rest_routes
        .iter()
        .any(|route| route.request == "EmptyReq")
        || spec
            .rpc_methods
            .iter()
            .any(|method| method.request == "EmptyReq");
    let uses_empty_resp = spec
        .rest_routes
        .iter()
        .any(|route| route.response == "EmptyResp")
        || spec
            .rpc_methods
            .iter()
            .any(|method| method.response == "EmptyResp");

    for ty in &spec.types {
        let is_used_reserved_empty = (ty.name == "EmptyReq" && uses_empty_req)
            || (ty.name == "EmptyResp" && uses_empty_resp);
        if is_used_reserved_empty && !ty.fields.is_empty() {
            issues.push(api_validation_issue(format!(
                "{} is reserved for omitted request/response bodies and must not declare fields",
                ty.name
            )));
        }
    }
}

fn validate_generated_service_identifiers(
    spec: &parser::ApiSpec,
    issues: &mut Vec<ApiValidationIssue>,
) {
    let module = generator::to_snake_case(&spec.service);
    if !is_valid_generated_rust_ident(&module) {
        issues.push(api_validation_issue(format!(
            "service {} generates invalid Rust module `{module}`",
            spec.service
        )));
    }

    let service = generator::to_pascal_case(&spec.service);
    if !is_valid_rust_type_name(&service) {
        issues.push(api_validation_issue(format!(
            "service {} generates invalid Rust service type `{service}`",
            spec.service
        )));
    }
}

fn validate_generated_type_names(spec: &parser::ApiSpec, issues: &mut Vec<ApiValidationIssue>) {
    let mut seen = BTreeMap::<String, String>::new();
    for ty in &spec.types {
        if !is_valid_rust_type_name(&ty.name) {
            issues.push(api_validation_issue(format!(
                "type {} is not a valid Rust type name; use UpperCamelCase such as {}",
                ty.name,
                generator::to_pascal_case(&ty.name)
            )));
        }
        let generated = generator::to_pascal_case(&ty.name);
        if let Some(previous) = seen.insert(generated.clone(), ty.name.clone()) {
            issues.push(api_validation_issue(format!(
                "duplicate generated type name `{generated}`: {previous} and {}",
                ty.name
            )));
        }
    }
}

fn validate_unique_generated_type_fields(
    spec: &parser::ApiSpec,
    issues: &mut Vec<ApiValidationIssue>,
) {
    for ty in &spec.types {
        let mut seen = BTreeMap::<String, String>::new();
        for field in &ty.fields {
            let generated = generator::rust_identifier(&field.name);
            if !is_valid_rust_field_name(&generated) {
                issues.push(api_validation_issue(format!(
                    "field {}.{} generates invalid Rust field name `{generated}`",
                    ty.name, field.name
                )));
            }
            if let Some(previous) = seen.insert(generated.clone(), field.name.clone()) {
                issues.push(api_validation_issue(format!(
                    "duplicate generated field name `{generated}` in type {}: {previous} and {}",
                    ty.name, field.name
                )));
            }
        }
    }
}

fn is_valid_rust_type_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if name == "_" {
        return false;
    }
    (first == '_' || first.is_ascii_uppercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn is_valid_rust_field_name(name: &str) -> bool {
    let name = name.strip_prefix("r#").unwrap_or(name);
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if name == "_" {
        return false;
    }
    (first == '_' || first.is_ascii_lowercase())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

fn validate_unique_rest_routes(spec: &parser::ApiSpec, issues: &mut Vec<ApiValidationIssue>) {
    let mut seen = BTreeSet::new();
    for route in &spec.rest_routes {
        let key = rest_route_key(spec, route);
        if !seen.insert(key.clone()) {
            issues.push(api_validation_issue(format!("duplicate REST route: {key}")));
        }
    }
}

fn validate_unique_rpc_methods(spec: &parser::ApiSpec, issues: &mut Vec<ApiValidationIssue>) {
    let mut seen = BTreeSet::new();
    for method in &spec.rpc_methods {
        if !seen.insert(method.name.as_str()) {
            issues.push(api_validation_issue(format!(
                "duplicate RPC method: {}",
                method.name
            )));
        }
    }
}

fn validate_unique_generated_names(spec: &parser::ApiSpec, issues: &mut Vec<ApiValidationIssue>) {
    let mut route_handlers = BTreeMap::<(String, String), String>::new();
    for route in &spec.rest_routes {
        let group = validation_route_group_name(route);
        let handler = validation_resolved_handler_name(route);
        let key = (group.clone(), handler.clone());
        let route_key = rest_route_key(spec, route);
        if let Some(previous) = route_handlers.insert(key, route_key.clone()) {
            issues.push(api_validation_issue(format!(
                "duplicate generated REST handler `{handler}` in group `{group}`: {previous} and {route_key}"
            )));
        }
    }

    let mut rpc_methods = BTreeMap::<String, String>::new();
    for method in &spec.rpc_methods {
        let generated = generator::to_snake_case(&method.name);
        if let Some(previous) = rpc_methods.insert(generated.clone(), method.name.clone()) {
            issues.push(api_validation_issue(format!(
                "duplicate generated RPC method `{generated}`: {previous} and {}",
                method.name
            )));
        }
    }
}

fn validate_generated_rest_rpc_identifiers(
    spec: &parser::ApiSpec,
    issues: &mut Vec<ApiValidationIssue>,
) {
    for route in &spec.rest_routes {
        let group = validation_route_group_name(route);
        let handler = validation_resolved_handler_name(route);
        if !is_valid_generated_rust_ident(&group) {
            issues.push(api_validation_issue(format!(
                "REST route {} generates invalid Rust route group `{group}`",
                rest_route_key(spec, route)
            )));
        }
        if !is_valid_generated_rust_ident(&handler) {
            issues.push(api_validation_issue(format!(
                "REST route {} generates invalid Rust handler `{handler}`",
                rest_route_key(spec, route)
            )));
        }
    }

    for method in &spec.rpc_methods {
        let generated = generator::to_snake_case(&method.name);
        if !is_valid_generated_rust_ident(&generated) {
            issues.push(api_validation_issue(format!(
                "RPC method {} generates invalid Rust method `{generated}`",
                method.name
            )));
        }
    }
}

fn is_valid_generated_rust_ident(name: &str) -> bool {
    is_valid_rust_field_name(name) && generator::rust_identifier(name) == name
}

fn validate_generated_middleware_identifiers(
    spec: &parser::ApiSpec,
    issues: &mut Vec<ApiValidationIssue>,
) {
    let mut generated_names = BTreeMap::<String, String>::new();
    for route in &spec.rest_routes {
        for middleware in validation_route_middlewares(spec, route) {
            if roze_middleware::BuiltInMiddleware::parse(&middleware).is_some() {
                continue;
            }
            let generated = generator::to_snake_case(&middleware);
            if !is_valid_generated_rust_ident(&generated) {
                issues.push(api_validation_issue(format!(
                    "REST route {} custom middleware {} generates invalid Rust identifier `{generated}`",
                    rest_route_key(spec, route),
                    middleware
                )));
            }
            if let Some(previous) = generated_names.insert(generated.clone(), middleware.clone()) {
                if previous != middleware {
                    issues.push(api_validation_issue(format!(
                        "duplicate generated custom middleware `{generated}`: {previous} and {middleware}"
                    )));
                }
            }
        }
    }
}

fn validation_route_middlewares(spec: &parser::ApiSpec, route: &parser::RestRoute) -> Vec<String> {
    let mut names = spec
        .server
        .as_ref()
        .map(|server| server.middlewares.clone())
        .unwrap_or_default();
    if let Some(server) = &route.server {
        names.extend(server.middlewares.clone());
    }
    names.extend(route.middlewares.clone());
    names
}

fn validation_resolved_handler_name(route: &parser::RestRoute) -> String {
    route
        .handler
        .as_ref()
        .map(|handler| generator::to_snake_case(handler))
        .unwrap_or_else(|| validation_handler_name(&route.method, &route.path))
}

fn validation_handler_name(method: &parser::HttpMethod, path: &str) -> String {
    let method = match method {
        parser::HttpMethod::Get => "get",
        parser::HttpMethod::Head => "head",
        parser::HttpMethod::Post => "post",
        parser::HttpMethod::Put => "put",
        parser::HttpMethod::Patch => "patch",
        parser::HttpMethod::Delete => "delete",
    };
    let path_name = path
        .trim_matches('/')
        .replace(':', "")
        .replace(['{', '}'], "")
        .replace(['/', '-'], "_");

    format!("{method}_{path_name}")
}

fn validation_route_group_name(route: &parser::RestRoute) -> String {
    route
        .path
        .split('/')
        .find(|segment| !segment.is_empty() && !segment.starts_with(':'))
        .map(generator::to_snake_case)
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "base".to_string())
}

fn validate_referenced_types(spec: &parser::ApiSpec, issues: &mut Vec<ApiValidationIssue>) {
    let type_names = spec
        .types
        .iter()
        .map(|ty| ty.name.as_str())
        .collect::<BTreeSet<_>>();

    for route in &spec.rest_routes {
        validate_named_type(
            &route.request,
            &type_names,
            format_args!("REST route {} request type", rest_route_key(spec, route)),
            issues,
        );
        validate_named_type(
            &route.response,
            &type_names,
            format_args!("REST route {} response type", rest_route_key(spec, route)),
            issues,
        );
    }

    for method in &spec.rpc_methods {
        validate_named_type(
            &method.request,
            &type_names,
            format_args!("RPC method {} request type", method.name),
            issues,
        );
        validate_named_type(
            &method.response,
            &type_names,
            format_args!("RPC method {} response type", method.name),
            issues,
        );
    }

    for ty in &spec.types {
        for field in &ty.fields {
            for referenced in referenced_custom_type_names(&field.ty) {
                if !type_names.contains(referenced.as_str()) {
                    issues.push(api_validation_issue(format!(
                        "field {}.{} references unknown type: {}",
                        ty.name, field.name, referenced
                    )));
                }
            }
        }
    }
}

fn validate_named_type(
    ty: &str,
    type_names: &BTreeSet<&str>,
    context: std::fmt::Arguments<'_>,
    issues: &mut Vec<ApiValidationIssue>,
) {
    if !type_names.contains(ty) {
        issues.push(api_validation_issue(format!(
            "{context} references unknown type: {ty}"
        )));
    }
}

fn validate_route_path_params(spec: &parser::ApiSpec, issues: &mut Vec<ApiValidationIssue>) {
    let types = spec
        .types
        .iter()
        .map(|ty| (ty.name.as_str(), ty))
        .collect::<BTreeMap<_, _>>();

    for route in &spec.rest_routes {
        let required_params = route_path_params(&route.path);
        let mut seen_required = BTreeMap::<&str, &str>::new();
        for param in &required_params {
            if let Some(previous) =
                seen_required.insert(param.normalized.as_str(), param.raw.as_str())
            {
                issues.push(api_validation_issue(format!(
                    "REST route {} has duplicate path parameter `{}` and `{}`",
                    rest_route_key(spec, route),
                    previous,
                    param.raw
                )));
            }
        }
        let required_normalized = required_params
            .iter()
            .map(|param| param.normalized.as_str())
            .collect::<BTreeSet<_>>();
        let Some(request_ty) = types.get(route.request.as_str()) else {
            continue;
        };
        let declared_params = request_ty
            .fields
            .iter()
            .filter(|field| field.source == parser::FieldSource::Path)
            .map(|field| {
                field
                    .wire_name
                    .as_deref()
                    .or(field.json_name.as_deref())
                    .unwrap_or(field.name.as_str())
            })
            .map(|name| (name, normalize_path_ident(name)))
            .collect::<Vec<_>>();
        let declared_normalized = declared_params
            .iter()
            .map(|(_, normalized)| normalized.as_str())
            .collect::<BTreeSet<_>>();

        for param in &required_params {
            if !declared_normalized.contains(param.normalized.as_str()) {
                issues.push(api_validation_issue(format!(
                    "REST route {} path parameter `:{}` is missing from {} as a path field",
                    rest_route_key(spec, route),
                    param.raw,
                    route.request
                )));
            }
        }
        for (param, normalized) in declared_params {
            if !required_normalized.contains(normalized.as_str()) {
                issues.push(api_validation_issue(format!(
                    "REST route {} request type {} declares path field `{}` that is not present in the route path",
                    rest_route_key(spec, route),
                    route.request,
                    param
                )));
            }
        }
    }
}

#[derive(Debug, Clone)]
struct RoutePathParam {
    raw: String,
    normalized: String,
}

fn route_path_params(path: &str) -> Vec<RoutePathParam> {
    let mut params = Vec::new();
    let mut chars = path.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            ':' => {
                let mut raw = String::new();
                while let Some(&next) = chars.peek() {
                    if next == '/' {
                        break;
                    }
                    raw.push(next);
                    chars.next();
                }
                push_route_path_param(&mut params, raw);
            }
            '{' => {
                let mut raw = String::new();
                for next in chars.by_ref() {
                    if next == '}' {
                        break;
                    }
                    raw.push(next);
                }
                push_route_path_param(&mut params, raw);
            }
            _ => {}
        }
    }

    params
}

fn push_route_path_param(params: &mut Vec<RoutePathParam>, raw: String) {
    let raw = raw.split(['?', '#']).next().unwrap_or_default();
    if raw.is_empty() {
        return;
    }
    params.push(RoutePathParam {
        raw: raw.to_string(),
        normalized: normalize_path_ident(raw),
    });
}

fn normalize_path_ident(input: &str) -> String {
    input.replace('-', "_")
}

fn referenced_custom_type_names(ty: &str) -> Vec<String> {
    type_tokens(ty)
        .into_iter()
        .filter(|token| !is_builtin_api_type(token))
        .map(ToString::to_string)
        .collect()
}

fn type_tokens(ty: &str) -> Vec<&str> {
    ty.split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_'))
        .filter(|token| !token.is_empty())
        .filter(|token| {
            !matches!(
                *token,
                "Vec"
                    | "vec"
                    | "Option"
                    | "option"
                    | "HashMap"
                    | "hashmap"
                    | "BTreeMap"
                    | "btreemap"
                    | "Map"
                    | "map"
                    | "List"
                    | "list"
            )
        })
        .collect()
}

fn is_builtin_api_type(ty: &str) -> bool {
    matches!(
        ty,
        "bool"
            | "Boolean"
            | "string"
            | "String"
            | "str"
            | "bytes"
            | "Bytes"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "isize"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "usize"
            | "f32"
            | "f64"
            | "float"
            | "double"
            | "int"
            | "int8"
            | "int16"
            | "int32"
            | "int64"
            | "integer"
            | "uint"
            | "uint8"
            | "uint16"
            | "uint32"
            | "uint64"
            | "long"
            | "datetime"
            | "DateTime"
            | "time"
            | "Time"
            | "date"
            | "Date"
    )
}

fn api_validation_issue(detail: impl Into<String>) -> ApiValidationIssue {
    ApiValidationIssue {
        detail: detail.into(),
    }
}

fn check_contract_breaking_changes(
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
        parser::HttpMethod::Head => "HEAD",
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

fn run_dockerfile_validate(file: &Path) -> anyhow::Result<()> {
    let content = fs::read_to_string(file)
        .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", file.display()))?;
    let issues = validate_dockerfile_content(&content);
    if issues.is_empty() {
        println!("Dockerfile validation passed: {}", file.display());
        return Ok(());
    }

    eprintln!("Dockerfile validation failed: {} issue(s)", issues.len());
    for issue in issues {
        eprintln!("- {issue}");
    }
    anyhow::bail!("Dockerfile validation failed")
}

fn validate_dockerfile_content(content: &str) -> Vec<String> {
    let mut issues = Vec::new();
    let required = [
        "FROM ",
        " AS builder",
        "cargo build --release --bin",
        "LABEL org.opencontainers.image.title=",
        "ENV TZ=",
        "WORKDIR /app",
        "groupadd --system roze",
        "useradd --system --gid roze",
        "COPY --from=builder --chown=roze:roze",
        "COPY --chown=roze:roze config.yaml",
        "EXPOSE ",
        "USER roze:roze",
        "CMD [\"/usr/local/bin/",
    ];
    for fragment in required {
        if !content.contains(fragment) {
            issues.push(format!("Dockerfile is missing `{fragment}`"));
        }
    }

    let from_count = content
        .lines()
        .filter(|line| line.trim_start().starts_with("FROM "))
        .count();
    if from_count < 2 {
        issues.push("Dockerfile must use a multi-stage build".to_string());
    }

    issues
}

fn run_kube_validate(file: &Path) -> anyhow::Result<()> {
    let content = fs::read_to_string(file)
        .map_err(|err| anyhow::anyhow!("failed to read {}: {err}", file.display()))?;
    let issues = validate_kube_manifest_content(&content);
    if issues.is_empty() {
        println!("kube manifest validation passed: {}", file.display());
        return Ok(());
    }

    eprintln!("kube manifest validation failed: {} issue(s)", issues.len());
    for issue in issues {
        eprintln!("- {issue}");
    }
    anyhow::bail!("kube manifest validation failed")
}

fn validate_kube_manifest_content(content: &str) -> Vec<String> {
    let documents = kube_documents_by_kind(content);
    let mut issues = Vec::new();
    let required_kinds = [
        "ServiceAccount",
        "Deployment",
        "Service",
        "HorizontalPodAutoscaler",
        "PodDisruptionBudget",
        "NetworkPolicy",
    ];
    for kind in required_kinds {
        if !documents.contains_key(kind) {
            issues.push(format!(
                "missing required Kubernetes resource kind `{kind}`"
            ));
        }
    }

    if let Some(deployment) = documents.get("Deployment") {
        require_manifest_fragment(&mut issues, "Deployment", deployment, "serviceAccountName:");
        require_manifest_fragment(
            &mut issues,
            "Deployment",
            deployment,
            "terminationGracePeriodSeconds:",
        );
        require_manifest_fragment(&mut issues, "Deployment", deployment, "resources:");
        require_manifest_fragment(&mut issues, "Deployment", deployment, "requests:");
        require_manifest_fragment(&mut issues, "Deployment", deployment, "limits:");
        require_manifest_fragment(&mut issues, "Deployment", deployment, "livenessProbe:");
        require_manifest_fragment(&mut issues, "Deployment", deployment, "path: /healthz");
        require_manifest_fragment(&mut issues, "Deployment", deployment, "readinessProbe:");
        require_manifest_fragment(&mut issues, "Deployment", deployment, "path: /readyz");
        require_manifest_fragment(&mut issues, "Deployment", deployment, "startupProbe:");
        require_manifest_fragment(&mut issues, "Deployment", deployment, "path: /startupz");
    }
    if let Some(service) = documents.get("Service") {
        require_manifest_fragment(&mut issues, "Service", service, "targetPort:");
    }
    if let Some(hpa) = documents.get("HorizontalPodAutoscaler") {
        require_manifest_fragment(&mut issues, "HorizontalPodAutoscaler", hpa, "minReplicas:");
        require_manifest_fragment(&mut issues, "HorizontalPodAutoscaler", hpa, "maxReplicas:");
        require_manifest_fragment(
            &mut issues,
            "HorizontalPodAutoscaler",
            hpa,
            "averageUtilization:",
        );
    }
    if let Some(pdb) = documents.get("PodDisruptionBudget") {
        require_manifest_fragment(&mut issues, "PodDisruptionBudget", pdb, "minAvailable:");
        require_manifest_fragment(&mut issues, "PodDisruptionBudget", pdb, "selector:");
    }
    if let Some(policy) = documents.get("NetworkPolicy") {
        require_manifest_fragment(&mut issues, "NetworkPolicy", policy, "policyTypes:");
        require_manifest_fragment(&mut issues, "NetworkPolicy", policy, "- Ingress");
        require_manifest_fragment(&mut issues, "NetworkPolicy", policy, "podSelector:");
        require_manifest_fragment(&mut issues, "NetworkPolicy", policy, "port:");
    }

    issues
}

fn kube_documents_by_kind(content: &str) -> BTreeMap<String, String> {
    let mut documents = BTreeMap::new();
    for document in content.split("\n---") {
        let document = document.trim();
        if document.is_empty() {
            continue;
        }
        if let Some(kind) = kube_document_kind(document) {
            documents.insert(kind.to_string(), document.to_string());
        }
    }
    documents
}

fn kube_document_kind(document: &str) -> Option<&str> {
    document.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("kind:").map(str::trim)
    })
}

fn require_manifest_fragment(issues: &mut Vec<String>, kind: &str, document: &str, fragment: &str) {
    if !document.contains(fragment) {
        issues.push(format!("`{kind}` is missing `{fragment}`"));
    }
}

fn run_helm_validate(chart: &Path) -> anyhow::Result<()> {
    let issues = validate_helm_chart_dir(chart);
    if issues.is_empty() {
        println!("helm chart validation passed: {}", chart.display());
        return Ok(());
    }

    eprintln!("helm chart validation failed: {} issue(s)", issues.len());
    for issue in issues {
        eprintln!("- {issue}");
    }
    anyhow::bail!("helm chart validation failed")
}

fn validate_helm_chart_dir(chart: &Path) -> Vec<String> {
    let mut issues = Vec::new();
    if !chart.is_dir() {
        issues.push(format!("{} is not a chart directory", chart.display()));
        return issues;
    }

    let required_files = [
        "Chart.yaml",
        "values.yaml",
        "templates/deployment.yaml",
        "templates/service.yaml",
        "templates/hpa.yaml",
        "templates/serviceaccount.yaml",
        "templates/pdb.yaml",
        "templates/networkpolicy.yaml",
        "templates/_helpers.tpl",
    ];
    for file in required_files {
        if !chart.join(file).is_file() {
            issues.push(format!("missing chart file `{file}`"));
        }
    }

    check_helm_file(
        &mut issues,
        chart,
        "Chart.yaml",
        &["apiVersion: v2", "type: application", "appVersion:"],
    );
    check_helm_file(
        &mut issues,
        chart,
        "values.yaml",
        &[
            "image:",
            "service:",
            "resources:",
            "autoscaling:",
            "serviceAccount:",
            "podDisruptionBudget:",
            "envFrom:",
        ],
    );
    check_helm_file(
        &mut issues,
        chart,
        "templates/deployment.yaml",
        &[
            "kind: Deployment",
            "serviceAccountName:",
            "terminationGracePeriodSeconds:",
            "livenessProbe:",
            "readinessProbe:",
            "startupProbe:",
        ],
    );
    check_helm_file(
        &mut issues,
        chart,
        "templates/service.yaml",
        &["kind: Service", "targetPort:"],
    );
    check_helm_file(
        &mut issues,
        chart,
        "templates/hpa.yaml",
        &["kind: HorizontalPodAutoscaler", "averageUtilization:"],
    );
    check_helm_file(
        &mut issues,
        chart,
        "templates/serviceaccount.yaml",
        &["kind: ServiceAccount"],
    );
    check_helm_file(
        &mut issues,
        chart,
        "templates/pdb.yaml",
        &["kind: PodDisruptionBudget", "minAvailable:"],
    );
    check_helm_file(
        &mut issues,
        chart,
        "templates/networkpolicy.yaml",
        &["kind: NetworkPolicy", "policyTypes:", "Ingress"],
    );
    check_helm_file(
        &mut issues,
        chart,
        "templates/_helpers.tpl",
        &["roze.fullname", "roze.labels", "roze.selectorLabels"],
    );

    issues
}

fn check_helm_file(issues: &mut Vec<String>, chart: &Path, file: &str, fragments: &[&str]) {
    let path = chart.join(file);
    let Ok(content) = fs::read_to_string(&path) else {
        return;
    };
    for fragment in fragments {
        if !content.contains(fragment) {
            issues.push(format!("`{file}` is missing `{fragment}`"));
        }
    }
}

fn run_dev(command: DevCommands) -> anyhow::Result<()> {
    let args = dev_compose_args(&command)?;
    let docker = std::env::var_os("ROZECTL_DOCKER_BIN").unwrap_or_else(|| OsString::from("docker"));
    let mut process = docker_command(docker);
    let status = process
        .args(args)
        .status()
        .map_err(|err| anyhow::anyhow!("failed to run docker compose: {err}"))?;
    if !status.success() {
        anyhow::bail!("docker compose exited with {status}");
    }
    Ok(())
}

fn docker_command(docker: OsString) -> Command {
    if cfg!(target_os = "windows") && is_windows_command_script(&docker) {
        let mut command = Command::new("cmd");
        command.arg("/C").arg(docker);
        command
    } else {
        Command::new(docker)
    }
}

fn is_windows_command_script(path: &OsStr) -> bool {
    Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("cmd") || extension.eq_ignore_ascii_case("bat")
        })
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
            validate_api_for_generation(&api)?;
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
            validate_api_for_generation(&api)?;
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

fn render_completion(shell: CompletionShell) -> &'static str {
    match shell {
        CompletionShell::Bash => {
            r#"_rozectl()
{
    local cur prev commands
    COMPREPLY=()
    cur="${COMP_WORDS[COMP_CWORD]}"
    prev="${COMP_WORDS[COMP_CWORD-1]}"
    commands="api rpc model search stream template diff contract mock test dev doctor doc openapi docker kube helm completion env upgrade quickstart help"
    case "$prev" in
        rozectl)
            COMPREPLY=( $(compgen -W "$commands" -- "$cur") )
            return 0
            ;;
        api)
            COMPREPLY=( $(compgen -W "generate go swagger new client ts js dart java kotlin swift ios android doc plugin validate format" -- "$cur") )
            return 0
            ;;
        rpc)
            COMPREPLY=( $(compgen -W "generate new protoc template" -- "$cur") )
            return 0
            ;;
        model)
            COMPREPLY=( $(compgen -W "generate inspect mysql pg mongo" -- "$cur") )
            return 0
            ;;
        completion)
            COMPREPLY=( $(compgen -W "bash zsh fish powershell" -- "$cur") )
            return 0
            ;;
    esac
}
complete -F _rozectl rozectl
"#
        }
        CompletionShell::Zsh => {
            r#"#compdef rozectl
_rozectl() {
  local -a commands
  commands=(
    'api:generate REST services, clients, docs, and plugins'
    'rpc:generate RPC services and templates'
    'model:generate or inspect database models'
    'search:generate or inspect search repositories'
    'stream:generate stream workers'
    'template:manage starter templates'
    'diff:preview generated changes'
    'contract:check contract compatibility'
    'mock:generate mock servers'
    'test:generate contract tests'
    'dev:manage local dependency stack'
    'doctor:check local environment'
    'doc:generate service documentation'
    'openapi:write OpenAPI documents'
    'docker:write Dockerfile'
    'kube:write or validate Kubernetes manifests'
    'helm:write or validate Helm charts'
    'completion:print shell completion'
    'env:print rozectl environment'
    'upgrade:upgrade rozectl'
    'quickstart:create a starter Roze project'
  )
  _describe 'command' commands
}
compdef _rozectl rozectl
"#
        }
        CompletionShell::Fish => {
            r#"complete -c rozectl -f
complete -c rozectl -n "__fish_use_subcommand" -a "api rpc model search stream template diff contract mock test dev doctor doc openapi docker kube helm completion env upgrade quickstart help"
complete -c rozectl -n "__fish_seen_subcommand_from api" -a "generate go swagger new client ts js dart java kotlin swift ios android doc plugin validate format"
complete -c rozectl -n "__fish_seen_subcommand_from rpc" -a "generate new protoc template"
complete -c rozectl -n "__fish_seen_subcommand_from model" -a "generate inspect mysql pg mongo"
complete -c rozectl -n "__fish_seen_subcommand_from completion" -a "bash zsh fish powershell"
"#
        }
        CompletionShell::Powershell => {
            r#"Register-ArgumentCompleter -Native -CommandName rozectl -ScriptBlock {
    param($wordToComplete, $commandAst, $cursorPosition)
    $commands = 'api','rpc','model','search','stream','template','diff','contract','mock','test','dev','doctor','doc','openapi','docker','kube','helm','completion','env','upgrade','quickstart','help'
    $commands |
        Where-Object { $_ -like "$wordToComplete*" } |
        ForEach-Object { [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_) }
}
"#
        }
    }
}

fn run_env() -> anyhow::Result<()> {
    println!("ROZECTL_BIN={}", std::env::current_exe()?.display());
    println!(
        "ROZECTL_VERSION={}",
        option_env!("CARGO_PKG_VERSION").unwrap_or("unknown")
    );
    println!("CARGO_HOME={}", env_or_empty("CARGO_HOME"));
    println!("RUSTUP_HOME={}", env_or_empty("RUSTUP_HOME"));
    println!("RUST_LOG={}", env_or_empty("RUST_LOG"));
    println!("PATH={}", env_or_empty("PATH"));
    Ok(())
}

fn env_or_empty(name: &str) -> String {
    std::env::var(name).unwrap_or_default()
}

fn run_upgrade(
    repo: &str,
    branch: Option<&str>,
    rev: Option<&str>,
    dry_run: bool,
) -> anyhow::Result<()> {
    let mut args = vec![
        "install".to_string(),
        "--git".to_string(),
        repo.to_string(),
        "rozectl".to_string(),
        "--force".to_string(),
    ];
    if let Some(branch) = branch {
        args.push("--branch".to_string());
        args.push(branch.to_string());
    }
    if let Some(rev) = rev {
        args.push("--rev".to_string());
        args.push(rev.to_string());
    }

    if dry_run {
        println!("cargo {}", args.join(" "));
        return Ok(());
    }

    let status = Command::new("cargo")
        .args(&args)
        .status()
        .context("failed to run cargo install for rozectl upgrade")?;
    if !status.success() {
        anyhow::bail!("rozectl upgrade failed with status {status}");
    }
    Ok(())
}

fn resolve_client_out(dir: PathBuf, out: Option<PathBuf>, default_name: &str) -> PathBuf {
    out.unwrap_or_else(|| dir.join(default_name))
}

fn resolve_input_path(
    positional: Option<PathBuf>,
    flagged: Option<PathBuf>,
    name: &str,
) -> anyhow::Result<PathBuf> {
    match (positional, flagged) {
        (Some(positional), None) => Ok(positional),
        (None, Some(flagged)) => Ok(flagged),
        (Some(_), Some(_)) => {
            anyhow::bail!("pass `{name}` either positionally or with --{name}, not both")
        }
        (None, None) => anyhow::bail!("missing required `{name}` input"),
    }
}

fn resolve_new_out(name: &str, out: Option<PathBuf>) -> PathBuf {
    out.unwrap_or_else(|| PathBuf::from(name))
}

fn run_sql_model_compat(
    command: ModelSqlCompatCommands,
    db_kind: DbKind,
    registry: &generator::GeneratorRegistry,
) -> anyhow::Result<()> {
    match command {
        ModelSqlCompatCommands::Datasource {
            db_url,
            table,
            schema,
            dir,
            force,
            update,
            roze_source,
            orm,
        } => registry.dispatch(GeneratorCommand::ModelInspect {
            table,
            schema,
            db_url,
            db_kind: db_kind.into(),
            sample_size: 100,
            out: dir,
            options: options(force, update, roze_source),
            orm: orm.into(),
        }),
        ModelSqlCompatCommands::Ddl {
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
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn parse(args: impl IntoIterator<Item = &'static str>) -> Cli {
        try_parse_cli_from(args).expect("parse cli")
    }

    fn temp_test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
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
    fn dockerfile_validator_accepts_production_dockerfile() {
        let dockerfile = r#"
FROM rust:1-bookworm AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin user-api

FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="user-api"
ENV TZ=UTC
WORKDIR /app
RUN groupadd --system roze \
    && useradd --system --gid roze --home-dir /app --shell /usr/sbin/nologin roze
COPY --from=builder --chown=roze:roze /app/target/release/user-api /usr/local/bin/user-api
COPY --chown=roze:roze config.yaml ./config.yaml
EXPOSE 8080
USER roze:roze
CMD ["/usr/local/bin/user-api"]
"#;
        assert!(validate_dockerfile_content(dockerfile).is_empty());
    }

    #[test]
    fn dockerfile_validator_reports_missing_non_root_user() {
        let dockerfile = r#"
FROM rust:1-bookworm AS builder
RUN cargo build --release --bin user-api
FROM debian:bookworm-slim
LABEL org.opencontainers.image.title="user-api"
ENV TZ=UTC
WORKDIR /app
COPY --from=builder --chown=roze:roze /app/target/release/user-api /usr/local/bin/user-api
COPY --chown=roze:roze config.yaml ./config.yaml
EXPOSE 8080
CMD ["/usr/local/bin/user-api"]
"#;
        let issues = validate_dockerfile_content(dockerfile);
        assert!(issues.iter().any(|issue| issue.contains("USER roze:roze")));
        assert!(issues
            .iter()
            .any(|issue| issue.contains("groupadd --system roze")));
    }

    #[test]
    fn kube_manifest_validator_accepts_complete_manifest() {
        let manifest = r#"
apiVersion: v1
kind: ServiceAccount
metadata:
  name: user
---
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      serviceAccountName: user
      terminationGracePeriodSeconds: 30
      containers:
      - name: user
        resources:
          requests:
            cpu: 100m
          limits:
            cpu: 500m
        livenessProbe:
          httpGet:
            path: /healthz
        readinessProbe:
          httpGet:
            path: /readyz
        startupProbe:
          httpGet:
            path: /startupz
---
apiVersion: v1
kind: Service
spec:
  ports:
  - targetPort: 3000
---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
spec:
  minReplicas: 1
  maxReplicas: 5
  metrics:
  - resource:
      target:
        averageUtilization: 70
---
apiVersion: policy/v1
kind: PodDisruptionBudget
spec:
  minAvailable: 1
  selector: {}
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
spec:
  podSelector: {}
  policyTypes:
  - Ingress
  ingress:
  - ports:
    - port: 3000
"#;
        assert!(validate_kube_manifest_content(manifest).is_empty());
    }

    #[test]
    fn kube_manifest_validator_reports_missing_production_resources() {
        let manifest = r#"
apiVersion: apps/v1
kind: Deployment
spec:
  template:
    spec:
      containers: []
"#;
        let issues = validate_kube_manifest_content(manifest);
        assert!(issues.iter().any(|issue| issue.contains("ServiceAccount")));
        assert!(issues
            .iter()
            .any(|issue| issue.contains("serviceAccountName")));
        assert!(issues.iter().any(|issue| issue.contains("NetworkPolicy")));
    }

    #[test]
    fn helm_chart_validator_accepts_complete_chart() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-helm-validate-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        let templates = root.join("templates");
        fs::create_dir_all(&templates).expect("create chart templates");
        fs::write(
            root.join("Chart.yaml"),
            "apiVersion: v2\ntype: application\nappVersion: \"0.1.0\"\n",
        )
        .expect("write chart");
        fs::write(
            root.join("values.yaml"),
            "image:\nservice:\nresources:\nautoscaling:\nserviceAccount:\npodDisruptionBudget:\nenvFrom:\n",
        )
        .expect("write values");
        fs::write(
            templates.join("deployment.yaml"),
            "kind: Deployment\nserviceAccountName:\nterminationGracePeriodSeconds:\nlivenessProbe:\nreadinessProbe:\nstartupProbe:\n",
        )
        .expect("write deployment");
        fs::write(
            templates.join("service.yaml"),
            "kind: Service\ntargetPort:\n",
        )
        .expect("write service");
        fs::write(
            templates.join("hpa.yaml"),
            "kind: HorizontalPodAutoscaler\naverageUtilization:\n",
        )
        .expect("write hpa");
        fs::write(
            templates.join("serviceaccount.yaml"),
            "kind: ServiceAccount\n",
        )
        .expect("write service account");
        fs::write(
            templates.join("pdb.yaml"),
            "kind: PodDisruptionBudget\nminAvailable:\n",
        )
        .expect("write pdb");
        fs::write(
            templates.join("networkpolicy.yaml"),
            "kind: NetworkPolicy\npolicyTypes:\nIngress\n",
        )
        .expect("write network policy");
        fs::write(
            templates.join("_helpers.tpl"),
            "roze.fullname\nroze.labels\nroze.selectorLabels\n",
        )
        .expect("write helpers");

        assert!(validate_helm_chart_dir(&root).is_empty());

        fs::remove_dir_all(root).expect("remove helm validate chart");
    }

    #[test]
    fn helm_chart_validator_reports_missing_files() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-helm-validate-missing-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create chart root");
        fs::write(root.join("Chart.yaml"), "apiVersion: v2\n").expect("write chart");

        let issues = validate_helm_chart_dir(&root);
        assert!(issues.iter().any(|issue| issue.contains("values.yaml")));
        assert!(issues
            .iter()
            .any(|issue| issue.contains("templates/deployment.yaml")));

        fs::remove_dir_all(root).expect("remove incomplete chart");
    }

    #[test]
    fn parses_native_commands() {
        let api = Cli::try_parse_from(["rozectl", "api", "generate", "user.api", "--out", "out"])
            .expect("parse api generate");
        assert!(matches!(
            api.command,
            Commands::Api {
                command: ApiCommands::Generate { .. }
            }
        ));

        let api_gen = Cli::try_parse_from(["rozectl", "api", "gen", "user.api", "-o", "out"])
            .expect("parse api gen alias");
        assert!(matches!(
            api_gen.command,
            Commands::Api {
                command: ApiCommands::Generate { .. }
            }
        ));

        let api_gen_flagged =
            Cli::try_parse_from(["rozectl", "api", "gen", "--api", "user.api", "--dir", "out"])
                .expect("parse api gen with api flag");
        assert!(matches!(
            api_gen_flagged.command,
            Commands::Api {
                command: ApiCommands::Generate {
                    api_file: Some(_),
                    dir: Some(_),
                    ..
                }
            }
        ));

        let api_go = Cli::try_parse_from([
            "rozectl", "api", "go", "--api", "user.api", "--dir", "out", "--update", "--style",
            "go_zero",
        ])
        .expect("parse goctl-style api go");
        assert!(matches!(
            api_go.command,
            Commands::Api {
                command: ApiCommands::Go { update: true, .. }
            }
        ));

        let api_go_single_dash = parse([
            "rozectl", "api", "go", "-api", "user.api", "-dir", "out", "-style", "go_zero",
        ]);
        assert!(matches!(
            api_go_single_dash.command,
            Commands::Api {
                command: ApiCommands::Go { .. }
            }
        ));

        let api_swagger = Cli::try_parse_from([
            "rozectl", "api", "swagger", "--api", "user.api", "--dir", "docs",
        ])
        .expect("parse goctl-style api swagger");
        assert!(matches!(
            api_swagger.command,
            Commands::Api {
                command: ApiCommands::Swagger { yaml: false, .. }
            }
        ));

        let api_swagger_yaml = Cli::try_parse_from([
            "rozectl", "api", "swagger", "--api", "user.api", "--dir", "docs", "--yaml",
        ])
        .expect("parse goctl-style api swagger yaml");
        assert!(matches!(
            api_swagger_yaml.command,
            Commands::Api {
                command: ApiCommands::Swagger { yaml: true, .. }
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

        let rpc_gen = Cli::try_parse_from(["rozectl", "rpc", "gen", "user.api", "--o", "out"])
            .expect("parse rpc gen alias");
        assert!(matches!(
            rpc_gen.command,
            Commands::Rpc {
                command: RpcCommands::Generate { .. }
            }
        ));

        let rpc_gen_single_dash =
            parse(["rozectl", "rpc", "gen", "-api", "user.api", "-dir", "out"]);
        assert!(matches!(
            rpc_gen_single_dash.command,
            Commands::Rpc {
                command: RpcCommands::Generate {
                    api_file: Some(_),
                    dir: Some(_),
                    ..
                }
            }
        ));

        let rpc_gen_flagged =
            Cli::try_parse_from(["rozectl", "rpc", "gen", "--api", "user.api", "--dir", "out"])
                .expect("parse rpc gen with api flag");
        assert!(matches!(
            rpc_gen_flagged.command,
            Commands::Rpc {
                command: RpcCommands::Generate {
                    api_file: Some(_),
                    dir: Some(_),
                    ..
                }
            }
        ));

        let rpc_gen_multiple =
            Cli::try_parse_from(["rozectl", "rpc", "gen", "--api", "user.api", "-m"])
                .expect("parse rpc gen multiple");
        assert!(matches!(
            rpc_gen_multiple.command,
            Commands::Rpc {
                command: RpcCommands::Generate { multiple: true, .. }
            }
        ));

        let rpc_protoc_multiple =
            Cli::try_parse_from(["rozectl", "rpc", "protoc", "user.proto", "--multiple"])
                .expect("parse rpc protoc multiple");
        assert!(matches!(
            rpc_protoc_multiple.command,
            Commands::Rpc {
                command: RpcCommands::Protoc { multiple: true, .. }
            }
        ));

        let rpc_template = Cli::try_parse_from(["rozectl", "rpc", "template", "-o", "rpc.api"])
            .expect("parse goctl-style rpc template");
        assert!(matches!(
            rpc_template.command,
            Commands::Rpc {
                command: RpcCommands::Template { out: Some(_), .. }
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

        let template_list =
            Cli::try_parse_from(["rozectl", "template", "list", "--home", "templates"])
                .expect("parse template list home");
        assert!(matches!(
            template_list.command,
            Commands::Template {
                command: TemplateCommands::List { dir: Some(_) }
            }
        ));

        let template_list_single_dash =
            parse(["rozectl", "template", "list", "-home", "templates"]);
        assert!(matches!(
            template_list_single_dash.command,
            Commands::Template {
                command: TemplateCommands::List { dir: Some(_) }
            }
        ));

        let template_diff =
            Cli::try_parse_from(["rozectl", "template", "diff", "api", "--dir", "templates"])
                .expect("parse template diff");
        assert!(matches!(
            template_diff.command,
            Commands::Template {
                command: TemplateCommands::Diff { .. }
            }
        ));

        let template_update = Cli::try_parse_from([
            "rozectl",
            "template",
            "update",
            "api",
            "--dir",
            "templates",
            "--force",
        ])
        .expect("parse template update");
        assert!(matches!(
            template_update.command,
            Commands::Template {
                command: TemplateCommands::Update { force: true, .. }
            }
        ));

        let template_update_remote = Cli::try_parse_from([
            "rozectl",
            "template",
            "update",
            "api",
            "--home",
            "templates",
            "--remote",
            "https://example.com/templates.git",
            "--branch",
            "main",
        ])
        .expect("parse template update remote");
        assert!(matches!(
            template_update_remote.command,
            Commands::Template {
                command: TemplateCommands::Update {
                    remote: Some(_),
                    branch: Some(_),
                    ..
                }
            }
        ));

        let template_revert = Cli::try_parse_from([
            "rozectl",
            "template",
            "revert",
            "api",
            "--dir",
            "templates",
            "--no-backup",
        ])
        .expect("parse template revert");
        assert!(matches!(
            template_revert.command,
            Commands::Template {
                command: TemplateCommands::Revert {
                    no_backup: true,
                    ..
                }
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

        let openapi_gen = Cli::try_parse_from([
            "rozectl",
            "openapi",
            "gen",
            "user.api",
            "--o",
            "openapi.json",
        ])
        .expect("parse openapi gen alias");
        assert!(matches!(
            openapi_gen.command,
            Commands::Openapi {
                command: OpenApiCommands::Generate { .. }
            }
        ));

        let openapi_gen_flagged = Cli::try_parse_from([
            "rozectl",
            "openapi",
            "gen",
            "--api",
            "user.api",
            "--o",
            "openapi.json",
        ])
        .expect("parse openapi gen with api flag");
        assert!(matches!(
            openapi_gen_flagged.command,
            Commands::Openapi {
                command: OpenApiCommands::Generate {
                    api_file: Some(_),
                    ..
                }
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

        let search_generate = Cli::try_parse_from([
            "rozectl",
            "search",
            "generate",
            "search.schema",
            "--engine",
            "opensearch",
            "--out",
            "out",
        ])
        .expect("parse search generate");
        assert!(matches!(
            search_generate.command,
            Commands::Search {
                command: SearchCommands::Generate {
                    engine: SearchEngine::Opensearch,
                    ..
                }
            }
        ));

        let search_gen =
            Cli::try_parse_from(["rozectl", "search", "gen", "search.schema", "-o", "out"])
                .expect("parse search gen alias");
        assert!(matches!(
            search_gen.command,
            Commands::Search {
                command: SearchCommands::Generate { .. }
            }
        ));

        let search_inspect = Cli::try_parse_from([
            "rozectl",
            "search",
            "inspect",
            "users",
            "--engine",
            "meilisearch",
            "--url",
            "http://127.0.0.1:7700",
            "--api-key",
            "master",
            "--sample-size",
            "25",
        ])
        .expect("parse search inspect");
        assert!(matches!(
            search_inspect.command,
            Commands::Search {
                command: SearchCommands::Inspect {
                    engine: SearchEngine::Meilisearch,
                    sample_size: 25,
                    ..
                }
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

        let direct_ts_client =
            Cli::try_parse_from(["rozectl", "api", "ts", "--api", "user.api", "--dir", "sdk"])
                .expect("parse goctl-style api ts");
        assert!(matches!(
            direct_ts_client.command,
            Commands::Api {
                command: ApiCommands::Ts { .. }
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

        let direct_js_client =
            Cli::try_parse_from(["rozectl", "api", "js", "-a", "user.api", "-d", "sdk"])
                .expect("parse goctl-style api js");
        assert!(matches!(
            direct_js_client.command,
            Commands::Api {
                command: ApiCommands::Js { .. }
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

        let direct_dart_client = Cli::try_parse_from([
            "rozectl",
            "api",
            "dart",
            "-a",
            "user.api",
            "-o",
            "sdk/user.dart",
        ])
        .expect("parse goctl-style api dart");
        assert!(matches!(
            direct_dart_client.command,
            Commands::Api {
                command: ApiCommands::Dart { out: Some(_), .. }
            }
        ));

        let java_client = Cli::try_parse_from([
            "rozectl",
            "api",
            "client",
            "java",
            "user.api",
            "--out",
            "sdk/RozeApiClient.java",
        ])
        .expect("parse api client java");
        assert!(matches!(
            java_client.command,
            Commands::Api {
                command: ApiCommands::Client {
                    command: ClientCommands::Java { .. }
                }
            }
        ));

        let kotlin_client = Cli::try_parse_from([
            "rozectl",
            "api",
            "client",
            "kotlin",
            "user.api",
            "--out",
            "sdk/RozeApiClient.kt",
        ])
        .expect("parse api client kotlin");
        assert!(matches!(
            kotlin_client.command,
            Commands::Api {
                command: ApiCommands::Client {
                    command: ClientCommands::Kotlin { .. }
                }
            }
        ));

        let direct_java_client =
            Cli::try_parse_from(["rozectl", "api", "java", "-a", "user.api", "-d", "sdk"])
                .expect("parse goctl-style api java");
        assert!(matches!(
            direct_java_client.command,
            Commands::Api {
                command: ApiCommands::Java { .. }
            }
        ));

        let direct_kotlin_client =
            Cli::try_parse_from(["rozectl", "api", "kotlin", "-a", "user.api", "-d", "sdk"])
                .expect("parse goctl-style api kotlin");
        assert!(matches!(
            direct_kotlin_client.command,
            Commands::Api {
                command: ApiCommands::Kotlin { .. }
            }
        ));

        let swift_client = Cli::try_parse_from([
            "rozectl",
            "api",
            "client",
            "swift",
            "user.api",
            "--out",
            "sdk/RozeApiClient.swift",
        ])
        .expect("parse api client swift");
        assert!(matches!(
            swift_client.command,
            Commands::Api {
                command: ApiCommands::Client {
                    command: ClientCommands::Swift { .. }
                }
            }
        ));

        let ios_client = Cli::try_parse_from([
            "rozectl",
            "api",
            "client",
            "ios",
            "user.api",
            "--out",
            "sdk/RozeApiClient.swift",
        ])
        .expect("parse api client ios");
        assert!(matches!(
            ios_client.command,
            Commands::Api {
                command: ApiCommands::Client {
                    command: ClientCommands::Ios { .. }
                }
            }
        ));

        let android_client = Cli::try_parse_from([
            "rozectl",
            "api",
            "client",
            "android",
            "user.api",
            "--out",
            "sdk/RozeApiClient.kt",
        ])
        .expect("parse api client android");
        assert!(matches!(
            android_client.command,
            Commands::Api {
                command: ApiCommands::Client {
                    command: ClientCommands::Android { .. }
                }
            }
        ));

        let direct_ios_client =
            Cli::try_parse_from(["rozectl", "api", "ios", "-a", "user.api", "-d", "sdk"])
                .expect("parse goctl-style api ios");
        assert!(matches!(
            direct_ios_client.command,
            Commands::Api {
                command: ApiCommands::Ios { .. }
            }
        ));

        let direct_android_client =
            Cli::try_parse_from(["rozectl", "api", "android", "-a", "user.api", "-d", "sdk"])
                .expect("parse goctl-style api android");
        assert!(matches!(
            direct_android_client.command,
            Commands::Api {
                command: ApiCommands::Android { .. }
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

        let model_gen = Cli::try_parse_from([
            "rozectl", "model", "gen", "user.sql", "--format", "sql", "--o", "models",
        ])
        .expect("parse model gen alias");
        assert!(matches!(
            model_gen.command,
            Commands::Model {
                command: ModelCommands::Generate {
                    format: ModelFormat::Sql,
                    ..
                }
            }
        ));

        let mysql_datasource = Cli::try_parse_from([
            "rozectl",
            "model",
            "mysql",
            "datasource",
            "--url",
            "mysql://root:root@127.0.0.1:3306/roze",
            "--table",
            "users",
            "--dir",
            "models",
        ])
        .expect("parse goctl-style model mysql datasource");
        assert!(matches!(
            mysql_datasource.command,
            Commands::Model {
                command: ModelCommands::Mysql {
                    command: ModelSqlCompatCommands::Datasource { .. }
                }
            }
        ));

        let pg_ddl = Cli::try_parse_from([
            "rozectl",
            "model",
            "pg",
            "ddl",
            "--src",
            "schema.sql",
            "--dir",
            "models",
        ])
        .expect("parse goctl-style model pg ddl");
        assert!(matches!(
            pg_ddl.command,
            Commands::Model {
                command: ModelCommands::Pg {
                    command: ModelSqlCompatCommands::Ddl { .. }
                }
            }
        ));

        let pg_ddl_single_dash = parse([
            "rozectl",
            "model",
            "pg",
            "ddl",
            "-src",
            "schema.sql",
            "-dir",
            "models",
        ]);
        assert!(matches!(
            pg_ddl_single_dash.command,
            Commands::Model {
                command: ModelCommands::Pg {
                    command: ModelSqlCompatCommands::Ddl { .. }
                }
            }
        ));

        let mongo_compat = Cli::try_parse_from([
            "rozectl",
            "model",
            "mongo",
            "--collection",
            "users",
            "--db-url",
            "mongodb://127.0.0.1:27017/roze",
            "--dir",
            "models",
        ])
        .expect("parse goctl-style model mongo");
        assert!(matches!(
            mongo_compat.command,
            Commands::Model {
                command: ModelCommands::Mongo {
                    collection: Some(_),
                    ..
                }
            }
        ));

        let mongo_compat_single_dash = parse([
            "rozectl",
            "model",
            "mongo",
            "-collection",
            "users",
            "-db-url",
            "mongodb://127.0.0.1:27017/roze",
            "-dir",
            "models",
        ]);
        assert!(matches!(
            mongo_compat_single_dash.command,
            Commands::Model {
                command: ModelCommands::Mongo {
                    collection: Some(_),
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

        let api_generate = parse(["rozectl", "api", "generate", "user.api", "--out", "out"]);
        assert!(matches!(
            api_generate.command,
            Commands::Api {
                command: ApiCommands::Generate { .. }
            }
        ));

        let client_o = parse([
            "rozectl",
            "api",
            "client",
            "ts",
            "user.api",
            "--o",
            "sdk/user.ts",
        ]);
        assert!(matches!(
            client_o.command,
            Commands::Api {
                command: ApiCommands::Client {
                    command: ClientCommands::Ts { .. }
                }
            }
        ));

        let api_validate = parse(["rozectl", "api", "validate", "user.api"]);
        assert!(matches!(
            api_validate.command,
            Commands::Api {
                command: ApiCommands::Validate { .. }
            }
        ));

        let api_validate_flagged = parse(["rozectl", "api", "validate", "--api", "user.api"]);
        assert!(matches!(
            api_validate_flagged.command,
            Commands::Api {
                command: ApiCommands::Validate {
                    api_file: Some(_),
                    ..
                }
            }
        ));

        let api_format = parse(["rozectl", "api", "format", "user.api", "--check"]);
        assert!(matches!(
            api_format.command,
            Commands::Api {
                command: ApiCommands::Format { check: true, .. }
            }
        ));

        let api_format_flagged =
            parse(["rozectl", "api", "format", "--api", "user.api", "--write"]);
        assert!(matches!(
            api_format_flagged.command,
            Commands::Api {
                command: ApiCommands::Format {
                    api_file: Some(_),
                    write: true,
                    ..
                }
            }
        ));

        let openapi = Cli::try_parse_from([
            "rozectl",
            "openapi",
            "generate",
            "user.api",
            "--out",
            "docs/openapi.json",
        ])
        .expect("parse openapi generate");
        assert!(matches!(
            openapi.command,
            Commands::Openapi {
                command: OpenApiCommands::Generate { .. }
            }
        ));

        let stream = Cli::try_parse_from([
            "rozectl",
            "stream",
            "gen",
            "--api",
            "user.api",
            "--out",
            "stream",
            "--update",
            "--roze-source",
            "path",
        ])
        .expect("parse stream gen");
        assert!(matches!(
            stream.command,
            Commands::Stream {
                command: StreamCommands::Gen { .. }
            }
        ));

        let stream_generate = Cli::try_parse_from([
            "rozectl", "stream", "generate", "--api", "user.api", "--o", "stream",
        ])
        .expect("parse stream generate alias");
        assert!(matches!(
            stream_generate.command,
            Commands::Stream {
                command: StreamCommands::Gen { .. }
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

        let plugin = Cli::try_parse_from([
            "rozectl", "api", "plugin", "--plugin", "cat", "--api", "user.api", "--dir", "out",
        ])
        .expect("parse api plugin");
        assert!(matches!(
            plugin.command,
            Commands::Api {
                command: ApiCommands::Plugin { .. }
            }
        ));

        let docker = parse(["rozectl", "docker", "--binary", "user-api"]);
        assert!(matches!(docker.command, Commands::Docker { .. }));

        let completion = parse(["rozectl", "completion", "powershell"]);
        assert!(matches!(
            completion.command,
            Commands::Completion {
                shell: CompletionShell::Powershell
            }
        ));

        let env = parse(["rozectl", "env"]);
        assert!(matches!(env.command, Commands::Env));

        let upgrade = parse([
            "rozectl",
            "upgrade",
            "--repo",
            "https://example.com/roze.git",
            "--branch",
            "main",
            "--dry-run",
        ]);
        assert!(matches!(
            upgrade.command,
            Commands::Upgrade {
                branch: Some(_),
                dry_run: true,
                ..
            }
        ));

        let quickstart = parse([
            "rozectl",
            "quickstart",
            "demo",
            "--kind",
            "rpc",
            "--o",
            "out",
        ]);
        assert!(matches!(
            quickstart.command,
            Commands::Quickstart {
                name,
                kind: QuickstartKind::Rpc,
                out: Some(_),
                ..
            } if name == "demo"
        ));

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
            "--min-available",
            "1",
        ])
        .expect("parse kube deploy");
        assert!(matches!(
            kube.command,
            Commands::Kube {
                command: KubeCommands::Deploy { .. }
            }
        ));

        let kube_validate = Cli::try_parse_from([
            "rozectl",
            "kube",
            "validate",
            "--file",
            "deploy/kubernetes.yaml",
        ])
        .expect("parse kube validate");
        assert!(matches!(
            kube_validate.command,
            Commands::Kube {
                command: KubeCommands::Validate { .. }
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
            "--min-available",
            "1",
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

        let helm_validate = Cli::try_parse_from([
            "rozectl",
            "helm",
            "validate",
            "--chart",
            "deploy/user-chart",
        ])
        .expect("parse helm validate");
        assert!(matches!(
            helm_validate.command,
            Commands::Helm {
                command: HelmCommands::Validate { .. }
            }
        ));
    }

    #[test]
    fn renders_completion_scripts() {
        assert!(render_completion(CompletionShell::Bash).contains("complete -F _rozectl rozectl"));
        assert!(render_completion(CompletionShell::Zsh).contains("#compdef rozectl"));
        assert!(render_completion(CompletionShell::Fish).contains("complete -c rozectl"));
        assert!(
            render_completion(CompletionShell::Powershell).contains("Register-ArgumentCompleter")
        );
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

        let cli = Cli::try_parse_from([
            "rozectl",
            "model",
            "inspect",
            "users",
            "--schema",
            "roze",
            "--db-kind",
            "mongo",
            "--db-url",
            "mongodb://127.0.0.1:27017",
            "--sample-size",
            "25",
        ])
        .expect("parse mongo inspect");

        match cli.command {
            Commands::Model {
                command:
                    ModelCommands::Inspect {
                        table,
                        schema,
                        db_url,
                        db_kind,
                        sample_size,
                        ..
                    },
            } => {
                assert_eq!(table, "users");
                assert_eq!(schema.as_deref(), Some("roze"));
                assert_eq!(db_url, "mongodb://127.0.0.1:27017");
                assert!(matches!(db_kind, DbKind::Mongo));
                assert_eq!(sample_size, 25);
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

        let issues = check_contract_breaking_changes(&old_spec, &new_spec);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn api_validate_accepts_consistent_contract() {
        let spec = parser::parse_api(
            r#"
            service user {
                @handler getUser
                get /users/:id (GetUserReq) returns (GetUserResp)
                @handler getUserByTenant
                get /tenants/:tenant-id/users/:id (TenantUserReq) returns (GetUserResp)
                rpc Ping (PingReq) returns (PingResp)
            }

            type GetUserReq {
                id string `path:"id"`
                filter UserFilter `json:"filter,optional" validate:"optional"`
            }
            type UserFilter {
                keyword string `json:"keyword,optional" validate:"optional"`
            }
            type GetUserResp {
                id string `json:"id"`
            }
            type TenantUserReq {
                tenantId string `path:"tenant-id"`
                id string `path:"id"`
            }
            type PingReq {
                requestId string `json:"requestId"`
            }
            type PingResp {
                ok bool `json:"ok"`
            }
            "#,
        )
        .expect("parse spec");

        let issues = validate_api_spec(&spec);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn api_validate_accepts_idl_integer_aliases() {
        let spec = parser::parse_api(
            r#"
            service catalog {
                @handler listProducts
                get /products (PageReq) returns (PageResp)
            }

            type PageReq {
                page int64 `query:"page"`
                limit int64 `query:"limit"`
                categoryIds []int64 `query:"categoryIds,optional" validate:"optional"`
            }

            type PageResp {
                page int64 `json:"page"`
                limit int64 `json:"limit"`
                total uint64 `json:"total"`
                ids []int64 `json:"ids"`
                counters map[string]int64 `json:"counters"`
            }
            "#,
        )
        .expect("parse spec");

        let issues = validate_api_spec(&spec);
        assert!(issues.is_empty(), "{issues:?}");
    }

    #[test]
    fn api_validate_reports_generation_blocking_contract_issues() {
        let spec = parser::parse_api(
            r#"
            service user {
                get /users/:id (GetUserReq) returns (MissingResp)
                get /users/:id (GetUserReq) returns (MissingResp)
                @handler getUser
                post /users/:id (GetUserReq) returns (PingResp)
                @handler get_user
                patch /users/:id (GetUserReq) returns (PingResp)
                @handler ping
                get /ping
                @middleware(type, audit.v1)
                get /assets/logo.png (PingReq) returns (PingResp)
                @handler type
                put /keyword-handler (PingReq) returns (PingResp)
                @handler duplicatedPath
                get /duplicate/:id/{id} (DuplicatePathReq) returns (PingResp)
                @handler middlewareCollision
                @middleware(audit-log, audit_log)
                get /middleware-collision (PingReq) returns (PingResp)
                rpc Ping (PingReq) returns (PingResp)
                rpc Ping (PingReq) returns (PingResp)
                rpc GetUser (PingReq) returns (PingResp)
                rpc get_user (PingReq) returns (PingResp)
                rpc type (PingReq) returns (PingResp)
            }

            type GetUserReq {
                id string `query:"id"`
                tenantId string `path:"tenantId"`
                name string `json:"name"`
                displayName string `json:"name"`
                profile MissingProfile `json:"profile"`
            }
            type DuplicateReq {
                id string `path:"id"`
            }
            type DuplicateReq {
                id string `path:"id"`
            }
            type user_req {
                field-name string `json:"field-name"`
                field_name string `json:"field_name"`
            }
            type UserReq {
                ok bool `json:"ok"`
            }
            type DuplicatePathReq {
                id string `path:"id"`
            }
            type PingReq {
                requestId string `json:"requestId"`
            }
            type PingResp {
                ok bool `json:"ok"`
            }
            type EmptyReq {
                unexpected string `json:"unexpected"`
            }
            type EmptyResp {
                unexpected string `json:"unexpected"`
            }
            "#,
        )
        .expect("parse spec");

        let issues = validate_api_spec(&spec);
        let report = issues
            .iter()
            .map(|issue| issue.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(report.contains("duplicate type: DuplicateReq"));
        assert!(report.contains("type user_req is not a valid Rust type name"));
        assert!(report.contains("duplicate generated type name `UserReq`: user_req and UserReq"));
        assert!(report.contains("duplicate wire field: GetUserReq Json `name`"));
        assert!(report.contains(
            "EmptyReq is reserved for omitted request/response bodies and must not declare fields"
        ));
        assert!(report.contains(
            "EmptyResp is reserved for omitted request/response bodies and must not declare fields"
        ));
        assert!(report.contains(
            "duplicate generated field name `field_name` in type user_req: field-name and field_name"
        ));
        assert!(report.contains("duplicate REST route: GET /users/:id"));
        assert!(report.contains("duplicate RPC method: Ping"));
        assert!(report.contains("duplicate generated REST handler `get_user` in group `users`"));
        assert!(report.contains("duplicate generated RPC method `get_user`: GetUser and get_user"));
        assert!(report.contains("REST route GET /assets/logo.png generates invalid Rust handler"));
        assert!(report.contains(
            "REST route GET /assets/logo.png custom middleware type generates invalid Rust identifier `type`"
        ));
        assert!(report.contains(
            "REST route GET /assets/logo.png custom middleware audit.v1 generates invalid Rust identifier `audit.v1`"
        ));
        assert!(report.contains("REST route PUT /keyword-handler generates invalid Rust handler"));
        assert!(report.contains(
            "REST route GET /duplicate/:id/{id} has duplicate path parameter `id` and `id`"
        ));
        assert!(report.contains(
            "duplicate generated custom middleware `audit_log`: audit-log and audit_log"
        ));
        assert!(report.contains("RPC method type generates invalid Rust method `type`"));
        assert!(report.contains(
            "REST route GET /users/:id response type references unknown type: MissingResp"
        ));
        assert!(report.contains("field GetUserReq.profile references unknown type: MissingProfile"));
        assert!(report.contains("path parameter `:id` is missing from GetUserReq as a path field"));
        assert!(report.contains(
            "request type GetUserReq declares path field `tenantId` that is not present in the route path"
        ));
    }

    #[test]
    fn api_validate_reports_invalid_generated_service_identifiers() {
        let spec = parser::parse_api(
            r#"
            service 123-api {
                rpc Ping (PingReq) returns (PingResp)
            }

            type PingReq {
                requestId string `json:"requestId"`
            }
            type PingResp {
                ok bool `json:"ok"`
            }
            type _ {
                _ string `json:"_"`
            }
            "#,
        )
        .expect("parse spec");

        let issues = validate_api_spec(&spec);
        let report = issues
            .iter()
            .map(|issue| issue.detail.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(report.contains("service 123-api generates invalid Rust module `123_api`"));
        assert!(report.contains("service 123-api generates invalid Rust service type `123Api`"));
        assert!(report.contains("type _ is not a valid Rust type name"));
        assert!(report.contains("field _._ generates invalid Rust field name `_`"));
    }

    #[test]
    fn api_generation_preflight_rejects_invalid_contract() {
        let root = temp_test_root("rozectl-api-generation-preflight-test");
        fs::create_dir_all(&root).expect("create test root");
        let api = root.join("user.api");
        fs::write(
            &api,
            r#"
            service user {
                @handler type
                get /users (UserReq) returns (UserResp)
            }

            type UserReq {
                id string `json:"id"`
            }
            type UserResp {
                ok bool `json:"ok"`
            }
            "#,
        )
        .expect("write api");

        let err = validate_api_for_generation(&api).expect_err("invalid contract rejected");
        let message = err.to_string();
        assert!(message.contains("api validation failed before generation"));
        assert!(message.contains("REST route GET /users generates invalid Rust handler `type`"));

        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn diff_api_uses_generation_preflight_validation() {
        let root = temp_test_root("rozectl-diff-api-preflight-test");
        fs::create_dir_all(&root).expect("create test root");
        let api = root.join("user.api");
        let out = root.join("out");
        fs::write(
            &api,
            r#"
            service user {
                rpc type (PingReq) returns (PingResp)
            }

            type PingReq {
                requestId string `json:"requestId"`
            }
            type PingResp {
                ok bool `json:"ok"`
            }
            "#,
        )
        .expect("write api");

        let err = run_diff(
            DiffCommands::Api {
                api,
                out,
                roze_source: RozeSource::Git,
            },
            &generator::registry(),
        )
        .expect_err("invalid diff contract rejected");
        let message = err.to_string();
        assert!(message.contains("api validation failed before generation"));
        assert!(message.contains("RPC method type generates invalid Rust method `type`"));

        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn optional_api_generation_preflight_allows_missing_api() {
        validate_optional_api_for_generation(None).expect("missing optional api allowed");
    }

    #[test]
    fn api_format_renders_canonical_contract() {
        let spec = parser::parse_api(
            r#"
            syntax="v1"
            info(
                title: "User API"
                desc: "Example"
            )
            service user-api {
                @server(
                    prefix: /api/v1
                    group: user
                    middleware: auth, trace
                )
                @doc("Get user")
                @middleware(audit)
                @handler(getUser)
                get   /users/:id(GetUserReq)returns(GetUserResp)
                rpc Ping (PingReq)returns(PingResp)
            }
            type(
                GetUserReq {
                    id string `path:"id"`
                    token string `header:"X-Token"`
                    name string `json:"name" validate:"optional"`
                }
                GetUserResp {
                    id string `json:"id"`
                }
                PingReq {
                    requestId string `json:"requestId"`
                }
                PingResp {
                    ok bool `json:"ok"`
                }
            )
            "#,
        )
        .expect("parse spec");

        let formatted = format_api_spec(&spec);

        assert_eq!(
            formatted,
            r#"syntax = "v1"

info (
    title: "User API"
    desc: "Example"
)

service user-api {
    @server (
        prefix: /api/v1
        group: user
        middleware: auth, trace
    )
    @doc "Get user"
    @middleware audit
    @handler getUser
    get /users/:id (GetUserReq) returns (GetUserResp)
    rpc Ping (PingReq) returns (PingResp)
}

type (
    GetUserReq {
        id string `path:"id"`
        token string `header:"X-Token"`
        name string `json:"name" validate:"optional"`
    }
    GetUserResp {
        id string `json:"id"`
    }
    PingReq {
        requestId string `json:"requestId"`
    }
    PingResp {
        ok bool `json:"ok"`
    }
)
"#
        );
        assert_eq!(
            normalize_line_endings(&formatted.replace('\n', "\r\n")),
            formatted
        );
    }

    #[test]
    fn template_diff_renders_file_level_changes() {
        assert_eq!(template_file_name("api").expect("template"), "api.api");
        assert!(template_file_name("missing").is_err());

        let diff = render_unified_diff("line1\nold\n", "line1\nnew\nadded\n", "local", "builtin");

        assert!(diff.contains("--- local"));
        assert!(diff.contains("+++ builtin"));
        assert!(diff.contains(" line1"));
        assert!(diff.contains("-old"));
        assert!(diff.contains("+new"));
        assert!(diff.contains("+added"));
    }

    #[test]
    fn template_source_reads_local_home() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-template-home-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create template root");
        fs::write(root.join("api.api"), "custom api template\n").expect("write local template");

        let source = template_source("api", Some(&root), None, None).expect("read local template");
        assert_eq!(source, "custom api template\n");

        fs::remove_dir_all(root).expect("remove template root");
    }

    #[test]
    fn template_update_protects_local_customizations() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-template-update-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create template root");
        let api_template = root.join("api.api");
        fs::write(&api_template, "custom template\n").expect("write custom template");

        let err = run_template_update("api", &root, None, None, false)
            .expect_err("custom template rejected");
        assert!(err.to_string().contains("diff api"));
        assert_eq!(
            fs::read_to_string(&api_template).expect("read custom template"),
            "custom template\n"
        );

        run_template_update("api", &root, None, None, true).expect("force update template");
        assert_eq!(
            fs::read_to_string(&api_template).expect("read updated template"),
            generator::template("api").expect("builtin template")
        );

        fs::remove_dir_all(root).expect("remove template root");
    }

    #[test]
    fn template_revert_restores_builtin_template_with_optional_backup() {
        let root = std::env::temp_dir().join(format!(
            "rozectl-template-revert-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::create_dir_all(&root).expect("create template root");
        let api_template = root.join("api.api");
        fs::write(&api_template, "custom template\n").expect("write custom template");

        run_template_revert("api", &root, None, None, true).expect("revert template");
        assert_eq!(
            fs::read_to_string(&api_template).expect("read reverted template"),
            generator::template("api").expect("builtin template")
        );
        assert_eq!(
            fs::read_to_string(template_backup_path(&api_template)).expect("read backup"),
            "custom template\n"
        );

        fs::write(&api_template, "custom again\n").expect("write custom template again");
        let backup_path = template_backup_path(&api_template);
        fs::remove_file(&backup_path).expect("remove backup");
        run_template_revert("api", &root, None, None, false).expect("revert without backup");
        assert!(!backup_path.exists());
        assert_eq!(
            fs::read_to_string(&api_template).expect("read reverted template"),
            generator::template("api").expect("builtin template")
        );

        fs::remove_dir_all(root).expect("remove template root");
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

        let issues = check_contract_breaking_changes(&old_spec, &new_spec);
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
