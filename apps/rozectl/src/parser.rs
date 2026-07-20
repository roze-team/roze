use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiSpec {
    pub service: String,
    pub rpc_package: Option<String>,
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    pub embedded: bool,
    pub json_name: Option<String>,
    pub source: FieldSource,
    pub wire_name: Option<String>,
    pub validate: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestRoute {
    pub handler: Option<String>,
    pub doc: Option<String>,
    pub middlewares: Vec<String>,
    pub permissions: Vec<String>,
    pub server: Option<ServerSpec>,
    pub method: HttpMethod,
    pub path: String,
    pub request: String,
    pub response: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
}

const EMPTY_REQUEST_TYPE: &str = "EmptyReq";
const EMPTY_RESPONSE_TYPE: &str = "EmptyResp";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RpcMethod {
    pub name: String,
    pub request: String,
    pub response: String,
    pub middlewares: Vec<String>,
    pub permissions: Vec<String>,
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

        if is_syntax_decl(line) {
            continue;
        }

        if is_block_start(line, "info") {
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

        if is_block_start(line, "@server") {
            server = Some(parse_server_block(&lines, &mut i)?);
            continue;
        }

        if is_block_start(line, "type") {
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
                let mut current_permissions: Vec<String> = Vec::new();
                let mut current_server = None;
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
                    if is_block_start(svc_line, "@server") {
                        current_server = Some(parse_server_block(&lines, &mut i)?);
                        if service_server.is_none() {
                            service_server = current_server.clone();
                        }
                        continue;
                    }
                    if let Some(doc) = parse_annotation_arg(svc_line, "@doc") {
                        current_doc = Some(trim_annotation_string(doc).to_string());
                        continue;
                    }
                    if let Some(handler) = parse_annotation_arg(svc_line, "@handler") {
                        current_handler = Some(trim_annotation_string(handler).to_string());
                        continue;
                    }
                    if let Some(middleware) = parse_annotation_arg(svc_line, "@middleware") {
                        current_middlewares.extend(parse_name_list(middleware));
                        continue;
                    }
                    if let Some(permission) = parse_annotation_arg(svc_line, "@permission") {
                        current_permissions.extend(parse_name_list(permission));
                        continue;
                    }
                    if let Some(method) = svc_line.strip_prefix("rpc ") {
                        let mut method = parse_rpc_method(method, svc_line_no)?;
                        method.middlewares = std::mem::take(&mut current_middlewares);
                        method.permissions = std::mem::take(&mut current_permissions);
                        rpc_methods.push(method);
                        continue;
                    }
                    if let Some(mut route) = parse_rest_route(svc_line, svc_line_no)? {
                        route.handler = current_handler.take();
                        route.doc = current_doc.take();
                        route.middlewares = std::mem::take(&mut current_middlewares);
                        route.permissions = std::mem::take(&mut current_permissions);
                        route.server = current_server.clone();
                        rest_routes.push(route);
                        continue;
                    }

                    return invalid(
                        svc_line_no,
                        "expected `@server (...)`, `@handler name`, `@doc text`, `@middleware name`, `@permission name`, RPC method or route declaration",
                    );
                }

                if server.is_none() && service_server.is_some() {
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

    normalize_empty_requests(&mut types, &mut rest_routes, &mut rpc_methods);

    Ok(ApiSpec {
        service: service.ok_or(ParseError::MissingService)?,
        rpc_package: None,
        server,
        info,
        types,
        rest_routes,
        rpc_methods,
    })
}

fn normalize_empty_requests(
    types: &mut Vec<TypeDef>,
    rest_routes: &mut [RestRoute],
    rpc_methods: &mut [RpcMethod],
) {
    let mut needs_empty_req = false;
    let mut needs_empty_resp = false;
    for route in rest_routes {
        if route.request.trim().is_empty() {
            route.request = EMPTY_REQUEST_TYPE.to_string();
            needs_empty_req = true;
        } else if route.request == EMPTY_REQUEST_TYPE {
            needs_empty_req = true;
        }
        if route.response.trim().is_empty() {
            route.response = EMPTY_RESPONSE_TYPE.to_string();
            needs_empty_resp = true;
        } else if route.response == EMPTY_RESPONSE_TYPE {
            needs_empty_resp = true;
        }
    }
    for method in rpc_methods {
        if method.request.trim().is_empty() {
            method.request = EMPTY_REQUEST_TYPE.to_string();
            needs_empty_req = true;
        } else if method.request == EMPTY_REQUEST_TYPE {
            needs_empty_req = true;
        }
        if method.response.trim().is_empty() {
            method.response = EMPTY_RESPONSE_TYPE.to_string();
            needs_empty_resp = true;
        } else if method.response == EMPTY_RESPONSE_TYPE {
            needs_empty_resp = true;
        }
    }
    if needs_empty_req && !types.iter().any(|ty| ty.name == EMPTY_REQUEST_TYPE) {
        types.push(TypeDef {
            name: EMPTY_REQUEST_TYPE.to_string(),
            fields: Vec::new(),
        });
    }
    if needs_empty_resp && !types.iter().any(|ty| ty.name == EMPTY_RESPONSE_TYPE) {
        types.push(TypeDef {
            name: EMPTY_RESPONSE_TYPE.to_string(),
            fields: Vec::new(),
        });
    }
}

fn parse_rest_route(line: &str, line_no: usize) -> Result<Option<RestRoute>, ParseError> {
    let Some((method_name, rest)) = split_first_token(line) else {
        return Ok(None);
    };
    let method = match method_name {
        "get" => HttpMethod::Get,
        "head" => HttpMethod::Head,
        "post" => HttpMethod::Post,
        "put" => HttpMethod::Put,
        "patch" => HttpMethod::Patch,
        "delete" => HttpMethod::Delete,
        _ => return Ok(None),
    };

    let (path, signature) = split_route_path_and_signature(rest);
    if path.is_empty() {
        return Err(ParseError::InvalidLine {
            line: line_no + 1,
            message: "expected route path and optional `(Req) returns (Resp)`".to_string(),
        });
    }
    let (request, response) = parse_signature(signature, line_no)?;

    Ok(Some(RestRoute {
        handler: None,
        doc: None,
        middlewares: Vec::new(),
        permissions: Vec::new(),
        server: None,
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
        .map(trim_annotation_string)
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
            embedded: false,
            json_name: None,
            source: FieldSource::Auto,
            wire_name: None,
            validate: None,
        }
    } else {
        let parts = line.split_whitespace().collect::<Vec<_>>();
        let field_name = parts.first().copied().unwrap_or_default();
        let field_ty = parts.get(1).copied().unwrap_or_default();
        if field_name.is_empty() {
            return invalid(line_no, "expected `Field Type `json:\"field\"``");
        }

        if field_ty.is_empty() || field_ty.starts_with('`') {
            Field {
                name: embedded_field_name(field_name),
                ty: field_name.trim_start_matches('*').to_string(),
                embedded: true,
                json_name: None,
                source: FieldSource::Auto,
                wire_name: None,
                validate: None,
            }
        } else {
            Field {
                name: field_name.to_string(),
                ty: field_ty.to_string(),
                embedded: false,
                json_name: None,
                source: FieldSource::Auto,
                wire_name: None,
                validate: None,
            }
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
    if let Some(value) = parse_tag_value_full(line, "validate") {
        field.validate = Some(value);
    }

    Ok(field)
}

fn embedded_field_name(ty: &str) -> String {
    let ty = ty
        .trim_start_matches('*')
        .rsplit(['.', '/'])
        .next()
        .unwrap_or(ty);
    let mut out = String::new();
    let mut prev_lower_or_digit = false;
    for ch in ty.chars() {
        if ch.is_ascii_uppercase() {
            if prev_lower_or_digit && !out.is_empty() {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
            prev_lower_or_digit = false;
        } else if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_lower_or_digit = ch.is_ascii_lowercase() || ch.is_ascii_digit();
        } else if !out.ends_with('_') && !out.is_empty() {
            out.push('_');
            prev_lower_or_digit = false;
        }
    }
    out.trim_matches('_').to_string()
}

fn parse_tag_value(line: &str, tag: &str) -> Option<String> {
    let value = parse_tag_value_full(line, tag)?;
    let name = value.split(',').next().unwrap_or_default();
    if name.is_empty() || name == "-" {
        None
    } else {
        Some(name.to_string())
    }
}

fn parse_tag_value_full(line: &str, tag: &str) -> Option<String> {
    let needle = format!("{tag}:\"");
    let start = line.find(&needle)? + needle.len();
    let rest = &line[start..];
    let value = rest.split_once('"')?.0;
    if value.is_empty() || value == "-" {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_rpc_method(input: &str, line_no: usize) -> Result<RpcMethod, ParseError> {
    let (name, signature) = split_first_token(input).ok_or_else(|| ParseError::InvalidLine {
        line: line_no + 1,
        message: "expected `rpc Name (Req) returns (Resp)`".to_string(),
    })?;
    let (request, response) = parse_signature(signature, line_no)?;

    Ok(RpcMethod {
        name: name.to_string(),
        request,
        response,
        middlewares: Vec::new(),
        permissions: Vec::new(),
    })
}

fn parse_signature(input: &str, _line_no: usize) -> Result<(String, String), ParseError> {
    let trimmed = input.trim();
    let Some(returns_at) = trimmed.find("returns") else {
        return Ok((
            trim_wrapping_parens(trimmed).to_string(),
            EMPTY_RESPONSE_TYPE.to_string(),
        ));
    };
    let request = &trimmed[..returns_at];
    let response = &trimmed[returns_at + "returns".len()..];

    Ok((
        trim_wrapping_parens(request).to_string(),
        trim_wrapping_parens(response).to_string(),
    ))
}

fn is_block_start(line: &str, name: &str) -> bool {
    let Some(rest) = line.strip_prefix(name) else {
        return false;
    };
    let rest = rest.trim_start();
    rest == "("
}

fn is_syntax_decl(line: &str) -> bool {
    let Some((key, _)) = line.split_once('=') else {
        return false;
    };
    key.trim() == "syntax"
}

fn parse_annotation_arg<'a>(line: &'a str, name: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(name)?;
    let rest = rest.trim_start();
    if rest.is_empty() {
        return None;
    }
    if let Some(inner) = rest
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
    {
        return Some(inner.trim());
    }
    Some(rest.trim())
}

fn trim_annotation_string(input: &str) -> &str {
    let trimmed = input.trim();
    trimmed
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .unwrap_or(trimmed)
        .trim()
}

fn split_first_token(input: &str) -> Option<(&str, &str)> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let idx = input.find(char::is_whitespace)?;
    let (head, tail) = input.split_at(idx);
    Some((head, tail.trim_start()))
}

fn split_route_path_and_signature(input: &str) -> (&str, &str) {
    let input = input.trim();
    if input.is_empty() {
        return ("", "");
    }

    let first_space = input.find(char::is_whitespace);
    let first_paren = input.find('(');
    let split_at = match (first_space, first_paren) {
        (Some(space), Some(paren)) => space.min(paren),
        (Some(space), None) => space,
        (None, Some(paren)) => paren,
        (None, None) => return (input, ""),
    };
    let (path, signature) = input.split_at(split_at);
    (path.trim(), signature.trim_start())
}

fn trim_wrapping_parens(input: &str) -> &str {
    let trimmed = input.trim();
    trimmed
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
        .map(str::trim)
        .unwrap_or(trimmed)
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
                middlewares: Vec::new(),
                permissions: Vec::new(),
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
        assert_eq!(
            spec.rest_routes[0]
                .server
                .as_ref()
                .and_then(|server| server.prefix.as_deref()),
            Some("/api/v1")
        );
    }

    #[test]
    fn parses_multiple_server_blocks_as_route_scoped_config() {
        let spec = parse_api(
            r#"
            @server (
                prefix: /api
                middleware: global
            )

            service user-api {
                @server (
                    prefix: /api/v1
                    group: user
                    middleware: auth
                    jwt: Auth
                )
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)

                @server (
                    prefix: /internal
                    group: admin
                    middleware: audit
                )
                @handler getStats
                get /stats (StatsReq) returns (StatsResp)
            }

            type (
                GetUserReq {
                    id u64 `path:"id"`
                }
                UserResp {
                    id u64 `json:"id"`
                }
                StatsReq {
                    q string `query:"q"`
                }
                StatsResp {
                    ok bool `json:"ok"`
                }
            )
            "#,
        )
        .unwrap();

        assert_eq!(
            spec.server
                .as_ref()
                .and_then(|server| server.prefix.as_deref()),
            Some("/api")
        );
        assert_eq!(
            spec.rest_routes[0]
                .server
                .as_ref()
                .and_then(|server| server.prefix.as_deref()),
            Some("/api/v1")
        );
        assert_eq!(
            spec.rest_routes[0]
                .server
                .as_ref()
                .and_then(|server| server.group.as_deref()),
            Some("user")
        );
        assert_eq!(
            spec.rest_routes[0]
                .server
                .as_ref()
                .and_then(|server| server.jwt.as_deref()),
            Some("Auth")
        );
        assert_eq!(
            spec.rest_routes[1]
                .server
                .as_ref()
                .and_then(|server| server.prefix.as_deref()),
            Some("/internal")
        );
        assert_eq!(
            spec.rest_routes[1]
                .server
                .as_ref()
                .map(|server| server.middlewares.as_slice()),
            Some(&["audit".to_string()][..])
        );
    }

    #[test]
    fn parses_compact_go_zero_block_and_signature_spacing() {
        let spec = parse_api(
            r#"
            syntax = "v1"

            info(
                title: "用户服务"
            )

            type(
                GetUserReq {
                    id u64 `path:"id"`
                }
                UserResp {
                    id u64 `json:"id"`
                }
            )

            service user-api {
                @server(
                    prefix: /api/v1
                )
                @doc("获取用户")
                @middleware(auth, trace)
                @handler(getUser)
                patch   /users/:id   (GetUserReq)returns(UserResp)
                @handler(ping)
                head /ping ()
                rpc   Ping   (GetUserReq)returns(UserResp)
            }
            "#,
        )
        .unwrap();

        assert_eq!(spec.info[0].key, "title");
        assert_eq!(spec.types.len(), 4);
        assert_eq!(spec.rest_routes.len(), 2);
        assert_eq!(spec.rest_routes[0].method, HttpMethod::Patch);
        assert_eq!(spec.rest_routes[1].method, HttpMethod::Head);
        assert_eq!(spec.rest_routes[1].request, "EmptyReq");
        assert_eq!(spec.rest_routes[1].response, "EmptyResp");
        assert_eq!(spec.rest_routes[0].doc.as_deref(), Some("获取用户"));
        assert_eq!(spec.rest_routes[0].middlewares, vec!["auth", "trace"]);
        assert_eq!(spec.rest_routes[0].handler.as_deref(), Some("getUser"));
        assert_eq!(spec.rest_routes[0].request, "GetUserReq");
        assert_eq!(spec.rest_routes[0].response, "UserResp");
        assert_eq!(
            spec.rest_routes[0]
                .server
                .as_ref()
                .and_then(|server| server.prefix.as_deref()),
            Some("/api/v1")
        );
        assert_eq!(spec.rpc_methods.len(), 1);
        assert_eq!(spec.rpc_methods[0].request, "GetUserReq");
        assert_eq!(spec.rpc_methods[0].response, "UserResp");
    }

    #[test]
    fn parses_legacy_goctl_route_signature_without_space_after_path() {
        let spec = parse_api(
            r#"
            service shorturl-api {
                @handler shorten
                get /shorten(shortenReq) returns(shortenResp)
            }
            "#,
        )
        .unwrap();

        assert_eq!(spec.rest_routes.len(), 1);
        assert_eq!(spec.rest_routes[0].path, "/shorten");
        assert_eq!(spec.rest_routes[0].request, "shortenReq");
        assert_eq!(spec.rest_routes[0].response, "shortenResp");
    }

    #[test]
    fn parses_route_without_request_as_empty_request() {
        let spec = parse_api(
            r#"
            service health-api {
                @handler health
                get /health returns (HealthResp)
                @handler ping
                get /ping
                @handler logout
                post /logout (LogoutReq)
            }

            type LogoutReq {
                token string `json:"token"`
            }

            type HealthResp {
                ok bool `json:"ok"`
            }
            "#,
        )
        .unwrap();

        assert_eq!(spec.rest_routes[0].request, "EmptyReq");
        assert_eq!(spec.rest_routes[1].request, "EmptyReq");
        assert_eq!(spec.rest_routes[1].response, "EmptyResp");
        assert_eq!(spec.rest_routes[2].request, "LogoutReq");
        assert_eq!(spec.rest_routes[2].response, "EmptyResp");
        assert!(spec
            .types
            .iter()
            .any(|ty| ty.name == "EmptyReq" && ty.fields.is_empty()));
        assert!(spec
            .types
            .iter()
            .any(|ty| ty.name == "EmptyResp" && ty.fields.is_empty()));
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
                nickname String `json:"nickname" validate:"required,min=2,max=16"`
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
        assert_eq!(
            spec.types[0].fields[4].validate.as_deref(),
            Some("required,min=2,max=16")
        );
    }

    #[test]
    fn parses_route_and_rpc_permissions() {
        let spec = parse_api(
            r#"
            service user {
                @permission users:read, users:write
                get /users (ListUsersReq) returns (ListUsersResp)

                @permission users:write
                rpc CreateUser (CreateUserReq) returns (UserResp)
            }

            type ListUsersReq {
            }
            type ListUsersResp {
            }
            type CreateUserReq {
            }
            type UserResp {
            }
            "#,
        )
        .expect("valid api");

        assert_eq!(
            spec.rest_routes[0].permissions,
            vec!["users:read", "users:write"]
        );
        assert_eq!(spec.rpc_methods[0].permissions, vec!["users:write"]);
    }

    #[test]
    fn parses_rpc_method_middlewares() {
        let spec = parse_api(
            r#"
            service user-api {
                @middleware idempotency
                rpc CreateUser (CreateUserReq) returns (UserResp)

                get /users (ListUsersReq) returns (ListUsersResp)
            }

            type CreateUserReq {
            }
            type UserResp {
            }
            type ListUsersReq {
            }
            type ListUsersResp {
            }
            "#,
        )
        .expect("valid api");

        assert_eq!(spec.rpc_methods[0].middlewares, vec!["idempotency"]);
        assert!(spec.rest_routes[0].middlewares.is_empty());
    }

    #[test]
    fn parses_multiple_same_name_service_blocks() {
        let spec = parse_api(
            r#"
            service user-api {
                @handler getUser
                get /users/:id (GetUserReq) returns (UserResp)
            }

            service user-api {
                @handler createUser
                post /users (CreateUserReq) returns (UserResp)
            }

            type (
                GetUserReq {
                    id u64 `path:"id"`
                }
                CreateUserReq {
                    name string `json:"name"`
                }
                UserResp {
                    id u64 `json:"id"`
                }
            )
            "#,
        )
        .unwrap();

        assert_eq!(spec.service, "user-api");
        assert_eq!(spec.rest_routes.len(), 2);
        assert_eq!(spec.rest_routes[0].handler.as_deref(), Some("getUser"));
        assert_eq!(spec.rest_routes[1].handler.as_deref(), Some("createUser"));
    }

    #[test]
    fn parses_go_zero_anonymous_embedded_types() {
        let spec = parse_api(
            r#"
            service user-api

            type (
                BaseReq {
                    traceId string `json:"traceId,optional" validate:"optional"`
                }
                CreateUserReq {
                    BaseReq
                    *AuditMeta `json:",inline"`
                    name string `json:"name"`
                }
            )
            "#,
        )
        .unwrap();

        let req = spec
            .types
            .iter()
            .find(|ty| ty.name == "CreateUserReq")
            .expect("request type");
        assert_eq!(req.fields[0].name, "base_req");
        assert_eq!(req.fields[0].ty, "BaseReq");
        assert!(req.fields[0].embedded);
        assert_eq!(req.fields[1].name, "audit_meta");
        assert_eq!(req.fields[1].ty, "AuditMeta");
        assert!(req.fields[1].embedded);
        assert_eq!(req.fields[2].name, "name");
        assert!(!req.fields[2].embedded);
    }
}
