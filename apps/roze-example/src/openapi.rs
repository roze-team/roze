use std::collections::BTreeMap;

use roze_openapi::{HttpMethod, OpenApiBuilder, Operation, Schema, SecurityScheme};

pub fn document() -> serde_json::Value {
    let mut builder = OpenApiBuilder::new("user-api", "0.1.0").description("user-api/api");
    builder = builder.server("/", "service: user-api");
    builder = builder.security_scheme(
        "bearerAuth",
        SecurityScheme::Http {
            scheme: "bearer".to_string(),
            bearer_format: Some("JWT".to_string()),
        },
    );
    builder = builder.component_schema(
        "LoginReq",
        Schema::object(
            BTreeMap::from([
                ("username".to_string(), Schema::string()),
                ("password".to_string(), Schema::string()),
            ]),
            vec!["username".to_string(), "password".to_string()],
        ),
    );
    builder = builder.component_schema(
        "LoginResp",
        Schema::object(
            BTreeMap::from([
                ("token".to_string(), Schema::string()),
                ("expiresAt".to_string(), Schema::integer("int64")),
            ]),
            vec!["token".to_string(), "expiresAt".to_string()],
        ),
    );
    let op = Operation::new("login")
        .tag("user-api")
        .request_body("LoginReq")
        .response("200", "OK", "LoginResp")
        .require_security("bearerAuth");
    builder.add_operation("/user/login", HttpMethod::Post, op);
    roze_openapi::to_json_value(&builder.finish())
}
