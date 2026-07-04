use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{bail, Context};

use crate::parser::{
    ApiSpec, Field, FieldSource, InfoPair, RestRoute, RpcMethod, ServerSpec, TypeDef,
};

use super::{read_api_source, GenerateMode, GenerateOptions};

#[derive(Debug, Clone)]
pub struct DockerOptions {
    pub out: PathBuf,
    pub builder_image: String,
    pub base_image: String,
    pub port: u16,
    pub timezone: String,
    pub binary: String,
}

#[derive(Debug, Clone)]
pub struct KubeDeployOptions {
    pub name: String,
    pub image: String,
    pub namespace: String,
    pub replicas: u32,
    pub port: u16,
    pub cpu_request: String,
    pub cpu_limit: String,
    pub memory_request: String,
    pub memory_limit: String,
    pub min_replicas: u32,
    pub max_replicas: u32,
    pub target_cpu: u32,
    pub env: Vec<String>,
    pub env_file: Option<PathBuf>,
    pub config_map: Option<String>,
    pub min_available: String,
    pub out: PathBuf,
}

#[derive(Debug, Clone)]
pub struct HelmOptions {
    pub deploy: KubeDeployOptions,
    pub chart_version: String,
    pub app_version: String,
}

pub fn write_api_markdown_doc(api: Option<&Path>, dir: &Path, out: &Path) -> anyhow::Result<()> {
    let api_path = match api {
        Some(api) => api.to_path_buf(),
        None => find_first_api_file(dir)?,
    };
    let source = read_api_source(&api_path)?;
    let spec = crate::parser::parse_api(&source)?;
    fs::create_dir_all(out).with_context(|| format!("failed to create {}", out.display()))?;
    fs::write(out.join("api.md"), render_markdown_doc(&spec))
        .with_context(|| format!("failed to write {}", out.join("api.md").display()))
}

pub fn run_api_plugin(plugin: &str, api: &Path, dir: &Path) -> anyhow::Result<()> {
    let source = read_api_source(api)?;
    let spec = crate::parser::parse_api(&source)?;
    fs::create_dir_all(dir).with_context(|| format!("failed to create {}", dir.display()))?;
    let payload = api_spec_json(&spec);
    let payload_text = serde_json::to_string_pretty(&payload)?;

    let mut command = if cfg!(target_os = "windows") {
        let mut command = Command::new("cmd");
        command.args(["/C", plugin]);
        command
    } else {
        let mut command = Command::new("sh");
        command.args(["-c", plugin]);
        command
    };
    let mut child = command
        .current_dir(dir)
        .env("ROZECTL_API_SPEC_JSON", &payload_text)
        .env("ROZECTL_API_FILE", api)
        .env("ROZECTL_OUT_DIR", dir)
        .stdin(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start plugin `{plugin}`"))?;
    child
        .stdin
        .as_mut()
        .context("plugin stdin was not available")?
        .write_all(payload_text.as_bytes())?;
    let status = child.wait()?;
    if !status.success() {
        bail!("plugin `{plugin}` exited with status {status}");
    }
    Ok(())
}

pub fn write_dockerfile(options: DockerOptions) -> anyhow::Result<()> {
    if let Some(parent) = options
        .out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(&options.out, render_dockerfile(&options))
        .with_context(|| format!("failed to write {}", options.out.display()))
}

pub fn write_kube_deploy(options: KubeDeployOptions) -> anyhow::Result<()> {
    validate_kube_options(&options)?;
    let env_file_entries = match options.env_file.as_deref() {
        Some(path) => Some(read_env_file(path)?),
        None => None,
    };
    if let Some(parent) = options
        .out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        &options.out,
        render_kube_deploy(&options, env_file_entries.as_deref()),
    )
    .with_context(|| format!("failed to write {}", options.out.display()))
}

pub fn write_helm_chart(options: HelmOptions) -> anyhow::Result<()> {
    validate_kube_options(&options.deploy)?;
    let out = &options.deploy.out;
    fs::create_dir_all(out.join("templates"))
        .with_context(|| format!("failed to create {}", out.join("templates").display()))?;
    fs::write(out.join("Chart.yaml"), render_helm_chart_yaml(&options))
        .with_context(|| format!("failed to write {}", out.join("Chart.yaml").display()))?;
    fs::write(
        out.join("values.yaml"),
        render_helm_values_yaml(&options.deploy),
    )
    .with_context(|| format!("failed to write {}", out.join("values.yaml").display()))?;
    fs::write(
        out.join("templates/deployment.yaml"),
        render_helm_deployment(),
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            out.join("templates/deployment.yaml").display()
        )
    })?;
    fs::write(out.join("templates/service.yaml"), render_helm_service()).with_context(|| {
        format!(
            "failed to write {}",
            out.join("templates/service.yaml").display()
        )
    })?;
    fs::write(out.join("templates/hpa.yaml"), render_helm_hpa()).with_context(|| {
        format!(
            "failed to write {}",
            out.join("templates/hpa.yaml").display()
        )
    })?;
    fs::write(
        out.join("templates/serviceaccount.yaml"),
        render_helm_service_account(),
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            out.join("templates/serviceaccount.yaml").display()
        )
    })?;
    fs::write(out.join("templates/pdb.yaml"), render_helm_pdb()).with_context(|| {
        format!(
            "failed to write {}",
            out.join("templates/pdb.yaml").display()
        )
    })?;
    fs::write(
        out.join("templates/networkpolicy.yaml"),
        render_helm_network_policy(),
    )
    .with_context(|| {
        format!(
            "failed to write {}",
            out.join("templates/networkpolicy.yaml").display()
        )
    })?;
    fs::write(out.join("templates/_helpers.tpl"), render_helm_helpers()).with_context(|| {
        format!(
            "failed to write {}",
            out.join("templates/_helpers.tpl").display()
        )
    })?;
    Ok(())
}

