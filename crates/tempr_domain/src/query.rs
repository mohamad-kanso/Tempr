use crate::ids::{ConnectionId, QueryId, QueryRunId, SqlFileId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    pub id: QueryId,
    pub text: String,
    pub source_file: Option<SqlFileId>,
    pub offset_start: usize,
    pub offset_end: usize,
    pub fingerprint: [u8; 32],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryRun {
    pub id: QueryRunId,
    pub query: Query,
    pub connection_id: ConnectionId,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    pub outcome: QueryOutcome,
    pub result_set: Option<ResultSet>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueryOutcome {
    Success,
    Error(String),
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultSet {
    pub columns: Vec<ColumnMeta>,
    pub rows: Vec<Vec<Value>>,
    pub total_rows: usize,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnMeta {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub ordinal: usize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    Null,
    Bool(bool),
    Int8(i64),
    Float8(f64),
    Text(String),
    Bytes(Vec<u8>),
    Uuid(uuid::Uuid),
    Json(serde_json::Value),
    Timestamp(chrono::DateTime<chrono::Utc>),
    Date(chrono::NaiveDate),
    Time(chrono::NaiveTime),
    Numeric(String),
    Array(Vec<Value>),
    Custom {
        type_name: String,
        raw_bytes: Vec<u8>,
    },
}

/// Discriminant-level type tag for `Value` — used for column-level dispatch
/// (sort, render, completion) without matching on every cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueType {
    Null,
    Bool,
    Int,
    Float,
    String,
    Bytes,
    Uuid,
    Json,
    Timestamp,
    Date,
    Time,
    Numeric,
    Array,
    Custom,
}

impl Value {
    pub fn value_type(&self) -> ValueType {
        match self {
            Value::Null => ValueType::Null,
            Value::Bool(_) => ValueType::Bool,
            Value::Int8(_) => ValueType::Int,
            Value::Float8(_) => ValueType::Float,
            Value::Text(_) => ValueType::String,
            Value::Bytes(_) => ValueType::Bytes,
            Value::Uuid(_) => ValueType::Uuid,
            Value::Json(_) => ValueType::Json,
            Value::Timestamp(_) => ValueType::Timestamp,
            Value::Date(_) => ValueType::Date,
            Value::Time(_) => ValueType::Time,
            Value::Numeric(_) => ValueType::Numeric,
            Value::Array(_) => ValueType::Array,
            Value::Custom { .. } => ValueType::Custom,
        }
    }
}

/// Column metadata delivered alongside a `QueryStream` — available
/// immediately after `execute()` returns, before any rows are fetched.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSpec {
    pub name: String,
    pub ordinal: usize,
    pub data_type: String,
    pub value_type: ValueType,
    pub nullable: bool,
    pub table_schema: Option<String>,
    pub table_name: Option<String>,
}

/// A bounded batch of rows from a streaming query result.
#[derive(Debug, Clone)]
pub struct Batch {
    pub rows: Vec<Vec<Value>>,
    pub batch_index: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{ConnectionId, QueryId, QueryRunId};

    fn make_query(text: &str) -> Query {
        Query {
            id: QueryId::new(),
            text: text.to_string(),
            source_file: None,
            offset_start: 0,
            offset_end: text.len(),
            fingerprint: [0u8; 32],
        }
    }

    #[test]
    fn query_outcome_serde_roundtrip() {
        let cases = [
            QueryOutcome::Success,
            QueryOutcome::Error("syntax error".to_string()),
            QueryOutcome::Cancelled,
        ];
        for outcome in cases {
            let json = serde_json::to_string(&outcome).expect("serialize");
            let back: QueryOutcome = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(outcome, back);
        }
    }

    #[test]
    fn result_set_row_values() {
        let rs = ResultSet {
            columns: vec![
                ColumnMeta {
                    name: "id".into(),
                    data_type: "int8".into(),
                    nullable: false,
                    ordinal: 0,
                },
                ColumnMeta {
                    name: "name".into(),
                    data_type: "text".into(),
                    nullable: true,
                    ordinal: 1,
                },
            ],
            rows: vec![
                vec![Value::Int8(1), Value::Text("Alice".into())],
                vec![Value::Int8(2), Value::Null],
            ],
            total_rows: 2,
            truncated: false,
        };
        assert_eq!(rs.rows.len(), 2);
        assert_eq!(rs.columns.len(), 2);
    }

    #[test]
    fn query_run_serde_roundtrip() {
        let run = QueryRun {
            id: QueryRunId::new(),
            query: make_query("SELECT 1"),
            connection_id: ConnectionId::new(),
            started_at: Utc::now(),
            finished_at: None,
            outcome: QueryOutcome::Success,
            result_set: None,
        };
        let json = serde_json::to_string(&run).expect("serialize");
        let back: QueryRun = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(run.id, back.id);
        assert_eq!(back.outcome, QueryOutcome::Success);
    }

    #[test]
    fn value_variants_serde_roundtrip() {
        use chrono::NaiveDate;
        let values = vec![
            Value::Null,
            Value::Bool(true),
            Value::Int8(-42),
            Value::Float8(1.5),
            Value::Text("hello".to_string()),
            Value::Bytes(vec![0, 1, 2]),
            Value::Uuid(uuid::Uuid::new_v4()),
            Value::Json(serde_json::json!({"key": "value"})),
            Value::Timestamp(chrono::Utc::now()),
            Value::Date(NaiveDate::from_ymd_opt(2025, 1, 15).unwrap()),
            Value::Time(chrono::NaiveTime::from_hms_opt(10, 30, 0).unwrap()),
            Value::Numeric("123.45".to_string()),
            Value::Array(vec![Value::Int8(1), Value::Int8(2)]),
            Value::Custom {
                type_name: "point".to_string(),
                raw_bytes: vec![1, 2, 3],
            },
        ];
        let json = serde_json::to_string(&values).expect("serialize");
        let back: Vec<Value> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.len(), values.len());
    }

    #[test]
    fn value_type_discriminant() {
        assert_eq!(Value::Null.value_type(), ValueType::Null);
        assert_eq!(Value::Bool(false).value_type(), ValueType::Bool);
        assert_eq!(Value::Int8(0).value_type(), ValueType::Int);
        assert_eq!(Value::Float8(0.0).value_type(), ValueType::Float);
        assert_eq!(Value::Text(String::new()).value_type(), ValueType::String);
        assert_eq!(Value::Bytes(vec![]).value_type(), ValueType::Bytes);
        assert_eq!(Value::Uuid(uuid::Uuid::nil()).value_type(), ValueType::Uuid);
        assert_eq!(
            Value::Json(serde_json::Value::Null).value_type(),
            ValueType::Json
        );
    }
}
