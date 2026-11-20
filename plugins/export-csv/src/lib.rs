//! `export-csv` — the reference plugin (docs/08-plugin-api.md, phase 4 spec
//! §10.2).
//!
//! It depends on `tempr_plugin` and nothing else: no `tempr_services`, no
//! `tempr_ui`, no database driver. That is the proof the API is sufficient —
//! a real command with real output, written from outside the host crates.
//!
//! The plugin reads the last completed run from the [`CommandContext`] the
//! window hands it, writes `<workspace>/last-run.csv`, and reports through the
//! [`CommandOutcome`] the host puts in its status bar. It never resolves a
//! service, never touches storage and never blocks a thread other than the UI
//! frame that invoked it.

#![deny(unsafe_code)]
#![cfg_attr(test, allow(clippy::expect_used, clippy::unwrap_used))]

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use tempr_plugin::reexport::gpui::{App, Window};
use tempr_plugin::reexport::tempr_domain::{ResultSet, Value};
use tempr_plugin::{
    Capability, CommandContext, CommandOutcome, Plugin, PluginCommandSpec, PluginContext,
    PluginError, PluginManifest,
};

/// Command id, namespaced with the plugin's own id.
pub const EXPORT_COMMAND: &str = "dev.tempr.export-csv::export_last_run";
/// File written inside the workspace directory.
pub const EXPORT_FILE: &str = "last-run.csv";
/// Default keystroke: no mouse-only features, ever.
pub const EXPORT_KEYSTROKE: &str = "ctrl-alt-e";

pub struct ExportCsvPlugin {
    manifest: PluginManifest,
}

impl Default for ExportCsvPlugin {
    fn default() -> Self {
        Self {
            manifest: PluginManifest {
                id: "dev.tempr.export-csv",
                name: "CSV export",
                version: env!("CARGO_PKG_VERSION"),
                api_version: tempr_plugin::PLUGIN_API_VERSION,
                requires: &[],
                capabilities: &[Capability::CommandContribution],
            },
        }
    }
}

impl Plugin for ExportCsvPlugin {
    fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    fn activate(&mut self, cx: &mut PluginContext<'_>) -> Result<(), PluginError> {
        cx.register_command(PluginCommandSpec::new(
            tempr_plugin::reexport::tempr_domain::CommandId::new(EXPORT_COMMAND),
            "Export last run as CSV",
            "Plugin",
            &[EXPORT_KEYSTROKE],
            dispatch_export,
        ))
    }
}

/// The command entry point. The window and app are part of the dispatch
/// contract; this command needs neither, so it forwards to
/// [`export_last_run`], which tests call directly.
fn dispatch_export(
    _window: &mut Window,
    _cx: &mut App,
    context: &CommandContext,
) -> CommandOutcome {
    export_last_run(context)
}

/// The command body: read the host-supplied context, write the file, report.
pub fn export_last_run(context: &CommandContext) -> CommandOutcome {
    let Some(run) = &context.last_run else {
        return CommandOutcome::failed("No completed query to export");
    };
    let Some(result) = &run.result_set else {
        return CommandOutcome::failed("The last run returned no result set");
    };
    let Some(root) = &context.workspace_root else {
        return CommandOutcome::failed("No workspace directory; cannot export");
    };

    match write_result_set(root, result) {
        Ok(rows) => CommandOutcome::done(format!("Exported {rows} rows to {EXPORT_FILE}")),
        Err(error) => CommandOutcome::failed(format!("Export failed: {error}")),
    }
}

/// Write `result` to `<root>/last-run.csv`, returning the row count written.
pub fn write_result_set(root: &Path, result: &ResultSet) -> std::io::Result<usize> {
    std::fs::create_dir_all(root)?;
    let path: PathBuf = root.join(EXPORT_FILE);
    let file = File::create(path)?;
    let mut out = BufWriter::new(file);

    let header: Vec<String> = result.columns.iter().map(|c| c.name.clone()).collect();
    write_record(&mut out, &header)?;
    let mut written = 0;
    for row in &result.rows {
        let fields: Vec<String> = row.iter().map(cell_text).collect();
        write_record(&mut out, &fields)?;
        written += 1;
    }
    out.flush()?;
    Ok(written)
}

/// One RFC 4180 record: fields joined by commas, CRLF terminated.
fn write_record(out: &mut impl Write, fields: &[String]) -> std::io::Result<()> {
    for (index, field) in fields.iter().enumerate() {
        if index > 0 {
            out.write_all(b",")?;
        }
        out.write_all(quote_field(field).as_bytes())?;
    }
    out.write_all(b"\r\n")
}

