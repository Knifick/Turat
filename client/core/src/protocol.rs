//! Формат событий, которые уходят собеседнику: подписанное событие, полезная нагрузка
//! и «пакет доставки» с обратным адресом. Кодирование повторяет протокол v2 старого
//! клиента, поэтому подписи проверяются одинаково на всех платформах.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use p256::ecdsa::{Signature, VerifyingKey, signature::Verifier};
use p256::pkcs8::DecodePublicKey;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{CoreError, models::PublicIdentity};

pub const PROTOCOL_VERSION: i32 = 2;
const ALGORITHM: &str = "ECDSA-P256-SHA256";

/// Публичная часть личности в том виде, в каком она уходит в сеть. От локальной
/// [`PublicIdentity`] отличается отсутствием флага `isAuthority`: это домашняя пометка,
/// собеседнику её знать незачем, а подпись считается ровно по этим девяти полям.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireIdentity {
    pub version: i32,
    pub user_id: String,
    pub identity_algorithm: String,
    pub identity_public_key: String,
    pub device_id: String,
    pub device_algorithm: String,
    pub device_public_key: String,
    pub created_at_unix_milliseconds: i64,
    pub device_certificate: String,
}

impl From<&PublicIdentity> for WireIdentity {
    fn from(value: &PublicIdentity) -> Self {
        Self {
            version: value.version,
            user_id: value.user_id.clone(),
            identity_algorithm: value.identity_algorithm.clone(),
            identity_public_key: value.identity_public_key.clone(),
            device_id: value.device_id.clone(),
            device_algorithm: value.device_algorithm.clone(),
            device_public_key: value.device_public_key.clone(),
            created_at_unix_milliseconds: value.created_at_unix_milliseconds,
            device_certificate: value.device_certificate.clone(),
        }
    }
}

impl WireIdentity {
    /// UserID и DeviceID — это хеши ключей, а сертификат подписан identity-ключом.
    /// Проверка связывает всё вместе: подставить чужое устройство в личность нельзя.
    pub fn verify_certificate(&self) -> bool {
        if self.version != PROTOCOL_VERSION
            || self.identity_algorithm != ALGORITHM
            || self.device_algorithm != ALGORITHM
        {
            return false;
        }
        let Ok(identity_spki) = STANDARD.decode(&self.identity_public_key) else {
            return false;
        };
        let Ok(device_spki) = STANDARD.decode(&self.device_public_key) else {
            return false;
        };
        if format!("tt1-{}", hex::encode(Sha256::digest(&identity_spki))) != self.user_id
            || format!("ttd1-{}", hex::encode(Sha256::digest(&device_spki))) != self.device_id
        {
            return false;
        }
        verify_p256(
            &identity_spki,
            &certificate_bytes(self),
            &self.device_certificate,
        )
    }

    /// Проверка произвольных данных, подписанных ключом устройства.
    pub fn verify_device_data(&self, value: &[u8], signature: &str) -> bool {
        let Ok(spki) = STANDARD.decode(&self.device_public_key) else {
            return false;
        };
        verify_p256(&spki, value, signature)
    }
}

fn certificate_bytes(value: &WireIdentity) -> Vec<u8> {
    let mut result = Vec::new();
    write_dotnet_string(&mut result, "TuratText.DeviceCertificate");
    result.extend_from_slice(&value.version.to_le_bytes());
    for text in [
        &value.user_id,
        &value.identity_algorithm,
        &value.identity_public_key,
        &value.device_id,
        &value.device_algorithm,
        &value.device_public_key,
    ] {
        write_i32_bytes(&mut result, text.as_bytes());
    }
    result.extend_from_slice(&value.created_at_unix_milliseconds.to_le_bytes());
    result
}

/// Событие диалога: текст, правка, реакция или квитанция. Подпись накрывает
/// канонические байты, а не JSON, поэтому переупаковка формата ничего не ломает.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedProtocolEvent {
    pub version: i32,
    pub event_id: String,
    pub conversation_id: String,
    pub sender_user_id: String,
    pub sender_device_id: String,
    pub device_sequence: i64,
    pub kind: String,
    pub created_at_unix_milliseconds: i64,
    /// base64 полезной нагрузки: ядро не разбирает её до проверки подписи.
    pub payload: String,
    pub signature: String,
}