pub fn generate_rpc_from_proto(
    proto: &Path,
    out: &Path,
    options: GenerateOptions,
) -> anyhow::Result<()> {
    let source =
        fs::read_to_string(proto).with_context(|| format!("failed to read {}", proto.display()))?;
    let spec = parse_proto_api_spec(&source)?;
    if matches!(options.mode, GenerateMode::Force) {
        super::cleanup_rpc_project(out)?;
    }
    super::generate_rpc_project(&spec, out, options)?;
    fs::create_dir_all(out.join("proto"))?;
    fs::write(out.join("proto/source.proto"), source).with_context(|| {
        format!(
            "failed to write {}",
            out.join("proto/source.proto").display()
        )
    })
}

fn render_dockerfile(options: &DockerOptions) -> String {
    let binary = &options.binary;
    format!(
        r#"# Generated by rozectl.
FROM {builder} AS builder
WORKDIR /app
COPY . .
RUN cargo build --release --bin {binary}

FROM {base}
LABEL org.opencontainers.image.title="{binary}" \
      org.opencontainers.image.source="roze" \
      org.opencontainers.image.description="Roze service image generated by rozectl"
ENV TZ={timezone}
WORKDIR /app
RUN groupadd --system roze \
    && useradd --system --gid roze --home-dir /app --shell /usr/sbin/nologin roze \
    && chown -R roze:roze /app
COPY --from=builder --chown=roze:roze /app/target/release/{binary} /usr/local/bin/{binary}
COPY --chown=roze:roze config.yaml ./config.yaml
EXPOSE {port}
USER roze:roze
CMD ["/usr/local/bin/{binary}"]
"#,
        builder = options.builder_image,
        base = options.base_image,
        timezone = options.timezone,
        binary = binary,
        port = options.port
    )
}

fn render_kube_deploy(
    options: &KubeDeployOptions,
    env_file_entries: Option<&[(String, String)]>,
) -> String {
    let env = render_kube_env(options);
    let env_from = render_kube_env_from(options);
    let service_account = render_kube_service_account(options);
    let service_account_name = render_kube_service_account_name(options);
    let pdb = render_kube_pdb(options);
    let network_policy = render_kube_network_policy(options);
    let env_config_map = env_file_entries
        .map(|entries| render_kube_config_map(&format!("{}-env", options.name), options, entries))
        .unwrap_or_default();
    format!(
        r#"{env_config_map}{service_account}apiVersion: apps/v1
kind: Deployment
metadata:
  name: {name}
  namespace: {namespace}
spec:
  replicas: {replicas}
  selector:
    matchLabels:
      app: {name}
  template:
    metadata:
      labels:
        app: {name}
    spec:
{service_account_name}      terminationGracePeriodSeconds: 30
      containers:
      - name: {name}
        image: {image}
        ports:
        - containerPort: {port}
{env}{env_from}        resources:
          requests:
            cpu: {cpu_request}
            memory: {memory_request}
          limits:
            cpu: {cpu_limit}
            memory: {memory_limit}
        livenessProbe:
          httpGet:
            path: /healthz
            port: {port}
          initialDelaySeconds: 10
          periodSeconds: 10
          timeoutSeconds: 2
          failureThreshold: 3
        readinessProbe:
          httpGet:
            path: /readyz
            port: {port}
          initialDelaySeconds: 5
          periodSeconds: 5
          timeoutSeconds: 2
          failureThreshold: 3
        startupProbe:
          httpGet:
            path: /startupz
            port: {port}
          periodSeconds: 5
          timeoutSeconds: 2
          failureThreshold: 12
---
apiVersion: v1
kind: Service
metadata:
  name: {name}
  namespace: {namespace}
spec:
  selector:
    app: {name}
  ports:
  - port: {port}
    targetPort: {port}
---
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: {name}
  namespace: {namespace}
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: {name}
  minReplicas: {min_replicas}
  maxReplicas: {max_replicas}
  metrics:
  - type: Resource
    resource:
      name: cpu
      target:
        type: Utilization
        averageUtilization: {target_cpu}
{pdb}{network_policy}
"#,
        name = options.name,
        namespace = options.namespace,
        replicas = options.replicas,
        image = options.image,
        port = options.port,
        env_config_map = env_config_map,
        service_account = service_account,
        service_account_name = service_account_name,
        env = env,
        env_from = env_from,
        cpu_request = options.cpu_request,
        memory_request = options.memory_request,
        cpu_limit = options.cpu_limit,
        memory_limit = options.memory_limit,
        min_replicas = options.min_replicas,
        max_replicas = options.max_replicas,
        target_cpu = options.target_cpu,
        pdb = pdb,
        network_policy = network_policy
    )
}

fn render_helm_chart_yaml(options: &HelmOptions) -> String {
    format!(
        r#"apiVersion: v2
name: {name}
description: Roze service chart for {name}
type: application
version: {chart_version}
appVersion: {app_version:?}
"#,
        name = options.deploy.name,
        chart_version = options.chart_version,
        app_version = options.app_version
    )
}

fn render_helm_values_yaml(options: &KubeDeployOptions) -> String {
    let env = options
        .env
        .iter()
        .filter_map(|entry| entry.split_once('='))
        .map(|(name, value)| format!("  {name}: {value:?}\n"))
        .collect::<String>();
    let env_from = options
        .config_map
        .as_deref()
        .map(|name| format!("  - configMapRef:\n      name: {name}\n"))
        .unwrap_or_default();
    format!(
        r#"replicaCount: {replicas}

image:
  repository: {image_repository:?}
  tag: {image_tag:?}
  pullPolicy: IfNotPresent

service:
  type: ClusterIP
  port: {port}

resources:
  requests:
    cpu: {cpu_request}
    memory: {memory_request}
  limits:
    cpu: {cpu_limit}
    memory: {memory_limit}

autoscaling:
  enabled: true
  minReplicas: {min_replicas}
  maxReplicas: {max_replicas}
  targetCPUUtilizationPercentage: {target_cpu}

serviceAccount:
  name: ""

podDisruptionBudget:
  minAvailable: {min_available}

probes:
  liveness:
    path: /healthz
  readiness:
    path: /readyz
  startup:
    path: /startupz

env:
{env}envFrom:
{env_from}"#,
        replicas = options.replicas,
        image_repository = helm_image_repository(&options.image),
        image_tag = helm_image_tag(&options.image),
        port = options.port,
        cpu_request = options.cpu_request,
        memory_request = options.memory_request,
        cpu_limit = options.cpu_limit,
        memory_limit = options.memory_limit,
        min_replicas = options.min_replicas,
        max_replicas = options.max_replicas,
        target_cpu = options.target_cpu,
        min_available = options.min_available,
        env = if env.is_empty() {
            "  {}\n".to_string()
        } else {
            env
        },
        env_from = if env_from.is_empty() {
            "  []\n".to_string()
        } else {
            env_from
        }
    )
}

