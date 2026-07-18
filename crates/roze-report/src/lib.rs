use anyhow::{anyhow, ensure, Result};
use async_trait::async_trait;
use rust_xlsxwriter::Workbook;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::Notify;

#[derive(Debug)]
struct CancellationState {
    cancelled: AtomicBool,
    notify: Notify,
}

#[derive(Debug, Clone)]
pub struct ReportCancellation {
    state: Arc<CancellationState>,
}

impl ReportCancellation {
    pub fn new() -> Self {
        Self {
            state: Arc::new(CancellationState {
                cancelled: AtomicBool::new(false),
                notify: Notify::new(),
            }),
        }
    }

    pub fn cancel(&self) {
        if !self.state.cancelled.swap(true, Ordering::SeqCst) {
            self.state.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.state.cancelled.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        loop {
            let notified = self.state.notify.notified();
            if self.is_cancelled() {
                return;
            }
            notified.await;
        }
    }
}

impl Default for ReportCancellation {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ReportQueryContext {
    pub subject: String,
    pub tenant_id: String,
    pub cancellation: ReportCancellation,
}

impl ReportQueryContext {
    pub fn ensure_active(&self) -> Result<()> {
        ensure!(
            !self.cancellation.is_cancelled(),
            "report query was cancelled"
        );
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReportDataQuery {
    pub report: String,
    #[serde(default)]
    pub columns: Vec<String>,
    #[serde(default)]
    pub filters: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    pub max_rows: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ReportDataset {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<serde_json::Value>>,
    pub scanned_rows: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartDataQuery {
    pub chart: String,
    #[serde(default)]
    pub dimensions: Vec<String>,
    #[serde(default)]
    pub measures: Vec<String>,
    #[serde(default)]
    pub filters: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub group_by: Vec<String>,
    #[serde(default)]
    pub sort: Vec<ChartDataSort>,
    #[serde(default)]
    pub time_bucket: Option<String>,
    #[serde(default)]
    pub from: Option<String>,
    #[serde(default)]
    pub to: Option<String>,
    #[serde(default)]
    pub timezone: Option<String>,
    pub limit: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChartDataSort {
    pub field: String,
    pub direction: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChartDataset {
    pub scanned_rows: u64,
    pub series: Vec<ChartDataSeries>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChartDataSeries {
    pub name: String,
    pub points: Vec<ChartDataPoint>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChartDataPoint {
    pub timestamp: String,
    pub value: f64,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

#[async_trait]
pub trait ReportDataSource: std::fmt::Debug + Send + Sync + 'static {
    async fn export(
        &self,
        context: ReportQueryContext,
        query: ReportDataQuery,
    ) -> Result<ReportDataset>;

    async fn chart(
        &self,
        context: ReportQueryContext,
        query: ChartDataQuery,
    ) -> Result<ChartDataset>;
}

type ExportFuture = Pin<Box<dyn Future<Output = Result<ReportDataset>> + Send + 'static>>;
type ChartFuture = Pin<Box<dyn Future<Output = Result<ChartDataset>> + Send + 'static>>;
type ExportHandler =
    Arc<dyn Fn(ReportQueryContext, ReportDataQuery) -> ExportFuture + Send + Sync + 'static>;
type ChartHandler =
    Arc<dyn Fn(ReportQueryContext, ChartDataQuery) -> ChartFuture + Send + Sync + 'static>;

#[derive(Clone, Default)]
pub struct ReportCatalog {
    exports: BTreeMap<String, ExportHandler>,
    charts: BTreeMap<String, ChartHandler>,
}

impl std::fmt::Debug for ReportCatalog {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReportCatalog")
            .field("exports", &self.exports.keys().collect::<Vec<_>>())
            .field("charts", &self.charts.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl ReportCatalog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_export<F, Fut>(mut self, name: impl Into<String>, handler: F) -> Result<Self>
    where
        F: Fn(ReportQueryContext, ReportDataQuery) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ReportDataset>> + Send + 'static,
    {
        let name = name.into();
        ensure!(!name.trim().is_empty(), "report name must not be empty");
        ensure!(
            !self.exports.contains_key(&name),
            "duplicate report `{name}`"
        );
        self.exports.insert(
            name,
            Arc::new(move |context, query| Box::pin(handler(context, query))),
        );
        Ok(self)
    }

    pub fn register_chart<F, Fut>(mut self, name: impl Into<String>, handler: F) -> Result<Self>
    where
        F: Fn(ReportQueryContext, ChartDataQuery) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ChartDataset>> + Send + 'static,
    {
        let name = name.into();
        ensure!(!name.trim().is_empty(), "chart name must not be empty");
        ensure!(!self.charts.contains_key(&name), "duplicate chart `{name}`");
        self.charts.insert(
            name,
            Arc::new(move |context, query| Box::pin(handler(context, query))),
        );
        Ok(self)
    }
}

#[async_trait]
impl ReportDataSource for ReportCatalog {
    async fn export(
        &self,
        context: ReportQueryContext,
        query: ReportDataQuery,
    ) -> Result<ReportDataset> {
        context.ensure_active()?;
        let handler = self
            .exports
            .get(&query.report)
            .ok_or_else(|| anyhow!("unknown report `{}`", query.report))?
            .clone();
        let result = handler(context.clone(), query).await?;
        context.ensure_active()?;
        Ok(result)
    }

    async fn chart(
        &self,
        context: ReportQueryContext,
        query: ChartDataQuery,
    ) -> Result<ChartDataset> {
        context.ensure_active()?;
        let handler = self
            .charts
            .get(&query.chart)
            .ok_or_else(|| anyhow!("unknown chart `{}`", query.chart))?
            .clone();
        let result = handler(context.clone(), query).await?;
        context.ensure_active()?;
        Ok(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedExport {
    pub bytes: Vec<u8>,
    pub content_type: String,
    pub extension: String,
    pub row_count: u64,
    pub scanned_rows: u64,
}

pub async fn execute_export(
    source: Arc<dyn ReportDataSource>,
    context: ReportQueryContext,
    query: ReportDataQuery,
    format: ExportFormat,
    limits: ExportLimits,
) -> Result<RenderedExport> {
    ensure!(
        query.max_rows > 0,
        "report query row limit must be positive"
    );
    ensure!(
        query.max_rows <= limits.max_rows,
        "report query row limit exceeds export limit"
    );
    let cancellation = context.cancellation.clone();
    let dataset = tokio::select! {
        _ = cancellation.cancelled() => return Err(anyhow!("report export cancelled")),
        result = source.export(context, query) => result?,
    };
    ensure!(
        dataset.rows.len() <= limits.max_rows,
        "report data source exceeded export row limit"
    );
    ensure!(
        dataset.scanned_rows >= dataset.rows.len() as u64,
        "report scanned row count is smaller than result row count"
    );
    let bytes = render_export(format, &dataset.columns, &dataset.rows, limits)?;
    Ok(RenderedExport {
        bytes,
        content_type: format.content_type().to_string(),
        extension: format.extension().to_string(),
        row_count: dataset.rows.len() as u64,
        scanned_rows: dataset.scanned_rows,
    })
}

pub async fn execute_chart(
    source: Arc<dyn ReportDataSource>,
    context: ReportQueryContext,
    query: ChartDataQuery,
    timeout: Duration,
) -> Result<ChartDataset> {
    ensure!(query.limit > 0, "chart query limit must be positive");
    let limit = query.limit;
    let cancellation = context.cancellation.clone();
    let dataset = tokio::select! {
        _ = cancellation.cancelled() => return Err(anyhow!("chart query cancelled")),
        result = tokio::time::timeout(timeout, source.chart(context, query)) => {
            result.map_err(|_| anyhow!("chart query timed out"))??
        }
    };
    let result_rows = dataset
        .series
        .iter()
        .map(|series| series.points.len() as u64)
        .sum::<u64>();
    ensure!(
        result_rows <= limit,
        "report data source exceeded chart result limit"
    );
    ensure!(
        dataset.scanned_rows >= result_rows,
        "chart scanned row count is smaller than result row count"
    );
    Ok(dataset)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Csv,
    Xlsx,
}

impl ExportFormat {
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "csv" => Ok(Self::Csv),
            "xlsx" => Ok(Self::Xlsx),
            _ => Err(anyhow!("report format must be csv or xlsx")),
        }
    }

    pub fn content_type(self) -> &'static str {
        match self {
            Self::Csv => "text/csv; charset=utf-8",
            Self::Xlsx => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Csv => "csv",
            Self::Xlsx => "xlsx",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ExportLimits {
    pub max_columns: usize,
    pub max_rows: usize,
    pub max_file_bytes: usize,
}

impl Default for ExportLimits {
    fn default() -> Self {
        Self {
            max_columns: 128,
            max_rows: 100_000,
            max_file_bytes: 64 * 1024 * 1024,
        }
    }
}

pub fn render_export(
    format: ExportFormat,
    columns: &[String],
    rows: &[Vec<serde_json::Value>],
    limits: ExportLimits,
) -> Result<Vec<u8>> {
    ensure!(
        !columns.is_empty(),
        "report export requires at least one column"
    );
    ensure!(
        columns.len() <= limits.max_columns,
        "report export has too many columns"
    );
    ensure!(
        rows.len() <= limits.max_rows,
        "report export has too many rows"
    );
    ensure!(
        rows.iter().all(|row| row.len() == columns.len()),
        "report row width does not match columns"
    );
    let bytes = match format {
        ExportFormat::Csv => render_csv(columns, rows),
        ExportFormat::Xlsx => render_xlsx(columns, rows)?,
    };
    ensure!(
        bytes.len() <= limits.max_file_bytes,
        "report export exceeds file size limit"
    );
    Ok(bytes)
}

fn render_csv(columns: &[String], rows: &[Vec<serde_json::Value>]) -> Vec<u8> {
    let mut out = String::from("\u{feff}");
    write_csv_row(&mut out, columns.iter().map(String::as_str));
    for row in rows {
        let cells = row.iter().map(value_text).collect::<Vec<_>>();
        write_csv_row(&mut out, cells.iter().map(String::as_str));
    }
    out.into_bytes()
}

fn write_csv_row<'a>(out: &mut String, values: impl IntoIterator<Item = &'a str>) {
    let mut first = true;
    for value in values {
        if !first {
            out.push(',');
        }
        first = false;
        let value = spreadsheet_safe(value);
        out.push('"');
        out.push_str(&value.replace('"', "\"\""));
        out.push('"');
    }
    out.push_str("\r\n");
}

fn render_xlsx(columns: &[String], rows: &[Vec<serde_json::Value>]) -> Result<Vec<u8>> {
    let mut workbook = Workbook::new();
    let worksheet = workbook.add_worksheet();
    for (column, value) in columns.iter().enumerate() {
        worksheet.write_string(0, column as u16, spreadsheet_safe(value))?;
    }
    for (row_index, row) in rows.iter().enumerate() {
        for (column_index, value) in row.iter().enumerate() {
            worksheet.write_string(
                (row_index + 1) as u32,
                column_index as u16,
                spreadsheet_safe(&value_text(value)),
            )?;
        }
    }
    Ok(workbook.save_to_buffer()?)
}

fn value_text(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(value) => value.clone(),
        other => other.to_string(),
    }
}

pub fn spreadsheet_safe(value: &str) -> String {
    let trimmed = value.trim_start();
    if trimmed.starts_with(['=', '+', '-', '@', '\t', '\r', '\n']) {
        format!("'{value}")
    } else {
        value.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::{sqlite::SqlitePoolOptions, Row};

    #[tokio::test]
    async fn cancellation_is_shared_and_wakes_waiters() {
        let cancellation = ReportCancellation::new();
        let waiter = cancellation.clone();
        let task = tokio::spawn(async move {
            waiter.cancelled().await;
            waiter.is_cancelled()
        });

        cancellation.cancel();

        assert!(task.await.expect("join cancellation waiter"));
        assert!(cancellation.is_cancelled());
    }

    #[tokio::test]
    async fn sqlite_catalog_enforces_tenant_queries_and_renders_real_exports() {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("connect sqlite");
        sqlx::query(
            "CREATE TABLE sales (tenant_id TEXT NOT NULL, bucket TEXT NOT NULL, name TEXT NOT NULL, amount REAL NOT NULL)",
        )
        .execute(&pool)
        .await
        .expect("create sales");
        for (tenant, bucket, name, amount) in [
            ("tenant-a", "2026-07-01", "=formula", 10.0_f64),
            ("tenant-a", "2026-07-01", "second", 15.0_f64),
            ("tenant-b", "2026-07-01", "hidden", 999.0_f64),
        ] {
            sqlx::query("INSERT INTO sales (tenant_id, bucket, name, amount) VALUES (?, ?, ?, ?)")
                .bind(tenant)
                .bind(bucket)
                .bind(name)
                .bind(amount)
                .execute(&pool)
                .await
                .expect("insert sale");
        }

        let export_pool = pool.clone();
        let chart_pool = pool.clone();
        let catalog = ReportCatalog::new()
            .register_export("sales", move |context, query| {
                let pool = export_pool.clone();
                async move {
                    context.ensure_active()?;
                    let rows = sqlx::query(
                        "SELECT name, amount FROM sales WHERE tenant_id = ? ORDER BY name LIMIT ?",
                    )
                    .bind(&context.tenant_id)
                    .bind(query.max_rows as i64)
                    .fetch_all(&pool)
                    .await?;
                    Ok(ReportDataset {
                        columns: vec!["name".to_string(), "amount".to_string()],
                        scanned_rows: rows.len() as u64,
                        rows: rows
                            .into_iter()
                            .map(|row| {
                                vec![
                                    serde_json::json!(row.get::<String, _>("name")),
                                    serde_json::json!(row.get::<f64, _>("amount")),
                                ]
                            })
                            .collect(),
                    })
                }
            })
            .expect("register sales report")
            .register_chart("sales-total", move |context, _query| {
                let pool = chart_pool.clone();
                async move {
                    context.ensure_active()?;
                    let rows = sqlx::query(
                        "SELECT bucket, SUM(amount) AS total FROM sales WHERE tenant_id = ? GROUP BY bucket ORDER BY bucket",
                    )
                    .bind(&context.tenant_id)
                    .fetch_all(&pool)
                    .await?;
                    Ok(ChartDataset {
                        scanned_rows: rows.len() as u64,
                        series: vec![ChartDataSeries {
                            name: "sales-total".to_string(),
                            points: rows
                                .into_iter()
                                .map(|row| ChartDataPoint {
                                    timestamp: row.get("bucket"),
                                    value: row.get("total"),
                                    labels: BTreeMap::from([(
                                        "tenant".to_string(),
                                        context.tenant_id.clone(),
                                    )]),
                                })
                                .collect(),
                        }],
                    })
                }
            })
            .expect("register sales chart");
        let context = ReportQueryContext {
            subject: "user-a".to_string(),
            tenant_id: "tenant-a".to_string(),
            cancellation: ReportCancellation::new(),
        };
        let export = execute_export(
            Arc::new(catalog.clone()),
            context.clone(),
            ReportDataQuery {
                report: "sales".to_string(),
                columns: Vec::new(),
                filters: BTreeMap::new(),
                from: None,
                to: None,
                timezone: None,
                max_rows: 100,
            },
            ExportFormat::Csv,
            ExportLimits::default(),
        )
        .await
        .expect("query and render tenant export");
        assert_eq!(export.row_count, 2);
        let csv = String::from_utf8(export.bytes).expect("CSV utf8");
        assert!(csv.contains("'=formula"));
        assert!(!csv.contains("hidden"));

        let chart = execute_chart(
            Arc::new(catalog.clone()),
            context.clone(),
            ChartDataQuery {
                chart: "sales-total".to_string(),
                dimensions: vec!["bucket".to_string()],
                measures: vec!["sum(amount)".to_string()],
                filters: BTreeMap::new(),
                group_by: vec!["bucket".to_string()],
                sort: Vec::new(),
                time_bucket: Some("day".to_string()),
                from: None,
                to: None,
                timezone: Some("UTC".to_string()),
                limit: 100,
            },
            Duration::from_secs(1),
        )
        .await
        .expect("query tenant chart");
        assert_eq!(chart.series[0].points[0].value, 25.0);

        let dataset = catalog
            .export(
                context.clone(),
                ReportDataQuery {
                    report: "sales".to_string(),
                    columns: Vec::new(),
                    filters: BTreeMap::new(),
                    from: None,
                    to: None,
                    timezone: None,
                    max_rows: 100,
                },
            )
            .await
            .expect("query tenant export");
        assert!(dataset.rows.iter().all(|row| row[0] != "hidden"));
        let xlsx = render_export(
            ExportFormat::Xlsx,
            &dataset.columns,
            &dataset.rows,
            ExportLimits::default(),
        )
        .expect("render XLSX");
        assert!(xlsx.starts_with(b"PK"));

        context.cancellation.cancel();
        let error = catalog
            .export(
                context,
                ReportDataQuery {
                    report: "sales".to_string(),
                    columns: Vec::new(),
                    filters: BTreeMap::new(),
                    from: None,
                    to: None,
                    timezone: None,
                    max_rows: 100,
                },
            )
            .await
            .expect_err("cancelled query must fail");
        assert!(error.to_string().contains("cancelled"));
    }

    #[tokio::test]
    async fn executors_cancel_in_flight_work_and_enforce_chart_limits() {
        let catalog = ReportCatalog::new()
            .register_export("slow", |_context, _query| async {
                std::future::pending::<()>().await;
                unreachable!()
            })
            .expect("register slow report")
            .register_chart("oversized", |_context, _query| async {
                Ok(ChartDataset {
                    scanned_rows: 2,
                    series: vec![ChartDataSeries {
                        name: "values".to_string(),
                        points: vec![
                            ChartDataPoint {
                                timestamp: "1".to_string(),
                                value: 1.0,
                                labels: BTreeMap::new(),
                            },
                            ChartDataPoint {
                                timestamp: "2".to_string(),
                                value: 2.0,
                                labels: BTreeMap::new(),
                            },
                        ],
                    }],
                })
            })
            .expect("register oversized chart");
        let cancellation = ReportCancellation::new();
        let cancel_from_test = cancellation.clone();
        let source: Arc<dyn ReportDataSource> = Arc::new(catalog.clone());
        let export = tokio::spawn(execute_export(
            source,
            ReportQueryContext {
                subject: "user".to_string(),
                tenant_id: "tenant".to_string(),
                cancellation,
            },
            ReportDataQuery {
                report: "slow".to_string(),
                columns: Vec::new(),
                filters: BTreeMap::new(),
                from: None,
                to: None,
                timezone: None,
                max_rows: 10,
            },
            ExportFormat::Csv,
            ExportLimits::default(),
        ));
        cancel_from_test.cancel();
        let error = tokio::time::timeout(Duration::from_secs(1), export)
            .await
            .expect("cancelled export must wake")
            .expect("join export")
            .expect_err("cancelled export must fail");
        assert!(error.to_string().contains("cancelled"));

        let error = execute_chart(
            Arc::new(catalog),
            ReportQueryContext {
                subject: "user".to_string(),
                tenant_id: "tenant".to_string(),
                cancellation: ReportCancellation::new(),
            },
            ChartDataQuery {
                chart: "oversized".to_string(),
                dimensions: Vec::new(),
                measures: Vec::new(),
                filters: BTreeMap::new(),
                group_by: Vec::new(),
                sort: Vec::new(),
                time_bucket: None,
                from: None,
                to: None,
                timezone: None,
                limit: 1,
            },
            Duration::from_secs(1),
        )
        .await
        .expect_err("oversized chart result must fail");
        assert!(error.to_string().contains("result limit"));
    }

    #[test]
    fn catalog_rejects_empty_and_duplicate_names() {
        assert!(ReportCatalog::new()
            .register_export("", |_context, _query| async {
                unreachable!("empty report handler must not run")
            })
            .is_err());
        let catalog = ReportCatalog::new()
            .register_chart("sales", |_context, _query| async {
                Ok(ChartDataset {
                    scanned_rows: 0,
                    series: Vec::new(),
                })
            })
            .expect("register chart");
        assert!(catalog
            .register_chart("sales", |_context, _query| async {
                unreachable!("duplicate chart handler must not run")
            })
            .is_err());
    }

    #[test]
    fn csv_and_xlsx_are_real_bounded_formats_and_escape_formulas() {
        let columns = vec!["name".to_string(), "value".to_string()];
        let rows = vec![vec![
            serde_json::json!("=cmd|' /C calc'!A0"),
            serde_json::json!(7),
        ]];
        let csv = render_export(ExportFormat::Csv, &columns, &rows, ExportLimits::default())
            .expect("csv");
        assert!(String::from_utf8(csv).expect("utf8").contains("'=cmd"));
        let xlsx = render_export(ExportFormat::Xlsx, &columns, &rows, ExportLimits::default())
            .expect("xlsx");
        assert!(xlsx.starts_with(b"PK"));
    }
}
