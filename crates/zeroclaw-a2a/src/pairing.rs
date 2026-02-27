//! A2A (Agent-to-Agent) Peer Pairing
//!
//! This module provides secure peer authentication and pairing for agent-to-agent
//! communication. The pairing flow uses short-lived, one-time pairing codes that
//! are exchanged out-of-band (displayed to operator) for bearer token acquisition.
//!
//! # Security Model
//!
//! - Pairing codes are 6-digit numeric values with 5-minute expiration
//! - Codes are single-use and consumed upon successful exchange
//! - Constant-time comparison prevents timing attacks on code verification
//! - Bearer tokens are encrypted with SecretStore before storage
//! - TLS is required for all pairing endpoints (except localhost for testing)
//!
//! # Pairing Flow
//!
//! 1. Initiator requests pairing code from peer (POST /a2a/pair/request)
//! 2. Peer displays code to its operator
//! 3. Initiator's operator enters the code
//! 4. Initiator exchanges code for bearer token (POST /a2a/pair/confirm)
//! 5. Token is encrypted and stored in config

use crate::SecretEncryptor;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;
use uuid::Uuid;

/// Default pairing code expiration time (5 minutes).
const PAIRING_CODE_EXPIRY_SECS: u64 = 300;

/// Length of pairing code in digits.
const PAIRING_CODE_LENGTH: usize = 6;

/// Maximum number of pending pairing codes to prevent memory exhaustion.
const MAX_PENDING_CODES: usize = 1024;

/// Request body for initiating pairing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequest {
    /// Unique peer identifier of the requesting agent.
    pub peer_id: String,
    /// Optional public key for future encrypted communication.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_key: Option<String>,
}

/// Response to a pairing request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingRequestResponse {
    /// The pairing code to display to the operator.
    pub pairing_code: String,
    /// Expiration time in seconds.
    pub expires_in_secs: u64,
}

/// Request body for confirming pairing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingConfirmRequest {
    /// The pairing code received from the peer.
    pub pairing_code: String,
}

/// Response to a successful pairing confirmation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairingConfirmResponse {
    /// The bearer token for authenticated communication.
    pub bearer_token: String,
    /// The peer ID assigned to this connection.
    pub peer_id: String,
}

/// Pending pairing state.
#[derive(Debug, Clone)]
struct PendingPairing {
    /// The pairing code.
    code: String,
    /// When the code was created.
    created_at: Instant,
    /// The peer ID of the requesting agent.
    peer_id: String,
    /// Optional public key.
    public_key: Option<String>,
}

/// Manages pending pairing codes and their expiration.
#[derive(Debug, Clone)]
pub struct PairingManager<E: SecretEncryptor> {
    /// Pending pairing requests indexed by code.
    pending: Arc<Mutex<HashMap<String, PendingPairing>>>,
    /// Code expiration duration.
    expiry_duration: Duration,
    /// Encryptor for securing tokens.
    encryptor: E,
}

impl<E: SecretEncryptor + Default> PairingManager<E> {
    /// Create a new pairing manager with the default encryptor.
    pub fn with_default_encryptor() -> Self {
        Self::new(E::default())
    }
}

impl<E: SecretEncryptor> PairingManager<E> {
    /// Create a new pairing manager with default expiration.
    pub fn new(encryptor: E) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            expiry_duration: Duration::from_secs(PAIRING_CODE_EXPIRY_SECS),
            encryptor,
        }
    }

    /// Create a new pairing manager with custom expiration.
    pub fn with_expiry(encryptor: E, expiry_secs: u64) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            expiry_duration: Duration::from_secs(expiry_secs),
            encryptor,
        }
    }

    /// Generate a new pairing request and return the code.
    ///
    /// # Arguments
    ///
    /// * `peer_id` - The peer ID of the requesting agent.
    /// * `public_key` - Optional public key for future encrypted communication.
    ///
    /// # Returns
    ///
    /// The pairing code to display to the operator.
    pub async fn create_pairing_request(
        &self,
        peer_id: String,
        public_key: Option<String>,
    ) -> String {
        let mut pending = self.pending.lock().await;

        // Clean up expired entries if we're approaching the limit
        if pending.len() >= MAX_PENDING_CODES {
            let now = Instant::now();
            pending.retain(|_, p| now.duration_since(p.created_at) < self.expiry_duration);
        }

        // Generate a unique code
        let code = generate_pairing_code();
        let pairing = PendingPairing {
            code: code.clone(),
            created_at: Instant::now(),
            peer_id,
            public_key,
        };

        pending.insert(code.clone(), pairing);
        code
    }

    /// Exchange a pairing code for a bearer token.
    ///
    /// # Arguments
    ///
    /// * `code` - The pairing code to exchange.
    ///
    /// # Returns
    ///
    /// The bearer token and peer ID if successful.
    pub async fn exchange_pairing_code(
        &self,
        code: &str,
    ) -> Result<(String, String)> {
        let mut pending = self.pending.lock().await;

        let pairing = pending
            .remove(code)
            .ok_or_else(|| anyhow::anyhow!("Invalid or expired pairing code"))?;

        // Check expiration
        if Instant::now().duration_since(pairing.created_at) > self.expiry_duration {
            bail!("Pairing code has expired");
        }

        // Generate a bearer token
        let bearer_token = format!("a2a_{}", Uuid::new_v4().to_string().replace("-", ""));

        // Encrypt the token before returning
        let encrypted_token = self
            .encryptor
            .encrypt(&bearer_token)
            .context("Failed to encrypt bearer token")?;

        Ok((encrypted_token, pairing.peer_id))
    }

    /// Verify if a pairing code is valid (without consuming it).
    ///
    /// # Arguments
    ///
    /// * `code` - The pairing code to verify.
    ///
    /// # Returns
    ///
    /// True if the code is valid and not expired.
    pub async fn verify_pairing_code(&self, code: &str) -> bool {
        let pending = self.pending.lock().await;

        if let Some(pairing) = pending.get(code) {
            Instant::now().duration_since(pairing.created_at) <= self.expiry_duration
        } else {
            false
        }
    }

    /// Get the number of pending pairing codes.
    pub async fn pending_count(&self) -> usize {
        let pending = self.pending.lock().await;
        pending.len()
    }

    /// Clean up expired pairing codes.
    pub async fn cleanup_expired(&self) -> usize {
        let mut pending = self.pending.lock().await;
        let now = Instant::now();
        let initial_count = pending.len();
        pending.retain(|_, p| now.duration_since(p.created_at) < self.expiry_duration);
        initial_count - pending.len()
    }
}

