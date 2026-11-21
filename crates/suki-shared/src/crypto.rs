use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Hash version and length framing are part of the persisted format.
fn digest(domain: &str, fields: &[&str]) -> [u8; 32] {
    let mut hash = Sha256::new();
    for field in std::iter::once(&domain).chain(fields) {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field.as_bytes());
    }
    hash.finalize().into()
}

pub fn hash_message(
    order: &str,
    sender: &str,
    role: &str,
    content: &str,
    timestamp: i64,
    nonce: &str,
) -> String {
    hex::encode(digest(
        "suki.chat.v1",
        &[order, sender, role, content, &timestamp.to_string(), nonce],
    ))
}

pub fn hash_transaction(
    order: &str,
    id: i64,
    event: &str,
    actor: &str,
    timestamp: i64,
    payload: &str,
    previous: &str,
) -> String {
    hex::encode(digest(
        "suki.transaction.v1",
        &[
            order,
            &id.to_string(),
            event,
            actor,
            &timestamp.to_string(),
            payload,
            previous,
        ],
    ))
}

pub fn hash_snapshot(payload: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(b"suki.checkout.v1\0");
    hash.update(payload);
    hex::encode(hash.finalize())
}

pub fn verify_hash(computed: &str, stored: &str) -> bool {
    computed == stored
}

pub fn gcash_reference_hash(reference: &str) -> String {
    hex::encode(Sha256::digest(
        reference.trim().to_ascii_uppercase().as_bytes(),
    ))
}

/// Keep the key on the server. Domain separation prevents reusing a tag
/// from one record type as authentication for another.
#[derive(Clone)]
pub struct IntegrityKey([u8; 32]);

impl IntegrityKey {
    pub fn from_hex(value: &str) -> Result<Self, hex::FromHexError> {
        let mut key = [0; 32];
        hex::decode_to_slice(value, &mut key)?;
        Ok(Self(key))
    }

    fn mac(&self, domain: &str, hash: &str) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("HMAC accepts any key length");
        mac.update(&digest(domain, &[hash]));
        mac
    }

    pub fn sign(&self, domain: &str, hash: &str) -> String {
        hex::encode(self.mac(domain, hash).finalize().into_bytes())
    }

    pub fn verify(&self, domain: &str, hash: &str, tag: &str) -> bool {
        let mut decoded = [0; 32];
        hex::decode_to_slice(tag, &mut decoded).is_ok()
            && self.mac(domain, hash).verify_slice(&decoded).is_ok()
    }
}

pub const SNAPSHOT_AUTH: &str = "suki.checkout.auth.v1";
pub const CHAT_AUTH: &str = "suki.chat.auth.v1";
pub const TRANSACTION_AUTH: &str = "suki.transaction.auth.v1";

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IntegrityReport {
    pub order_id: String,
    pub transactions_verified: u64,
    pub first_invalid_transaction: Option<i64>,
    pub messages_verified: u64,
    pub legacy_messages: u64,
    pub first_invalid_message: Option<i64>,
    #[serde(default)]
    pub snapshot: SnapshotVerification,
    pub recent: Vec<IntegrityEntry>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotVerification {
    #[default]
    Legacy,
    Verified,
    Invalid,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct IntegrityEntry {
    pub id: i64,
    pub event: String,
    pub created_at: i64,
    pub tag: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_payload_and_link_changes() {
        let key = IntegrityKey::from_hex(&"ab".repeat(32)).unwrap();
        let message = hash_message("order", "sender", "customer", "Hello", 1, "nonce");
        let altered = hash_message("order", "sender", "customer", "Bye", 1, "nonce");
        let tag = key.sign(CHAT_AUTH, &message);
        assert!(verify_hash(&message, &message));
        assert!(!verify_hash(&altered, &message));
        assert!(key.verify(CHAT_AUTH, &message, &tag));
        assert!(!key.verify(CHAT_AUTH, &altered, &tag));
        assert!(!key.verify(TRANSACTION_AUTH, &message, &tag));

        let expected = hash_transaction("order", 9, "status", "actor", 2, "paid", "prev");
        let tag = key.sign(TRANSACTION_AUTH, &expected);
        assert!(key.verify(TRANSACTION_AUTH, &expected, &tag));
        for changed in [
            hash_transaction("order", 9, "status", "actor", 2, "unpaid", "prev"),
            hash_transaction("order", 9, "status", "actor", 2, "paid", "other"),
            hash_transaction("other", 9, "status", "actor", 2, "paid", "prev"),
        ] {
            assert!(!key.verify(TRANSACTION_AUTH, &changed, &tag));
        }
    }
}
