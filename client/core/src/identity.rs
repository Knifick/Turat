use base64::{Engine as _, engine::general_purpose::STANDARD};
use p256::ecdsa::{Signature, SigningKey, signature::Signer};
use p256::pkcs8::{DecodePrivateKey, EncodePrivateKey, EncodePublicKey};
use rand_core::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{CoreError, models::PublicIdentity};

const VERSION: i32 = 2;
const ALGORITHM: &str = "ECDSA-P256-SHA256";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredIdentity {
    identity_private_key: Option<String>,
    device_private_key: String,
    pub public: PublicIdentity,
}

impl StoredIdentity {
    pub fn create() -> Result<Self, CoreError> {
        let identity_key = SigningKey::random(&mut OsRng);
        let device_key = SigningKey::random(&mut OsRng);
        let identity_spki = identity_key
            .verifying_key()
            .to_public_key_der()?
            .as_bytes()
            .to_vec();
        let device_spki = device_key
            .verifying_key()
            .to_public_key_der()?
            .as_bytes()
            .to_vec();
        let identity_public_key = STANDARD.encode(&identity_spki);
        let device_public_key = STANDARD.encode(&device_spki);
        let user_id = id_from_spki("tt1", &identity_spki);
        let device_id = id_from_spki("ttd1", &device_spki);
        let created_at = chrono::Utc::now().timestamp_millis();
        let certificate_data = device_certificate_data(
            &user_id,
            &identity_public_key,
            &device_id,
            &device_public_key,
            created_at,
        );
        let signature: Signature = identity_key.sign(&certificate_data);
        let public = PublicIdentity {
            version: VERSION,
            user_id,
            identity_algorithm: ALGORITHM.to_owned(),
            identity_public_key,
            device_id,
            device_algorithm: ALGORITHM.to_owned(),
            device_public_key,
            created_at_unix_milliseconds: created_at,
            device_certificate: STANDARD.encode(signature.to_bytes()),
            is_authority: true,
        };
        Ok(Self {
            identity_private_key: Some(STANDARD.encode(identity_key.to_pkcs8_der()?.as_bytes())),
            device_private_key: STANDARD.encode(device_key.to_pkcs8_der()?.as_bytes()),
            public,
        })
    }

    pub fn sign_device(&self, value: &[u8]) -> Result<String, CoreError> {
        let der = STANDARD.decode(&self.device_private_key)?;
        let key = SigningKey::from_pkcs8_der(&der)?;
        let signature: Signature = key.sign(value);
        Ok(STANDARD.encode(signature.to_bytes()))
    }

    #[allow(dead_code)]
    pub fn sign_identity(&self, value: &[u8]) -> Result<String, CoreError> {
        let encoded = self.identity_private_key.as_ref().ok_or_else(|| {
            CoreError::InvalidInput("Операция доступна только корневому устройству".to_owned())
        })?;
        let der = STANDARD.decode(encoded)?;
        let key = SigningKey::from_pkcs8_der(&der)?;
        let signature: Signature = key.sign(value);
        Ok(STANDARD.encode(signature.to_bytes()))
    }

    pub fn is_authority(&self) -> bool {
        self.identity_private_key.is_some()
    }

    pub fn linked_device(&self) -> Result<Self, CoreError> {
        let root_encoded = self.identity_private_key.as_ref().ok_or_else(|| {
            CoreError::InvalidInput(
                "Связать устройство можно только на корневом устройстве".to_owned(),
            )
        })?;
        let root = SigningKey::from_pkcs8_der(&STANDARD.decode(root_encoded)?)?;
        let device = SigningKey::random(&mut OsRng);
        let spki = device
            .verifying_key()
            .to_public_key_der()?
            .as_bytes()
            .to_vec();
        let public_key = STANDARD.encode(&spki);
        let device_id = id_from_spki("ttd1", &spki);
        let created_at = chrono::Utc::now().timestamp_millis();
        let data = device_certificate_data(
            &self.public.user_id,
            &self.public.identity_public_key,
            &device_id,
            &public_key,
            created_at,
        );
        let signature: Signature = root.sign(&data);
        Ok(Self {
            identity_private_key: None,
            device_private_key: STANDARD.encode(device.to_pkcs8_der()?.as_bytes()),
            public: PublicIdentity {
                version: VERSION,
                user_id: self.public.user_id.clone(),
                identity_algorithm: ALGORITHM.to_owned(),
                identity_public_key: self.public.identity_public_key.clone(),
                device_id,
                device_algorithm: ALGORITHM.to_owned(),
                device_public_key: public_key,
                created_at_unix_milliseconds: created_at,
                device_certificate: STANDARD.encode(signature.to_bytes()),
                is_authority: false,
            },
        })
    }
}

pub fn conversation_id(left: &str, right: &str) -> String {
    let mut values = [left, right];
    values.sort_unstable();
    let mut hasher = Sha256::new();
    hasher.update(b"TuratText.Conversation.v2\0");
    hasher.update(values[0].as_bytes());
    hasher.update([0]);
    hasher.update(values[1].as_bytes());
    format!("ttc1-{}", hex::encode(hasher.finalize()))
}

fn id_from_spki(prefix: &str, spki: &[u8]) -> String {
    format!("{prefix}-{}", hex::encode(Sha256::digest(spki)))
}

fn device_certificate_data(
    user_id: &str,
    identity_public_key: &str,
    device_id: &str,
    device_public_key: &str,
    created_at: i64,
) -> Vec<u8> {
    let mut result = Vec::new();
    write_dotnet_string(&mut result, "TuratText.DeviceCertificate");
    result.extend_from_slice(&VERSION.to_le_bytes());
    for value in [
        user_id,
        ALGORITHM,
        identity_public_key,
        device_id,
        ALGORITHM,
        device_public_key,
    ] {
        write_i32_bytes(&mut result, value.as_bytes());
    }
    result.extend_from_slice(&created_at.to_le_bytes());
    result
}

fn write_i32_bytes(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as i32).to_le_bytes());
    target.extend_from_slice(value);
}

fn write_dotnet_string(target: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    let mut length = bytes.len() as u32;
    while length >= 0x80 {
        target.push((length as u8) | 0x80);
        length >>= 7;
    }
    target.push(length as u8);
    target.extend_from_slice(bytes);
}