fn render_helm_deployment() -> &'static str {
    r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: {{ include "roze.fullname" . }}
  labels:
    {{- include "roze.labels" . | nindent 4 }}
spec:
  replicas: {{ .Values.replicaCount }}
  selector:
    matchLabels:
      {{- include "roze.selectorLabels" . | nindent 6 }}
  template:
    metadata:
      labels:
        {{- include "roze.selectorLabels" . | nindent 8 }}
    spec:
      serviceAccountName: {{ default (include "roze.fullname" .) .Values.serviceAccount.name }}
      terminationGracePeriodSeconds: 30
      containers:
        - name: {{ .Chart.Name }}
          image: "{{ .Values.image.repository }}:{{ .Values.image.tag }}"
          imagePullPolicy: {{ .Values.image.pullPolicy }}
          ports:
            - containerPort: {{ .Values.service.port }}
          {{- with .Values.env }}
          env:
            {{- range $name, $value := . }}
            - name: {{ $name }}
              value: {{ $value | quote }}
            {{- end }}
          {{- end }}
          {{- with .Values.envFrom }}
          envFrom:
            {{- toYaml . | nindent 12 }}
          {{- end }}
          resources:
            {{- toYaml .Values.resources | nindent 12 }}
          livenessProbe:
            httpGet:
              path: {{ .Values.probes.liveness.path }}
              port: {{ .Values.service.port }}
          readinessProbe:
            httpGet:
              path: {{ .Values.probes.readiness.path }}
              port: {{ .Values.service.port }}
          startupProbe:
            httpGet:
              path: {{ .Values.probes.startup.path }}
              port: {{ .Values.service.port }}
"#
}

fn render_helm_service() -> &'static str {
    r#"apiVersion: v1
kind: Service
metadata:
  name: {{ include "roze.fullname" . }}
  labels:
    {{- include "roze.labels" . | nindent 4 }}
spec:
  type: {{ .Values.service.type }}
  selector:
    {{- include "roze.selectorLabels" . | nindent 4 }}
  ports:
    - port: {{ .Values.service.port }}
      targetPort: {{ .Values.service.port }}
"#
}

fn render_helm_hpa() -> &'static str {
    r#"{{- if .Values.autoscaling.enabled }}
apiVersion: autoscaling/v2
kind: HorizontalPodAutoscaler
metadata:
  name: {{ include "roze.fullname" . }}
  labels:
    {{- include "roze.labels" . | nindent 4 }}
spec:
  scaleTargetRef:
    apiVersion: apps/v1
    kind: Deployment
    name: {{ include "roze.fullname" . }}
  minReplicas: {{ .Values.autoscaling.minReplicas }}
  maxReplicas: {{ .Values.autoscaling.maxReplicas }}
  metrics:
    - type: Resource
      resource:
        name: cpu
        target:
          type: Utilization
          averageUtilization: {{ .Values.autoscaling.targetCPUUtilizationPercentage }}
{{- end }}
"#
}

fn render_helm_service_account() -> &'static str {
    r#"apiVersion: v1
kind: ServiceAccount
metadata:
  name: {{ default (include "roze.fullname" .) .Values.serviceAccount.name }}
  labels:
    {{- include "roze.labels" . | nindent 4 }}
"#
}

fn render_helm_pdb() -> &'static str {
    r#"apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: {{ include "roze.fullname" . }}
  labels:
    {{- include "roze.labels" . | nindent 4 }}
spec:
  minAvailable: {{ .Values.podDisruptionBudget.minAvailable }}
  selector:
    matchLabels:
      {{- include "roze.selectorLabels" . | nindent 6 }}
"#
}

fn render_helm_network_policy() -> &'static str {
    r#"apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: {{ include "roze.fullname" . }}
  labels:
    {{- include "roze.labels" . | nindent 4 }}
spec:
  podSelector:
    matchLabels:
      {{- include "roze.selectorLabels" . | nindent 6 }}
  policyTypes:
    - Ingress
  ingress:
    - from:
        - namespaceSelector: {}
      ports:
        - protocol: TCP
          port: {{ .Values.service.port }}
"#
}

