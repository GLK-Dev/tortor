use anyhow::Result;
use quinn::{ClientConfig, ServerConfig};
use quinn::crypto::rustls::{QuicServerConfig, QuicClientConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime};
use rustls::client::danger::{ServerCertVerified, ServerCertVerifier};
use std::sync::Arc;
use rustls::crypto::ring;

#[derive(Debug)]
struct SkipServerVerification;

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self)
    }
}

impl ServerCertVerifier for SkipServerVerification {
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
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        vec![
            rustls::SignatureScheme::RSA_PKCS1_SHA1,
            rustls::SignatureScheme::ECDSA_NISTP256_SHA256,
            rustls::SignatureScheme::RSA_PKCS1_SHA256,
            rustls::SignatureScheme::ECDSA_NISTP384_SHA384,
            rustls::SignatureScheme::RSA_PKCS1_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA256,
            rustls::SignatureScheme::RSA_PSS_SHA384,
            rustls::SignatureScheme::RSA_PSS_SHA512,
            rustls::SignatureScheme::ED25519,
            rustls::SignatureScheme::ED448,
        ]
    }
}

pub fn configure_quic() -> Result<(ServerConfig, ClientConfig)> {
    let cert = rcgen::generate_simple_self_signed(vec!["tortor.local".into()])?;
    let cert_der = cert.cert.der().to_vec();
    let priv_key = cert.key_pair.serialize_der();
    
    let cert_chain = vec![CertificateDer::from(cert_der)];
    let key = PrivateKeyDer::try_from(priv_key).map_err(|e| anyhow::anyhow!(e))?;

    let mut server_crypto = rustls::ServerConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(cert_chain, key)?;
    
    server_crypto.alpn_protocols = vec![b"tortor/1.0".to_vec()];
    let server_config = ServerConfig::with_crypto(Arc::new(QuicServerConfig::try_from(server_crypto)?));

    let mut client_crypto = rustls::ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(SkipServerVerification::new())
        .with_no_client_auth();
    client_crypto.alpn_protocols = vec![b"tortor/1.0".to_vec()];
    
    let mut client_config = ClientConfig::new(Arc::new(QuicClientConfig::try_from(client_crypto)?));

    Ok((server_config, client_config))
}
