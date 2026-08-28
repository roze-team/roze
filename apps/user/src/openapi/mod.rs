use std::collections::BTreeMap;

use roze_openapi::{HttpMethod, OpenApiBuilder, Operation, Schema};

pub fn document() -> serde_json::Value {
    let mut builder = OpenApiBuilder::new("user", "0.1.0").description("service group: user");
    builder = builder.server("/api", "service: user");
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
    {
        let mut properties = BTreeMap::new();
        properties.insert("report".to_string(), Schema::string());
        properties.insert("format".to_string(), Schema::string());
        properties.insert("columns".to_string(), Schema::array(Schema::string()));
        properties.insert(
            "filters".to_string(),
            Schema::object(BTreeMap::new(), Vec::new()),
        );
        properties.insert("from".to_string(), Schema::string());
        properties.insert("to".to_string(), Schema::string());
        properties.insert("timezone".to_string(), Schema::string());
        builder = builder.component_schema(
            "ReportExportRequest",
            Schema::object(properties, vec!["report".to_string()]),
        );
    }
    {
        let mut properties = BTreeMap::new();
        properties.insert("id".to_string(), Schema::string());
        properties.insert("report".to_string(), Schema::string());
        properties.insert("format".to_string(), Schema::string());
        properties.insert("status".to_string(), Schema::string());
        properties.insert("progress_percent".to_string(), Schema::integer("int32"));
        properties.insert("object_key".to_string(), Schema::string());
        properties.insert("download_url".to_string(), Schema::string());
        properties.insert("expires_at".to_string(), Schema::string());
        properties.insert("error".to_string(), Schema::string());
        properties.insert("tenant_id".to_string(), Schema::string());
        builder = builder.component_schema(
            "ReportExportResource",
            Schema::object(
                properties,
                vec![
                    "id".to_string(),
                    "report".to_string(),
                    "format".to_string(),
                    "status".to_string(),
                    "progress_percent".to_string(),
                    "tenant_id".to_string(),
                ],
            ),
        );
    }
    {
        let mut properties = BTreeMap::new();
        properties.insert("chart".to_string(), Schema::string());
        properties.insert("dimensions".to_string(), Schema::array(Schema::string()));
        properties.insert("measures".to_string(), Schema::array(Schema::string()));
        properties.insert(
            "filters".to_string(),
            Schema::object(BTreeMap::new(), Vec::new()),
        );
        properties.insert("group_by".to_string(), Schema::array(Schema::string()));
        properties.insert("time_bucket".to_string(), Schema::string());
        properties.insert("from".to_string(), Schema::string());
        properties.insert("to".to_string(), Schema::string());
        properties.insert("timezone".to_string(), Schema::string());
        properties.insert("limit".to_string(), Schema::integer("int64"));
        builder = builder.component_schema(
            "ChartQueryRequest",
            Schema::object(properties, vec!["chart".to_string()]),
        );
    }
    {
        let mut properties = BTreeMap::new();
        properties.insert("timestamp".to_string(), Schema::string());
        properties.insert("value".to_string(), Schema::number("double"));
        properties.insert(
            "labels".to_string(),
            Schema::object(BTreeMap::new(), Vec::new()),
        );
        builder = builder.component_schema(
            "ChartPoint",
            Schema::object(
                properties,
                vec![
                    "timestamp".to_string(),
                    "value".to_string(),
                    "labels".to_string(),
                ],
            ),
        );
    }
    {
        let mut properties = BTreeMap::new();
        properties.insert("name".to_string(), Schema::string());
        properties.insert(
            "points".to_string(),
            Schema::array(Schema::reference("ChartPoint")),
        );
        builder = builder.component_schema(
            "ChartSeries",
            Schema::object(properties, vec!["name".to_string(), "points".to_string()]),
        );
    }
    {
        let mut properties = BTreeMap::new();
        properties.insert("chart".to_string(), Schema::string());
        properties.insert("dimensions".to_string(), Schema::array(Schema::string()));
        properties.insert("measures".to_string(), Schema::array(Schema::string()));
        properties.insert("time_bucket".to_string(), Schema::string());
        properties.insert("timezone".to_string(), Schema::string());
        properties.insert("scanned_rows".to_string(), Schema::integer("int64"));
        properties.insert("result_rows".to_string(), Schema::integer("int64"));
        properties.insert(
            "series".to_string(),
            Schema::array(Schema::reference("ChartSeries")),
        );
        builder = builder.component_schema(
            "ChartQueryResponse",
            Schema::object(
                properties,
                vec![
                    "chart".to_string(),
                    "dimensions".to_string(),
                    "measures".to_string(),
                    "scanned_rows".to_string(),
                    "result_rows".to_string(),
                    "series".to_string(),
                ],
            ),
        );
    }
    let op = Operation::new("createReportExport")
        .summary("Create an asynchronous report export")
        .tag("user")
        .request_body("ReportExportRequest")
        .response("200", "Accepted", "ReportExportResource");
    builder.add_operation("/api/reports/exports", HttpMethod::Post, op);
    let op = Operation::new("getReportExport")
        .summary("Get report export status")
        .tag("user")
        .parameter("id", roze_openapi::ParameterLocation::Path, "String", true)
        .response("200", "OK", "ReportExportResource");
    builder.add_operation("/api/reports/exports/{id}", HttpMethod::Get, op);
    let op = Operation::new("cancelReportExport")
        .summary("Cancel a report export")
        .tag("user")
        .parameter("id", roze_openapi::ParameterLocation::Path, "String", true)
        .response("200", "OK", "ReportExportResource");
    builder.add_operation("/api/reports/exports/{id}", HttpMethod::Delete, op);
    let op = Operation::new("chartQuery")
        .summary("Run a bounded chart query")
        .tag("user")
        .request_body("ChartQueryRequest")
        .response("200", "OK", "ChartQueryResponse");
    builder.add_operation("/api/charts/query", HttpMethod::Post, op);
    let op = Operation::new("post_user_login")
        .tag("user")
        .parameter(
            "x-roze-locale",
            roze_openapi::ParameterLocation::Header,
            "String",
            false,
        )
        .request_body("LoginReq")
        .response_with_schema("200", "OK", "application/json", {
            let mut properties = BTreeMap::new();
            properties.insert("code".to_string(), Schema::integer("int32"));
            properties.insert("msg".to_string(), Schema::string());
            properties.insert("data".to_string(), Schema::reference("LoginResp"));
            Schema::object(
                properties,
                vec!["code".to_string(), "msg".to_string(), "data".to_string()],
            )
        });
    builder.add_operation("/api/user/login", HttpMethod::Post, op);
    roze_openapi::to_json_value(&builder.finish())
}
