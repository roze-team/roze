use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiDocument {
    pub openapi: String,
    pub info: OpenApiInfo,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub servers: Vec<OpenApiServer>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub components: Option<OpenApiComponents>,
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
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub security: Vec<SecurityRequirement>,
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
    pub schema: Schema,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub schema: Option<Schema>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenApiServer {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenApiComponents {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub schemas: BTreeMap<String, Schema>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub security_schemes: BTreeMap<String, SecurityScheme>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityRequirement(pub BTreeMap<String, Vec<String>>);

impl SecurityRequirement {
    pub fn bearer(name: impl Into<String>) -> Self {
        let mut map = BTreeMap::new();
        map.insert(name.into(), Vec::new());
        Self(map)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SecurityScheme {
    Http {
        scheme: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        bearer_format: Option<String>,
    },
    ApiKey {
        name: String,
        #[serde(rename = "in")]
        location: ParameterLocation,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schema {
    #[serde(rename = "$ref", skip_serializing_if = "Option::is_none")]
    pub reference: Option<String>,
    #[serde(rename = "type", skip_serializing_if = "Option::is_none")]
    pub kind: Option<SchemaKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub items: Option<Box<Schema>>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub properties: BTreeMap<String, Schema>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nullable: Option<bool>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SchemaKind {
    String,
    Integer,
    Number,
    Boolean,
    Object,
    Array,
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
                components: None,
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

    pub fn component_schema(mut self, name: impl Into<String>, schema: Schema) -> Self {
        self.document
            .components
            .get_or_insert_with(OpenApiComponents::default)
            .schemas
            .insert(name.into(), schema);
        self
    }

    pub fn security_scheme(mut self, name: impl Into<String>, scheme: SecurityScheme) -> Self {
        self.document
            .components
            .get_or_insert_with(OpenApiComponents::default)
            .security_schemes
            .insert(name.into(), scheme);
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
            security: Vec::new(),
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
            schema: Schema::reference(ty),
        });
        self
    }

    pub fn request_body_with_content_type(
        mut self,
        content_type: impl Into<String>,
        schema: Schema,
    ) -> Self {
        self.request_body = Some(RequestBody {
            content_type: content_type.into(),
            schema,
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
                content_type: Some("application/json".to_string()),
                schema: Some(Schema::reference(ty)),
            },
        );
        self
    }

    pub fn response_with_schema(
        mut self,
        status: impl Into<String>,
        description: impl Into<String>,
        content_type: impl Into<String>,
        schema: Schema,
    ) -> Self {
        self.responses.insert(
            status.into(),
            Response {
                description: description.into(),
                content_type: Some(content_type.into()),
                schema: Some(schema),
            },
        );
        self
    }

    pub fn empty_response(mut self, status: impl Into<String>, description: impl Into<String>) -> Self {
        self.responses.insert(
            status.into(),
            Response {
                description: description.into(),
                content_type: None,
                schema: None,
            },
        );
        self
    }

    pub fn require_security(mut self, name: impl Into<String>) -> Self {
        self.security.push(SecurityRequirement::bearer(name));
        self
    }
}

pub fn to_json_value(document: &OpenApiDocument) -> serde_json::Value {
    serde_json::to_value(document).expect("openapi document serializes")
}

pub fn to_json_pretty(document: &OpenApiDocument) -> String {
    serde_json::to_string_pretty(document).expect("openapi document serializes")
}

impl Schema {
    pub fn reference(name: impl Into<String>) -> Self {
        Self {
            reference: Some(format!("#/components/schemas/{}", name.into())),
            kind: None,
            format: None,
            items: None,
            properties: BTreeMap::new(),
            required: Vec::new(),
            description: None,
            nullable: None,
        }
    }

    pub fn string() -> Self {
        Self::primitive(SchemaKind::String)
    }

    pub fn boolean() -> Self {
        Self::primitive(SchemaKind::Boolean)
    }

    pub fn integer(format: impl Into<String>) -> Self {
        let mut schema = Self::primitive(SchemaKind::Integer);
        schema.format = Some(format.into());
        schema
    }

    pub fn number(format: impl Into<String>) -> Self {
        let mut schema = Self::primitive(SchemaKind::Number);
        schema.format = Some(format.into());
        schema
    }

    pub fn array(items: Schema) -> Self {
        Self {
            reference: None,
            kind: Some(SchemaKind::Array),
            format: None,
            items: Some(Box::new(items)),
            properties: BTreeMap::new(),
            required: Vec::new(),
            description: None,
            nullable: None,
        }
    }

    pub fn object(properties: BTreeMap<String, Schema>, required: Vec<String>) -> Self {
        Self {
            reference: None,
            kind: Some(SchemaKind::Object),
            format: None,
            items: None,
            properties,
            required,
            description: None,
            nullable: None,
        }
    }

    fn primitive(kind: SchemaKind) -> Self {
        Self {
            reference: None,
            kind: Some(kind),
            format: None,
            items: None,
            properties: BTreeMap::new(),
            required: Vec::new(),
            description: None,
            nullable: None,
        }
    }
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
        builder = builder.component_schema(
            "LoginReq",
            Schema::object(
                BTreeMap::from([("name".to_string(), Schema::string())]),
                vec!["name".to_string()],
            ),
        );
        builder.add_operation("/login", HttpMethod::Post, op);

        let json = to_json_value(&builder.finish());

        assert_eq!(json["openapi"], "3.0.3");
        assert_eq!(json["info"]["title"], "roze");
        assert_eq!(json["servers"][0]["url"], "/api");
        assert_eq!(json["paths"]["/login"]["post"]["operation_id"], "login");
        assert_eq!(
            json["paths"]["/login"]["post"]["request_body"]["schema"]["$ref"],
            "#/components/schemas/LoginReq"
        );
        assert_eq!(
            json["components"]["schemas"]["LoginReq"]["properties"]["name"]["type"],
            "string"
        );
    }
}