fn render_helm_helpers() -> &'static str {
    r#"{{- define "roze.fullname" -}}
{{- .Chart.Name | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "roze.labels" -}}
helm.sh/chart: {{ .Chart.Name }}-{{ .Chart.Version }}
{{ include "roze.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
{{- end -}}

{{- define "roze.selectorLabels" -}}
app.kubernetes.io/name: {{ .Chart.Name }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}
"#
}

fn helm_image_repository(image: &str) -> String {
    helm_image_parts(image)
        .map(|(repo, _)| repo.to_string())
        .unwrap_or_else(|| image.to_string())
}

fn helm_image_tag(image: &str) -> String {
    helm_image_parts(image)
        .map(|(_, tag)| tag.to_string())
        .unwrap_or_else(|| "latest".to_string())
}

fn helm_image_parts(image: &str) -> Option<(&str, &str)> {
    let (repo, tag) = image.rsplit_once(':')?;
    if tag.contains('/') {
        None
    } else {
        Some((repo, tag))
    }
}

fn validate_kube_options(options: &KubeDeployOptions) -> anyhow::Result<()> {
    if options.min_replicas > options.max_replicas {
        bail!("--min-replicas must be less than or equal to --max-replicas");
    }
    if options.target_cpu == 0 || options.target_cpu > 100 {
        bail!("--target-cpu must be between 1 and 100");
    }
    if options.min_available.trim().is_empty() {
        bail!("--min-available cannot be empty");
    }
    for entry in &options.env {
        let Some((name, _)) = entry.split_once('=') else {
            bail!("--env entries must use KEY=VALUE format: {entry}");
        };
        validate_env_name(name)?;
    }
    Ok(())
}

fn render_kube_service_account(options: &KubeDeployOptions) -> String {
    format!(
        r#"apiVersion: v1
kind: ServiceAccount
metadata:
  name: {name}
  namespace: {namespace}
---
"#,
        name = options.name,
        namespace = options.namespace
    )
}

fn render_kube_service_account_name(options: &KubeDeployOptions) -> String {
    format!("      serviceAccountName: {}\n", options.name)
}

fn render_kube_pdb(options: &KubeDeployOptions) -> String {
    format!(
        r#"---
apiVersion: policy/v1
kind: PodDisruptionBudget
metadata:
  name: {name}
  namespace: {namespace}
spec:
  minAvailable: {min_available}
  selector:
    matchLabels:
      app: {name}
"#,
        name = options.name,
        namespace = options.namespace,
        min_available = options.min_available
    )
}

fn render_kube_network_policy(options: &KubeDeployOptions) -> String {
    format!(
        r#"---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: {name}
  namespace: {namespace}
spec:
  podSelector:
    matchLabels:
      app: {name}
  policyTypes:
  - Ingress
  ingress:
  - from:
    - namespaceSelector: {{}}
    ports:
    - protocol: TCP
      port: {port}
"#,
        name = options.name,
        namespace = options.namespace,
        port = options.port
    )
}

fn read_env_file(path: &Path) -> anyhow::Result<Vec<(String, String)>> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read {}", path.display()))?;
    let mut entries = Vec::new();
    for (idx, raw_line) in content.lines().enumerate() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            bail!("{}:{} must use KEY=VALUE format", path.display(), idx + 1);
        };
        let name = name.trim();
        validate_env_name(name).with_context(|| format!("{}:{}", path.display(), idx + 1))?;
        entries.push((
            name.to_string(),
            unquote_env_value(value.trim()).to_string(),
        ));
    }
    Ok(entries)
}

fn validate_env_name(name: &str) -> anyhow::Result<()> {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        bail!("environment variable name cannot be empty");
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        bail!("invalid environment variable name `{name}`");
    }
    if !chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric()) {
        bail!("invalid environment variable name `{name}`");
    }
    Ok(())
}

fn unquote_env_value(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
        .or_else(|| {
            value
                .strip_prefix('\'')
                .and_then(|value| value.strip_suffix('\''))
        })
        .unwrap_or(value)
}

fn render_kube_config_map(
    name: &str,
    options: &KubeDeployOptions,
    entries: &[(String, String)],
) -> String {
    let data = entries
        .iter()
        .map(|(name, value)| format!("  {name}: {value:?}\n"))
        .collect::<String>();
    format!(
        r#"apiVersion: v1
kind: ConfigMap
metadata:
  name: {name}
  namespace: {namespace}
data:
{data}---
"#,
        name = name,
        namespace = options.namespace,
        data = data
    )
}

fn render_kube_env(options: &KubeDeployOptions) -> String {
    let entries = options
        .env
        .iter()
        .filter_map(|entry| entry.split_once('='))
        .map(|(name, value)| format!("        - name: {name}\n          value: {value:?}\n"))
        .collect::<String>();
    if entries.is_empty() {
        String::new()
    } else {
        format!("        env:\n{entries}")
    }
}

fn render_kube_env_from(options: &KubeDeployOptions) -> String {
    let mut refs = Vec::new();
    if let Some(name) = options.config_map.as_deref() {
        refs.push(name.to_string());
    }
    if options.env_file.is_some() {
        refs.push(format!("{}-env", options.name));
    }
    if refs.is_empty() {
        return String::new();
    }
    let refs = refs
        .into_iter()
        .map(|name| format!("        - configMapRef:\n            name: {name}\n"))
        .collect::<String>();
    format!("        envFrom:\n{refs}")
}

fn render_markdown_doc(spec: &ApiSpec) -> String {
    let mut out = format!("# {} API\n\n", spec.service);
    if !spec.info.is_empty() {
        out.push_str("## Info\n\n");
        for pair in &spec.info {
            out.push_str(&format!("- `{}`: {}\n", pair.key, pair.value));
        }
        out.push('\n');
    }
    out.push_str("## Routes\n\n| Method | Path | Handler | Request | Response | Middleware | JWT |\n| --- | --- | --- | --- | --- | --- | --- |\n");
    for route in &spec.rest_routes {
        let server = route.server.as_ref().or(spec.server.as_ref());
        out.push_str(&format!(
            "| {} | `{}` | `{}` | `{}` | `{}` | `{}` | {} |\n",
            method_name(&route.method),
            route.path,
            route.handler.as_deref().unwrap_or("-"),
            route.request,
            route.response,
            route.middlewares.join(", "),
            if server.and_then(|server| server.jwt.as_ref()).is_some() {
                "yes"
            } else {
                "no"
            }
        ));
    }
    out.push_str("\n## Types\n\n");
    for ty in &spec.types {
        out.push_str(&format!("### {}\n\n| Field | Type | Source | Wire name | Validate |\n| --- | --- | --- | --- | --- |\n", ty.name));
        for field in &ty.fields {
            out.push_str(&format!(
                "| `{}` | `{}` | `{}` | `{}` | `{}` |\n",
                field.name,
                field.ty,
                field_source_name(field.source),
                field
                    .wire_name
                    .as_deref()
                    .or(field.json_name.as_deref())
                    .unwrap_or("-"),
                field.validate.as_deref().unwrap_or("-")
            ));
        }
        out.push('\n');
    }
    out
}

