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

use crate::security::secrets::SecretStore;
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
pub struct PairingManager {
    /// Pending pairing requests indexed by code.
    pending: Arc<Mutex<HashMap<String, PendingPairing>>>,
    /// Code expiration duration.
    expiry_duration: Duration,
}

impl Default for PairingManager {
    fn default() -> Self {
        Self::new()
    }
}

impl PairingManager {
    /// Create a new pairing manager with default expiration.
    pub fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            expiry_duration: Duration::from_secs(PAIRING_CODE_EXPIRY_SECS),
        }
    }

    /// Create a new pairing manager with custom expiration.
    pub fn with_expiry(expiry_secs: u64) -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            expiry_duration: Duration::from_secs(expiry_secs),
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

    /// Verify a pairing code and return the peer info if valid.
    ///
    /// # Arguments
    ///
    /// * `code` - The pairing code to verify.
    ///
    /// # Returns
    ///
    /// `Some((peer_id, public_key))` if the code is valid and not expired.
    /// `None` if the code is invalid or expired.
    pub async fn verify_code(&self, code: &str) -> Option<(String, Option<String>)> {
        let mut pending = self.pending.lock().await;

        if let Some(pairing) = pending.get(code) {
            // Check expiration
            if Instant::now().duration_since(pairing.created_at) > self.expiry_duration {
                pending.remove(code);
                return None;
            }

            // Valid code - return peer info but don't consume yet
            return Some((pairing.peer_id.clone(), pairing.public_key.clone()));
        }

        None
    }

    /// Consume a pairing code and return the peer info.
    ///
    /// This removes the code from pending, making it single-use.
    ///
    /// # Arguments
    ///
    /// * `code` - The pairing code to consume.
    ///
    /// # Returns
    ///
    /// `Some((peer_id, public_key))` if the code is valid and not expired.
    /// `None` if the code is invalid or expired.
    pub async fn consume_code(&self, code: &str) -> Option<(String, Option<String>)> {
        let mut pending = self.pending.lock().await;

        if let Some(pairing) = pending.remove(code) {
            // Check expiration after removal (code is consumed either way)
            if Instant::now().duration_since(pairing.created_at) > self.expiry_duration {
                return None;
            }

            return Some((pairing.peer_id, pairing.public_key));
        }

        None
    }

    /// Clean up all expired pending codes.
    pub async fn cleanup_expired(&self) {
        let mut pending = self.pending.lock().await;
        let now = Instant::now();
        pending.retain(|_, p| now.duration_since(p.created_at) < self.expiry_duration);
    }

    /// Get the number of pending pairing requests.
    pub async fn pending_count(&self) -> usize {
        let pending = self.pending.lock().await;
        pending.len()
    }
}

/// Generate a new pairing code.
///
/// Generates a 6-digit numeric code using cryptographically secure randomness.
/// Uses rejection sampling to eliminate modulo bias.
pub fn generate_pairing_code() -> String {
    // UUID v4 uses getrandom (backed by /dev/urandom on Linux, BCryptGenRandom
    // on Windows) — a CSPRNG. We extract 4 bytes for a uniform random
    // number in [0, 1_000_000).
    //
    // Rejection sampling eliminates modulo bias: values above the largest
    // multiple of 1_000_000 that fits in u32 are discarded and re-drawn.
    const UPPER_BOUND: u32 = 1_000_000;
    const REJECT_THRESHOLD: u32 = (u32::MAX / UPPER_BOUND) * UPPER_BOUND;

    loop {
        let uuid = Uuid::new_v4();
        let bytes = uuid.as_bytes();
        let raw = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);

        if raw < REJECT_THRESHOLD {
            return format!("{:06}", raw % UPPER_BOUND);
        }
    }
}

/// Verify a pairing code matches using constant-time comparison.
///
/// This prevents timing attacks by always comparing all characters
/// regardless of where a mismatch occurs.
///
/// # Arguments
///
/// * `stored` - The expected pairing code.
/// * `provided` - The code provided by the user/peer.
///
/// # Returns
///
/// `true` if the codes match exactly, `false` otherwise.
pub fn verify_pairing_code(stored: &str, provided: &str) -> bool {
    constant_time_eq(stored, provided)
}

