pub mod rest;
pub mod rpc;
pub mod types;

use std::{collections::HashSet, fs, path::Path};

use anyhow::{bail, Context};

use crate::parser::ApiSpec;

pub fn generate_project(spec: &ApiSpec, out: &Path, force: bool) -> anyhow::Result<()> {
    ensure_output(out, force)?;
    let proto = render_proto(spec)?;

    fs::create_dir_all(out.join("src/handler"))?;
    fs::create_dir_all(out.join("src/logic"))?;
    fs::create_dir_all(out.join("src/svc"))?;
    fs::create_dir_all(out.join("proto"))?;
    fs::write(out.join("Cargo.toml"), cargo_toml(spec))?;
    fs::write(out.join("build.rs"), build_rs())?;
    fs::write(out.join("config.yaml"), config_yaml(spec))?;
    fs::write(out.join("src/config.rs"), config_rs())?;
    fs::write(out.join("src/pb.rs"), render_pb(spec))?;
    fs::write(out.join("src/types.rs"), types::render_types(&spec.types))?;
    fs::write(out.join("src/handler/mod.rs"), rest::render_handlers(spec))?;
    fs::write(out.join("src/logic/mod.rs"), rest::render_logic(spec))?;
    fs::write(out.join("src/svc/mod.rs"), service_context_rs())?;
    fs::write(out.join("src/rpc.rs"), rpc::render_rpc(spec))?;
    fs::write(out.join("src/client.rs"), rpc::render_client(spec))?;
    fs::write(out.join("src/main.rs"), rest::render_main(spec))?;
    fs::write(out.join("proto/service.proto"), proto)?;

    Ok(())
}

fn ensure_output(out: &Path, force: bool) -> anyhow::Result<()> {
    if out.exists() && !force && has_entries(out)? {
        bail!(
            "{} already exists and is not empty; pass --force to overwrite generated files",
            out.display()
        );
    }
    fs::create_dir_all(out).with_context(|| format!("failed to create {}", out.display()))
}

fn has_entries(path: &Path) -> anyhow::Result<bool> {
    Ok(fs::read_dir(path)?.next().is_some())
}

fn cargo_toml(spec: &ApiSpec) -> String {
    format!(
        r#"[package]
name = "{}-service"
edition.workspace = true
license.workspace = true
version.workspace = true

[dependencies]
anyhow.workspace = true
config.workspace = true
poem.workspace = true
prost.workspace = true
roze-core = {{ path = "../../roze-core" }}
serde.workspace = true
sea-orm.workspace = true
tokio.workspace = true
tonic.workspace = true
tracing.workspace = true

[build-dependencies]
protoc-bin-vendored.workspace = true
tonic-build.workspace = true
"#,
        spec.service
    )
}

fn build_rs() -> String {
    r#"fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    std::env::set_var("PROTOC", protoc);

    tonic_build::configure()
        .build_server(true)
        .build_client(true)
        .compile(&["proto/service.proto"], &["proto"])?;

    println!("cargo:rerun-if-changed=proto/service.proto");
    Ok(())
}
"#
    .to_string()
}

fn config_yaml(spec: &ApiSpec) -> String {
    format!(
        r#"name: {}
rest:
  addr: 127.0.0.1:3000
registry:
  kind: memory
  endpoints: []
  # ttl_seconds: 10
  # renew_interval_secs: 3
# database:
#   url: sqlite://data/{}.db?mode=rwc
# cache:
#   url: redis://127.0.0.1/
#   namespace: {}
# auth:
#   jwt_secret: change-me
#   jwt_issuer: {}
#   jwt_expiration_secs: 86400
"#,
        spec.service, spec.service, spec.service, spec.service
    )
}

fn config_rs() -> String {
    r#"pub type Config = roze_core::config::ServiceConfig;

pub fn load(path: impl AsRef<std::path::Path>) -> Result<Config, config::ConfigError> {
    roze_core::config::load(path)
}
"#
    .to_string()
}

fn service_context_rs() -> String {
    r#"#![allow(dead_code)]

use crate::config::Config;
use sea_orm::DatabaseConnection;

#[derive(Clone, Debug)]
pub struct ServiceContext {
    pub config: Config,
    pub db: Option<DatabaseConnection>,
    pub cache: Option<roze_core::cache::RedisCache>,
}

impl ServiceContext {
    pub async fn new(config: Config) -> anyhow::Result<Self> {
        let db = roze_core::db::connect_optional(config.database.as_ref()).await?;
        let cache = match config.cache.as_ref() {
            Some(cache) => Some(
                roze_core::cache::RedisCache::connect(&roze_core::cache::CacheConfig {
                    url: cache.url.clone(),
                    namespace: cache.namespace.clone(),
                    default_ttl_secs: cache.default_ttl_secs,
                })
                .await?,
            ),
            None => None,
        };
        Ok(Self { config, db, cache })
    }

    pub fn jwt_config(&self) -> Option<roze_core::shared::auth::JwtConfig> {
        self.config.auth.as_ref().map(Into::into)
    }
}
"#
    .to_string()
}

fn render_pb(spec: &ApiSpec) -> String {
    let package = spec.service.replace('-', "_");
    format!(
        r#"pub mod {package} {{
    tonic::include_proto!("{package}");
}}
"#
    )
}