impl SignedProtocolEvent {
    pub fn canonical_bytes(&self) -> Result<Vec<u8>, CoreError> {
        let mut result = Vec::new();
        write_dotnet_string(&mut result, "TuratText.SignedEvent");
        result.extend_from_slice(&self.version.to_le_bytes());
        for text in [
            &self.event_id,
            &self.conversation_id,
            &self.sender_user_id,
            &self.sender_device_id,
        ] {
            write_i32_bytes(&mut result, text.as_bytes());
        }
        result.extend_from_slice(&self.device_sequence.to_le_bytes());
        write_i32_bytes(&mut result, self.kind.as_bytes());
        result.extend_from_slice(&self.created_at_unix_milliseconds.to_le_bytes());
        write_i32_bytes(&mut result, &STANDARD.decode(&self.payload)?);
        Ok(result)
    }

    pub fn verify(&self, sender: &WireIdentity) -> bool {
        if self.version != PROTOCOL_VERSION
            || self.device_sequence <= 0
            || self.sender_user_id != sender.user_id
            || self.sender_device_id != sender.device_id
            || !sender.verify_certificate()
        {
            return false;
        }
        match self.canonical_bytes() {
            Ok(bytes) => sender.verify_device_data(&bytes, &self.signature),
            Err(_) => false,
        }
    }

    pub fn decode_payload<T: serde::de::DeserializeOwned>(&self) -> Result<T, CoreError> {
        Ok(serde_json::from_slice(&STANDARD.decode(&self.payload)?)?)
    }
}

/// Виды событий. Строки уходят в сеть, поэтому меняться они не могут.
pub const KIND_TEXT: &str = "message.text";
pub const KIND_EDIT: &str = "message.edit";
pub const KIND_DELETE: &str = "message.delete";
pub const KIND_REACTION: &str = "message.reaction";
pub const KIND_ATTACHMENT: &str = "message.attachment";
pub const KIND_RECEIPT_DELIVERY: &str = "receipt.delivery";
pub const KIND_RECEIPT_READ: &str = "receipt.read";
/// Полное новое состояние группы: участники, роли, права, название.
pub const KIND_GROUP_STATE: &str = "group.state";
/// Приглашённый принял приглашение. Заодно раздаёт всем участникам свой личный
/// обратный адрес: без него им пришлось бы писать в узкий публичный ящик.
pub const KIND_GROUP_JOINED: &str = "group.joined";
/// Ответ на `group.joined`: новый участник получает обратный адрес в ответ.
pub const KIND_GROUP_ACK: &str = "group.ack";

/// Полное новое состояние канала: администраторы, права, настройки.
pub const KIND_CHANNEL_STATE: &str = "channel.state";
/// Администратор приглашает контакт: к нему приходит состояние, а решает он сам.
pub const KIND_CHANNEL_INVITE: &str = "channel.invite";
/// Подписка: уходит администратору из ссылки или пригласившему.
pub const KIND_CHANNEL_SUBSCRIBE: &str = "channel.subscribe";
/// Отписка: уходит всем администраторам.
pub const KIND_CHANNEL_LEAVE: &str = "channel.leave";
/// Уведомление подписчику, что его удалили.
pub const KIND_CHANNEL_REMOVED: &str = "channel.removed";
/// Изменения списка подписчиков — только между администраторами.
pub const KIND_CHANNEL_ROSTER: &str = "channel.roster";
/// Пересылка чужих подписанных событий: история для нового подписчика и комментарии.
/// Подписчики друг друга не знают, поэтому их голоса разносит администратор.
pub const KIND_CHANNEL_RELAY: &str = "channel.relay";
pub const KIND_CHANNEL_COMMENT: &str = "channel.comment";
pub const KIND_CHANNEL_COMMENT_DELETE: &str = "channel.comment.delete";
/// Подписчик прочитал посты — уходит их автору.
pub const KIND_CHANNEL_VIEWS: &str = "channel.views";
/// Автор рассылает счётчики своих постов и число подписчиков.
pub const KIND_CHANNEL_STATS: &str = "channel.stats";