fn find_first_api_file(dir: &Path) -> anyhow::Result<PathBuf> {
    for entry in fs::read_dir(dir).with_context(|| format!("failed to read {}", dir.display()))? {
        let path = entry?.path();
        if path.extension().and_then(|ext| ext.to_str()) == Some("api") {
            return Ok(path);
        }
    }
    bail!(
        "no .api file found in {}; pass --api explicitly",
        dir.display()
    )
}

fn api_spec_json(spec: &ApiSpec) -> serde_json::Value {
    serde_json::json!({
        "service": spec.service,
        "info": spec.info.iter().map(info_pair_json).collect::<Vec<_>>(),
        "server": spec.server.as_ref().map(server_json),
        "types": spec.types.iter().map(type_json).collect::<Vec<_>>(),
        "rest_routes": spec.rest_routes.iter().map(route_json).collect::<Vec<_>>(),
        "rpc_methods": spec.rpc_methods.iter().map(rpc_method_json).collect::<Vec<_>>(),
    })
}

fn info_pair_json(pair: &InfoPair) -> serde_json::Value {
    serde_json::json!({ "key": pair.key, "value": pair.value })
}

fn server_json(server: &ServerSpec) -> serde_json::Value {
    serde_json::json!({
        "prefix": server.prefix,
        "group": server.group,
        "middlewares": server.middlewares,
        "jwt": server.jwt,
    })
}

fn type_json(ty: &TypeDef) -> serde_json::Value {
    serde_json::json!({
        "name": ty.name,
        "fields": ty.fields.iter().map(field_json).collect::<Vec<_>>(),
    })
}

fn field_json(field: &Field) -> serde_json::Value {
    serde_json::json!({
        "name": field.name,
        "ty": field.ty,
        "json_name": field.json_name,
        "source": field_source_name(field.source),
        "wire_name": field.wire_name,
        "validate": field.validate,
    })
}

fn route_json(route: &RestRoute) -> serde_json::Value {
    serde_json::json!({
        "handler": route.handler,
        "doc": route.doc,
        "middlewares": route.middlewares,
        "server": route.server.as_ref().map(server_json),
        "method": method_name(&route.method),
        "path": route.path,
        "request": route.request,
        "response": route.response,
    })
}

fn rpc_method_json(method: &RpcMethod) -> serde_json::Value {
    serde_json::json!({
        "name": method.name,
        "request": method.request,
        "response": method.response,
    })
}

fn field_source_name(source: FieldSource) -> &'static str {
    match source {
        FieldSource::Auto => "auto",
        FieldSource::Json => "json",
        FieldSource::Query => "query",
        FieldSource::Form => "form",
        FieldSource::Path => "path",
        FieldSource::Header => "header",
    }
}

fn method_name(method: &crate::parser::HttpMethod) -> &'static str {
    match method {
        crate::parser::HttpMethod::Get => "GET",
        crate::parser::HttpMethod::Post => "POST",
        crate::parser::HttpMethod::Put => "PUT",
        crate::parser::HttpMethod::Patch => "PATCH",
        crate::parser::HttpMethod::Delete => "DELETE",
    }
}

fn parse_proto_api_spec(source: &str) -> anyhow::Result<ApiSpec> {
    let source = strip_proto_comments(source);
    let service = parse_proto_service_name(&source)
        .or_else(|| parse_proto_package(&source))
        .unwrap_or_else(|| "service".to_string());
    let types = parse_proto_messages(&source)?;
    let rpc_methods = parse_proto_rpcs(&source)?;
    if rpc_methods.is_empty() {
        bail!("proto file must contain at least one rpc method");
    }
    Ok(ApiSpec {
        service,
        server: None,
        info: Vec::new(),
        types,
        rest_routes: Vec::new(),
        rpc_methods,
    })
}

fn strip_proto_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    let mut in_block = false;
    while let Some(ch) = chars.next() {
        if in_block {
            if ch == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block = false;
            } else if ch == '\n' {
                out.push('\n');
            }
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            in_block = true;
            continue;
        }
        if ch == '/' && chars.peek() == Some(&'/') {
            for next in chars.by_ref() {
                if next == '\n' {
                    out.push('\n');
                    break;
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

fn parse_proto_package(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("package ")
            .and_then(|rest| rest.trim_end_matches(';').split('.').next_back())
            .map(|name| name.trim().replace('-', "_"))
    })
}

fn parse_proto_service_name(source: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("service ")
            .and_then(|rest| rest.split_whitespace().next())
            .map(|name| name.trim_end_matches('{').to_string())
            .filter(|name| !name.is_empty())
    })
}

fn parse_proto_messages(source: &str) -> anyhow::Result<Vec<TypeDef>> {
    let mut types = Vec::new();
    let lines = source.lines().collect::<Vec<_>>();
    let mut idx = 0;
    while idx < lines.len() {
        let line = lines[idx].trim();
        if let Some(raw_name) = line
            .strip_prefix("message ")
            .and_then(|rest| rest.split_whitespace().next())
        {
            let name = raw_name.trim_end_matches('{').to_string();
            let inline_body = line
                .split_once('{')
                .and_then(|(_, rest)| rest.split_once('}').map(|(body, _)| body.trim()));
            if let Some(body) = inline_body {
                let mut fields = Vec::new();
                for field_line in body
                    .split(';')
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                {
                    if let Some(field) = parse_proto_field(field_line)? {
                        fields.push(field);
                    }
                }
                types.push(TypeDef { name, fields });
                idx += 1;
                continue;
            }

            idx += 1;
            let mut fields = Vec::new();
            while idx < lines.len() {
                let field_line = lines[idx].trim();
                idx += 1;
                if field_line.starts_with('}') {
                    break;
                }
                if field_line.is_empty() || field_line.starts_with("option ") {
                    continue;
                }
                if let Some(field) = parse_proto_field(field_line)? {
                    fields.push(field);
                }
            }
            types.push(TypeDef { name, fields });
            continue;
        }
        idx += 1;
    }
    Ok(types)
}