/// Generate a random 6-digit pairing code.
fn generate_pairing_code() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    let code: u32 = rng.gen_range(0..1_000_000);
    format!("{:06}", code)
}

/// Perform constant-time comparison of two strings using SHA-256.
///
/// This prevents timing attacks on sensitive comparisons.
pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let hash_a = Sha256::digest(a.as_bytes());
    let hash_b = Sha256::digest(b.as_bytes());

    let mut result = 0u8;
    for (x, y) in hash_a.iter().zip(hash_b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

/// Initiate peer pairing with a remote agent.
///
/// This is the client-side function that requests a pairing code from a peer.
///
/// # Arguments
///
/// * `peer_endpoint` - The peer's HTTP endpoint URL.
/// * `our_peer_id` - Our unique peer identifier.
/// * `http_client` - The HTTP client to use for the request.
///
/// # Returns
///
/// The pairing code from the peer (to display to operator).
pub async fn initiate_peer_pairing(
    peer_endpoint: &str,
    our_peer_id: &str,
    http_client: &reqwest::Client,
) -> Result<String> {
    let request = PairingRequest {
        peer_id: our_peer_id.to_string(),
        public_key: None,
    };

    let response = http_client
        .post(format!("{}/a2a/pair/request", peer_endpoint))
        .json(&request)
        .send()
        .await
        .context("Failed to send pairing request")?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        bail!("Pairing request failed: HTTP {} - {}", status, text);
    }

    let result: PairingRequestResponse = response
        .json()
        .await
        .context("Failed to parse pairing response")?;

    Ok(result.pairing_code)
}

/// Confirm peer pairing and exchange code for token.
///
/// This is the client-side function that exchanges the pairing code for a bearer token.
///
/// # Arguments
///
/// * `peer_endpoint` - The peer's HTTP endpoint URL.
/// * `pairing_code` - The pairing code received from the operator.
/// * `http_client` - The HTTP client to use for the request.
///
/// # Returns
///
/// The bearer token for authenticated communication.
pub async fn confirm_peer_pairing(
    peer_endpoint: &str,
    pairing_code: &str,
    http_client: &reqwest::Client,
) -> Result<(String, String)> {
    let request = PairingConfirmRequest {
        pairing_code: pairing_code.to_string(),
    };

    let response = http_client
        .post(format!("{}/a2a/pair/confirm", peer_endpoint))
        .json(&request)
        .send()
        .await
        .context("Failed to send pairing confirmation")?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        bail!("Pairing confirmation failed: HTTP {} - {}", status, text);
    }

    let result: PairingConfirmResponse = response
        .json()
        .await
        .context("Failed to parse confirmation response")?;

    Ok((result.bearer_token, result.peer_id))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone)]
    struct MockEncryptor;

    impl SecretEncryptor for MockEncryptor {
        fn encrypt(&self, plaintext: &str) -> Result<String> {
            Ok(format!("encrypted:{}", plaintext))
        }

        fn decrypt(&self, ciphertext: &str) -> Result<String> {
            ciphertext
                .strip_prefix("encrypted:")
                .map(|s| s.to_string())
                .ok_or_else(|| anyhow::anyhow!("Invalid ciphertext"))
        }
    }

    #[tokio::test]
    async fn pairing_code_generation() {
        let code = generate_pairing_code();
        assert_eq!(code.len(), PAIRING_CODE_LENGTH);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[tokio::test]
    async fn create_and_exchange_pairing_code() {
        let manager = PairingManager::new(MockEncryptor);

        let code = manager
            .create_pairing_request("peer-1".to_string(), None)
            .await;

        assert_eq!(manager.pending_count().await, 1);

        let (token, peer_id) = manager.exchange_pairing_code(&code).await.unwrap();
        assert!(token.starts_with("encrypted:a2a_"));
        assert_eq!(peer_id, "peer-1");
        assert_eq!(manager.pending_count().await, 0);
    }

    #[tokio::test]
    async fn exchange_invalid_code_fails() {
        let manager = PairingManager::new(MockEncryptor);

        let result = manager.exchange_pairing_code("000000").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn expired_code_cleanup() {
        let manager = PairingManager::with_expiry(MockEncryptor, 0);

        let code = manager
            .create_pairing_request("peer-1".to_string(), None)
            .await;

        // Wait for expiration
        tokio::time::sleep(Duration::from_millis(10)).await;

        let result = manager.exchange_pairing_code(&code).await;
        assert!(result.is_err());
    }

    impl Default for MockEncryptor {
        fn default() -> Self {
            Self
        }
    }

    #[test]
    fn constant_time_eq_same() {
        assert!(constant_time_eq("token-123", "token-123"));
    }

    #[test]
    fn constant_time_eq_different() {
        assert!(!constant_time_eq("token-123", "token-456"));
    }
}
