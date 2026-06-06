use roze_openapi::{HttpMethod, OpenApiBuilder, Operation};

pub fn document() -> serde_json::Value {
    let mut builder = OpenApiBuilder::new("user-api", "0.1.0").description("user-api/api");
    builder = builder.server("/", "service: user-api");
    let op = Operation::new("login").tag("user-api").request_body("LoginReq").response("200", "OK", "LoginResp");
    builder.add_operation("/user/login", HttpMethod::Post, op);
    roze_openapi::to_json_value(&builder.finish())
}
