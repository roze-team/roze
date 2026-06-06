use std::collections::BTreeMap;

use roze_openapi::{OpenApiBuilder, Schema, HttpMethod, Operation};

pub fn document() -> serde_json::Value {
    let mut builder = OpenApiBuilder::new("user", "0.1.0").description("service group: user");
    builder = builder.server("/api", "service: user");
    {
        let mut properties = BTreeMap::new();
        properties.insert("username".to_string(), Schema::string());
        properties.insert("password".to_string(), Schema::string());
        builder = builder.component_schema("LoginReq", Schema::object(properties, vec!["username".to_string(), "password".to_string()]));
    }
    {
        let mut properties = BTreeMap::new();
        properties.insert("token".to_string(), Schema::string());
        properties.insert("expires_at".to_string(), Schema::integer("uint64"));
        builder = builder.component_schema("LoginResp", Schema::object(properties, vec!["token".to_string(), "expires_at".to_string()]));
    }
    let op = Operation::new("post_user_login").tag("user").request_body("LoginReq").response("200", "OK", "LoginResp");
    builder.add_operation("/api/user/login", HttpMethod::Post, op);
    roze_openapi::to_json_value(&builder.finish())
}
