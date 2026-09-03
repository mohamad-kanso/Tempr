//! `Value` → display text for grid cells. Pure; unit-tested.
//!
//! NULL renders as `∅` (dimmed by the grid); empty string renders blank —
//! the distinction is required by docs/13-result-grid.md.

use tempr_domain::Value;

pub const NULL_TEXT: &str = "∅";
const MAX_BYTES_SHOWN: usize = 16;

pub fn format_value(value: &Value) -> String {
    match value {
        Value::Null => NULL_TEXT.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Int8(i) => i.to_string(),
        Value::Float8(f) => f.to_string(),
        Value::Text(s) => s.clone(),
        Value::Bytes(b) => format_bytes(b),
        Value::Uuid(u) => u.to_string(),
        Value::Json(j) => j.to_string(),
        Value::Timestamp(t) => t.to_rfc3339(),
        Value::Date(d) => d.to_string(),
        Value::Time(t) => t.to_string(),
        Value::Numeric(n) => n.clone(),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(format_value).collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Custom {
            type_name,
            raw_bytes,
        } => format!("<{type_name}:{} bytes>", raw_bytes.len()),
    }
}

fn format_bytes(bytes: &[u8]) -> String {
    let shown = &bytes[..bytes.len().min(MAX_BYTES_SHOWN)];
    let hex: String = shown.iter().map(|b| format!("{b:02x}")).collect();
    if bytes.len() > MAX_BYTES_SHOWN {
        format!("\\x{hex}… ({} bytes)", bytes.len())
    } else {
        format!("\\x{hex}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn null_and_empty_string_differ() {
        assert_eq!(format_value(&Value::Null), "∅");
        assert_eq!(format_value(&Value::Text(String::new())), "");
    }

    #[test]
    fn scalars() {
        assert_eq!(format_value(&Value::Bool(true)), "true");
        assert_eq!(format_value(&Value::Int8(-42)), "-42");
        assert_eq!(format_value(&Value::Float8(1.5)), "1.5");
        assert_eq!(format_value(&Value::Numeric("12.30".into())), "12.30");
    }

    #[test]
    fn bytes_hex_and_truncation() {
        assert_eq!(format_value(&Value::Bytes(vec![0xde, 0xad])), "\\xdead");
        let long = Value::Bytes(vec![0xab; 40]);
        let s = format_value(&long);
        assert!(s.starts_with("\\x") && s.ends_with("(40 bytes)"), "{s}");
    }

    #[test]
    fn arrays_nest() {
        let v = Value::Array(vec![Value::Int8(1), Value::Null, Value::Text("a".into())]);
        assert_eq!(format_value(&v), "{1,∅,a}");
    }
}
