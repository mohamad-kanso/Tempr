use bytes::BytesMut;
use postgres_types::{IsNull, ToSql, Type, to_sql_checked};
use tempr_domain::Value;

/// Bridges `tempr_domain::Value` (defined outside this crate) to
/// `tokio_postgres`'s `ToSql` (also defined outside this crate) — a direct
/// `impl ToSql for Value` would violate the orphan rule.
#[derive(Debug)]
pub(crate) struct ValueParam<'a>(pub &'a Value);

impl ToSql for ValueParam<'_> {
    fn to_sql(
        &self,
        ty: &Type,
        out: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        match self.0 {
            Value::Null => Ok(IsNull::Yes),
            Value::Bool(v) => v.to_sql(ty, out),
            Value::Int8(v) => v.to_sql(ty, out),
            Value::Float8(v) => v.to_sql(ty, out),
            Value::Text(v) => v.to_sql(ty, out),
            Value::Bytes(v) => v.to_sql(ty, out),
            Value::Uuid(v) => v.to_sql(ty, out),
            Value::Json(v) => v.to_sql(ty, out),
            Value::Timestamp(v) => v.to_sql(ty, out),
            Value::Date(v) => v.to_sql(ty, out),
            Value::Time(v) => v.to_sql(ty, out),
            Value::Numeric(s) => {
                let d: rust_decimal::Decimal = s.parse()?;
                d.to_sql(ty, out)
            }
            Value::Array(_) | Value::Custom { .. } => {
                Err(format!("unsupported query parameter value: {:?}", self.0).into())
            }
        }
    }

    fn accepts(_ty: &Type) -> bool {
        true
    }

    to_sql_checked!();
}

pub(crate) fn to_sql_params(values: &[Value]) -> Vec<ValueParam<'_>> {
    values.iter().map(ValueParam).collect()
}

pub(crate) fn as_sql_refs<'a>(params: &'a [ValueParam<'a>]) -> Vec<&'a (dyn ToSql + Sync)> {
    params.iter().map(|p| p as &(dyn ToSql + Sync)).collect()
}
