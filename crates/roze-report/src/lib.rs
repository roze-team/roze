use anyhow::{anyhow, ensure, Result};
use rust_xlsxwriter::Workbook;
use serde::{Deserialize, Serialize};

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
