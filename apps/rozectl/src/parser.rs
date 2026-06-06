use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSpec {
    pub service: String,
    pub server: Option<ServerSpec>,
    pub info: Vec<InfoPair>,
    pub types: Vec<TypeDef>,
    pub rest_routes: Vec<RestRoute>,
    pub rpc_methods: Vec<RpcMethod>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ServerSpec {
    pub prefix: Option<String>,
    pub group: Option<String>,
    pub middlewares: Vec<String>,
    pub jwt: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InfoPair {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDef {
    pub name: String,
    pub fields: Vec<Field>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FieldSource {
    Auto,
    Json,
    Query,
    Form,
    Path,
    Header,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    pub ty: String,
    pub json_name: Option<String>,
    pub source: FieldSource,
    pub wire_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestRoute {
    pub handler: Option<String>,
    pub doc: Option<String>,
    pub middlewares: Vec<String>,
    pub method: HttpMethod,
    pub path: String,
    pub request: String,
    pub response: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcMethod {
    pub name: String,
    pub request: String,
    pub response: String,
}

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("line {line}: {message}")]
    InvalidLine { line: usize, message: String },
    #[error("missing service declaration")]
    MissingService,
}

pub fn parse_api(source: &str) -> Result<ApiSpec, ParseError> {
    let mut service = None;
    let mut server = None;
    let mut info = Vec::new();
    let mut types = Vec::new();
    let mut rest_routes = Vec::new();
    let mut rpc_methods = Vec::new();

    let lines = source.lines().enumerate().collect::<Vec<_>>();
    let mut i = 0;

    while i < lines.len() {
        let (line_no, raw) = lines[i];
        let line = strip_comment(raw).trim();
        i += 1;

        if line.is_empty() {
            continue;
        }

        if line == "info (" {
            while i < lines.len() {
                let (info_line_no, info_raw) = lines[i];
                i += 1;
                let info_line = strip_comment(info_raw).trim();
                if info_line.is_empty() {
                    continue;
                }
                if info_line == ")" {
                    break;
                }

                let mut parts = info_line.splitn(2, ':');
                let key = parts.next().unwrap_or_default().trim();
                let value = parts
                    .next()
                    .unwrap_or_default()
                    .trim()
                    .trim_matches('"')
                    .to_string();
                if key.is_empty() || value.is_empty() {
                    return invalid(info_line_no, "expected `key: \"value\"`");
                }
                info.push(InfoPair {
                    key: key.to_string(),
                    value,
                });
            }
            continue;
        }

        if line == "@server (" {
            server = Some(parse_server_block(&lines, &mut i)?);
            continue;
        }

        if line == "type (" {
            while i < lines.len() {
                let (type_line_no, type_raw) = lines[i];
                let type_line = strip_comment(type_raw).trim();
                i += 1;

                if type_line.is_empty() {
                    continue;
                }
                if type_line == ")" {
                    break;
                }
                if !type_line.ends_with('{') {
                    return invalid(type_line_no, "expected `TypeName {` inside type block");
                }

                let name = type_line.trim_end_matches('{').trim();
                let fields = parse_fields_until_brace(&lines, &mut i)?;
                types.push(TypeDef {
                    name: name.to_string(),
                    fields,
                });
            }
            continue;
        }

        if let Some(name) = line.strip_prefix("service ") {
            if line.ends_with('{') {
                let service_name = name.trim_end_matches('{').trim().to_string();
                service = Some(service_name);
                let mut current_handler = None;
                let mut current_doc = None;
                let mut current_middlewares: Vec<String> = Vec::new();
                let mut service_server = None;

                while i < lines.len() {
                    let (svc_line_no, svc_raw) = lines[i];
                    let svc_line = strip_comment(svc_raw).trim();
                    i += 1;

                    if svc_line.is_empty() {
                        continue;
                    }
                    if svc_line == "}" {
                        break;
                    }
                    if svc_line == "@server (" {
                        service_server = Some(parse_server_block(&lines, &mut i)?);
                        continue;
                    }
                    if let Some(doc) = svc_line.strip_prefix("@doc ") {
                        current_doc = Some(doc.trim().to_string());
                        continue;
                    }
                    if let Some(handler) = svc_line.strip_prefix("@handler ") {
                        current_handler = Some(handler.trim().to_string());
                        continue;
                    }
                    if let Some(middleware) = svc_line.strip_prefix("@middleware ") {
                        current_middlewares.extend(parse_name_list(middleware));
                        continue;
                    }
                    if let Some(method) = svc_line.strip_prefix("rpc ") {
                        rpc_methods.push(parse_rpc_method(method, svc_line_no)?);
                        continue;
                    }
                    if let Some(mut route) = parse_rest_route(svc_line, svc_line_no)? {
                        route.handler = current_handler.take();
                        route.doc = current_doc.take();
                        route.middlewares = std::mem::take(&mut current_middlewares);
                        rest_routes.push(route);
                        continue;
                    }

                    return invalid(
                        svc_line_no,
                        "expected `@server (...)`, `@handler name`, `@doc text`, `@middleware name`, RPC method or route declaration",
                    );
                }

                if service_server.is_some() {
                    server = service_server;
                }
            } else {
                service = Some(name.trim().to_string());
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("type ") {
            let name = rest.trim_end_matches('{').trim();
            if name.is_empty() || !line.ends_with('{') {
                return invalid(line_no, "expected `type Name {`");
            }

            let fields = parse_fields_until_brace(&lines, &mut i)?;

            types.push(TypeDef {
                name: name.to_string(),
                fields,
            });
            continue;
        }

        if let Some(route) = parse_rest_route(line, line_no)? {
            rest_routes.push(route);
            continue;
        }

        if let Some(method) = line.strip_prefix("rpc ") {
            rpc_methods.push(parse_rpc_method(method, line_no)?);
            continue;
        }

        return invalid(line_no, "unrecognized declaration");
    }

    Ok(ApiSpec {
        service: service.ok_or(ParseError::MissingService)?,
        server,
        info,
        types,
        rest_routes,
        rpc_methods,
    })
}

fn parse_rest_route(line: &str, line_no: usize) -> Result<Option<RestRoute>, ParseError> {
    let (method, rest) = match line.split_once(' ') {
        Some(("get", rest)) => (HttpMethod::Get, rest),
        Some(("post", rest)) => (HttpMethod::Post, rest),
        Some(("put", rest)) => (HttpMethod::Put, rest),
        Some(("delete", rest)) => (HttpMethod::Delete, rest),
        _ => return Ok(None),
    };

    let (path, signature) = rest
        .trim()
        .split_once(' ')
        .ok_or_else(|| ParseError::InvalidLine {
            line: line_no + 1,
            message: "expected route path and `(Req) returns (Resp)`".to_string(),
        })?;
    let (request, response) = parse_signature(signature, line_no)?;

    Ok(Some(RestRoute {
        handler: None,
        doc: None,
        middlewares: Vec::new(),
        method,
        path: path.to_string(),
        request,
        response,
    }))
}

fn parse_server_block(lines: &[(usize, &str)], i: &mut usize) -> Result<ServerSpec, ParseError> {
    let mut server = ServerSpec::default();

    while *i < lines.len() {
        let (line_no, raw) = lines[*i];
        *i += 1;
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line == ")" {
            break;
        }

        let mut parts = line.splitn(2, ':');
        let key = parts.next().unwrap_or_default().trim();
        let value = parts.next().unwrap_or_default().trim().trim_matches('"');
        if key.is_empty() || value.is_empty() {
            return invalid(line_no, "expected `key: value` inside @server block");
        }

        match key {
            "prefix" => server.prefix = Some(value.to_string()),
            "group" => server.group = Some(value.to_string()),
            "jwt" => server.jwt = Some(value.to_string()),
            "middleware" | "middlewares" => {
                server.middlewares.extend(parse_name_list(value));
            }
            _ => {
                return invalid(
                    line_no,
                    "unsupported @server key; expected `prefix`, `group`, `jwt` or `middleware`",
                )
            }
        }
    }

    Ok(server)
}

fn parse_name_list(input: &str) -> Vec<String> {
    input
        .split(|ch: char| ch == ',' || ch.is_whitespace())
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn parse_fields_until_brace(
    lines: &[(usize, &str)],
    i: &mut usize,
) -> Result<Vec<Field>, ParseError> {
    let mut fields = Vec::new();

    while *i < lines.len() {
        let (field_line_no, field_raw) = lines[*i];
        *i += 1;
        let field_line = strip_comment(field_raw).trim();

        if field_line.is_empty() {
            continue;
        }
        if field_line == "}" {
            break;
        }

        fields.push(parse_field(field_line, field_line_no)?);
    }

    Ok(fields)
}

fn parse_field(line: &str, line_no: usize) -> Result<Field, ParseError> {
    let mut field = if line
        .split_whitespace()
        .next()
        .is_some_and(|first| first.ends_with(':'))
    {
        let mut parts = line.splitn(2, ':');
        let field_name = parts.next().unwrap_or_default().trim();
        let rest = parts.next().unwrap_or_default().trim();
        let field_ty = rest
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_end_matches(',');
        if field_name.is_empty() || field_ty.is_empty() {
            return invalid(line_no, "expected `field: Type`");
        }

        Field {
            name: field_name.to_string(),
            ty: field_ty.to_string(),
            json_name: None,
            source: FieldSource::Auto,
            wire_name: None,
        }
    } else {
        let mut parts = line.split_whitespace();
        let field_name = parts.next().unwrap_or_default();
        let field_ty = parts.next().unwrap_or_default();
        if field_name.is_empty() || field_ty.is_empty() {
            return invalid(line_no, "expected `Field Type `json:\"field\"``");
        }

        Field {
            name: field_name.to_string(),
            ty: field_ty.to_string(),
            json_name: None,
            source: FieldSource::Auto,
            wire_name: None,
        }
    };

    if let Some(value) = parse_tag_value(line, "json") {
        field.json_name = Some(value.clone());
        field.source = FieldSource::Json;
        field.wire_name = Some(value);
    }
    if let Some(value) = parse_tag_value(line, "query") {
        field.json_name = Some(value.clone());
        field.source = FieldSource::Query;
        field.wire_name = Some(value);
    }
    if let Some(value) = parse_tag_value(line, "form") {
        field.json_name = Some(value.clone());
        field.source = FieldSource::Form;
        field.wire_name = Some(value);
    }
    if let Some(value) = parse_tag_value(line, "path") {
        field.json_name = Some(value.clone());
        field.source = FieldSource::Path;
        field.wire_name = Some(value);
    }
    if let Some(value) = parse_tag_value(line, "header") {
        field.source = FieldSource::Header;
        field.wire_name = Some(value);
    }

    Ok(field)
}

fn parse_tag_value(line: &str, tag: &str) -> Option<String> {
    let needle = format!("{tag}:\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let value = rest.split_once('"')?.0;
    let name = value.split(',').next().unwrap_or_default();
    if name.is_empty() || name == "-" {
        None
    } else {
        Some(name.to_string())
    }
}

fn parse_rpc_method(input: &str, line_no: usize) -> Result<RpcMethod, ParseError> {
    let (name, signature) =
        input
            .trim()
            .split_once(' ')
            .ok_or_else(|| ParseError::InvalidLine {
                line: line_no + 1,
                message: "expected `rpc Name (Req) returns (Resp)`".to_string(),
            })?;
    let (request, response) = parse_signature(signature, line_no)?;

    Ok(RpcMethod {
        name: name.to_string(),
        request,
        response,
    })
}

fn parse_signature(input: &str, line_no: usize) -> Result<(String, String), ParseError> {
    let trimmed = input.trim();
    let (request, response) =
        trimmed
            .split_once(" returns ")
            .ok_or_else(|| ParseError::InvalidLine {
                line: line_no + 1,
                message: "expected `(Req) returns (Resp)`".to_string(),
            })?;

    Ok((
        request.trim().trim_matches(['(', ')']).to_string(),
        response.trim().trim_matches(['(', ')']).to_string(),
    ))
}

fn strip_comment(line: &str) -> &str {
    line.split_once("//").map_or(line, |(left, _)| left)
}

fn invalid<T>(line_no: usize, message: impl Into<String>) -> Result<T, ParseError> {
    Err(ParseError::InvalidLine {
        line: line_no + 1,
        message: message.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_service_types_routes_and_rpc() {
        let spec = parse_api(
            r#"
            @server (
                prefix: /v1
                group: user
                middleware: auth, trace
            )

            service user

            type GetUserReq {
                id: u64
            }

            type UserResp {
                id: u64
                name: String
            }

            get /users/:id (GetUserReq) returns (UserResp)
            rpc GetUser (GetUserReq) returns (UserResp)
            "#,
        )
        .unwrap();

        assert_eq!(spec.service, "user");
        assert_eq!(
            spec.server.as_ref().and_then(|s| s.prefix.as_deref()),
            Some("/v1")
        );
        assert_eq!(spec.types.len(), 2);
        assert_eq!(spec.rest_routes.len(), 1);
        assert_eq!(spec.rpc_methods.len(), 1);
    }

    #[test]
    fn parses_rpc_method_inside_service_block() {
        let spec = parse_api(
            r#"
            service user {
                @server (
                    group: user
                )
                rpc GetUser (GetUserReq) returns (UserResp)
            }

            type GetUserReq {
                id: u64
            }

            type UserResp {
                id: u64
                name: string
            }
            "#,
        )
        .expect("valid RPC service");

        assert_eq!(spec.service, "user");
        assert!(spec.rest_routes.is_empty());
        assert_eq!(
            spec.rpc_methods,
            vec![RpcMethod {
                name: "GetUser".to_string(),
                request: "GetUserReq".to_string(),
                response: "UserResp".to_string(),
            }]
        );
    }

    #[test]
    fn parses_go_zero_style_api() {
        let spec = parse_api(
            r#"
            info (
                title: "用户服务"
                desc: "用户登录、注册"
            )

            type (
                LoginReq {
                    Username string `json:"username"`
                    Password string `json:"password"`
                }
                LoginResp {
                    Token string `json:"token"`
                }
            )

            service user-api {
                @server (
                    prefix: /api/v1
                    group: user
                )
                @doc 登录接口
                @middleware auth
                @handler login
                post /user/login (LoginReq) returns (LoginResp)
            }
            "#,
        )
        .unwrap();

        assert_eq!(spec.info.len(), 2);
        assert_eq!(spec.service, "user-api");
        assert_eq!(
            spec.server.as_ref().and_then(|s| s.group.as_deref()),
            Some("user")
        );
        assert_eq!(
            spec.types[0].fields[0].json_name.as_deref(),
            Some("username")
        );
        assert_eq!(spec.types[0].fields[0].source, FieldSource::Json);
        assert_eq!(spec.rest_routes[0].handler.as_deref(), Some("login"));
        assert_eq!(spec.rest_routes[0].doc.as_deref(), Some("登录接口"));
        assert_eq!(spec.rest_routes[0].middlewares, vec!["auth"]);
    }

    #[test]
    fn parses_field_sources_and_wire_names() {
        let spec = parse_api(
            r#"
            service user

            type SampleReq {
                id u64 `path:"id"`
                query String `query:"q"`
                form_name String `form:"name"`
                token String `header:"X-Token"`
            }
            "#,
        )
        .unwrap();

        assert_eq!(spec.types[0].fields[0].source, FieldSource::Path);
        assert_eq!(spec.types[0].fields[0].wire_name.as_deref(), Some("id"));
        assert_eq!(spec.types[0].fields[1].source, FieldSource::Query);
        assert_eq!(spec.types[0].fields[1].wire_name.as_deref(), Some("q"));
        assert_eq!(spec.types[0].fields[2].source, FieldSource::Form);
        assert_eq!(spec.types[0].fields[2].wire_name.as_deref(), Some("name"));
        assert_eq!(spec.types[0].fields[3].source, FieldSource::Header);
        assert_eq!(
            spec.types[0].fields[3].wire_name.as_deref(),
            Some("X-Token")
        );
    }
}
