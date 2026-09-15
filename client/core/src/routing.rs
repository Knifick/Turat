//! Routing-запись: подписанный ответ на вопрос «в какой ящик писать этому человеку».
//! Node хранит её как есть и не может подменить адрес — запись подписана ключом
//! устройства, а список устройств — корневым ключом личности.

use serde::{Deserialize, Serialize};

use crate::{
    CoreError,
    identity::StoredIdentity,
    mailbox::{MailboxGrant, OwnedMailbox},
    protocol::WireIdentity,
};

const ROUTING_LIFETIME_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRevocation {
    pub device_id: String,
    pub revoked_at_unix_milliseconds: i64,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceListDocument {
    pub version: i32,
    pub user_id: String,
    pub sequence: i64,
    pub updated_at_unix_milliseconds: i64,
    pub devices: Vec<WireIdentity>,
    pub revocations: Vec<DeviceRevocation>,
}

/// Список устройств подписан корневым ключом: только он решает, кто говорит от лица личности.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedDeviceList {
    pub document: DeviceListDocument,
    pub document_json: String,
    pub signature: String,
}

impl SignedDeviceList {
    pub fn create(
        identity: &StoredIdentity,
        devices: Vec<WireIdentity>,
        revocations: Vec<DeviceRevocation>,
        sequence: i64,
    ) -> Result<Self, CoreError> {
        let document = DeviceListDocument {
            version: 2,
            user_id: identity.public.user_id.clone(),
            sequence,
            updated_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
            devices,
            revocations,
        };
        let document_json = serde_json::to_string(&document)?;
        let signature = identity.sign_identity(document_json.as_bytes())?;
        Ok(Self {
            document,
            document_json,
            signature,
        })
    }

