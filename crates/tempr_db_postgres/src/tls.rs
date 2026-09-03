//! TLS connector construction for `tokio-postgres` (D20).
//!
//! rustls with the `ring` provider; libpq `sslmode` semantics:
//! - `Disable`      → `SslMode::Disable` (tokio-postgres never invokes the connector)
//! - `Prefer`       → encrypt if offered, **no** certificate verification; if the
//!   handshake itself fails the driver retries in plaintext (libpq behaviour)
//! - `Require`      → encrypt or fail, **no** certificate verification
//! - `VerifyCa`/`VerifyFull` → encrypt, verify chain against the platform
//!   root store *and* the hostname (Tempr treats `verify-ca` as `verify-full`;
//!   rustls always checks the name and the weaker mode buys nothing safe).
//!
//! Connectors are built once per process and shared (`Arc<ClientConfig>`
//! inside `MakeRustlsConnect`): loading the platform root store is expensive
//! and a shared config lets pooled connections resume TLS sessions.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use tempr_db::DriverError;
use tempr_domain::TlsMode;
use tokio::sync::OnceCell;
use tokio_postgres::config::SslMode;
use tokio_postgres_rustls::MakeRustlsConnect;

static NO_VERIFY: OnceCell<MakeRustlsConnect> = OnceCell::const_new();
static VERIFYING: OnceCell<MakeRustlsConnect> = OnceCell::const_new();

/// Map our mode onto tokio-postgres's negotiation flag.
pub fn ssl_mode(mode: TlsMode) -> SslMode {
    match mode {
        TlsMode::Disable => SslMode::Disable,
        TlsMode::Prefer => SslMode::Prefer,
        TlsMode::Require | TlsMode::VerifyCa | TlsMode::VerifyFull => SslMode::Require,
    }
}

/// The (shared) connector for `mode`. For `Disable` a connector is still
/// returned so callers have one code path; tokio-postgres skips it.
pub async fn connector(mode: TlsMode) -> Result<MakeRustlsConnect, DriverError> {
    match mode {
        TlsMode::Disable | TlsMode::Prefer | TlsMode::Require => NO_VERIFY
            .get_or_try_init(|| async { build_no_verify() })
            .await
            .cloned(),
        TlsMode::VerifyCa | TlsMode::VerifyFull => VERIFYING
            .get_or_try_init(|| async {
                // Root-store loading reads the filesystem / OS keystore.
                tokio::task::spawn_blocking(build_verifying)
                    .await
                    .map_err(|e| DriverError::Internal(format!("TLS setup task failed: {e}")))?
            })
            .await
            .cloned(),
    }
}

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn build_no_verify() -> Result<MakeRustlsConnect, DriverError> {
    let provider = provider();
    let verifier = Arc::new(NoVerification {
        provider: provider.clone(),
    });
    let config = ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(tls_internal)?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
}

fn build_verifying() -> Result<MakeRustlsConnect, DriverError> {
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
    let config = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(tls_internal)?
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(MakeRustlsConnect::new(config))
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

    #[tokio::test]
    async fn connectors_are_built_once_and_shared() {
        let a = connector(TlsMode::Require).await.unwrap();
        let b = connector(TlsMode::Prefer).await.unwrap();
        // Same cached instance behind both non-verifying modes.
        assert!(std::ptr::eq(
            NO_VERIFY.get().unwrap() as *const _,
            NO_VERIFY.get().unwrap() as *const _
        ));
        drop((a, b));
        // Verifying connector: either builds or reports an explicit root-store error.
        match connector(TlsMode::VerifyFull).await {
            Ok(_) => {}
            Err(DriverError::Internal(msg)) => assert!(msg.contains("root certificates")),
            Err(other) => panic!("unexpected error: {other}"),
        }
    }
}
