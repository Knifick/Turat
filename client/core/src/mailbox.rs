//! Почтовый ящик на Node: внешний слой доставки. Node видит только непрозрачный конверт
//! фиксированного размера и возможности (capability), по которым разрешает запись и чтение.
//! Ни адресата, ни отправителя, ни длины сообщения в открытом виде здесь нет.

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{CoreError, prekeys::random_token};

const FORMAT_VERSION: u8 = 1;
const NONCE_SIZE: usize = 12;
const TAG_SIZE: usize = 16;
/// Все конверты выравниваются до одного из этих размеров: по длине шифротекста
/// нельзя отличить «ок» от абзаца текста.
const SIZE_CLASSES: [usize; 6] = [1024, 4096, 16384, 65536, 262144, 524288];

/// Собственный ящик: сюда приходят конверты, отсюда мы их забираем и подтверждаем.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OwnedMailbox {
    pub node_id: String,
    pub base_url: String,
    pub mailbox_id: String,
    pub device_hint: String,
    pub read_capability: String,
    pub write_capability: String,
    /// Публичная возможность записи: её узнаёт любой, кто видит routing-запись,
    /// поэтому Node держит для неё отдельную жёсткую квоту.
    pub contact_capability: String,
    pub created_at_unix_milliseconds: i64,
    pub expires_at_unix_milliseconds: i64,
}

/// Право писать в чужой ящик: либо публичное из routing-записи, либо личное,
/// присланное собеседником внутри зашифрованного пакета.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxGrant {
    pub node_id: String,
    pub base_url: String,
    pub mailbox_id: String,
    pub device_hint: String,
    pub write_capability: String,
    pub expires_at_unix_milliseconds: i64,
}

impl MailboxGrant {
    pub fn from_owned(value: &OwnedMailbox) -> Self {
        Self {
            node_id: value.node_id.clone(),
            base_url: value.base_url.clone(),
            mailbox_id: value.mailbox_id.clone(),
            device_hint: value.device_hint.clone(),
            write_capability: value.write_capability.clone(),
            expires_at_unix_milliseconds: value.expires_at_unix_milliseconds,
        }
    }

    /// Публичный адрес для routing-записи: наружу уходит contact-возможность,
    /// личный ключ записи остаётся только у тех, кому мы уже ответили.
    pub fn contact_of(value: &OwnedMailbox) -> Self {
        Self {
            node_id: value.node_id.clone(),
            base_url: value.base_url.clone(),
            mailbox_id: value.mailbox_id.clone(),
            device_hint: value.device_hint.clone(),
            write_capability: value.contact_capability.clone(),
            expires_at_unix_milliseconds: value.expires_at_unix_milliseconds,
        }
    }

    pub fn alive(&self) -> bool {
        self.expires_at_unix_milliseconds > chrono::Utc::now().timestamp_millis()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MailboxEnvelope {
    pub envelope_id: String,
    pub protocol_version: i32,
    pub recipient_device_hint: Option<String>,
    pub opaque_payload: String,
    pub size_class: i32,
    pub created_at: String,
    pub expires_at: String,
}

/// Упаковка внутреннего сообщения в конверт: паддинг до класса размера,
/// затем AES-GCM на ключе, выведенном из возможности записи.
pub fn encode(
    grant: &MailboxGrant,
    inner: &[u8],
    ttl_hours: i64,
) -> Result<MailboxEnvelope, CoreError> {
    let minimum = 1 + NONCE_SIZE + 4 + inner.len() + TAG_SIZE;
    let size_class = SIZE_CLASSES
        .into_iter()
        .find(|value| *value >= minimum)
        .ok_or_else(|| CoreError::InvalidInput("Сообщение не помещается в конверт".to_owned()))?;
    let padded = size_class - 1 - NONCE_SIZE - TAG_SIZE;
    let mut plaintext = vec![0u8; padded];
    OsRng.fill_bytes(&mut plaintext);
    plaintext[..4].copy_from_slice(&(inner.len() as i32).to_be_bytes());
    plaintext[4..4 + inner.len()].copy_from_slice(inner);

    let mut nonce = [0u8; NONCE_SIZE];
    OsRng.fill_bytes(&mut nonce);
    let key = outer_key(&grant.mailbox_id, &grant.write_capability);
    let sealed = Aes256Gcm::new_from_slice(&key)
        .map_err(|_| CoreError::Crypto("Некорректный ключ конверта".to_owned()))?
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &plaintext,
                aad: grant.node_id.as_bytes(),
            },
        )
        .map_err(|_| CoreError::Crypto("Не удалось запечатать конверт".to_owned()))?;

    let mut packed = Vec::with_capacity(1 + NONCE_SIZE + sealed.len());
    packed.push(FORMAT_VERSION);
    packed.extend_from_slice(&nonce);
    packed.extend_from_slice(&sealed);
    let now = chrono::Utc::now();
    Ok(MailboxEnvelope {
        envelope_id: format!("env1-{}", random_token(18)),
        protocol_version: 2,
        recipient_device_hint: None,
        opaque_payload: STANDARD.encode(&packed),
        size_class: size_class as i32,
        created_at: rfc3339(now),
        expires_at: rfc3339(now + chrono::Duration::hours(ttl_hours)),
    })
}

