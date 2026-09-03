//! TLS connector construction for `tokio-postgres` (D20).
//!
//! rustls with the `ring` provider; libpq `sslmode` semantics:
//! - `Disable`      → no TLS connector, `SslMode::Disable`
//! - `Prefer`       → encrypt if offered, **no** certificate verification
//! - `Require`      → encrypt or fail, **no** certificate verification
//! - `VerifyCa`/`VerifyFull` → encrypt, verify chain against the platform
//!   root store *and* the hostname (Tempr treats `verify-ca` as `verify-full`;
//!   rustls always checks the name and the weaker mode buys nothing safe).

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tempr_db::DriverError;
use tempr_domain::TlsMode;
use tokio_postgres::config::SslMode;
use tokio_postgres_rustls::MakeRustlsConnect;

/// The connector chosen for a connection; cloned into the cancel handle so
/// cancel requests use the same transport policy.
#[derive(Clone)]
pub enum TlsChoice {
    None,
    Rustls(MakeRustlsConnect),
}

/// Map our mode onto tokio-postgres's negotiation flag.
pub fn ssl_mode(mode: TlsMode) -> SslMode {
    match mode {
        TlsMode::Disable => SslMode::Disable,
        TlsMode::Prefer => SslMode::Prefer,
        TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull => SslMode::Require,
    }
}

pub fn connector(mode: TlsMode) -> Result<TlsChoice, DriverError> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = match mode {
        TlsMode::Disable => return Ok(TlsChoice::None),
        TlsMode::Prefer | TlsMode::Require => {
            let verifier = Arc::new(NoVerification {
                provider: provider.clone(),
            });
            ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(tls_internal)?
                .dangerous()
                .with_custom_certificate_verifier(verifier)
                .with_no_client_auth()
        }
        TlsMode::VerifyCa | TlsMode::VerifyFull => {
            let mut roots = RootCertStore::empty();
            let native = rustls_native_certs::load_native_certs();
            for e in &native.errors {
                tracing::warn!(error = %e, "TLS: could not load a native root certificate");
            }
            let (added, _ignored) = roots.add_parsable_certificates(native.certs);
            if added == 0 {
                return Err(DriverError::Internal(
                    "TLS: no trusted root certificates found in the platform store".to_string(),
                ));
            }
            ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(tls_internal)?
                .with_root_certificates(roots)
                .with_no_client_auth()
        }
    };
    Ok(TlsChoice::Rustls(MakeRustlsConnect::new(config)))
}

fn tls_internal(e: rustls::Error) -> DriverError {
    DriverError::Internal(format!("TLS configuration: {e}"))
}

/// libpq `prefer`/`require` semantics: encrypt, but accept any server
/// certificate. Handshake signatures are still verified so the channel is
/// at least bound to *some* key the server holds.
#[derive(Debug)]
struct NoVerification {
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for NoVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssl_mode_mapping() {
        assert!(matches!(ssl_mode(TlsMode::Disable), SslMode::Disable));
        assert!(matches!(ssl_mode(TlsMode::Prefer), SslMode::Prefer));
        assert!(matches!(ssl_mode(TlsMode::Require), SslMode::Require));
        assert!(matches!(ssl_mode(TlsMode::VerifyFull), SslMode::Require));
    }

    #[test]
    fn connector_kind_follows_mode() {
        assert!(matches!(
            connector(TlsMode::Disable).unwrap(),
            TlsChoice::None
        ));
        assert!(matches!(
            connector(TlsMode::Require).unwrap(),
            TlsChoice::Rustls(_)
        ));
        // Native roots exist on any CI/dev box; if not, the error is explicit.
        match connector(TlsMode::VerifyFull) {
            Ok(TlsChoice::Rustls(_)) => {}
            Err(DriverError::Internal(msg)) => assert!(msg.contains("root certificates")),
            Ok(TlsChoice::None) => panic!("verify-full must produce a rustls connector"),
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