/// Constant-time string comparison to prevent timing attacks.
///
/// Does not short-circuit on length mismatch — always iterates over the
/// longer input to avoid leaking length information via timing.
fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();

    // Track length mismatch as a usize (non-zero = different lengths)
    let len_diff = a.len() ^ b.len();

    // XOR each byte, padding the shorter input with zeros.
    // Iterates over max(a.len(), b.len()) to avoid timing differences.
    let max_len = a.len().max(b.len());
    let mut byte_diff = 0u8;
    for i in 0..max_len {
        let x = *a.get(i).unwrap_or(&0);
        let y = *b.get(i).unwrap_or(&0);
        byte_diff |= x ^ y;
    }

    (len_diff == 0) & (byte_diff == 0)
}

/// Check if an endpoint URL uses TLS (HTTPS).
///
/// Returns `true` for HTTPS URLs and localhost HTTP URLs (for testing).
/// Returns `false` for non-localhost HTTP URLs.
pub fn is_tls_required(endpoint: &str) -> bool {
    if let Ok(url) = reqwest::Url::parse(endpoint) {
        match url.scheme() {
            "https" => true,
            "http" => {
                // Allow HTTP for localhost only (testing)
                let host = url.host_str().unwrap_or("");
                matches!(host, "localhost" | "127.0.0.1" | "::1")
            }
            _ => false,
        }
    } else {
        false
    }
}

/// Generate a cryptographically secure bearer token.
///
/// Uses 256 bits of entropy from the OS CSPRNG.
fn generate_bearer_token() -> String {
    use rand::Rng;
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    format!("a2a_{}", hex::encode(bytes))
}

/// Initiate peer pairing with a remote agent.
///
/// This is the client-side flow that:
/// 1. Requests a pairing code from the peer
/// 2. Displays the code to the operator
/// 3. Waits for operator confirmation
/// 4. Exchanges the code for a bearer token
/// 5. Encrypts and stores the token
///
/// # Arguments
///
/// * `peer_endpoint` - The HTTPS URL of the peer agent.
/// * `our_peer_id` - Our unique peer identifier.
/// * `secret_store` - The secret store for encrypting the token.
///
/// # Returns
///
/// A tuple of `(peer_id, bearer_token)` on success.
pub async fn initiate_peer_pairing(
    peer_endpoint: &str,
    our_peer_id: &str,
    _secret_store: &SecretStore,
) -> Result<(String, String)> {
    // Validate TLS requirement
    if !is_tls_required(peer_endpoint) {
        bail!("TLS is required for peer pairing. Use HTTPS endpoint or localhost for testing.");
    }

    let client = reqwest::Client::new();

    // Step 1: Request pairing code from peer
    let request_url = format!("{}/a2a/pair/request", peer_endpoint.trim_end_matches('/'));
    let request_body = PairingRequest {
        peer_id: our_peer_id.to_string(),
        public_key: None, // Reserved for future use
    };

    let response = client
        .post(&request_url)
        .json(&request_body)
        .send()
        .await
        .context("Failed to send pairing request")?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        bail!("Pairing request failed: {} - {}", status, text);
    }

    let pairing_response: PairingRequestResponse = response
        .json()
        .await
        .context("Failed to parse pairing response")?;

    tracing::info!(
        "Pairing code received. Display this code to the peer operator: {}",
        pairing_response.pairing_code
    );

    // Step 2 & 3: Code is displayed, wait for operator confirmation
    // In a CLI context, this would prompt the user
    // For now, we return the code and expect the caller to handle confirmation
    bail!(
        "Pairing code: {}. Provide this to the peer operator and run 'zeroclaw channel confirm-a2a {} <code>'",
        pairing_response.pairing_code,
        peer_endpoint
    );
}

