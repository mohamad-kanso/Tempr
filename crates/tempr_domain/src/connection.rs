use crate::ids::ConnectionId;
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
pub struct Connection {
    pub id: ConnectionId,
    pub name: String,
    pub driver: DriverKind,
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
    pub secret_ref: SecretRef,
    /// Transport security, libpq `sslmode` semantics. Default `Prefer`.
    #[serde(default)]
    pub tls: TlsMode,
}

/// TLS policy for a connection — mirrors PostgreSQL's `sslmode` values so
/// connection strings and user expectations carry over unchanged.
///
/// | mode | encrypted | server cert verified | hostname verified |
/// |---|---|---|---|
/// | `Disable` | no | — | — |
/// | `Prefer` (default) | if the server supports it | no | no |
/// | `Require` | yes, or fail | no | no |
/// | `VerifyCa` | yes | yes (system roots) | **yes** (Tempr is stricter than libpq here) |
/// | `VerifyFull` | yes | yes (system roots) | yes |
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TlsMode {
    Disable,
    #[default]
    Prefer,
    Require,
    VerifyCa,
    VerifyFull,
}

impl TlsMode {
    /// The `sslmode` spelling used in connection strings and configs.
    pub fn as_sslmode(&self) -> &'static str {
        match self {
            TlsMode::Disable => "disable",
            TlsMode::Prefer => "prefer",
            TlsMode::Require => "require",
            TlsMode::VerifyCa => "verify-ca",
            TlsMode::VerifyFull => "verify-full",
        }
    }

    /// Whether this mode encrypts unconditionally (fails without TLS).
    pub fn requires_tls(&self) -> bool {
        !matches!(self, TlsMode::Disable | TlsMode::Prefer)
    }

    /// Whether the server certificate is verified against trusted roots.
    pub fn verifies_certificate(&self) -> bool {
        matches!(self, TlsMode::VerifyCa | TlsMode::VerifyFull)
    }
}

impl std::str::FromStr for TlsMode {
    type Err = String;

    /// Accepts libpq spellings; `allow` (libpq's "TLS only if the server
    /// insists") is mapped to `Prefer`.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "disable" => TlsMode::Disable,
            "allow" | "prefer" => TlsMode::Prefer,
            "require" => TlsMode::Require,
            "verify-ca" | "verify_ca" => TlsMode::VerifyCa,
            "verify-full" | "verify_full" => TlsMode::VerifyFull,
            other => {
                return Err(format!(
                    "unknown sslmode '{other}' (expected disable, prefer, require, verify-ca, verify-full)"
                ));
            }
        })
    }
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("driver", &self.driver)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("database", &self.database)
            .field("username", &self.username)
            .field("password", &"[REDACTED]")
            .field("secret_ref", &self.secret_ref)
            .field("tls", &self.tls)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriverKind {
    Postgres,
    MySQL,
    SQLite,
}

impl DriverKind {
    pub fn engine_name(&self) -> &'static str {
        match self {
            DriverKind::Postgres => "postgresql",
            DriverKind::MySQL => "mysql",
            DriverKind::SQLite => "sqlite",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRef {
    pub vault_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectionState {
    Connecting,
    Connected,
    Reconnecting,
    Failed,
    Disconnected,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::ConnectionId;

    #[test]
    fn driver_kind_serde_roundtrip() {
        let cases = [DriverKind::Postgres, DriverKind::MySQL, DriverKind::SQLite];
        for driver in cases {
            let json = serde_json::to_string(&driver).expect("serialize");
            let back: DriverKind = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(driver, back);
        }
    }

    #[test]
    fn connection_state_serde_roundtrip() {
        let cases = [
            ConnectionState::Connecting,
            ConnectionState::Connected,
            ConnectionState::Reconnecting,
            ConnectionState::Failed,
            ConnectionState::Disconnected,
        ];
        for state in cases {
            let json = serde_json::to_string(&state).expect("serialize");
            let back: ConnectionState = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(state, back);
        }
    }

    #[test]
    fn connection_serde_roundtrip() {
        let conn = Connection {
            id: ConnectionId::new(),
            name: "prod-db".to_string(),
            driver: DriverKind::Postgres,
            host: "localhost".to_string(),
            port: 5432,
            database: "mydb".to_string(),
            username: "admin".to_string(),
            password: "secret".to_string(),
            secret_ref: SecretRef {
                vault_key: "keychain://tempr/prod".to_string(),
            },
            tls: TlsMode::VerifyFull,
        };
        let json = serde_json::to_string(&conn).expect("serialize");
        let back: Connection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(conn.id, back.id);
        assert_eq!(conn.driver, back.driver);
        assert_eq!(conn.port, back.port);
    }

    #[test]
    fn tls_mode_parses_libpq_spellings_and_defaults_to_prefer() {
        assert_eq!(TlsMode::default(), TlsMode::Prefer);
        assert_eq!("require".parse::<TlsMode>().unwrap(), TlsMode::Require);
        assert_eq!(
            "VERIFY-FULL".parse::<TlsMode>().unwrap(),
            TlsMode::VerifyFull
        );
        assert_eq!("verify_ca".parse::<TlsMode>().unwrap(), TlsMode::VerifyCa);
        assert_eq!("allow".parse::<TlsMode>().unwrap(), TlsMode::Prefer);
        assert!("tls-please".parse::<TlsMode>().is_err());
        assert_eq!(TlsMode::VerifyFull.as_sslmode(), "verify-full");
        assert!(TlsMode::Require.requires_tls() && !TlsMode::Require.verifies_certificate());
        assert!(TlsMode::VerifyCa.verifies_certificate());
        assert!(!TlsMode::Prefer.requires_tls());
    }

    #[test]
    fn tls_mode_serde_is_kebab_case_and_defaults_when_missing() {
        assert_eq!(
            serde_json::to_string(&TlsMode::VerifyFull).unwrap(),
            "\"verify-full\""
        );
        #[derive(Deserialize)]
        struct Wrap {
            #[serde(default)]
            tls: TlsMode,
        }
        let w: Wrap = serde_json::from_str("{}").unwrap();
        assert_eq!(w.tls, TlsMode::Prefer);
    }
}
