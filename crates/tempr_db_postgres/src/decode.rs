use postgres_types::Type;
use tempr_domain::Value;
use tokio_postgres::Row;
use tokio_postgres::types::FromSql;

/// Captures a column's raw binary-protocol bytes for types with no
/// dedicated decoder below (accepts every OID).
struct RawBytes(Vec<u8>);

impl<'a> FromSql<'a> for RawBytes {
    fn from_sql(
        _ty: &Type,
        raw: &'a [u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(RawBytes(raw.to_vec()))
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }
}

/// Decode column `i` of `row` into a `Value`, dispatching on the column's
/// actual PostgreSQL type. `tokio-postgres` always transmits results in
/// binary format, so decoding must go through the Rust type each OID's
/// `FromSql` impl expects — coercing everything through `String` (as a
/// prior version of this function did) fails for every non-text-ish type.
pub(crate) fn decode_value(row: &Row, i: usize, pg_type: &Type) -> Result<Value, String> {
    macro_rules! get {
        ($t:ty) => {
            row.try_get::<_, Option<$t>>(i).map_err(|e| e.to_string())
        };
    }

    match *pg_type {
        Type::BOOL => get!(bool).map(|v| v.map_or(Value::Null, Value::Bool)),
        Type::INT2 => get!(i16).map(|v| v.map_or(Value::Null, |v| Value::Int8(v as i64))),
        Type::INT4 => get!(i32).map(|v| v.map_or(Value::Null, |v| Value::Int8(v as i64))),
        Type::INT8 => get!(i64).map(|v| v.map_or(Value::Null, Value::Int8)),
        Type::FLOAT4 => get!(f32).map(|v| v.map_or(Value::Null, |v| Value::Float8(v as f64))),
        Type::FLOAT8 => get!(f64).map(|v| v.map_or(Value::Null, Value::Float8)),
        Type::NUMERIC => get!(rust_decimal::Decimal)
            .map(|v| v.map_or(Value::Null, |d| Value::Numeric(d.to_string()))),
        Type::TEXT | Type::VARCHAR | Type::BPCHAR | Type::NAME | Type::UNKNOWN => {
            get!(String).map(|v| v.map_or(Value::Null, Value::Text))
        }
        Type::BYTEA => get!(Vec<u8>).map(|v| v.map_or(Value::Null, Value::Bytes)),
        Type::UUID => get!(uuid::Uuid).map(|v| v.map_or(Value::Null, Value::Uuid)),
        Type::JSON | Type::JSONB => {
            get!(serde_json::Value).map(|v| v.map_or(Value::Null, Value::Json))
        }
        Type::TIMESTAMPTZ => {
            get!(chrono::DateTime<chrono::Utc>).map(|v| v.map_or(Value::Null, Value::Timestamp))
        }
        Type::TIMESTAMP => get!(chrono::NaiveDateTime).map(|v| {
            v.map_or(Value::Null, |dt| {
                Value::Timestamp(chrono::DateTime::from_naive_utc_and_offset(dt, chrono::Utc))
            })
        }),
        Type::DATE => get!(chrono::NaiveDate).map(|v| v.map_or(Value::Null, Value::Date)),
        Type::TIME => get!(chrono::NaiveTime).map(|v| v.map_or(Value::Null, Value::Time)),
        _ => {
            let raw = row
                .try_get::<_, Option<RawBytes>>(i)
                .map_err(|e| e.to_string())?;
            Ok(raw.map_or(Value::Null, |RawBytes(bytes)| Value::Custom {
                type_name: pg_type.name().to_string(),
                raw_bytes: bytes,
            }))
        }
    }
}