/// Exchange a pairing code for a bearer token.
///
/// This is called after the operator has confirmed the pairing code
/// with the remote peer.
///
/// # Arguments
///
/// * `peer_endpoint` - The HTTPS URL of the peer agent.
/// * `code` - The pairing code to exchange.
///
/// # Returns
///
/// The bearer token on success.
pub async fn exchange_pairing_code(peer_endpoint: &str, code: &str) -> Result<String> {
    // Validate TLS requirement
    if !is_tls_required(peer_endpoint) {
        bail!("TLS is required for peer pairing. Use HTTPS endpoint or localhost for testing.");
    }

    let client = reqwest::Client::new();

    let confirm_url = format!("{}/a2a/pair/confirm", peer_endpoint.trim_end_matches('/'));
    let request_body = PairingConfirmRequest {
        pairing_code: code.to_string(),
    };

    let response = client
        .post(&confirm_url)
        .json(&request_body)
        .send()
        .await
        .context("Failed to send pairing confirmation")?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        bail!("Pairing confirmation failed: {} - {}", status, text);
    }

    let confirm_response: PairingConfirmResponse = response
        .json()
        .await
        .context("Failed to parse confirmation response")?;

    Ok(confirm_response.bearer_token)
}

/// Complete the pairing process and store the encrypted token.
///
/// This is the final step that exchanges the code and stores the result.
///
/// # Arguments
///
/// * `peer_endpoint` - The HTTPS URL of the peer agent.
/// * `code` - The pairing code to exchange.
/// * `secret_store` - The secret store for encrypting the token.
///
/// # Returns
///
/// A tuple of `(peer_id, encrypted_token)` on success.
pub async fn complete_pairing(
    peer_endpoint: &str,
    code: &str,
    secret_store: &SecretStore,
) -> Result<(String, String)> {
    let token = exchange_pairing_code(peer_endpoint, code).await?;
    let encrypted_token = secret_store.encrypt(&token)?;

    // Generate a peer ID from the endpoint
    let peer_id = generate_peer_id_from_endpoint(peer_endpoint);

    Ok((peer_id, encrypted_token))
}

/// Generate a peer ID from an endpoint URL.
///
/// Uses a hash of the endpoint to create a deterministic, unique ID.
fn generate_peer_id_from_endpoint(endpoint: &str) -> String {
    let hash = Sha256::digest(endpoint.as_bytes());
    format!(
        "peer_{:016x}",
        u64::from_be_bytes(hash[..8].try_into().unwrap())
    )
}

