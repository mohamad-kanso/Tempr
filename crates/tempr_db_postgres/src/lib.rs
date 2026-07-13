#![deny(unsafe_code)]
#![cfg_attr(
    test,
    allow(clippy::expect_used, clippy::unwrap_used, clippy::approx_constant)
)]

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_value;
    use tempr_domain::Value;

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

    // decode_value tests
    #[test]
    fn decode_null_returns_null() {
        assert_eq!(decode_value(&Type::TEXT, None).unwrap(), Value::Null);
    }

    #[test]
    fn decode_bool_true() {
        assert_eq!(
            decode_value(&Type::BOOL, Some("t")).unwrap(),
            Value::Bool(true)
        );
    }

    #[test]
    fn decode_bool_false() {
        assert_eq!(
            decode_value(&Type::BOOL, Some("f")).unwrap(),
            Value::Bool(false)
        );
    }

    #[test]
    fn decode_int2() {
        assert_eq!(
            decode_value(&Type::INT2, Some("42")).unwrap(),
            Value::Int8(42)
        );
    }

    #[test]
    fn decode_int4() {
        assert_eq!(
            decode_value(&Type::INT4, Some("-100")).unwrap(),
            Value::Int8(-100)
        );
    }

    #[test]
    fn decode_int8() {
        assert_eq!(
            decode_value(&Type::INT8, Some("9223372036854775807")).unwrap(),
            Value::Int8(9223372036854775807)
        );
    }

    #[test]
    fn decode_float4() {
        let val = decode_value(&Type::FLOAT4, Some("3.14")).unwrap();
        match val {
            Value::Float8(f) => assert!((f - 3.14).abs() < 0.001),
            _ => panic!("expected Float8"),
        }
    }

    #[test]
    fn decode_float8() {
        assert_eq!(
            decode_value(&Type::FLOAT8, Some("2.718281828")).unwrap(),
            Value::Float8(2.718281828)
        );
    }

    #[test]
    fn decode_numeric_preserves_string() {
        assert_eq!(
            decode_value(&Type::NUMERIC, Some("123456789.123456789")).unwrap(),
            Value::Numeric("123456789.123456789".to_string())
        );
    }

    #[test]
    fn decode_text() {
        assert_eq!(
            decode_value(&Type::TEXT, Some("hello world")).unwrap(),
            Value::Text("hello world".to_string())
        );
    }

    #[test]
    fn decode_varchar() {
        assert_eq!(
            decode_value(&Type::VARCHAR, Some("test")).unwrap(),
            Value::Text("test".to_string())
        );
    }

    #[test]
    fn decode_bpchar() {
        assert_eq!(
            decode_value(&Type::BPCHAR, Some("fixed")).unwrap(),
            Value::Text("fixed".to_string())
        );
    }

    #[test]
    fn decode_uuid_valid() {
        let val = decode_value(&Type::UUID, Some("550e8400-e29b-41d4-a716-446655440000")).unwrap();
        match val {
            Value::Uuid(u) => assert_eq!(u.to_string(), "550e8400-e29b-41d4-a716-446655440000"),
            _ => panic!("expected Uuid"),
        }
    }

    #[test]
    fn decode_uuid_invalid() {
        let result = decode_value(&Type::UUID, Some("not-a-uuid"));
        assert!(result.is_err());
    }

    #[test]
    fn decode_json_valid() {
        let val = decode_value(&Type::JSON, Some(r#"{"key": "value"}"#)).unwrap();
        match val {
            Value::Json(j) => assert_eq!(j["key"], "value"),
            _ => panic!("expected Json"),
        }
    }

    #[test]
    fn decode_json_invalid() {
        let result = decode_value(&Type::JSON, Some("not json"));
        assert!(result.is_err());
    }

    #[test]
    fn decode_jsonb() {
        let val = decode_value(&Type::JSONB, Some(r#"[1, 2, 3]"#)).unwrap();
        match val {
            Value::Json(j) => assert_eq!(j.as_array().unwrap().len(), 3),
            _ => panic!("expected Json"),
        }
    }

    #[test]
    fn decode_timestamp() {
        let val = decode_value(&Type::TIMESTAMP, Some("2025-01-15 10:30:00")).unwrap();
        assert!(matches!(val, Value::Timestamp(_)));
    }

    #[test]
    fn decode_timestamptz() {
        let val = decode_value(&Type::TIMESTAMPTZ, Some("2025-01-15 10:30:00+00")).unwrap();
        assert!(matches!(val, Value::Timestamp(_)));
    }

    #[test]
    fn decode_date() {
        let val = decode_value(&Type::DATE, Some("2025-01-15")).unwrap();
        assert!(matches!(val, Value::Date(_)));
    }

    #[test]
    fn decode_time() {
        let val = decode_value(&Type::TIME, Some("10:30:00")).unwrap();
        assert!(matches!(val, Value::Time(_)));
    }

    #[test]
    fn decode_unknown_type_falls_back_to_custom() {
        let val = decode_value(&Type::INET, Some("127.0.0.1")).unwrap();
        match val {
            Value::Custom {
                type_name,
                raw_bytes,
            } => {
                assert_eq!(type_name, "inet");
                assert_eq!(raw_bytes, b"127.0.0.1");
            }
            _ => panic!("expected Custom"),
        }
    }
}
