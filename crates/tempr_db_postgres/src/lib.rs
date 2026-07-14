#![deny(unsafe_code)]
#![cfg_attr(
    test,
    allow(clippy::expect_used, clippy::unwrap_used, clippy::approx_constant)
)]

use postgres_types::Type;
use tempr_domain::ValueType;

pub(crate) mod decode;
pub(crate) mod driver;
pub(crate) mod params;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pg_type_mapping_bool() {
        assert_eq!(pg_type_to_value_type(&Type::BOOL), ValueType::Bool);
    }

    #[test]
    fn pg_type_mapping_integers() {
        assert_eq!(pg_type_to_value_type(&Type::INT2), ValueType::Int);
        assert_eq!(pg_type_to_value_type(&Type::INT4), ValueType::Int);
        assert_eq!(pg_type_to_value_type(&Type::INT8), ValueType::Int);
    }

    #[test]
    fn pg_type_mapping_floats() {
        assert_eq!(pg_type_to_value_type(&Type::FLOAT4), ValueType::Float);
        assert_eq!(pg_type_to_value_type(&Type::FLOAT8), ValueType::Float);
    }

    #[test]
    fn pg_type_mapping_text() {
        assert_eq!(pg_type_to_value_type(&Type::TEXT), ValueType::String);
        assert_eq!(pg_type_to_value_type(&Type::VARCHAR), ValueType::String);
        assert_eq!(pg_type_to_value_type(&Type::BPCHAR), ValueType::String);
    }

    #[test]
    fn pg_type_mapping_special() {
        assert_eq!(pg_type_to_value_type(&Type::NUMERIC), ValueType::Numeric);
        assert_eq!(pg_type_to_value_type(&Type::BYTEA), ValueType::Bytes);
        assert_eq!(pg_type_to_value_type(&Type::UUID), ValueType::Uuid);
        assert_eq!(pg_type_to_value_type(&Type::JSON), ValueType::Json);
        assert_eq!(pg_type_to_value_type(&Type::JSONB), ValueType::Json);
        assert_eq!(
            pg_type_to_value_type(&Type::TIMESTAMPTZ),
            ValueType::Timestamp
        );
        assert_eq!(
            pg_type_to_value_type(&Type::TIMESTAMP),
            ValueType::Timestamp
        );
        assert_eq!(pg_type_to_value_type(&Type::DATE), ValueType::Date);
        assert_eq!(pg_type_to_value_type(&Type::TIME), ValueType::Time);
    }

    #[test]
    fn pg_type_mapping_unknown() {
        assert_eq!(pg_type_to_value_type(&Type::INET), ValueType::Custom);
        assert_eq!(pg_type_to_value_type(&Type::MACADDR), ValueType::Custom);
    }

    // `decode::decode_value` now decodes directly from a live
    // `tokio_postgres::Row` (binary protocol), which can't be constructed
    // without a real connection — see the `pg_decodes_*` integration tests
    // in `crates/tempr/tests/integration.rs` for coverage of that path.
}
