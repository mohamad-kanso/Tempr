use postgres_types::Type;
use tempr_domain::ValueType;

pub(crate) mod decode;
pub(crate) mod driver;
pub(crate) mod stream;

pub use driver::PostgresDriver;

pub(crate) fn pg_type_to_value_type(pg_type: &Type) -> ValueType {
    match *pg_type {
        Type::BOOL => ValueType::Bool,
        Type::INT2 | Type::INT4 | Type::INT8 => ValueType::Int,
        Type::FLOAT4 | Type::FLOAT8 => ValueType::Float,
        Type::NUMERIC => ValueType::Numeric,
        Type::TEXT | Type::VARCHAR | Type::BPCHAR => ValueType::String,
        Type::BYTEA => ValueType::Bytes,
        Type::UUID => ValueType::Uuid,
        Type::JSON | Type::JSONB => ValueType::Json,
        Type::TIMESTAMPTZ | Type::TIMESTAMP => ValueType::Timestamp,
        Type::DATE => ValueType::Date,
        Type::TIME => ValueType::Time,
        _ => ValueType::Custom,
    }
}