/// Событие относится к каналу.
pub fn is_channel_id(value: &str) -> bool {
    value.strip_prefix("ttc1-").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// Событие относится к группе, а не к личному диалогу.
pub fn is_group_id(value: &str) -> bool {
    value.strip_prefix("ttg1-").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

/// UserID — это `tt1-` и SHA-256 identity-ключа в hex.
pub fn is_user_id(value: &str) -> bool {
    value.strip_prefix("tt1-").is_some_and(|hex| {
        hex.len() == 64 && hex.bytes().all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupStatePayload {
    pub version: i32,
    pub state: crate::models::GroupState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupNoticePayload {
    pub version: i32,
    pub group_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelStatePayload {
    pub version: i32,
    pub state: crate::models::ChannelState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelNoticePayload {
    pub version: i32,
    pub channel_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelSubscribePayload {
    pub version: i32,
    pub channel_id: String,
    /// Имя, под которым подписчика увидят администраторы: друг другу они незнакомы.
    pub display_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRosterPayload {
    pub version: i32,
    pub channel_id: String,
    pub entries: Vec<crate::models::ChannelSubscriber>,
}

/// Чужое событие вместе с личностью его автора: подпись проверяется у получателя,
/// а UserID — это хеш ключа, так что подменить автора пересылающий не может.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayedEvent {
    pub identity: WireIdentity,
    pub event: SignedProtocolEvent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelRelayPayload {
    pub version: i32,
    pub channel_id: String,
    pub events: Vec<RelayedEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelCommentPayload {
    pub version: i32,
    pub post_event_id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_event_id: Option<String>,
    pub author_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelCommentDeletePayload {
    pub version: i32,
    pub post_event_id: String,
    pub target_event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelViewsPayload {
    pub version: i32,
    pub post_event_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelPostStats {
    pub event_id: String,
    pub views: u32,
    pub comments: u32,
    pub reactions: Vec<crate::models::ReactionCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChannelStatsPayload {
    pub version: i32,
    pub subscriber_count: u32,
    pub posts: Vec<ChannelPostStats>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextPayload {
    pub version: i32,
    pub text: String,
    /// Ответ на сообщение и подпись «переслано от»: старые клиенты их просто не увидят.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_from: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditPayload {
    pub version: i32,
    pub target_event_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetPayload {
    pub version: i32,
    pub target_event_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionPayload {
    pub version: i32,
    pub target_event_id: String,
    pub reaction: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentPayload {
    pub version: i32,
    pub caption: String,
    pub manifest: crate::blobs::AttachmentManifest,
    /// Те же поля, что и у обычного текста: ответ и пересылка.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reply_to_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forwarded_from: Option<String>,
}

/// Внешняя обёртка внутри зашифрованного канала: событие плюс обратный адрес
/// отправителя. Без него собеседник смог бы отвечать только в публичный ящик.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeliveryPackageBody {
    pub version: i32,
    pub event: SignedProtocolEvent,
    pub sender_routing: crate::routing::SignedRoutingDescriptor,
    pub reply_grants: Vec<crate::mailbox::MailboxGrant>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedDeliveryPackage {
    pub body: DeliveryPackageBody,
    pub body_json: String,
    pub signature: String,
}

/// Самый внешний слой полезной нагрузки конверта: он говорит, чем расшифровывать тело.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireMessage {
    pub kind: String,
    pub event_id: String,
    pub sender_identity: WireIdentity,
    pub body_json: String,
}

pub const WIRE_SESSION_INIT: &str = "session.init";
pub const WIRE_RATCHET: &str = "ratchet";

pub fn verify_p256(spki: &[u8], value: &[u8], signature_base64: &str) -> bool {
    let result = (|| {
        let key = VerifyingKey::from_public_key_der(spki).ok()?;
        let signature = Signature::from_slice(&STANDARD.decode(signature_base64).ok()?).ok()?;
        key.verify(value, &signature).ok()
    })();
    result.is_some()
}

pub fn write_i32_bytes(target: &mut Vec<u8>, value: &[u8]) {
    target.extend_from_slice(&(value.len() as i32).to_le_bytes());
    target.extend_from_slice(value);
}

pub fn write_dotnet_string(target: &mut Vec<u8>, value: &str) {
    let bytes = value.as_bytes();
    let mut length = bytes.len() as u32;
    while length >= 0x80 {
        target.push((length as u8) | 0x80);
        length >>= 7;
    }
    target.push(length as u8);
    target.extend_from_slice(bytes);
}
