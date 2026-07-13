use crate::ids::{HistoryEntryId, QueryRunId, SqlFileId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub id: HistoryEntryId,
    pub query_run_id: QueryRunId,
    pub source_file: Option<SqlFileId>,
    pub timestamp: DateTime<Utc>,
    pub duration_ms: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::{HistoryEntryId, QueryRunId};

    #[test]
    fn history_entry_serde_roundtrip() {
        let entry = HistoryEntry {
            id: HistoryEntryId::new(),
            query_run_id: QueryRunId::new(),
            source_file: None,
            timestamp: Utc::now(),
            duration_ms: 42,
        };
        let json = serde_json::to_string(&entry).expect("serialize");
        let back: HistoryEntry = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(entry.id, back.id);
        assert_eq!(entry.duration_ms, back.duration_ms);
    }
}
