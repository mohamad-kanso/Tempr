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
        };
        let json = serde_json::to_string(&conn).expect("serialize");
        let back: Connection = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(conn.id, back.id);
        assert_eq!(conn.driver, back.driver);
        assert_eq!(conn.port, back.port);
    }
}
