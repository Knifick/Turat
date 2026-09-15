//! Предключи устройства: связка X25519 + ML-KEM-768, которую собеседник забирает с Node,
//! чтобы начать сессию, пока получатель офлайн. Гибрид нужен ради стойкости к «сохрани
//! сейчас — расшифруй потом»: классический DH и постквантовая инкапсуляция входят в один корень.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use ml_kem::{Encoded, EncodedSizeUser, KemCore, MlKem768, kem::Decapsulate, kem::Encapsulate};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use x25519_dalek::{PublicKey, StaticSecret};

use crate::{CoreError, identity::StoredIdentity, protocol::WireIdentity};

/// Сколько одноразовых предключей держим опубликованными: каждый первый контакт
/// съедает один, а пополняем мы их только во время синхронизации.
pub const DESIRED_ONE_TIME_PREKEYS: usize = 50;
const PREKEY_LIFETIME_DAYS: i64 = 30;
const RENEW_BEFORE_DAYS: i64 = 7;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DevicePrekeyDescriptor {
    pub version: i32,
    pub user_id: String,
    pub device_id: String,
    pub identity_dh_public_key: String,
    pub signed_prekey_id: String,
    pub signed_prekey_public_key: String,
    pub pq_prekey_id: String,
    pub pq_prekey_public_key: String,
    pub sequence: i64,
    pub expires_at_unix_milliseconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedDevicePrekey {
    pub descriptor: DevicePrekeyDescriptor,
    pub descriptor_json: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OneTimePrekeyPublic {
    pub prekey_id: String,
    pub public_key: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrekeyPublication {
    pub identity: WireIdentity,
    pub signed_prekey: SignedDevicePrekey,
    pub one_time_prekeys: Vec<OneTimePrekeyPublic>,
}

/// Связка, забранная с Node перед первым сообщением конкретному устройству.
#[derive(Debug, Clone)]
pub struct ClaimedPrekeyBundle {
    pub user_id: String,
    pub device_id: String,
    pub identity: WireIdentity,
    pub signed_prekey: SignedDevicePrekey,
    pub one_time_prekey: Option<OneTimePrekeyPublic>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredOneTimePrekey {
    pub prekey_id: String,
    pub private_key: String,
}

/// Локальные секреты предключей. Лежат в зашифрованном хранилище рядом с личностью.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrekeyState {
    pub sequence: i64,
    pub identity_dh_private_key: String,
    pub signed_prekey_id: String,
    pub signed_prekey_private_key: String,
    pub pq_prekey_id: String,
    pub pq_prekey_private_key: String,
    pub expires_at_unix_milliseconds: i64,
    pub one_time_prekeys: Vec<StoredOneTimePrekey>,
}

impl PrekeyState {
    pub fn create() -> Result<Self, CoreError> {
        let (decapsulation, _) = MlKem768::generate(&mut OsRng);
        let mut one_time = Vec::with_capacity(DESIRED_ONE_TIME_PREKEYS);
        for _ in 0..DESIRED_ONE_TIME_PREKEYS {
            one_time.push(create_one_time_prekey());
        }
        Ok(Self {
            sequence: 1,
            identity_dh_private_key: STANDARD.encode(random_secret().to_bytes()),
            signed_prekey_id: format!("spk1-{}", random_token(12)),
            signed_prekey_private_key: STANDARD.encode(random_secret().to_bytes()),
            pq_prekey_id: format!("pqk1-{}", random_token(12)),
            pq_prekey_private_key: STANDARD.encode(decapsulation.as_bytes()),
            expires_at_unix_milliseconds: chrono::Utc::now().timestamp_millis()
                + PREKEY_LIFETIME_DAYS * 86_400_000,
            one_time_prekeys: one_time,
        })
    }

    /// Обновляет связку, если она скоро истекает, и пополняет одноразовые предключи.
    /// Номер публикации всегда растёт: Node отвергает повтор с прежним номером.
    pub fn refresh(&mut self) -> Result<(), CoreError> {
        let horizon = chrono::Utc::now().timestamp_millis() + RENEW_BEFORE_DAYS * 86_400_000;
        if self.expires_at_unix_milliseconds < horizon {
            let sequence = self.sequence;
            *self = Self::create()?;
            self.sequence = sequence;
        }
        while self.one_time_prekeys.len() < DESIRED_ONE_TIME_PREKEYS {
            self.one_time_prekeys.push(create_one_time_prekey());
        }
        self.sequence += 1;
        Ok(())
    }

    /// Публичная связка с подписями устройства — ровно то, что уходит на Node.
    pub fn publication(
        &self,
        identity: &StoredIdentity,
    ) -> Result<PrekeyPublication, CoreError> {
        let wire = WireIdentity::from(&identity.public);
        let descriptor = DevicePrekeyDescriptor {
            version: 2,
            user_id: wire.user_id.clone(),
            device_id: wire.device_id.clone(),
            identity_dh_public_key: public_of(&self.identity_dh_private_key)?,
            signed_prekey_id: self.signed_prekey_id.clone(),
            signed_prekey_public_key: public_of(&self.signed_prekey_private_key)?,
            pq_prekey_id: self.pq_prekey_id.clone(),
            pq_prekey_public_key: STANDARD.encode(self.pq_encapsulation_key()?.as_bytes()),
            sequence: self.sequence,
            expires_at_unix_milliseconds: self.expires_at_unix_milliseconds,
        };
        let descriptor_json = serde_json::to_string(&descriptor)?;
        let signature = identity.sign_device(descriptor_json.as_bytes())?;
        let mut one_time = Vec::with_capacity(self.one_time_prekeys.len());
        for stored in &self.one_time_prekeys {
            let public_key = public_of(&stored.private_key)?;
            one_time.push(OneTimePrekeyPublic {
                prekey_id: stored.prekey_id.clone(),
                signature: identity
                    .sign_device(&one_time_signing_bytes(&stored.prekey_id, &public_key))?,
                public_key,
            });
        }
        Ok(PrekeyPublication {
            identity: wire,
            signed_prekey: SignedDevicePrekey {
                descriptor,
                descriptor_json,
                signature,
            },
            one_time_prekeys: one_time,
        })
    }

    /// Одноразовый предключ можно потратить только один раз — в этом весь смысл.
    pub fn consume_one_time(&mut self, prekey_id: &str) -> Option<String> {
        let index = self
            .one_time_prekeys
            .iter()
            .position(|value| value.prekey_id == prekey_id)?;
        Some(self.one_time_prekeys.remove(index).private_key)
    }

    pub fn pq_decapsulation_key(
        &self,
    ) -> Result<<MlKem768 as KemCore>::DecapsulationKey, CoreError> {
        let bytes = STANDARD.decode(&self.pq_prekey_private_key)?;
        let encoded = Encoded::<<MlKem768 as KemCore>::DecapsulationKey>::try_from(&bytes[..])
            .map_err(|_| CoreError::Crypto("Повреждён постквантовый предключ".to_owned()))?;
        Ok(<MlKem768 as KemCore>::DecapsulationKey::from_bytes(&encoded))
    }

    fn pq_encapsulation_key(&self) -> Result<<MlKem768 as KemCore>::EncapsulationKey, CoreError> {
        Ok(self.pq_decapsulation_key()?.encapsulation_key().clone())
    }
}

/// Проверка чужой связки: сертификат устройства, совпадение JSON с разобранным
/// описанием и подпись каждого одноразового ключа.
pub fn verify_publication(publication: &PrekeyPublication) -> bool {
    let descriptor = &publication.signed_prekey.descriptor;
    if !publication.identity.verify_certificate()
        || publication.identity.user_id != descriptor.user_id
        || publication.identity.device_id != descriptor.device_id
    {
        return false;
    }
    let Ok(canonical) = serde_json::to_string(descriptor) else {
        return false;
    };
    if canonical != publication.signed_prekey.descriptor_json
        || !publication.identity.verify_device_data(
            publication.signed_prekey.descriptor_json.as_bytes(),
            &publication.signed_prekey.signature,
        )
    {
        return false;
    }
    publication.one_time_prekeys.iter().all(|prekey| {
        publication.identity.verify_device_data(
            &one_time_signing_bytes(&prekey.prekey_id, &prekey.public_key),
            &prekey.signature,
        )
    })
}

pub fn encapsulate(public_key_base64: &str) -> Result<(String, [u8; 32]), CoreError> {
    let bytes = STANDARD.decode(public_key_base64)?;
    let encoded = Encoded::<<MlKem768 as KemCore>::EncapsulationKey>::try_from(&bytes[..])
        .map_err(|_| CoreError::Crypto("Некорректный постквантовый ключ".to_owned()))?;
    let key = <MlKem768 as KemCore>::EncapsulationKey::from_bytes(&encoded);
    let (ciphertext, shared) = key
        .encapsulate(&mut OsRng)
        .map_err(|_| CoreError::Crypto("Постквантовая инкапсуляция не удалась".to_owned()))?;
    Ok((STANDARD.encode(ciphertext), shared.into()))
}

pub fn decapsulate(state: &PrekeyState, ciphertext_base64: &str) -> Result<[u8; 32], CoreError> {
    let bytes = STANDARD.decode(ciphertext_base64)?;
    let ciphertext = ml_kem::Ciphertext::<MlKem768>::try_from(&bytes[..])
        .map_err(|_| CoreError::Crypto("Некорректный постквантовый шифротекст".to_owned()))?;
    let shared = state
        .pq_decapsulation_key()?
        .decapsulate(&ciphertext)
        .map_err(|_| CoreError::Crypto("Постквантовая декапсуляция не удалась".to_owned()))?;
    Ok(shared.into())
}

pub fn secret_from_base64(value: &str) -> Result<StaticSecret, CoreError> {
    let bytes: [u8; 32] = STANDARD
        .decode(value)?
        .try_into()
        .map_err(|_| CoreError::Crypto("Некорректный ключ X25519".to_owned()))?;
    Ok(StaticSecret::from(bytes))
}

pub fn public_from_base64(value: &str) -> Result<PublicKey, CoreError> {
    let bytes: [u8; 32] = STANDARD
        .decode(value)?
        .try_into()
        .map_err(|_| CoreError::Crypto("Некорректный публичный ключ X25519".to_owned()))?;
    Ok(PublicKey::from(bytes))
}

/// Согласование X25519: секрет пары «свой приватный — чужой публичный».
pub fn agree(private_key: &str, public_key: &str) -> Result<[u8; 32], CoreError> {
    Ok(*secret_from_base64(private_key)?
        .diffie_hellman(&public_from_base64(public_key)?)
        .as_bytes())
}

pub fn public_of(private_key_base64: &str) -> Result<String, CoreError> {
    Ok(STANDARD.encode(
        PublicKey::from(&secret_from_base64(private_key_base64)?).as_bytes(),
    ))
}

pub fn random_secret() -> StaticSecret {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    StaticSecret::from(bytes)
}

fn create_one_time_prekey() -> StoredOneTimePrekey {
    StoredOneTimePrekey {
        prekey_id: format!("otk1-{}", random_token(12)),
        private_key: STANDARD.encode(random_secret().to_bytes()),
    }
}

fn one_time_signing_bytes(prekey_id: &str, public_key: &str) -> Vec<u8> {
    format!("TuratText.OneTimePrekey.v2\n{prekey_id}\n{public_key}").into_bytes()
}

/// Идентификаторы должны быть безопасны для URL и путей — отсюда base64url без «=».
pub fn random_token(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    OsRng.fill_bytes(&mut value);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value)
}
