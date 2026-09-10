use std::{
    collections::HashMap,
    sync::Mutex,
    time::{Duration, Instant},
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{CoreError, models::DEFAULT_NODE_ID};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDescriptor {
    pub version: i32,
    pub node_id: String,
    pub name: String,
    pub base_url: String,
    pub public_key: String,
    pub algorithm: String,
    pub expires_at_unix_milliseconds: i64,
    pub transports: Vec<String>,
    pub registration_pow_bits: i32,
    pub envelope_pow_bits: i32,
    pub contact_pow_bits: i32,
    pub max_envelope_bytes: i32,
    pub max_mailbox_ttl_hours: i64,
    pub signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsernameWire {
    user_id: String,
    identity_public_key: String,
    claim_json: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UsernameClaim {
    version: i32,
    username: String,
    user_id: String,
    expires_at_unix_milliseconds: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ProfileWire {
    identity_public_key: String,
    claim_json: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresenceWire {
    identity_public_key: String,
    claim_json: String,
    signature: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PresenceClaim {
    version: i32,
    user_id: String,
    last_seen_unix_milliseconds: i64,
    expires_at_unix_milliseconds: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProfileClaim {
    pub user_id: String,
    pub display_name: String,
    pub about: String,
    pub avatar_base64: Option<String>,
    pub expires_at_unix_milliseconds: i64,
}

/// Descriptor меняется редко, а фоновая синхронизация идёт постоянно: без кэша каждый цикл
/// начинался бы с лишнего запроса и проверки подписи.
const DESCRIPTOR_CACHE_TTL: Duration = Duration::from_secs(600);

pub struct Network {
    http: reqwest::blocking::Client,
    descriptors: Mutex<HashMap<String, (Instant, NodeDescriptor)>>,
}

impl Network {
    pub fn new() -> Result<Self, CoreError> {
        Ok(Self {
            // Ядро выполняет команды по очереди, поэтому зависший запрос задерживает и действия
            // пользователя: фоновой синхронизации нужен короткий и предсказуемый предел.
            http: reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10))
                .https_only(false)
                .user_agent("Turat-Native/3.0")
                .build()?,
            descriptors: Mutex::new(HashMap::new()),
        })
    }

    pub fn descriptor(
        &self,
        base_url: &str,
        expected: Option<&str>,
    ) -> Result<NodeDescriptor, CoreError> {
        validate_node_url(base_url)?;
        let key = base_url.trim_end_matches('/').to_owned();
        if let Some(cached) = self.cached_descriptor(&key) {
            return Ok(cached);
        }
        let uri = format!("{key}/v2/node-descriptor");
        let descriptor: NodeDescriptor = self.http.get(uri).send()?.error_for_status()?.json()?;
        verify_descriptor(&descriptor, expected)?;
        if let Ok(mut cache) = self.descriptors.lock() {
            cache.insert(key, (Instant::now(), descriptor.clone()));
        }
        Ok(descriptor)
    }

    fn cached_descriptor(&self, key: &str) -> Option<NodeDescriptor> {
        let cache = self.descriptors.lock().ok()?;
        let (fetched, descriptor) = cache.get(key)?;
        let alive = fetched.elapsed() < DESCRIPTOR_CACHE_TTL
            && descriptor.expires_at_unix_milliseconds > chrono::Utc::now().timestamp_millis();
        alive.then(|| descriptor.clone())
    }

    /// Смена адреса Node должна проверяться заново, а не браться из кэша прошлого адреса.
    pub fn forget_descriptor(&self, base_url: &str) {
        if let Ok(mut cache) = self.descriptors.lock() {
            cache.remove(base_url.trim_end_matches('/'));
        }
    }

    pub fn resolve_username(
        &self,
        node: &NodeDescriptor,
        query: &str,
    ) -> Result<String, CoreError> {
        let username = normalize_username(query)?;
        let uri = format!(
            "{}/v2/usernames/{}",
            node.base_url.trim_end_matches('/'),
            username
        );
        let records: Vec<UsernameWire> = self.http.get(uri).send()?.error_for_status()?.json()?;
        let mut valid = Vec::new();
        for wire in records {
            let claim: UsernameClaim = serde_json::from_str(&wire.claim_json)?;
            if claim.version == 2
                && claim.username == username
                && claim.user_id == wire.user_id
                && claim.expires_at_unix_milliseconds > chrono::Utc::now().timestamp_millis()
                && verify_p256(
                    &wire.identity_public_key,
                    &wire.claim_json,
                    &wire.signature,
                    &wire.user_id,
                )
            {
                valid.push(wire.user_id);
            }
        }
        valid.sort();
        valid.dedup();
        match valid.len() {
            0 => Err(CoreError::InvalidInput("Username не найден".to_owned())),
            1 => Ok(valid.remove(0)),
            _ => Err(CoreError::InvalidInput(
                "Этот username связан с несколькими профилями. Для безопасного выбора введите полный UserID собеседника".to_owned(),
            )),
        }
    }

    pub fn profile(
        &self,
        node: &NodeDescriptor,
        user_id: &str,
    ) -> Result<Option<ProfileClaim>, CoreError> {
        let uri = format!(
            "{}/v2/profiles/{}",
            node.base_url.trim_end_matches('/'),
            user_id
        );
        let response = self.http.get(uri).send()?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let wire: ProfileWire = response.error_for_status()?.json()?;
        let claim: ProfileClaim = serde_json::from_str(&wire.claim_json)?;
        if claim.user_id != user_id
            || claim.expires_at_unix_milliseconds <= chrono::Utc::now().timestamp_millis()
            || !verify_p256(
                &wire.identity_public_key,
                &wire.claim_json,
                &wire.signature,
                user_id,
            )
        {
            return Err(CoreError::Crypto(
                "Directory вернул неподписанный профиль".to_owned(),
            ));
        }
        Ok(Some(claim))
    }
}

impl Network {
    /// Публикация подписанной записи профиля в directory Node.
    pub fn publish_profile(
        &self,
        node: &NodeDescriptor,
        user_id: &str,
        identity_public_key: &str,
        sequence: i64,
        claim_json: &str,
        signature: &str,
        expires_at_unix_milliseconds: i64,
    ) -> Result<(), CoreError> {
        let uri = format!(
            "{}/v2/profiles/{}",
            node.base_url.trim_end_matches('/'),
            user_id
        );
        self.put_claim(
            &uri,
            json!({
                "identityPublicKey": identity_public_key,
                "sequence": sequence,
                "claimJson": claim_json,
                "signature": signature,
                "expiresAt": rfc3339(expires_at_unix_milliseconds)?,
            }),
        )
    }

    /// Публикация username: только так собеседники смогут найти пользователя.
    pub fn publish_username(
        &self,
        node: &NodeDescriptor,
        username: &str,
        user_id: &str,
        identity_public_key: &str,
        sequence: i64,
        claim_json: &str,
        signature: &str,
        expires_at_unix_milliseconds: i64,
    ) -> Result<(), CoreError> {
        let uri = format!(
            "{}/v2/usernames/{}",
            node.base_url.trim_end_matches('/'),
            username
        );
        self.put_claim(
            &uri,
            json!({
                "userId": user_id,
                "identityPublicKey": identity_public_key,
                "sequence": sequence,
                "claimJson": claim_json,
                "signature": signature,
                "expiresAt": rfc3339(expires_at_unix_milliseconds)?,
            }),
        )
    }

    /// Публикация «последней активности». Вызывается только при явном согласии.
    pub fn publish_presence(
        &self,
        node: &NodeDescriptor,
        user_id: &str,
        identity_public_key: &str,
        sequence: i64,
        claim_json: &str,
        signature: &str,
        expires_at_unix_milliseconds: i64,
    ) -> Result<(), CoreError> {
        let uri = format!(
            "{}/v2/presence/{}",
            node.base_url.trim_end_matches('/'),
            user_id
        );
        self.put_claim(
            &uri,
            json!({
                "identityPublicKey": identity_public_key,
                "sequence": sequence,
                "claimJson": claim_json,
                "signature": signature,
                "expiresAt": rfc3339(expires_at_unix_milliseconds)?,
            }),
        )
    }

    /// «Последняя активность» собеседника, если он согласился её публиковать.
    pub fn presence(
        &self,
        node: &NodeDescriptor,
        user_id: &str,
    ) -> Result<Option<i64>, CoreError> {
        let uri = format!(
            "{}/v2/presence/{}",
            node.base_url.trim_end_matches('/'),
            user_id
        );
        let response = self.http.get(uri).send()?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let wire: PresenceWire = response.error_for_status()?.json()?;
        let claim: PresenceClaim = serde_json::from_str(&wire.claim_json)?;
        let now = chrono::Utc::now().timestamp_millis();
        // 0 — владелец скрыл активность: запись есть, но данных в ней нет.
        if claim.version != 1
            || claim.user_id != user_id
            || claim.last_seen_unix_milliseconds <= 0
            || claim.expires_at_unix_milliseconds <= now
            || claim.last_seen_unix_milliseconds > now + 60_000
            || !verify_p256(
                &wire.identity_public_key,
                &wire.claim_json,
                &wire.signature,
                user_id,
            )
        {
            return Ok(None);
        }
        Ok(Some(claim.last_seen_unix_milliseconds))
    }

    fn put_claim(&self, uri: &str, body: serde_json::Value) -> Result<(), CoreError> {
        self.http.put(uri).json(&body).send()?.error_for_status()?;
        Ok(())
    }
}

fn rfc3339(value: i64) -> Result<String, CoreError> {
    chrono::DateTime::from_timestamp_millis(value)
        .map(|moment| moment.to_rfc3339_opts(chrono::SecondsFormat::Millis, true))
        .ok_or_else(|| CoreError::InvalidInput("Некорректный срок записи".to_owned()))
}

pub fn normalize_username(value: &str) -> Result<String, CoreError> {
    let value = value.trim().trim_start_matches('@').to_ascii_lowercase();
    if value.len() < 3
        || value.len() > 32
        || !value.as_bytes()[0].is_ascii_alphanumeric()
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'))
    {
        return Err(CoreError::InvalidInput(
            "Username: 3–32 латинских символа, цифры, '.', '_' или '-'".to_owned(),
        ));
    }
    Ok(value)
}

fn verify_descriptor(value: &NodeDescriptor, expected: Option<&str>) -> Result<(), CoreError> {
    if value.version != 2
        || value.algorithm != "Ed25519"
        || value.expires_at_unix_milliseconds <= chrono::Utc::now().timestamp_millis()
        || expected.is_some_and(|id| id != value.node_id)
    {
        return Err(CoreError::Crypto(
            "Node descriptor не прошёл проверку".to_owned(),
        ));
    }
    let spki = STANDARD.decode(&value.public_key)?;
    let calculated = format!("ttn1-{}", hex::encode(Sha256::digest(&spki)));
    if calculated != value.node_id || spki.len() < 32 {
        return Err(CoreError::Crypto(
            "NodeID не соответствует публичному ключу".to_owned(),
        ));
    }
    let key_bytes: [u8; 32] = spki[spki.len() - 32..]
        .try_into()
        .map_err(|_| CoreError::Crypto("Некорректный ключ Node".to_owned()))?;
    let key = VerifyingKey::from_bytes(&key_bytes)?;
    let signature = Signature::from_slice(&STANDARD.decode(&value.signature)?)?;
    key.verify(&descriptor_canonical(value), &signature)
        .map_err(|_| CoreError::Crypto("Подпись Node descriptor недействительна".to_owned()))?;
    Ok(())
}

fn descriptor_canonical(value: &NodeDescriptor) -> Vec<u8> {
    let mut result = Vec::new();
    write_string(&mut result, "TuratText.NodeDescriptor");
    result.extend_from_slice(&value.version.to_be_bytes());
    for text in [
        &value.node_id,
        &value.name,
        &value.base_url,
        &value.public_key,
    ] {
        write_string(&mut result, text);
    }
    result.extend_from_slice(&value.expires_at_unix_milliseconds.to_be_bytes());
    result.extend_from_slice(&(value.transports.len() as i32).to_be_bytes());
    for transport in &value.transports {
        write_string(&mut result, transport);
    }
    result.extend_from_slice(&value.registration_pow_bits.to_be_bytes());
    result.extend_from_slice(&value.envelope_pow_bits.to_be_bytes());
    result.extend_from_slice(&value.contact_pow_bits.to_be_bytes());
    result.extend_from_slice(&value.max_envelope_bytes.to_be_bytes());
    result.extend_from_slice(&value.max_mailbox_ttl_hours.to_be_bytes());
    result
}

fn write_string(target: &mut Vec<u8>, value: &str) {
    target.extend_from_slice(&(value.len() as i32).to_be_bytes());
    target.extend_from_slice(value.as_bytes());
}

fn verify_p256(spki_base64: &str, content: &str, signature_base64: &str, user_id: &str) -> bool {
    use p256::ecdsa::{Signature as P256Signature, VerifyingKey, signature::Verifier as _};
    use p256::pkcs8::DecodePublicKey;
    let result = (|| {
        let spki = STANDARD.decode(spki_base64).ok()?;
        if format!("tt1-{}", hex::encode(Sha256::digest(&spki))) != user_id {
            return None;
        }
        let key = VerifyingKey::from_public_key_der(&spki).ok()?;
        let signature = P256Signature::from_slice(&STANDARD.decode(signature_base64).ok()?).ok()?;
        key.verify(content.as_bytes(), &signature).ok()
    })();
    result.is_some()
}

fn validate_node_url(url: &str) -> Result<(), CoreError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| CoreError::InvalidInput("Некорректный адрес Node".to_owned()))?;
    let local = matches!(parsed.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
        return Err(CoreError::InvalidInput(
            "Node должен использовать HTTPS".to_owned(),
        ));
    }
    Ok(())
}

pub fn expected_node_for(url: &str) -> Option<&'static str> {
    (url.trim_end_matches('/') == "https://turattext.rplacefree.store").then_some(DEFAULT_NODE_ID)
}
