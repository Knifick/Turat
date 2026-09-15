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

use crate::{
    CoreError,
    mailbox::{MailboxEnvelope, MailboxGrant, OwnedMailbox},
    models::DEFAULT_NODE_ID,
    prekeys::{
        ClaimedPrekeyBundle, OneTimePrekeyPublic, PrekeyPublication, SignedDevicePrekey,
        random_token,
    },
    routing::SignedRoutingDescriptor,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MailboxRegistrationWire {
    mailbox_id: String,
    device_hint: String,
    created_at: String,
    expires_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaimedPrekeyWire {
    user_id: String,
    device_id: String,
    identity_json: String,
    signed_prekey_json: String,
    one_time_prekey: Option<OneTimePrekeyWire>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OneTimePrekeyWire {
    prekey_json: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RoutingWire {
    user_id: String,
    sequence: i64,
    descriptor_json: String,
    signature: String,
}

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
    /// Предел Node на один блоб. По нему ядро отказывает слишком большому файлу заранее:
    /// иначе пользователь ждал бы шифрования гигабайта ради 400 на регистрации.
    pub max_blob_bytes: i64,
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

    /// Уже проверенный дескриптор без обращения к сети: нужен там, где ходить в Node незачем,
    /// например чтобы отказать слишком большому файлу до начала шифрования.
    pub fn known_descriptor(&self, base_url: &str) -> Option<NodeDescriptor> {
        self.cached_descriptor(base_url.trim_end_matches('/'))
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

    /// Регистрация собственного почтового ящика. Возможности придумывает клиент,
    /// Node хранит только их хеши — потерянный ящик восстановить нельзя, можно лишь создать новый.
    pub fn register_mailbox(
        &self,
        node: &NodeDescriptor,
        device_hint: &str,
    ) -> Result<OwnedMailbox, CoreError> {
        let read_capability = random_token(32);
        let write_capability = random_token(32);
        let contact_capability = random_token(32);
        let proof = crate::mailbox::registration_proof(
            &read_capability,
            &write_capability,
            &contact_capability,
            node.registration_pow_bits,
        );
        let expires_at =
            chrono::Utc::now() + chrono::Duration::hours(node.max_mailbox_ttl_hours.max(1));
        let uri = format!("{}/v2/mailboxes", node.base_url.trim_end_matches('/'));
        let registration: MailboxRegistrationWire = self
            .http
            .post(uri)
            .json(&json!({
                "readCapability": read_capability,
                "writeCapability": write_capability,
                "contactCapability": contact_capability,
                "deviceHint": device_hint,
                "expiresAt": crate::mailbox::rfc3339(expires_at),
                "proofNonce": proof,
            }))
            .send()?
            .error_for_status()?
            .json()?;
        Ok(OwnedMailbox {
            node_id: node.node_id.clone(),
            base_url: node.base_url.trim_end_matches('/').to_owned(),
            mailbox_id: registration.mailbox_id,
            device_hint: registration.device_hint,
            read_capability,
            write_capability,
            contact_capability,
            created_at_unix_milliseconds: crate::mailbox::parse_rfc3339(&registration.created_at)?,
            expires_at_unix_milliseconds: crate::mailbox::parse_rfc3339(&registration.expires_at)?,
        })
    }

    pub fn put_envelope(
        &self,
        grant: &MailboxGrant,
        envelope: &MailboxEnvelope,
        envelope_pow_bits: i32,
    ) -> Result<(), CoreError> {
        let uri = format!(
            "{}/v2/mailboxes/{}/envelopes",
            grant.base_url.trim_end_matches('/'),
            grant.mailbox_id
        );
        self.http
            .put(uri)
            .header("X-Mailbox-Write-Capability", &grant.write_capability)
            .header(
                "X-Envelope-Pow-Nonce",
                crate::mailbox::envelope_proof(&grant.mailbox_id, envelope, envelope_pow_bits),
            )
            .json(envelope)
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn fetch_envelopes(
        &self,
        mailbox: &OwnedMailbox,
        limit: u32,
    ) -> Result<Vec<MailboxEnvelope>, CoreError> {
        let uri = format!(
            "{}/v2/mailboxes/{}/envelopes?limit={}",
            mailbox.base_url.trim_end_matches('/'),
            mailbox.mailbox_id,
            limit.clamp(1, 200)
        );
        Ok(self
            .http
            .get(uri)
            .header("X-Mailbox-Read-Capability", &mailbox.read_capability)
            .send()?
            .error_for_status()?
            .json()?)
    }

    /// Подтверждение приёма: только после него Node удаляет конверт.
    pub fn acknowledge(
        &self,
        mailbox: &OwnedMailbox,
        envelope_ids: &[String],
    ) -> Result<(), CoreError> {
        if envelope_ids.is_empty() {
            return Ok(());
        }
        let uri = format!(
            "{}/v2/mailboxes/{}/ack",
            mailbox.base_url.trim_end_matches('/'),
            mailbox.mailbox_id
        );
        self.http
            .post(uri)
            .header("X-Mailbox-Read-Capability", &mailbox.read_capability)
            .json(&json!({ "envelopeIds": envelope_ids }))
            .send()?
            .error_for_status()?;
        Ok(())
    }

    pub fn publish_prekeys(
        &self,
        mailbox: &OwnedMailbox,
        publication: &PrekeyPublication,
    ) -> Result<(), CoreError> {
        let uri = format!(
            "{}/v2/prekeys/{}",
            mailbox.base_url.trim_end_matches('/'),
            mailbox.mailbox_id
        );
        let one_time: Vec<serde_json::Value> = publication
            .one_time_prekeys
            .iter()
            .map(|prekey| {
                Ok(json!({
                    "prekeyId": prekey.prekey_id,
                    "prekeyJson": serde_json::to_string(prekey)?,
                }))
            })
            .collect::<Result<_, CoreError>>()?;
        self.http
            .put(uri)
            .header("X-Mailbox-Write-Capability", &mailbox.write_capability)
            .json(&json!({
                "userId": publication.identity.user_id,
                "deviceId": publication.identity.device_id,
                "identityJson": serde_json::to_string(&publication.identity)?,
                "signedPrekeyJson": serde_json::to_string(&publication.signed_prekey)?,
                "sequence": publication.signed_prekey.descriptor.sequence,
                "expiresAt": rfc3339(
                    publication.signed_prekey.descriptor.expires_at_unix_milliseconds,
                )?,
                "oneTimePrekeys": one_time,
            }))
            .send()?
            .error_for_status()?;
        Ok(())
    }

    /// Забирает связку предключей устройства. Одноразовый ключ Node отдаёт ровно один раз,
    /// поэтому возвращённую связку нельзя терять — второй такой не будет.
    pub fn claim_prekeys(
        &self,
        base_url: &str,
        user_id: &str,
        device_id: &str,
    ) -> Result<ClaimedPrekeyBundle, CoreError> {
        let uri = format!(
            "{}/v2/prekeys/{user_id}/{device_id}/claim",
            base_url.trim_end_matches('/')
        );
        let wire: ClaimedPrekeyWire = self.http.get(uri).send()?.error_for_status()?.json()?;
        let identity: crate::protocol::WireIdentity = serde_json::from_str(&wire.identity_json)?;
        let signed_prekey: SignedDevicePrekey = serde_json::from_str(&wire.signed_prekey_json)?;
        let one_time = wire
            .one_time_prekey
            .map(|value| serde_json::from_str::<OneTimePrekeyPublic>(&value.prekey_json))
            .transpose()?;
        let publication = PrekeyPublication {
            identity: identity.clone(),
            signed_prekey: signed_prekey.clone(),
            one_time_prekeys: one_time.clone().into_iter().collect(),
        };
        if !crate::prekeys::verify_publication(&publication)
            || wire.user_id != identity.user_id
            || wire.device_id != identity.device_id
            || wire.user_id != user_id
            || wire.device_id != device_id
        {
            return Err(CoreError::Crypto(
                "Node вернул неподписанную связку предключей".to_owned(),
            ));
        }
        Ok(ClaimedPrekeyBundle {
            user_id: wire.user_id,
            device_id: wire.device_id,
            identity,
            signed_prekey,
            one_time_prekey: one_time,
        })
    }

    pub fn publish_routing(
        &self,
        node: &NodeDescriptor,
        routing: &SignedRoutingDescriptor,
    ) -> Result<(), CoreError> {
        let uri = format!(
            "{}/v2/routing/{}",
            node.base_url.trim_end_matches('/'),
            routing.descriptor.user_id
        );
        self.put_claim(
            &uri,
            json!({
                "identityPublicKey": routing
                    .descriptor
                    .device_list
                    .document
                    .devices
                    .first()
                    .map(|device| device.identity_public_key.clone())
                    .unwrap_or_default(),
                "sequence": routing.descriptor.sequence,
                "descriptorJson": routing.descriptor_json,
                "signature": routing.signature,
                "expiresAt": rfc3339(routing.descriptor.expires_at_unix_milliseconds)?,
            }),
        )
    }

    /// Адрес собеседника. `None` — записи ещё нет: человек не заходил в сеть с этого клиента.
    pub fn routing(
        &self,
        node: &NodeDescriptor,
        user_id: &str,
    ) -> Result<Option<SignedRoutingDescriptor>, CoreError> {
        let uri = format!(
            "{}/v2/routing/{user_id}",
            node.base_url.trim_end_matches('/')
        );
        let response = self.http.get(uri).send()?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let wire: RoutingWire = response.error_for_status()?.json()?;
        let descriptor: crate::routing::RoutingDescriptor =
            serde_json::from_str(&wire.descriptor_json)?;
        let signed = SignedRoutingDescriptor {
            descriptor,
            descriptor_json: wire.descriptor_json,
            signature: wire.signature,
        };
        if wire.user_id != user_id
            || wire.sequence != signed.descriptor.sequence
            || !signed.verify()
        {
            return Err(CoreError::Crypto(
                "Node вернул неподписанный адрес".to_owned(),
            ));
        }
        Ok(Some(signed))
    }

    pub fn blob_register(
        &self,
        base_url: &str,
        object_id: &str,
        read_capability: &str,
        write_capability: &str,
        expected_size: u64,
        chunk_size: i32,
        expires_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), CoreError> {
        let uri = format!("{}/v2/blobs", base_url.trim_end_matches('/'));
        checked(
            self.http
                .post(uri)
                .json(&json!({
                    "objectId": object_id,
                    "readCapability": read_capability,
                    "writeCapability": write_capability,
                    "expectedSize": expected_size,
                    "chunkSize": chunk_size,
                    "expiresAt": crate::mailbox::rfc3339(expires_at),
                }))
                .send()?,
        )?;
        Ok(())
    }

    pub fn blob_put_chunk(
        &self,
        base_url: &str,
        object_id: &str,
        index: i32,
        write_capability: &str,
        digest: &str,
        body: Vec<u8>,
    ) -> Result<(), CoreError> {
        let uri = format!(
            "{}/v2/blobs/{object_id}/chunks/{index}",
            base_url.trim_end_matches('/')
        );
        checked(
            self.http
                .put(uri)
                .header("X-Blob-Write-Capability", write_capability)
                .header("X-Chunk-SHA256", digest)
                .header("Content-Type", "application/octet-stream")
                .body(body)
                .send()?,
        )?;
        Ok(())
    }

    pub fn blob_get_chunk(
        &self,
        base_url: &str,
        object_id: &str,
        index: i32,
        read_capability: &str,
    ) -> Result<Vec<u8>, CoreError> {
        let uri = format!(
            "{}/v2/blobs/{object_id}/chunks/{index}",
            base_url.trim_end_matches('/')
        );
        Ok(self
            .http
            .get(uri)
            .header("X-Blob-Read-Capability", read_capability)
            .send()?
            .error_for_status()?
            .bytes()?
            .to_vec())
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
    result.extend_from_slice(&value.max_blob_bytes.to_be_bytes());
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

/// Ответ Node вместе с его объяснением. `error_for_status` оставляет от отказа только код,
/// а Node кладёт в тело причину — пользователю нужна именно она, а не «400 Bad Request».
fn checked(
    response: reqwest::blocking::Response,
) -> Result<reqwest::blocking::Response, CoreError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.text().unwrap_or_default();
    let message = serde_json::from_str::<serde_json::Value>(&body)
        .ok()
        .and_then(|value| value["message"].as_str().map(str::to_owned))
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| format!("HTTP {}", status.as_u16()));
    Err(CoreError::Node(message))
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

/// Адрес собственного ящика для фонового ожидания. Отдельная структура нужна затем, чтобы
/// ждать конверт, не удерживая ядро: в ней нет ничего, кроме того, что требуется одному
/// GET-запросу.
#[derive(Debug, Clone)]
pub struct MailboxWatch {
    pub base_url: String,
    pub mailbox_id: String,
    pub read_capability: String,
}

impl MailboxWatch {
    pub fn of(mailbox: &OwnedMailbox) -> Self {
        Self {
            base_url: mailbox.base_url.clone(),
            mailbox_id: mailbox.mailbox_id.clone(),
            read_capability: mailbox.read_capability.clone(),
        }
    }
}

/// Максимальное окно ожидания: Node ограничивает его своей настройкой, здесь — верхняя
/// граница, под которую строится таймаут HTTP-клиента.
pub const MAX_WATCH_SECONDS: u32 = 25;

/// Долгий опрос почтового ящика. У общего клиента таймаут 10 секунд — он рассчитан на
/// команды пользователя; здесь запрос обязан висеть всё окно ожидания, поэтому клиент свой.
pub struct MailboxWatcher {
    http: reqwest::blocking::Client,
}

impl MailboxWatcher {
    pub fn new() -> Result<Self, CoreError> {
        Ok(Self {
            http: reqwest::blocking::Client::builder()
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(MAX_WATCH_SECONDS as u64 + 15))
                .https_only(false)
                .user_agent("Turat-Native/3.0")
                .build()?,
        })
    }

    /// Ждёт конверт до `seconds` секунд. `true` — в ящике что-то есть и пора синхронизироваться,
    /// `false` — окно истекло впустую. Сами конверты забирает ядро: здесь только сигнал.
    pub fn wait(&self, watch: &MailboxWatch, seconds: u32) -> Result<bool, CoreError> {
        validate_node_url(&watch.base_url)?;
        let uri = format!(
            "{}/v2/mailboxes/{}/envelopes?limit=1&wait={}",
            watch.base_url.trim_end_matches('/'),
            watch.mailbox_id,
            seconds.clamp(1, MAX_WATCH_SECONDS)
        );
        let envelopes: Vec<MailboxEnvelope> = self
            .http
            .get(uri)
            .header("X-Mailbox-Read-Capability", &watch.read_capability)
            .send()?
            .error_for_status()?
            .json()?;
        Ok(!envelopes.is_empty())
    }
}