/// Распаковка: сначала пробуем личный ключ записи, затем публичный контактный.
pub fn decode(mailbox: &OwnedMailbox, envelope: &MailboxEnvelope) -> Result<Vec<u8>, CoreError> {
    let packed = STANDARD.decode(&envelope.opaque_payload)?;
    if packed.len() < 1 + NONCE_SIZE + TAG_SIZE + 4 || packed[0] != FORMAT_VERSION {
        return Err(CoreError::Crypto("Неизвестный формат конверта".to_owned()));
    }
    let nonce = &packed[1..1 + NONCE_SIZE];
    let sealed = &packed[1 + NONCE_SIZE..];
    let plaintext = [&mailbox.write_capability, &mailbox.contact_capability]
        .into_iter()
        .find_map(|capability| {
            let key = outer_key(&mailbox.mailbox_id, capability);
            Aes256Gcm::new_from_slice(&key).ok()?.decrypt(
                Nonce::from_slice(nonce),
                Payload {
                    msg: sealed,
                    aad: mailbox.node_id.as_bytes(),
                },
            )
            .ok()
        })
        .ok_or_else(|| CoreError::Crypto("Конверт не расшифровывается".to_owned()))?;
    if plaintext.len() < 4 {
        return Err(CoreError::Crypto("Пустой конверт".to_owned()));
    }
    let length = i32::from_be_bytes(plaintext[..4].try_into().expect("4 байта")) as usize;
    if length > plaintext.len() - 4 {
        return Err(CoreError::Crypto("Некорректная длина конверта".to_owned()));
    }
    Ok(plaintext[4..4 + length].to_vec())
}

/// Ключ внешнего слоя привязан к конкретному ящику и конкретной возможности:
/// отозвав возможность, владелец разом обесценивает и все старые конверты.
fn outer_key(mailbox_id: &str, capability: &str) -> [u8; 32] {
    let material = format!(
        "TuratText.MailboxOuter.v1\0{}\0{capability}",
        mailbox_id.replace('-', "")
    );
    Sha256::digest(material.as_bytes()).into()
}

/// Доказательство работы: Node включает его, когда нужно притормозить спам.
pub fn registration_proof(
    read: &str,
    write: &str,
    contact: &str,
    bits: i32,
) -> String {
    solve(&format!("{read}:{write}:{contact}:"), bits)
}

pub fn envelope_proof(mailbox_id: &str, envelope: &MailboxEnvelope, bits: i32) -> String {
    let payload_hash = hex::encode(Sha256::digest(envelope.opaque_payload.as_bytes()));
    solve(
        &format!("{mailbox_id}:{}:{payload_hash}:", envelope.envelope_id),
        bits,
    )
}

fn solve(prefix: &str, bits: i32) -> String {
    if bits <= 0 {
        return "0".to_owned();
    }
    let mut nonce: u64 = 0;
    loop {
        let candidate = nonce.to_string();
        if leading_zero_bits(&Sha256::digest(format!("{prefix}{candidate}").as_bytes())) >= bits {
            return candidate;
        }
        nonce += 1;
    }
}

fn leading_zero_bits(value: &[u8]) -> i32 {
    let mut count = 0;
    for byte in value {
        if *byte == 0 {
            count += 8;
            continue;
        }
        count += byte.leading_zeros() as i32;
        break;
    }
    count
}

pub fn rfc3339(value: chrono::DateTime<chrono::Utc>) -> String {
    value.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

pub fn parse_rfc3339(value: &str) -> Result<i64, CoreError> {
    Ok(chrono::DateTime::parse_from_rfc3339(value)
        .map_err(|_| CoreError::Crypto("Некорректная дата от Node".to_owned()))?
        .timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mailbox() -> OwnedMailbox {
        OwnedMailbox {
            node_id: "ttn1-test".to_owned(),
            base_url: "https://example.invalid".to_owned(),
            mailbox_id: "6f1f0e1e-2b3c-4d5e-8f90-a1b2c3d4e5f6".to_owned(),
            device_hint: "ttd1-test".to_owned(),
            read_capability: random_token(32),
            write_capability: random_token(32),
            contact_capability: random_token(32),
            created_at_unix_milliseconds: 0,
            expires_at_unix_milliseconds: i64::MAX,
        }
    }

    #[test]
    fn envelope_round_trips_through_both_capabilities() {
        let owned = mailbox();
        for grant in [
            MailboxGrant::from_owned(&owned),
            MailboxGrant::contact_of(&owned),
        ] {
            let envelope = encode(&grant, b"secret payload", 24).expect("конверт");
            assert_eq!(envelope.size_class, 1024);
            assert_eq!(decode(&owned, &envelope).expect("чтение"), b"secret payload");
        }
    }

    #[test]
    fn a_foreign_capability_cannot_open_the_envelope() {
        let owned = mailbox();
        let mut grant = MailboxGrant::from_owned(&owned);
        grant.write_capability = random_token(32);
        let envelope = encode(&grant, b"secret payload", 24).expect("конверт");
        assert!(decode(&owned, &envelope).is_err());
    }

    #[test]
    fn padding_hides_the_message_length() {
        let owned = mailbox();
        let grant = MailboxGrant::from_owned(&owned);
        let short = encode(&grant, b"ok", 24).expect("конверт");
        let long = encode(&grant, &vec![7u8; 900], 24).expect("конверт");
        assert_eq!(short.opaque_payload.len(), long.opaque_payload.len());
    }
}
