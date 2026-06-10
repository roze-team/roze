use std::collections::BTreeMap;

use roze_openapi::{HttpMethod, OpenApiBuilder, Operation, Schema};

pub fn document() -> serde_json::Value {
    let mut builder = OpenApiBuilder::new("user-api", "0.1.0").description("user-api/api");
    builder = builder.server("/", "service: user-api");
    {
        let mut properties = BTreeMap::new();
        properties.insert("username".to_string(), Schema::string());
        properties.insert("password".to_string(), Schema::string());
        builder = builder.component_schema(
            "LoginReq",
            Schema::object(
                properties,
                vec!["username".to_string(), "password".to_string()],
            ),
        );
    }
    {
        let mut properties = BTreeMap::new();
        properties.insert("token".to_string(), Schema::string());
        builder = builder.component_schema(
            "LoginResp",
            Schema::object(properties, vec!["token".to_string()]),
        );
    }
    {
        let mut properties = BTreeMap::new();
        properties.insert("id".to_string(), Schema::integer("int64"));
        builder = builder.component_schema(
            "GetUserReq",
            Schema::object(properties, vec!["id".to_string()]),
        );
    }
    {
        let mut properties = BTreeMap::new();
        properties.insert("id".to_string(), Schema::integer("int64"));
        properties.insert("username".to_string(), Schema::string());
        properties.insert("created_at".to_string(), Schema::integer("int64"));
        builder = builder.component_schema(
            "UserResp",
            Schema::object(
                properties,
                vec![
                    "id".to_string(),
                    "username".to_string(),
                    "created_at".to_string(),
                ],
            ),
        );
    }
    let op = Operation::new("login")
        .tag("user-api")
        .request_body("LoginReq")
        .response("200", "OK", "LoginResp");
    builder.add_operation("/user/login", HttpMethod::Post, op);
    let op = Operation::new("getUser")
        .tag("user-api")
        .parameter("id", roze_openapi::ParameterLocation::Path, "i64", true)
        .response("200", "OK", "UserResp");
    builder.add_operation("/user/:id", HttpMethod::Get, op);
    roze_openapi::to_json_value(&builder.finish())
}
