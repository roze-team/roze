use std::collections::BTreeMap;

use roze_openapi::{HttpMethod, OpenApiBuilder, Operation, Schema};

pub fn document() -> serde_json::Value {
    let mut builder =
        OpenApiBuilder::new("roze-sample", "0.1.0").description("service group: roze_sample");
    builder = builder.server("/api", "service: roze-sample");
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
        properties.insert("expires_at".to_string(), Schema::integer("uint64"));
        builder = builder.component_schema(
            "LoginResp",
            Schema::object(
                properties,
                vec!["token".to_string(), "expires_at".to_string()],
            ),
        );
    }
    let op = Operation::new("post_roze_sample_login")
        .tag("roze-sample")
        .request_body("LoginReq")
        .response("200", "OK", "LoginResp");
    builder.add_operation("/api/roze_sample/login", HttpMethod::Post, op);
    roze_openapi::to_json_value(&builder.finish())
}