pub fn to_pascal_case(input: &str) -> String {
    input
        .split(['_', '-', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect()
}

pub fn to_snake_case(input: &str) -> String {
    let mut out = String::new();
    for (idx, ch) in input.chars().enumerate() {
        if ch.is_uppercase() && idx > 0 {
            out.push('_');
        } else if ch == '-' || ch == ' ' {
            out.push('_');
            continue;
        }
        out.push(ch.to_ascii_lowercase());
    }
    out
}

fn render_proto(spec: &ApiSpec) -> anyhow::Result<String> {
    let package = spec.service.replace('-', "_");
    let known_types = spec
        .types
        .iter()
        .map(|ty| ty.name.as_str())
        .collect::<HashSet<_>>();
    let mut out = format!("syntax = \"proto3\";\n\npackage {};\n\n", package);

    out.push_str(&format!("service {} {{\n", to_pascal_case(&spec.service)));
    for route in &spec.rest_routes {
        let rpc_name = route.handler.clone().unwrap_or_else(|| {
            let method = match route.method {
                crate::parser::HttpMethod::Get => "get",
                crate::parser::HttpMethod::Post => "post",
                crate::parser::HttpMethod::Put => "put",
                crate::parser::HttpMethod::Delete => "delete",
            };
            format!("{}_{}", method, route_name_from_path(&route.path))
        });
        out.push_str(&format!(
            "  rpc {} ({}) returns ({});\n",
            to_pascal_case(&rpc_name),
            route.request,
            route.response
        ));
    }
    for method in &spec.rpc_methods {
        out.push_str(&format!(
            "  rpc {} ({}) returns ({});\n",
            to_pascal_case(&method.name),
            method.request,
            method.response
        ));
    }
    out.push_str("}\n\n");

    for ty in &spec.types {
        out.push_str(&format!("message {} {{\n", ty.name));
        for (idx, field) in ty.fields.iter().enumerate() {
            let name = field
                .json_name
                .clone()
                .unwrap_or_else(|| to_snake_case(&field.name));
            out.push_str(&format!(
                "  {} {} = {};\n",
                proto_type(&field.ty, &known_types)?,
                name,
                idx + 1
            ));
        }
        out.push_str("}\n\n");
    }

    Ok(out)
}

fn route_name_from_path(path: &str) -> String {
    path.trim_matches('/')
        .replace(':', "")
        .replace(['{', '}'], "")
        .replace('/', "_")
        .replace('-', "_")
}

fn proto_type<'a>(ty: &'a str, known_types: &HashSet<&str>) -> anyhow::Result<&'a str> {
    let proto = match ty {
        "String" | "string" => "string",
        "bool" => "bool",
        "i32" | "int32" => "int32",
        "i64" | "int" | "int64" => "int64",
        "u32" | "uint32" => "uint32",
        "u64" | "uint" | "uint64" => "uint64",
        "f32" | "float" => "float",
        "f64" | "double" => "double",
        known if known_types.contains(known) => known,
        other => anyhow::bail!("unsupported proto field type `{other}`"),
    };
    Ok(proto)
}

#[cfg(test)]
mod tests {
    use crate::parser::{parse_api, HttpMethod};

    use super::*;

    #[test]
    fn renders_valid_rpc_name_for_route_without_handler() {
        let spec = parse_api(
            r#"
            service user-api {
                get /users/:id (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                name: string
            }
            "#,
        )
        .expect("valid api");

        let proto = render_proto(&spec).expect("valid proto");

        assert!(proto.contains("rpc GetUsersId (GetUserReq) returns (UserResp);"));
        assert!(!proto.contains('/'));
        assert!(!proto.contains(':'));
    }

    #[test]
    fn rejects_unsupported_proto_field_type() {
        let spec = parse_api(
            r#"
            service user-api {
                post /users (CreateUserReq) returns (UserResp)
            }

            type CreateUserReq {
                profile: Profile
            }

            type UserResp {
                id: u64
            }
            "#,
        )
        .expect("valid api");

        let err = render_proto(&spec).expect_err("unsupported type should fail");

        assert!(err
            .to_string()
            .contains("unsupported proto field type `Profile`"));
    }

    #[test]
    fn allows_known_message_field_type() {
        let spec = parse_api(
            r#"
            service user-api {
                post /users (CreateUserReq) returns (UserResp)
            }

            type CreateUserReq {
                profile: Profile
            }

            type Profile {
                displayName: string
            }

            type UserResp {
                id: u64
            }
            "#,
        )
        .expect("valid api");

        let proto = render_proto(&spec).expect("known message type should render");

        assert!(proto.contains("Profile profile = 1;"));
    }

    #[test]
    fn route_name_from_path_strips_path_syntax() {
        assert_eq!(
            route_name_from_path("/teams/{team-id}/users/:id"),
            "teams_team_id_users_id"
        );
    }

    #[test]
    fn implicit_route_rpc_name_stays_aligned_with_handler_name() {
        let route = crate::parser::RestRoute {
            handler: None,
            doc: None,
            middlewares: Vec::new(),
            method: HttpMethod::Get,
            path: "/users/:id".to_string(),
            request: "GetUserReq".to_string(),
            response: "UserResp".to_string(),
        };
        let name = format!("get_{}", route_name_from_path(&route.path));

        assert_eq!(to_snake_case(&to_pascal_case(&name)), "get_users_id");
    }
}
