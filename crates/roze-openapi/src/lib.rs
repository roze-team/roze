use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiDocument {
    pub openapi: String,
    pub info: OpenApiInfo,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<OpenApiServer>,
    pub paths: BTreeMap<String, PathItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiInfo {
    pub title: String,
    pub version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PathItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub get: Option<Operation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub post: Option<Operation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub put: Option<Operation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delete: Option<Operation>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Operation {
    pub operation_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<Parameter>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_body: Option<RequestBody>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub responses: BTreeMap<String, Response>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Parameter {
    pub name: String,
    pub location: ParameterLocation,
    pub required: bool,
    pub ty: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ParameterLocation {
    Path,
    Query,
    Header,
    Cookie,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestBody {
    pub content_type: String,
    pub ty: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ty: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiServer {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone)]
pub struct OpenApiBuilder {
    document: OpenApiDocument,
}

#[derive(Debug, Clone, Copy)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
}

impl OpenApiBuilder {
    pub fn new(title: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            document: OpenApiDocument {
                openapi: "3.0.3".to_string(),
                info: OpenApiInfo {
                    title: title.into(),
                    version: version.into(),
                    description: None,
                },
                servers: Vec::new(),
                paths: BTreeMap::new(),
            },
        }
    }

    pub fn description(mut self, description: impl Into<String>) -> Self {
        self.document.info.description = Some(description.into());
        self
    }

    pub fn server(mut self, url: impl Into<String>, description: impl Into<String>) -> Self {
        self.document.servers.push(OpenApiServer {
            url: url.into(),
            description: Some(description.into()),
        });
        self
    }

    pub fn add_operation(
        &mut self,
        path: impl Into<String>,
        method: HttpMethod,
        operation: Operation,
    ) {
        let path = path.into();
        let item = self.document.paths.entry(path).or_default();
        match method {
            HttpMethod::Get => item.get = Some(operation),
            HttpMethod::Post => item.post = Some(operation),
            HttpMethod::Put => item.put = Some(operation),
            HttpMethod::Delete => item.delete = Some(operation),
        }
    }

    pub fn finish(self) -> OpenApiDocument {
        self.document
    }
}

impl Operation {
    pub fn new(operation_id: impl Into<String>) -> Self {
        Self {
            operation_id: operation_id.into(),
            summary: None,
            description: None,
            tags: Vec::new(),
            parameters: Vec::new(),
            request_body: None,
            responses: BTreeMap::new(),
        }
    }

    pub fn summary(mut self, value: impl Into<String>) -> Self {
        self.summary = Some(value.into());
        self
    }

    pub fn description(mut self, value: impl Into<String>) -> Self {
        self.description = Some(value.into());
        self
    }

    pub fn tag(mut self, value: impl Into<String>) -> Self {
        self.tags.push(value.into());
        self
    }

    pub fn parameter(
        mut self,
        name: impl Into<String>,
        location: ParameterLocation,
        ty: impl Into<String>,
        required: bool,
    ) -> Self {
        self.parameters.push(Parameter {
            name: name.into(),
            location,
            required,
            ty: ty.into(),
        });
        self
    }

    pub fn request_body(mut self, ty: impl Into<String>) -> Self {
        self.request_body = Some(RequestBody {
            content_type: "application/json".to_string(),
            ty: ty.into(),
        });
        self
    }

    pub fn response(
        mut self,
        status: impl Into<String>,
        description: impl Into<String>,
        ty: impl Into<String>,
    ) -> Self {
        self.responses.insert(
            status.into(),
            Response {
                description: description.into(),
                ty: Some(ty.into()),
            },
        );
        self
    }

    pub fn empty_response(mut self, status: impl Into<String>, description: impl Into<String>) -> Self {
        self.responses.insert(
            status.into(),
            Response {
                description: description.into(),
                ty: None,
            },
        );
        self
    }
}

pub fn to_json_value(document: &OpenApiDocument) -> serde_json::Value {
    serde_json::to_value(document).expect("openapi document serializes")
}

pub fn to_json_pretty(document: &OpenApiDocument) -> String {
    serde_json::to_string_pretty(document).expect("openapi document serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_a_minimal_document() {
        let mut builder = OpenApiBuilder::new("roze", "1.0.0")
            .description("api")
            .server("/api", "service: roze");
        let op = Operation::new("login")
            .tag("auth")
            .request_body("LoginReq")
            .response("200", "OK", "LoginResp");
        builder.add_operation("/login", HttpMethod::Post, op);

        let json = to_json_value(&builder.finish());

        assert_eq!(json["openapi"], "3.0.3");
        assert_eq!(json["info"]["title"], "roze");
        assert_eq!(json["servers"][0]["url"], "/api");
        assert_eq!(json["paths"]["/login"]["post"]["operation_id"], "login");
        assert_eq!(json["paths"]["/login"]["post"]["request_body"]["ty"], "LoginReq");
    }
}