/// RFC 4180 quoting: wrap when the field holds a comma, quote, CR or LF, and
/// double every embedded quote.
pub fn quote_field(field: &str) -> String {
    let needs_quotes = field.chars().any(|c| matches!(c, ',' | '"' | '\r' | '\n'));
    if !needs_quotes {
        return field.to_string();
    }
    let mut quoted = String::with_capacity(field.len() + 2);
    quoted.push('"');
    for c in field.chars() {
        if c == '"' {
            quoted.push('"');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

/// Cell text for CSV. Deliberately not the grid's formatter: a CSV must carry
/// full values, so bytes are written whole (no "… (N bytes)") and NULL is an
/// empty field rather than the grid's `∅` placeholder.
fn cell_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Int8(i) => i.to_string(),
        Value::Float8(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        Value::Bytes(b) => {
            let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
            format!("\\x{hex}")
        }
        Value::Uuid(u) => u.to_string(),
        Value::Json(j) => j.to_string(),
        Value::Timestamp(t) => t.to_rfc3339(),
        Value::Date(d) => d.to_string(),
        Value::Time(t) => t.to_string(),
        Value::Numeric(n) => n.clone(),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(cell_text).collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Custom {
            type_name,
            raw_bytes,
        } => format!("<{type_name}:{} bytes>", raw_bytes.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempr_plugin::reexport::tempr_domain::{
        ColumnMeta, ConnectionId, Query, QueryId, QueryOutcome, QueryRun, QueryRunId, Value,
    };

    fn column(name: &str, ordinal: usize) -> ColumnMeta {
        ColumnMeta {
            name: name.to_string(),
            data_type: "text".to_string(),
            nullable: true,
            ordinal,
        }
    }

    fn result(columns: &[&str], rows: Vec<Vec<Value>>) -> ResultSet {
        let total_rows = rows.len();
        ResultSet {
            columns: columns
                .iter()
                .enumerate()
                .map(|(ix, name)| column(name, ix))
                .collect(),
            rows,
            total_rows,
            truncated: false,
        }
    }

    fn run_with(result: Option<ResultSet>) -> Arc<QueryRun> {
        Arc::new(QueryRun {
            id: QueryRunId::new(),
            query: Query {
                id: QueryId::new(),
                text: "select 1".to_string(),
                source_file: None,
                offset_start: 0,
                offset_end: 8,
                fingerprint: [0u8; 32],
            },
            connection_id: ConnectionId::new(),
            started_at: chrono::Utc::now(),
            finished_at: Some(chrono::Utc::now()),
            outcome: QueryOutcome::Success,
            result_set: result,
        })
    }

    fn context(root: &Path, run: Option<Arc<QueryRun>>) -> CommandContext {
        CommandContext {
            last_run: run,
            workspace_root: Some(root.to_path_buf()),
        }
    }

    #[test]
    fn fields_are_quoted_per_rfc_4180() {
        assert_eq!(quote_field("plain"), "plain");
        assert_eq!(quote_field("a,b"), "\"a,b\"");
        assert_eq!(quote_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(quote_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(quote_field("carriage\rreturn"), "\"carriage\rreturn\"");
    }

    #[test]
    fn the_file_carries_a_header_and_crlf_records() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let result = result(
            &["id", "name"],
            vec![
                vec![Value::Int8(1), Value::Text("plain".into())],
                vec![Value::Int8(2), Value::Text("a,b".into())],
            ],
        );

        let rows = write_result_set(dir.path(), &result).expect("write");
        assert_eq!(rows, 2);

        let text = std::fs::read_to_string(dir.path().join(EXPORT_FILE)).expect("read");
        assert_eq!(
            text, "id,name\r\n1,plain\r\n2,\"a,b\"\r\n",
            "RFC 4180 records end in CRLF and only quote when needed"
        );
    }

    #[test]
    fn empty_results_write_a_header_only_file() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let rows = write_result_set(dir.path(), &result(&["only"], Vec::new())).expect("write");
        assert_eq!(rows, 0);
        assert_eq!(
            std::fs::read_to_string(dir.path().join(EXPORT_FILE)).expect("read"),
            "only\r\n"
        );
    }

    #[test]
    fn a_missing_run_reports_and_writes_nothing() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let outcome = export_last_run(&context(dir.path(), None));
        assert!(outcome.is_failure());
        assert!(outcome.message().contains("No completed query"));
        assert!(!dir.path().join(EXPORT_FILE).exists());
    }

    #[test]
    fn a_missing_result_set_reports_and_writes_nothing() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let outcome = export_last_run(&context(dir.path(), Some(run_with(None))));
        assert!(outcome.is_failure());
        assert!(outcome.message().contains("no result set"));
        assert!(!dir.path().join(EXPORT_FILE).exists());
    }

    #[test]
    fn a_missing_workspace_reports_instead_of_guessing_a_path() {
        let outcome = export_last_run(&CommandContext {
            last_run: Some(run_with(Some(result(&["a"], Vec::new())))),
            workspace_root: None,
        });
        assert!(outcome.is_failure());
        assert!(outcome.message().contains("No workspace directory"));
    }

    #[test]
    fn a_missing_workspace_directory_is_created() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let nested = dir.path().join("exports");
        let outcome = export_last_run(&context(
            &nested,
            Some(run_with(Some(result(&["a"], Vec::new())))),
        ));
        assert!(!outcome.is_failure(), "{}", outcome.message());
        assert!(nested.join(EXPORT_FILE).exists());
    }

    #[test]
    fn null_is_an_empty_field_not_a_placeholder() {
        let dir = tempfile::TempDir::new().expect("temp dir");
        let result = result(&["a"], vec![vec![Value::Null]]);
        write_result_set(dir.path(), &result).expect("write");
        assert_eq!(
            std::fs::read_to_string(dir.path().join(EXPORT_FILE)).expect("read"),
            "a\r\n\r\n"
        );
    }
}