    pub fn verify(&self) -> bool {
        let Ok(canonical) = serde_json::to_string(&self.document) else {
            return false;
        };
        if canonical != self.document_json
            || self.document.version != 2
            || self.document.devices.is_empty()
        {
            return false;
        }
        let Some(first) = self.document.devices.first() else {
            return false;
        };
        let Ok(identity_spki) =
            base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &first.identity_public_key)
        else {
            return false;
        };
        self.document
            .devices
            .iter()
            .all(|device| device.user_id == self.document.user_id && device.verify_certificate())
            && crate::protocol::verify_p256(
                &identity_spki,
                self.document_json.as_bytes(),
                &self.signature,
            )
    }

    pub fn contains(&self, device_id: &str) -> bool {
        self.document
            .devices
            .iter()
            .any(|device| device.device_id == device_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRoutingEntry {
    pub identity: WireIdentity,
    /// Публичные контактные ящики устройства: сюда пишут те, кому мы ещё не дали личный ключ.
    pub mailboxes: Vec<MailboxGrant>,
    pub prekey_sequence: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingDescriptor {
    pub version: i32,
    pub user_id: String,
    pub sequence: i64,
    pub created_at_unix_milliseconds: i64,
    pub expires_at_unix_milliseconds: i64,
    pub device_list: SignedDeviceList,
    pub devices: Vec<DeviceRoutingEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedRoutingDescriptor {
    pub descriptor: RoutingDescriptor,
    pub descriptor_json: String,
    pub signature: String,
}

impl SignedRoutingDescriptor {
    /// Новая запись поверх предыдущей: чужие устройства из старой записи сохраняются,
    /// своё — переписывается текущими ящиками.
    pub fn create(
        identity: &StoredIdentity,
        device_list: &SignedDeviceList,
        mailbox: &OwnedMailbox,
        prekey_sequence: i64,
        sequence: i64,
        previous: Option<&SignedRoutingDescriptor>,
    ) -> Result<Self, CoreError> {
        if !device_list.contains(&identity.public.device_id) {
            return Err(CoreError::Crypto(
                "Это устройство отозвано и не может публиковать адрес".to_owned(),
            ));
        }
        let mut devices: Vec<DeviceRoutingEntry> = previous
            .map(|value| {
                value
                    .descriptor
                    .devices
                    .iter()
                    .filter(|entry| {
                        entry.identity.device_id != identity.public.device_id
                            && device_list.contains(&entry.identity.device_id)
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        devices.push(DeviceRoutingEntry {
            identity: WireIdentity::from(&identity.public),
            mailboxes: vec![MailboxGrant::contact_of(mailbox)],
            prekey_sequence,
        });
        devices.sort_by(|left, right| left.identity.device_id.cmp(&right.identity.device_id));

        let now = chrono::Utc::now().timestamp_millis();
        let descriptor = RoutingDescriptor {
            version: 2,
            user_id: identity.public.user_id.clone(),
            sequence,
            created_at_unix_milliseconds: now,
            expires_at_unix_milliseconds: mailbox
                .expires_at_unix_milliseconds
                .min(now + ROUTING_LIFETIME_DAYS * 86_400_000),
            device_list: device_list.clone(),
            devices,
        };
        let descriptor_json = serde_json::to_string(&descriptor)?;
        let signature = identity.sign_device(descriptor_json.as_bytes())?;
        Ok(Self {
            descriptor,
            descriptor_json,
            signature,
        })
    }

    /// Проверка чужой записи: список устройств подписан личностью, сама запись —
    /// одним из перечисленных в нём устройств.
    pub fn verify(&self) -> bool {
        let Ok(canonical) = serde_json::to_string(&self.descriptor) else {
            return false;
        };
        if canonical != self.descriptor_json
            || self.descriptor.version != 2
            || self.descriptor.devices.is_empty()
            || self.descriptor.device_list.document.user_id != self.descriptor.user_id
            || self.descriptor.expires_at_unix_milliseconds
                <= chrono::Utc::now().timestamp_millis()
            || !self.descriptor.device_list.verify()
        {
            return false;
        }
        if self.descriptor.devices.iter().any(|entry| {
            entry.identity.user_id != self.descriptor.user_id
                || !self.descriptor.device_list.contains(&entry.identity.device_id)
                || !entry.identity.verify_certificate()
        }) {
            return false;
        }
        self.descriptor.devices.iter().any(|entry| {
            entry
                .identity
                .verify_device_data(self.descriptor_json.as_bytes(), &self.signature)
        })
    }

    /// Живые публичные ящики конкретного устройства.
    pub fn contact_mailboxes(&self, device_id: &str) -> Vec<MailboxGrant> {
        self.descriptor
            .devices
            .iter()
            .filter(|entry| entry.identity.device_id == device_id)
            .flat_map(|entry| entry.mailboxes.iter().cloned())
            .filter(MailboxGrant::alive)
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prekeys::random_token;

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
            expires_at_unix_milliseconds: chrono::Utc::now().timestamp_millis() + 86_400_000,
        }
    }

    #[test]
    fn a_published_descriptor_verifies() {
        let identity = StoredIdentity::create().expect("личность");
        let list = SignedDeviceList::create(
            &identity,
            vec![WireIdentity::from(&identity.public)],
            Vec::new(),
            1,
        )
        .expect("список устройств");
        assert!(list.verify());
        let routing =
            SignedRoutingDescriptor::create(&identity, &list, &mailbox(), 1, 1, None).expect("адрес");
        assert!(routing.verify());
        assert_eq!(
            routing.contact_mailboxes(&identity.public.device_id).len(),
            1
        );
    }

    #[test]
    fn a_tampered_descriptor_is_rejected() {
        let identity = StoredIdentity::create().expect("личность");
        let list = SignedDeviceList::create(
            &identity,
            vec![WireIdentity::from(&identity.public)],
            Vec::new(),
            1,
        )
        .expect("список устройств");
        let mut routing =
            SignedRoutingDescriptor::create(&identity, &list, &mailbox(), 1, 1, None).expect("адрес");
        routing.descriptor.devices[0].mailboxes[0].write_capability = random_token(32);
        assert!(!routing.verify());
    }
}