fn parse_proto_field(line: &str) -> anyhow::Result<Option<Field>> {
    let line = line.trim_end_matches(';').trim();
    if line.is_empty()
        || line.starts_with("reserved ")
        || line.starts_with("extensions ")
        || line.starts_with("oneof ")
        || line.starts_with("option ")
    {
        return Ok(None);
    }
    let Some((left, _)) = line.split_once('=') else {
        return Ok(None);
    };
    let parts = left.split_whitespace().collect::<Vec<_>>();
    if parts.is_empty() {
        return Ok(None);
    }
    let mut left = left.trim();
    let mut repeated = false;
    if let Some(rest) = left
        .strip_prefix("optional ")
        .or_else(|| left.strip_prefix("required "))
    {
        left = rest.trim_start();
    }
    if let Some(rest) = left.strip_prefix("repeated ") {
        repeated = true;
        left = rest.trim_start();
    }
    let (ty, name) = if let Some(rest) = left.strip_prefix("map<") {
        let Some(end) = rest.find('>') else {
            bail!("invalid proto map field `{line}`");
        };
        let ty = &left[..end + "map<>".len()];
        let name = rest[end + 1..].split_whitespace().next();
        let Some(name) = name else {
            return Ok(None);
        };
        (ty, name)
    } else {
        let parts = left.split_whitespace().collect::<Vec<_>>();
        let (Some(ty), Some(name)) = (parts.first(), parts.get(1)) else {
            return Ok(None);
        };
        (*ty, *name)
    };
    let ty = proto_to_api_type(ty, repeated);
    Ok(Some(Field {
        name: name.trim_end_matches(';').to_string(),
        ty,
        json_name: Some(name.trim_end_matches(';').to_string()),
        source: FieldSource::Auto,
        wire_name: Some(name.trim_end_matches(';').to_string()),
        validate: None,
    }))
}

fn proto_to_api_type(ty: &str, repeated: bool) -> String {
    let ty = ty.trim_start_matches('.');
    let base = if let Some(map) = proto_map_to_api_type(ty) {
        map
    } else {
        match canonical_proto_type(ty) {
            "string" => "string",
            "bool" => "bool",
            "int32" | "sint32" | "fixed32" | "sfixed32" => "i32",
            "int64" | "sint64" | "fixed64" | "sfixed64" => "i64",
            "uint32" => "u32",
            "uint64" => "u64",
            "float" => "f32",
            "double" => "f64",
            "bytes" => "bytes",
            other => other,
        }
        .to_string()
    };
    if repeated {
        format!("[]{base}")
    } else {
        base
    }
}

fn canonical_proto_type(ty: &str) -> &str {
    ty.rsplit('.').next().unwrap_or(ty)
}

fn proto_map_to_api_type(ty: &str) -> Option<String> {
    let inner = ty.strip_prefix("map<")?.strip_suffix('>')?;
    let (key, value) = inner.split_once(',')?;
    Some(format!(
        "map[{}]{}",
        proto_to_api_type(key.trim(), false),
        proto_to_api_type(value.trim(), false)
    ))
}

fn parse_proto_rpcs(source: &str) -> anyhow::Result<Vec<RpcMethod>> {
    let mut methods = Vec::new();
    let normalized = source.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut rest = normalized.as_str();
    while let Some(pos) = rest.find("rpc ") {
        rest = &rest[pos + "rpc ".len()..];
        let Some((name, after_name)) = rest.split_once('(') else {
            break;
        };
        let Some((request, after_request)) = after_name.split_once(')') else {
            break;
        };
        let Some((_, after_returns_keyword)) = after_request.split_once("returns") else {
            break;
        };
        let Some((_, after_returns_open)) = after_returns_keyword.split_once('(') else {
            break;
        };
        let Some((response, after_response)) = after_returns_open.split_once(')') else {
            break;
        };
        methods.push(RpcMethod {
            name: name.trim().to_string(),
            request: normalize_proto_rpc_type(request),
            response: normalize_proto_rpc_type(response),
        });
        rest = after_response
            .split_once(';')
            .map_or(after_response, |(_, tail)| tail);
    }
    Ok(methods)
}

