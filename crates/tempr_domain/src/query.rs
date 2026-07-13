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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Value {
    Null,
    Bool(bool),
    Int8(i64),
    Float8(f64),
    Text(String),
    Bytes(Vec<u8>),
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
        let values = vec![
            Value::Null,
            Value::Bool(true),
            Value::Int8(-42),
            Value::Float8(1.5),
            Value::Text("hello".to_string()),
            Value::Bytes(vec![0, 1, 2]),
        ];
        let json = serde_json::to_string(&values).expect("serialize");
        let back: Vec<Value> = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.len(), values.len());
    }
}