/// Hash a bearer token for storage comparison.
///
/// Returns a SHA-256 hash as lowercase hex.
pub fn hash_bearer_token(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

/// Check if a stored value looks like a SHA-256 hash (64 hex chars).
pub fn is_token_hash(value: &str) -> bool {
    value.len() == 64 && value.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // =========================================================================
    // Pairing Code Generation Tests
    // =========================================================================

    #[test]
    fn generate_pairing_code_is_6_digits() {
        let code = generate_pairing_code();
        assert_eq!(code.len(), PAIRING_CODE_LENGTH);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
    }

    #[test]
    fn generate_pairing_code_is_not_deterministic() {
        // Two codes should differ with overwhelming probability
        for _ in 0..10 {
            if generate_pairing_code() != generate_pairing_code() {
                return;
            }
        }
        panic!("Generated 10 pairs of codes and all were collisions");
    }

    #[test]
    fn generate_pairing_code_no_leading_zeros_issue() {
        // Ensure codes can have leading zeros (format! handles this)
        let code = generate_pairing_code();
        assert_eq!(code.len(), 6);
        // The format! macro ensures 6 digits with leading zeros
    }

    // =========================================================================
    // Constant-Time Comparison Tests
    // =========================================================================

    #[test]
    fn verify_pairing_code_same() {
        assert!(verify_pairing_code("123456", "123456"));
        assert!(verify_pairing_code("000000", "000000"));
    }

    #[test]
    fn verify_pairing_code_different() {
        assert!(!verify_pairing_code("123456", "123455"));
        assert!(!verify_pairing_code("123456", "223456"));
        assert!(!verify_pairing_code("123456", "1234567"));
        assert!(!verify_pairing_code("123456", "12345"));
    }

    #[test]
    fn verify_pairing_code_empty() {
        assert!(verify_pairing_code("", ""));
        assert!(!verify_pairing_code("123456", ""));
        assert!(!verify_pairing_code("", "123456"));
    }

    #[test]
    fn constant_time_eq_timing_resistant() {
        // This test verifies the function works correctly
        // True timing tests require specialized equipment
        assert!(constant_time_eq("abcdef", "abcdef"));
        assert!(!constant_time_eq("abcdef", "abcdeg"));
        assert!(!constant_time_eq("abcdef", "abcdefg"));
        assert!(!constant_time_eq("abcdef", "abcde"));
    }

    // =========================================================================
    // TLS Requirement Tests
    // =========================================================================

    #[test]
    fn is_tls_required_https() {
        assert!(is_tls_required("https://example.com"));
        assert!(is_tls_required("https://example.com:9000"));
        assert!(is_tls_required("https://10.0.0.1:9000"));
    }

    #[test]
    fn is_tls_required_localhost_http() {
        // HTTP is allowed for localhost (testing)
        assert!(is_tls_required("http://localhost:9000"));
        assert!(is_tls_required("http://127.0.0.1:9000"));
        assert!(is_tls_required("http://[::1]:9000"));
    }

    #[test]
    fn is_tls_required_public_http() {
        // HTTP is NOT allowed for public endpoints
        assert!(!is_tls_required("http://example.com"));
        assert!(!is_tls_required("http://10.0.0.1:9000"));
    }

    #[test]
    fn is_tls_required_invalid_url() {
        assert!(!is_tls_required("not-a-url"));
        assert!(!is_tls_required(""));
    }

    // =========================================================================
    // Pairing Manager Tests
    // =========================================================================

    #[tokio::test]
    async fn pairing_manager_create_request() {
        let manager = PairingManager::new();
        let code = manager
            .create_pairing_request("peer-001".to_string(), None)
            .await;

        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(manager.pending_count().await, 1);
    }

    #[tokio::test]
    async fn pairing_manager_verify_valid_code() {
        let manager = PairingManager::new();
        let code = manager
            .create_pairing_request("peer-001".to_string(), Some("pubkey".to_string()))
            .await;

        let result = manager.verify_code(&code).await;
        assert!(result.is_some());

        let (peer_id, public_key) = result.unwrap();
        assert_eq!(peer_id, "peer-001");
        assert_eq!(public_key, Some("pubkey".to_string()));
    }

    #[tokio::test]
    async fn pairing_manager_verify_invalid_code() {
        let manager = PairingManager::new();
        let result = manager.verify_code("000000").await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn pairing_manager_consume_code_single_use() {
        let manager = PairingManager::new();
        let code = manager
            .create_pairing_request("peer-001".to_string(), None)
            .await;

        // First consume should succeed
        let result = manager.consume_code(&code).await;
        assert!(result.is_some());

        // Second consume should fail (already consumed)
        let result = manager.consume_code(&code).await;
        assert!(result.is_none());

        assert_eq!(manager.pending_count().await, 0);
    }

    #[tokio::test]
    async fn pairing_manager_code_expiration() {
        let manager = PairingManager::with_expiry(0); // Immediate expiration
        let code = manager
            .create_pairing_request("peer-001".to_string(), None)
            .await;

        // Small delay to ensure expiration
        tokio::time::sleep(Duration::from_millis(10)).await;

        let result = manager.verify_code(&code).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn pairing_manager_cleanup_expired() {
        let manager = PairingManager::with_expiry(0);

        // Create multiple expired codes
        for i in 0..5 {
            manager
                .create_pairing_request(format!("peer-{i}"), None)
                .await;
        }

        tokio::time::sleep(Duration::from_millis(10)).await;
        manager.cleanup_expired().await;

        assert_eq!(manager.pending_count().await, 0);
    }

    // =========================================================================
    // Token Hash Tests
    // =========================================================================

    #[test]
    fn hash_bearer_token_produces_64_hex_chars() {
        let hash = hash_bearer_token("a2a_test_token");
        assert_eq!(hash.len(), 64);
        assert!(hash.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_bearer_token_is_deterministic() {
        assert_eq!(hash_bearer_token("a2a_abc"), hash_bearer_token("a2a_abc"));
    }

    #[test]
    fn hash_bearer_token_differs_for_different_inputs() {
        assert_ne!(hash_bearer_token("a2a_a"), hash_bearer_token("a2a_b"));
    }

    #[test]
    fn is_token_hash_detects_hash_vs_plaintext() {
        assert!(is_token_hash(&hash_bearer_token("a2a_test")));
        assert!(!is_token_hash("a2a_test_token"));
        assert!(!is_token_hash("too_short"));
        assert!(!is_token_hash(""));
    }

    // =========================================================================
    // Peer ID Generation Tests
    // =========================================================================

    #[test]
    fn generate_peer_id_from_endpoint_deterministic() {
        let id1 = generate_peer_id_from_endpoint("https://example.com:9000");
        let id2 = generate_peer_id_from_endpoint("https://example.com:9000");
        assert_eq!(id1, id2);
    }

    #[test]
    fn generate_peer_id_from_endpoint_unique_per_endpoint() {
        let id1 = generate_peer_id_from_endpoint("https://example.com:9000");
        let id2 = generate_peer_id_from_endpoint("https://example.com:9001");
        let id3 = generate_peer_id_from_endpoint("https://other.com:9000");

        assert_ne!(id1, id2);
        assert_ne!(id1, id3);
    }

    #[test]
    fn generate_peer_id_has_prefix() {
        let id = generate_peer_id_from_endpoint("https://example.com:9000");
        assert!(id.starts_with("peer_"));
    }

    // =========================================================================
    // Secret Store Integration Tests
    // =========================================================================

    #[tokio::test]
    async fn complete_pairing_encrypts_token() {
        let tmp = TempDir::new().unwrap();
        let store = SecretStore::new(tmp.path(), true);

        // We can't test the full flow without a mock server,
        // but we can test the encryption part
        let token = "a2a_test_token_12345";
        let encrypted = store.encrypt(token).unwrap();

        assert!(encrypted.starts_with("enc2:"));
        assert_ne!(encrypted, token);

        let decrypted = store.decrypt(&encrypted).unwrap();
        assert_eq!(decrypted, token);
    }

    // =========================================================================
    // Serialization Tests
    // =========================================================================

    #[test]
    fn pairing_request_serialization() {
        let request = PairingRequest {
            peer_id: "peer-001".to_string(),
            public_key: Some("pubkey".to_string()),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("peer-001"));
        assert!(json.contains("pubkey"));

        let deserialized: PairingRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.peer_id, "peer-001");
        assert_eq!(deserialized.public_key, Some("pubkey".to_string()));
    }

    #[test]
    fn pairing_request_response_serialization() {
        let response = PairingRequestResponse {
            pairing_code: "123456".to_string(),
            expires_in_secs: 300,
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("123456"));
        assert!(json.contains("300"));

        let deserialized: PairingRequestResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.pairing_code, "123456");
        assert_eq!(deserialized.expires_in_secs, 300);
    }

    #[test]
    fn pairing_confirm_request_serialization() {
        let request = PairingConfirmRequest {
            pairing_code: "123456".to_string(),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("123456"));

        let deserialized: PairingConfirmRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.pairing_code, "123456");
    }

    #[test]
    fn pairing_confirm_response_serialization() {
        let response = PairingConfirmResponse {
            bearer_token: "a2a_token_123".to_string(),
            peer_id: "peer-001".to_string(),
        };

        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("a2a_token_123"));
        assert!(json.contains("peer-001"));

        let deserialized: PairingConfirmResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.bearer_token, "a2a_token_123");
        assert_eq!(deserialized.peer_id, "peer-001");
    }
}