fn normalize_proto_rpc_type(ty: &str) -> String {
    canonical_proto_type(
        ty.trim()
            .trim_start_matches("stream ")
            .trim_start_matches('.'),
    )
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "rozectl-native-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ))
    }

    fn sample_api() -> &'static str {
        r#"
        syntax = "v1"

        info (
          title: "User API"
        )

        @server (
          prefix: /api/v1
          jwt: Auth
        )
        service user-api {
          @handler getUser
          @doc "Get user"
          get /users/:id (GetUserReq) returns (UserResp)
        }

        type GetUserReq {
          id u64 `path:"id" validate:"gte=1"`
        }

        type UserResp {
          name string `json:"name"`
        }
        "#
    }

    #[test]
    fn parses_proto_service_into_api_spec() {
        let spec = parse_proto_api_spec(
            r#"
            syntax = "proto3";
            /* generated API package */
            package company.user;
            service User {
              rpc GetUser (
                .company.user.GetUserReq
              ) returns (
                stream .company.user.UserResp
              );
            }
            message GetUserReq {
              uint64 id = 1;
              repeated string tags = 2;
              optional string trace_id = 3;
              map<string, int32> scores = 4;
            }
            message UserResp {
              string name = 1;
            }
            "#,
        )
        .expect("parse proto");

        assert_eq!(spec.service, "User");
        assert_eq!(spec.rpc_methods[0].name, "GetUser");
        assert_eq!(spec.rpc_methods[0].request, "GetUserReq");
        assert_eq!(spec.rpc_methods[0].response, "UserResp");
        assert_eq!(spec.types[0].fields[0].ty, "u64");
        assert_eq!(spec.types[0].fields[1].ty, "[]string");
        assert_eq!(spec.types[0].fields[2].ty, "string");
        assert_eq!(spec.types[0].fields[3].ty, "map[string]i32");
    }

    #[test]
    fn parses_proto_service_name_instead_of_package_tail() {
        let spec = parse_proto_api_spec(
            r#"
            syntax = "proto3";
            package hula.group;

            service HulaGroup {
              rpc ApplyGroup (ApplyGroupReq) returns (ApplyGroupResp);
            }

            message ApplyGroupReq {
              int64 type = 3;
            }

            message ApplyGroupResp {
              bool ok = 1;
            }
            "#,
        )
        .expect("parse proto");

        assert_eq!(spec.service, "HulaGroup");
        let proto = crate::generator::render_proto(&spec).expect("render proto");
        assert!(proto.contains("package hula_group;"));
        assert!(proto.contains("service HulaGroup {"));
        let rpc = crate::generator::rpc::render_rpc(&spec);
        assert!(rpc.contains("hula_group_server::HulaGroup"));
        assert!(rpc.contains("impl HulaGroup for RpcService"));
        assert!(rpc.contains("r#type: req.r#type"));
    }

    #[test]
    fn parses_inline_empty_proto_messages() {
        let spec = parse_proto_api_spec(
            r#"
            syntax = "proto3";
            package system;

            service SystemService {
              rpc ListPermissions (ListPermissionsRequest) returns (ListPermissionsResponse);
            }

            message ListPermissionsRequest {}
            message ListPermissionsResponse {
              repeated string permissions = 1;
            }
            "#,
        )
        .expect("parse proto");

        let request = spec
            .types
            .iter()
            .find(|ty| ty.name == "ListPermissionsRequest")
            .expect("request type");
        assert!(request.fields.is_empty());

        let proto = crate::generator::render_proto(&spec).expect("render proto");
        assert!(proto.contains("message ListPermissionsRequest {\n}\n"));
        assert!(proto.contains("repeated string permissions = 1;"));
    }

    #[test]
    fn renders_dockerfile() {
        let rendered = render_dockerfile(&DockerOptions {
            out: PathBuf::from("Dockerfile"),
            builder_image: "rust:1-bookworm".to_string(),
            base_image: "debian:bookworm-slim".to_string(),
            port: 8080,
            timezone: "UTC".to_string(),
            binary: "user".to_string(),
        });
        assert!(rendered.contains("FROM rust:1-bookworm AS builder"));
        assert!(rendered.contains("LABEL org.opencontainers.image.title=\"user\""));
        assert!(rendered.contains("useradd --system --gid roze"));
        assert!(rendered.contains("COPY --from=builder --chown=roze:roze"));
        assert!(rendered.contains("EXPOSE 8080"));
        assert!(rendered.contains("USER roze:roze"));
        assert!(rendered.contains("CMD [\"/usr/local/bin/user\"]"));
    }

    #[test]
    fn renders_kubernetes_manifest() {
        let entries = vec![
            (
                "DATABASE_URL".to_string(),
                "postgres://localhost/roze".to_string(),
            ),
            ("FEATURE_FLAG".to_string(), "enabled".to_string()),
        ];
        let rendered = render_kube_deploy(
            &KubeDeployOptions {
                name: "user".to_string(),
                image: "user:latest".to_string(),
                namespace: "default".to_string(),
                replicas: 2,
                port: 3000,
                cpu_request: "100m".to_string(),
                cpu_limit: "500m".to_string(),
                memory_request: "128Mi".to_string(),
                memory_limit: "512Mi".to_string(),
                min_replicas: 1,
                max_replicas: 5,
                target_cpu: 70,
                env: vec!["RUST_LOG=info".to_string()],
                env_file: Some(PathBuf::from(".env")),
                config_map: Some("user-config".to_string()),
                min_available: "1".to_string(),
                out: PathBuf::from("deploy/kubernetes.yaml"),
            },
            Some(&entries),
        );
        assert!(rendered.contains("kind: Deployment"));
        assert!(rendered.contains("kind: Service"));
        assert!(rendered.contains("kind: HorizontalPodAutoscaler"));
        assert!(rendered.contains("kind: ConfigMap"));
        assert!(rendered.contains("livenessProbe:"));
        assert!(rendered.contains("path: /healthz"));
        assert!(rendered.contains("readinessProbe:"));
        assert!(rendered.contains("path: /readyz"));
        assert!(rendered.contains("startupProbe:"));
        assert!(rendered.contains("path: /startupz"));
        assert!(rendered.contains("DATABASE_URL"));
        assert!(rendered.contains("name: RUST_LOG"));
        assert!(rendered.contains("envFrom:"));
        assert!(rendered.contains("name: user-config"));
        assert!(rendered.contains("name: user-env"));
        assert!(rendered.contains("kind: ServiceAccount"));
        assert!(rendered.contains("serviceAccountName: user"));
        assert!(rendered.contains("kind: PodDisruptionBudget"));
        assert!(rendered.contains("minAvailable: 1"));
        assert!(rendered.contains("kind: NetworkPolicy"));
        assert!(rendered.contains("port: 3000"));
    }

    #[test]
    fn renders_and_writes_helm_chart() {
        let root = temp_root("helm");
        let out = root.join("chart");
        let options = HelmOptions {
            deploy: KubeDeployOptions {
                name: "user".to_string(),
                image: "registry.example.com/user:1.2.3".to_string(),
                namespace: "default".to_string(),
                replicas: 2,
                port: 3000,
                cpu_request: "100m".to_string(),
                cpu_limit: "500m".to_string(),
                memory_request: "128Mi".to_string(),
                memory_limit: "512Mi".to_string(),
                min_replicas: 1,
                max_replicas: 5,
                target_cpu: 70,
                env: vec!["RUST_LOG=info".to_string()],
                env_file: None,
                config_map: Some("user-config".to_string()),
                min_available: "1".to_string(),
                out: out.clone(),
            },
            chart_version: "0.1.0".to_string(),
            app_version: "1.2.3".to_string(),
        };

        let values = render_helm_values_yaml(&options.deploy);
        assert!(values.contains(r#"repository: "registry.example.com/user""#));
        assert!(values.contains(r#"tag: "1.2.3""#));
        assert!(values.contains("RUST_LOG: \"info\""));
        assert!(values.contains("name: user-config"));
        assert!(values.contains("serviceAccount:\n  name: \"\""));
        assert!(values.contains("podDisruptionBudget:\n  minAvailable: 1"));

        write_helm_chart(options).expect("write helm chart");
        assert!(fs::read_to_string(out.join("Chart.yaml"))
            .expect("read chart")
            .contains("name: user"));
        assert!(fs::read_to_string(out.join("templates/deployment.yaml"))
            .expect("read deployment")
            .contains("kind: Deployment"));
        assert!(fs::read_to_string(out.join("templates/service.yaml"))
            .expect("read service")
            .contains("kind: Service"));
        assert!(fs::read_to_string(out.join("templates/hpa.yaml"))
            .expect("read hpa")
            .contains("kind: HorizontalPodAutoscaler"));
        assert!(
            fs::read_to_string(out.join("templates/serviceaccount.yaml"))
                .expect("read service account")
                .contains("kind: ServiceAccount")
        );
        assert!(fs::read_to_string(out.join("templates/pdb.yaml"))
            .expect("read pdb")
            .contains("kind: PodDisruptionBudget"));
        assert!(fs::read_to_string(out.join("templates/networkpolicy.yaml"))
            .expect("read network policy")
            .contains("kind: NetworkPolicy"));
        assert!(fs::read_to_string(out.join("templates/_helpers.tpl"))
            .expect("read helpers")
            .contains("roze.fullname"));

        fs::remove_dir_all(root).expect("remove helm temp");
    }

    #[test]
    fn helm_image_parser_allows_registry_ports_without_tags() {
        assert_eq!(
            helm_image_repository("localhost:5000/user-api"),
            "localhost:5000/user-api"
        );
        assert_eq!(helm_image_tag("localhost:5000/user-api"), "latest");
        assert_eq!(
            helm_image_repository("localhost:5000/user-api:1.2.3"),
            "localhost:5000/user-api"
        );
        assert_eq!(helm_image_tag("localhost:5000/user-api:1.2.3"), "1.2.3");
    }

    #[test]
    fn rejects_invalid_kubernetes_env() {
        let err = validate_kube_options(&KubeDeployOptions {
            name: "user".to_string(),
            image: "user:latest".to_string(),
            namespace: "default".to_string(),
            replicas: 2,
            port: 3000,
            cpu_request: "100m".to_string(),
            cpu_limit: "500m".to_string(),
            memory_request: "128Mi".to_string(),
            memory_limit: "512Mi".to_string(),
            min_replicas: 5,
            max_replicas: 1,
            target_cpu: 70,
            env: vec!["bad-entry".to_string()],
            env_file: None,
            config_map: None,
            min_available: "1".to_string(),
            out: PathBuf::from("deploy/kubernetes.yaml"),
        })
        .expect_err("reject invalid kube options");
        assert!(err.to_string().contains("--min-replicas"));
    }

    #[test]
    fn writes_markdown_doc() {
        let root = temp_root("doc");
        fs::create_dir_all(&root).expect("create temp");
        let api = root.join("user.api");
        fs::write(&api, sample_api()).expect("write api");
        let out = root.join("doc");

        write_api_markdown_doc(Some(&api), &root, &out).expect("write docs");

        let doc = fs::read_to_string(out.join("api.md")).expect("read doc");
        assert!(doc.contains("# user-api API"));
        assert!(doc.contains("| GET |"));
        assert!(doc.contains("GetUserReq"));
    }

    #[test]
    fn runs_api_plugin_with_json_payload() {
        let root = temp_root("plugin");
        fs::create_dir_all(&root).expect("create temp");
        let api = root.join("user.api");
        fs::write(&api, sample_api()).expect("write api");
        let out = root.join("out");

        let plugin = if cfg!(target_os = "windows") {
            "more > plugin.json"
        } else {
            "cat > plugin.json"
        };
        run_api_plugin(plugin, &api, &out).expect("run plugin");

        let payload = fs::read_to_string(out.join("plugin.json")).expect("read plugin output");
        assert!(payload.contains("\"service\": \"user-api\""));
        assert!(payload.contains("\"rest_routes\""));
    }

    #[test]
    fn generates_rpc_project_from_real_proto() {
        let root = temp_root("proto");
        fs::create_dir_all(&root).expect("create temp");
        let proto = root.join("user.proto");
        fs::write(
            &proto,
            r#"
            syntax = "proto3";
            package user;
            service User {
              rpc GetUser (GetUserReq) returns (UserResp);
            }
            message GetUserReq {
              uint64 id = 1;
              repeated string tags = 2;
              map<string, int64> scores = 3;
            }
            message UserResp {
              string name = 1;
              repeated string permissions = 2;
              int64 user_count = 3;
            }
            "#,
        )
        .expect("write proto");
        let out = root.join("rpc");

        generate_rpc_from_proto(
            &proto,
            &out,
            GenerateOptions::new(GenerateMode::Create, super::super::DependencySource::Git),
        )
        .expect("generate rpc");

        assert!(out.join("proto/service.proto").is_file());
        assert!(out.join("proto/source.proto").is_file());
        assert!(out.join("src/server/mod.rs").is_file());
        assert!(out.join("src/client/mod.rs").is_file());
        assert!(out.join("src/pb/mod.rs").is_file());
        assert!(out.join("src/logic/get_user.rs").is_file());
        let service_proto =
            fs::read_to_string(out.join("proto/service.proto")).expect("read proto");
        assert!(service_proto.contains("repeated string tags"));
        assert!(service_proto.contains("map<string, int64> scores"));
        assert!(service_proto.contains("repeated string permissions = 2;"));
        let logic = fs::read_to_string(out.join("src/logic/get_user.rs")).expect("read logic");
        assert!(logic.contains("Ok(UserResp::default())"));
        assert!(!logic.contains("Ok(UserResp {"));
    }
}
