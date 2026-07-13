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
        Type::BOOL => text
            .parse::<bool>()
            .map(Value::Bool)
            .map_err(|e| e.to_string()),
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
        Type::TIMESTAMPTZ | Type::TIMESTAMP => text
            .parse::<chrono::DateTime<chrono::Utc>>()
            .map(Value::Timestamp)
            .or_else(|_| {
                text.parse::<chrono::NaiveDateTime>().map(|dt| {
                    Value::Timestamp(chrono::DateTime::from_naive_utc_and_offset(dt, chrono::Utc))
                })
            })
            .map_err(|e| e.to_string()),
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
