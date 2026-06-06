#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwaggerUiConfig {
    pub title: String,
    pub spec_url: String,
    pub oauth2_redirect_url: Option<String>,
    pub persist_authorization: bool,
}

impl SwaggerUiConfig {
    pub fn new(title: impl Into<String>, spec_url: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            spec_url: spec_url.into(),
            oauth2_redirect_url: None,
            persist_authorization: true,
        }
    }

    pub fn render_html(&self) -> String {
        let redirect = self
            .oauth2_redirect_url
            .as_ref()
            .map(|url| format!("oauth2RedirectUrl: {:?},", url))
            .unwrap_or_default();
        format!(
            r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <title>{title}</title>
  <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css">
</head>
<body>
  <div id="swagger-ui"></div>
  <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
  <script>
    window.ui = SwaggerUIBundle({{
      url: {spec_url:?},
      dom_id: '#swagger-ui',
      persistAuthorization: {persist},
      {redirect}
    }});
  </script>
</body>
</html>"#,
            title = self.title,
            spec_url = self.spec_url,
            persist = self.persist_authorization,
            redirect = redirect,
        )
    }
}

pub fn swagger_ui_html(title: impl Into<String>, spec_url: impl Into<String>) -> String {
    SwaggerUiConfig::new(title, spec_url).render_html()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_ui_html() {
        let html = swagger_ui_html("Roze API", "/openapi.json");
        assert!(html.contains("swagger-ui"));
        assert!(html.contains("/openapi.json"));
    }
}
