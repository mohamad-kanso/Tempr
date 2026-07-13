use postgres_types::Type;
use tempr_domain::Value;

/// Map a PostgreSQL type OID and raw text value to a `Value`.
/// PostgreSQL text-format output is assumed (used by `query_raw` and `COPY`).
pub(crate) fn decode_value(pg_type: &Type, raw: Option<&str>) -> Result<Value, String> {
    let text = match raw {
        Some(t) => t,
        None => return Ok(Value::Null),
    };

    match *pg_type {
        Type::BOOL => match text {
            "t" | "true" | "1" => Ok(Value::Bool(true)),
            "f" | "false" | "0" => Ok(Value::Bool(false)),
            other => Err(format!(
                "provided string was not `true` or `false`: {other}"
            )),
        },
        Type::INT2 => text
            .parse::<i16>()
            .map(|v| Value::Int8(v as i64))
            .map_err(|e| e.to_string()),
        Type::INT4 => text
            .parse::<i32>()
            .map(|v| Value::Int8(v as i64))
            .map_err(|e| e.to_string()),
        Type::INT8 => text
            .parse::<i64>()
            .map(Value::Int8)
            .map_err(|e| e.to_string()),
        Type::FLOAT4 => text
            .parse::<f32>()
            .map(|v| Value::Float8(v as f64))
            .map_err(|e| e.to_string()),
        Type::FLOAT8 => text
            .parse::<f64>()
            .map(Value::Float8)
            .map_err(|e| e.to_string()),
        Type::NUMERIC => Ok(Value::Numeric(text.to_string())),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR => Ok(Value::Text(text.to_string())),
        Type::UUID => text
            .parse::<uuid::Uuid>()
            .map(Value::Uuid)
            .map_err(|e| e.to_string()),
        Type::JSON | Type::JSONB => serde_json::from_str(text)
            .map(Value::Json)
            .map_err(|e| e.to_string()),
        Type::TIMESTAMPTZ | Type::TIMESTAMP => {
            // Try RFC3339 first (e.g. "2025-01-15T10:30:00+00:00")
            if let Ok(dt) = text.parse::<chrono::DateTime<chrono::Utc>>() {
                return Ok(Value::Timestamp(dt));
            }
            // Try with timezone offset formats PostgreSQL uses
            for fmt in &["%Y-%m-%d %H:%M:%S%:z", "%Y-%m-%d %H:%M:%S%.f%:z"] {
                if let Ok(dt) = chrono::DateTime::parse_from_str(text, fmt) {
                    return Ok(Value::Timestamp(dt.with_timezone(&chrono::Utc)));
                }
            }
            // PostgreSQL sometimes emits just "+00" without minutes — expand it
            if text.ends_with("+00") || text.ends_with("-00") {
                let expanded = format!("{}:00", text);
                for fmt in &["%Y-%m-%d %H:%M:%S%:z", "%Y-%m-%d %H:%M:%S%.f%:z"] {
                    if let Ok(dt) = chrono::DateTime::parse_from_str(&expanded, fmt) {
                        return Ok(Value::Timestamp(dt.with_timezone(&chrono::Utc)));
                    }
                }
            }
            // Try naive datetime without timezone
            for fmt in &["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%d %H:%M:%S"] {
                if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(text, fmt) {
                    return Ok(Value::Timestamp(
                        chrono::DateTime::from_naive_utc_and_offset(dt, chrono::Utc),
                    ));
                }
            }
            Err(format!("cannot parse timestamp: {text}"))
        }
        Type::DATE => text
            .parse::<chrono::NaiveDate>()
            .map(Value::Date)
            .map_err(|e| e.to_string()),
        Type::TIME => text
            .parse::<chrono::NaiveTime>()
            .map(Value::Time)
            .map_err(|e| e.to_string()),
        _ => Ok(Value::Custom {
            type_name: pg_type.name().to_string(),
            raw_bytes: text.as_bytes().to_vec(),
        }),
    }
}
