//! Двойной храповик поверх гибридного рукопожатия. Каждое сообщение шифруется своим
//! ключом, ключи не возвращаются назад, а компрометация одного из них не раскрывает ни
//! прошлую, ни будущую переписку.

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::{
    CoreError,
    identity::StoredIdentity,
    prekeys::{self, ClaimedPrekeyBundle, PrekeyState},
    protocol::{PROTOCOL_VERSION, WireIdentity},
};

/// Сколько пропущенных сообщений держим в запасе: при перезаказе доставки ключи
/// нужны позже, но бесконечный список — это утечка памяти и подарок атакующему.
const MAX_SKIP: i32 = 2000;
const NONCE_SIZE: usize = 12;
const TAG_SIZE: usize = 16;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatchetMessage {
    pub version: i32,
    pub session_id: String,
    pub sender_user_id: String,
    pub sender_device_id: String,
    pub recipient_user_id: String,
    pub recipient_device_id: String,
    pub ratchet_public_key: String,
    pub previous_chain_length: i32,
    pub message_number: i32,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialHandshakeHeader {
    pub version: i32,
    pub session_id: String,
    pub sender_identity: WireIdentity,
    pub recipient_user_id: String,
    pub recipient_device_id: String,
    pub sender_identity_dh_public_key: String,
    pub sender_ephemeral_public_key: String,
    pub recipient_signed_prekey_id: String,
    pub recipient_pq_prekey_id: String,
    /// null, если у получателя кончились одноразовые предключи: сессия всё равно
    /// поднимается, просто без дополнительного слоя одноразовости.
    pub recipient_one_time_prekey_id: Option<String>,
    pub pq_ciphertext: String,
    pub created_at_unix_milliseconds: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitialSessionEnvelope {
    pub header: InitialHandshakeHeader,
    pub header_json: String,
    pub header_signature: String,
    pub message: RatchetMessage,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkippedKey {
    pub id: String,
    pub message_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RatchetSession {
    pub session_id: String,
    pub peer_user_id: String,
    pub peer_device_id: String,
    pub root_key: String,
    pub self_ratchet_private_key: String,
    pub peer_ratchet_public_key: String,
    pub send_chain_key: String,
    pub receive_chain_key: String,
    pub sending_number: i32,
    pub receiving_number: i32,
    pub previous_sending_chain_length: i32,
    /// Принимающая сторона обязана провернуть свой храповик перед первым ответом.
    pub needs_send_ratchet: bool,
    pub skipped_keys: Vec<SkippedKey>,
}

/// Начало сессии: X3DH-подобное согласование с постквантовой инкапсуляцией сверху.
pub fn initiate(
    identity: &StoredIdentity,
    state: &PrekeyState,
    bundle: &ClaimedPrekeyBundle,
    plaintext: &[u8],
) -> Result<(RatchetSession, InitialSessionEnvelope), CoreError> {
    let remote = &bundle.signed_prekey.descriptor;
    if remote.expires_at_unix_milliseconds <= chrono::Utc::now().timestamp_millis() {
        return Err(CoreError::Crypto("Предключи собеседника истекли".to_owned()));
    }
    // Связка должна принадлежать тому, кому мы пишем, и его же устройству.
    if !bundle.identity.verify_certificate()
        || bundle.identity.user_id != bundle.user_id
        || bundle.identity.device_id != bundle.device_id
    {
        return Err(CoreError::Crypto(
            "Связка предключей принадлежит другому устройству".to_owned(),
        ));
    }
    let ephemeral = prekeys::random_secret();
    let ephemeral_private = STANDARD.encode(ephemeral.to_bytes());
    let dh1 = prekeys::agree(&state.identity_dh_private_key, &remote.signed_prekey_public_key)?;
    let dh2 = prekeys::agree(&ephemeral_private, &remote.identity_dh_public_key)?;
    let dh3 = prekeys::agree(&ephemeral_private, &remote.signed_prekey_public_key)?;
    let dh4 = bundle
        .one_time_prekey
        .as_ref()
        .map(|value| prekeys::agree(&ephemeral_private, &value.public_key))
        .transpose()?;
    let (pq_ciphertext, pq_secret) = prekeys::encapsulate(&remote.pq_prekey_public_key)?;

    let session_id = format!("ses1-{}", prekeys::random_token(18));
    let root = handshake_root(
        &dh1,
        &dh2,
        &dh3,
        dh4.as_ref(),
        &pq_secret,
        &identity.public.user_id,
        &bundle.user_id,
        &session_id,
    )?;
    let (send_chain, receive_chain) = initial_chains(&root)?;
    let mut session = RatchetSession {
        session_id: session_id.clone(),
        peer_user_id: bundle.user_id.clone(),
        peer_device_id: bundle.device_id.clone(),
        root_key: STANDARD.encode(root),
        self_ratchet_private_key: ephemeral_private.clone(),
        peer_ratchet_public_key: remote.signed_prekey_public_key.clone(),
        send_chain_key: STANDARD.encode(send_chain),
        receive_chain_key: STANDARD.encode(receive_chain),
        sending_number: 0,
        receiving_number: 0,
        previous_sending_chain_length: 0,
        needs_send_ratchet: false,
        skipped_keys: Vec::new(),
    };

    let header = InitialHandshakeHeader {
        version: PROTOCOL_VERSION,
        session_id,
        sender_identity: WireIdentity::from(&identity.public),
        recipient_user_id: bundle.user_id.clone(),
        recipient_device_id: bundle.device_id.clone(),
        sender_identity_dh_public_key: prekeys::public_of(&state.identity_dh_private_key)?,
        sender_ephemeral_public_key: prekeys::public_of(&ephemeral_private)?,
        recipient_signed_prekey_id: remote.signed_prekey_id.clone(),
        recipient_pq_prekey_id: remote.pq_prekey_id.clone(),
        recipient_one_time_prekey_id: bundle
            .one_time_prekey
            .as_ref()
            .map(|value| value.prekey_id.clone()),
        pq_ciphertext,
        created_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
    };
    let header_json = serde_json::to_string(&header)?;
    let header_signature = identity.sign_device(header_json.as_bytes())?;
    let message = session.encrypt(identity, plaintext)?;
    Ok((
        session,
        InitialSessionEnvelope {
            header,
            header_json,
            header_signature,
            message,
        },
    ))
}

/// Принимающая сторона повторяет то же согласование своими секретами.
pub fn accept(
    identity: &StoredIdentity,
    state: &mut PrekeyState,
    envelope: &InitialSessionEnvelope,
) -> Result<(RatchetSession, Vec<u8>), CoreError> {
    let header = &envelope.header;
    if header.version != PROTOCOL_VERSION
        || header.recipient_user_id != identity.public.user_id
        || header.recipient_device_id != identity.public.device_id
        || serde_json::to_string(header)? != envelope.header_json
    {
        return Err(CoreError::Crypto(
            "Заголовок новой сессии не прошёл проверку".to_owned(),
        ));
    }
    if !header.sender_identity.verify_certificate()
        || !header
            .sender_identity
            .verify_device_data(envelope.header_json.as_bytes(), &envelope.header_signature)
    {
        return Err(CoreError::Crypto(
            "Подпись новой сессии недействительна".to_owned(),
        ));
    }
    // Рукопожатие с чужой датой — это либо переигранный конверт, либо сбитые часы.
    if (chrono::Utc::now().timestamp_millis() - header.created_at_unix_milliseconds).abs()
        > 7 * 86_400_000
    {
        return Err(CoreError::Crypto(
            "Рукопожатие вне допустимого окна времени".to_owned(),
        ));
    }
    if header.recipient_signed_prekey_id != state.signed_prekey_id
        || header.recipient_pq_prekey_id != state.pq_prekey_id
    {
        return Err(CoreError::Crypto(
            "Рукопожатие адресовано недоступному предключу".to_owned(),
        ));
    }
    let one_time_private = match &header.recipient_one_time_prekey_id {
        Some(prekey_id) => Some(state.consume_one_time(prekey_id).ok_or_else(|| {
            CoreError::Crypto("Одноразовый предключ уже использован".to_owned())
        })?),
        None => None,
    };

    let dh1 = prekeys::agree(
        &state.signed_prekey_private_key,
        &header.sender_identity_dh_public_key,
    )?;
    let dh2 = prekeys::agree(
        &state.identity_dh_private_key,
        &header.sender_ephemeral_public_key,
    )?;
    let dh3 = prekeys::agree(
        &state.signed_prekey_private_key,
        &header.sender_ephemeral_public_key,
    )?;
    let dh4 = one_time_private
        .as_ref()
        .map(|value| prekeys::agree(value, &header.sender_ephemeral_public_key))
        .transpose()?;
    let pq_secret = prekeys::decapsulate(state, &header.pq_ciphertext)?;
    let root = handshake_root(
        &dh1,
        &dh2,
        &dh3,
        dh4.as_ref(),
        &pq_secret,
        &header.sender_identity.user_id,
        &identity.public.user_id,
        &header.session_id,
    )?;
    let (initiator_send, initiator_receive) = initial_chains(&root)?;
    let mut session = RatchetSession {
        session_id: header.session_id.clone(),
        peer_user_id: header.sender_identity.user_id.clone(),
        peer_device_id: header.sender_identity.device_id.clone(),
        root_key: STANDARD.encode(root),
        self_ratchet_private_key: state.signed_prekey_private_key.clone(),
        peer_ratchet_public_key: header.sender_ephemeral_public_key.clone(),
        send_chain_key: STANDARD.encode(initiator_receive),
        receive_chain_key: STANDARD.encode(initiator_send),
        sending_number: 0,
        receiving_number: 0,
        previous_sending_chain_length: 0,
        needs_send_ratchet: true,
        skipped_keys: Vec::new(),
    };
    let plaintext = session.decrypt(identity, &envelope.message)?;
    Ok((session, plaintext))
}

impl RatchetSession {
    pub fn encrypt(
        &mut self,
        identity: &StoredIdentity,
        plaintext: &[u8],
    ) -> Result<RatchetMessage, CoreError> {
        if self.needs_send_ratchet {
            self.rotate_sending()?;
        }
        let (message_key, next_chain) = advance_chain(&decode32(&self.send_chain_key)?)?;
        let mut message = RatchetMessage {
            version: PROTOCOL_VERSION,
            session_id: self.session_id.clone(),
            sender_user_id: identity.public.user_id.clone(),
            sender_device_id: identity.public.device_id.clone(),
            recipient_user_id: self.peer_user_id.clone(),
            recipient_device_id: self.peer_device_id.clone(),
            ratchet_public_key: prekeys::public_of(&self.self_ratchet_private_key)?,
            previous_chain_length: self.previous_sending_chain_length,
            message_number: self.sending_number,
            nonce: String::new(),
            ciphertext: String::new(),
        };
        let mut nonce = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce);
        let sealed = Aes256Gcm::new_from_slice(&message_key)
            .map_err(|_| CoreError::Crypto("Некорректный ключ сообщения".to_owned()))?
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: &message_aad(&message),
                },
            )
            .map_err(|_| CoreError::Crypto("Не удалось зашифровать сообщение".to_owned()))?;
        message.nonce = STANDARD.encode(nonce);
        message.ciphertext = STANDARD.encode(sealed);
        self.send_chain_key = STANDARD.encode(next_chain);
        self.sending_number += 1;
        Ok(message)
    }

    pub fn decrypt(
        &mut self,
        identity: &StoredIdentity,
        message: &RatchetMessage,
    ) -> Result<Vec<u8>, CoreError> {
        self.validate_address(identity, message)?;
        let skipped_id = skipped_id(&message.ratchet_public_key, message.message_number);
        if let Some(index) = self
            .skipped_keys
            .iter()
            .position(|value| value.id == skipped_id)
        {
            let key = decode32(&self.skipped_keys.remove(index).message_key)?;
            return decrypt_payload(message, &key);
        }
        if message.ratchet_public_key != self.peer_ratchet_public_key {
            self.skip_keys(message.previous_chain_length)?;
            self.rotate_receiving(&message.ratchet_public_key)?;
        }
        if message.message_number < self.receiving_number {
            return Err(CoreError::Crypto(
                "Ключ сообщения уже использован или недоступен".to_owned(),
            ));
        }
        if message.message_number - self.receiving_number > MAX_SKIP {
            return Err(CoreError::Crypto(
                "Слишком много пропущенных сообщений".to_owned(),
            ));
        }
        self.skip_keys(message.message_number)?;
        let (message_key, next_chain) = advance_chain(&decode32(&self.receive_chain_key)?)?;
        let plaintext = decrypt_payload(message, &message_key)?;
        // Состояние двигаем только после успешной расшифровки: испорченный конверт
        // не должен сбивать цепочку и обесценивать следующие сообщения.
        self.receive_chain_key = STANDARD.encode(next_chain);
        self.receiving_number += 1;
        Ok(plaintext)
    }

    fn rotate_sending(&mut self) -> Result<(), CoreError> {
        let secret = prekeys::random_secret();
        let private_key = STANDARD.encode(secret.to_bytes());
        let dh = prekeys::agree(&private_key, &self.peer_ratchet_public_key)?;
        let (root, chain) = root_kdf(&decode32(&self.root_key)?, &dh)?;
        self.previous_sending_chain_length = self.sending_number;
        self.sending_number = 0;
        self.self_ratchet_private_key = private_key;
        self.root_key = STANDARD.encode(root);
        self.send_chain_key = STANDARD.encode(chain);
        self.needs_send_ratchet = false;
        Ok(())
    }

    fn rotate_receiving(&mut self, peer_public_key: &str) -> Result<(), CoreError> {
        let receive_dh = prekeys::agree(&self.self_ratchet_private_key, peer_public_key)?;
        let (root_after_receive, receive_chain) =
            root_kdf(&decode32(&self.root_key)?, &receive_dh)?;
        let secret = prekeys::random_secret();
        let private_key = STANDARD.encode(secret.to_bytes());
        let send_dh = prekeys::agree(&private_key, peer_public_key)?;
        let (root_after_send, send_chain) = root_kdf(&root_after_receive, &send_dh)?;
        self.previous_sending_chain_length = self.sending_number;
        self.sending_number = 0;
        self.receiving_number = 0;
        self.peer_ratchet_public_key = peer_public_key.to_owned();
        self.self_ratchet_private_key = private_key;
        self.root_key = STANDARD.encode(root_after_send);
        self.receive_chain_key = STANDARD.encode(receive_chain);
        self.send_chain_key = STANDARD.encode(send_chain);
        self.needs_send_ratchet = false;
        Ok(())
    }

    fn skip_keys(&mut self, until: i32) -> Result<(), CoreError> {
        if until - self.receiving_number > MAX_SKIP {
            return Err(CoreError::Crypto(
                "Слишком много пропущенных сообщений".to_owned(),
            ));
        }
        while self.receiving_number < until {
            let (message_key, next_chain) = advance_chain(&decode32(&self.receive_chain_key)?)?;
            self.skipped_keys.push(SkippedKey {
                id: skipped_id(&self.peer_ratchet_public_key, self.receiving_number),
                message_key: STANDARD.encode(message_key),
            });
            self.receive_chain_key = STANDARD.encode(next_chain);
            self.receiving_number += 1;
        }
        let overflow = self.skipped_keys.len().saturating_sub(MAX_SKIP as usize);
        if overflow > 0 {
            self.skipped_keys.drain(..overflow);
        }
        Ok(())
    }

    fn validate_address(
        &self,
        identity: &StoredIdentity,
        message: &RatchetMessage,
    ) -> Result<(), CoreError> {
        if message.version != PROTOCOL_VERSION
            || message.session_id != self.session_id
            || message.sender_user_id != self.peer_user_id
            || message.sender_device_id != self.peer_device_id
            || message.recipient_user_id != identity.public.user_id
            || message.recipient_device_id != identity.public.device_id
            || message.message_number < 0
            || message.previous_chain_length < 0
        {
            return Err(CoreError::Crypto(
                "Сообщение адресовано не этой сессии".to_owned(),
            ));
        }
        Ok(())
    }
}

fn decrypt_payload(message: &RatchetMessage, message_key: &[u8; 32]) -> Result<Vec<u8>, CoreError> {
    let packed = STANDARD.decode(&message.ciphertext)?;
    if packed.len() < TAG_SIZE {
        return Err(CoreError::Crypto("Шифротекст обрезан".to_owned()));
    }
    let nonce = STANDARD.decode(&message.nonce)?;
    if nonce.len() != NONCE_SIZE {
        return Err(CoreError::Crypto("Некорректный nonce".to_owned()));
    }
    Aes256Gcm::new_from_slice(message_key)
        .map_err(|_| CoreError::Crypto("Некорректный ключ сообщения".to_owned()))?
        .decrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: &packed,
                aad: &message_aad(message),
            },
        )
        .map_err(|_| CoreError::Crypto("Сообщение не расшифровывается".to_owned()))
}

/// Заголовок сообщения входит в AAD: подменить адресата или номер, не сломав тег, нельзя.
fn message_aad(value: &RatchetMessage) -> Vec<u8> {
    let mut result = Vec::new();
    crate::protocol::write_dotnet_string(&mut result, "TuratText.DoubleRatchetMessage.v2");
    result.extend_from_slice(&value.version.to_le_bytes());
    for text in [
        &value.session_id,
        &value.sender_user_id,
        &value.sender_device_id,
        &value.recipient_user_id,
        &value.recipient_device_id,
        &value.ratchet_public_key,
    ] {
        crate::protocol::write_i32_bytes(&mut result, text.as_bytes());
    }
    result.extend_from_slice(&value.previous_chain_length.to_le_bytes());
    result.extend_from_slice(&value.message_number.to_le_bytes());
    result
}

fn advance_chain(chain: &[u8; 32]) -> Result<([u8; 32], [u8; 32]), CoreError> {
    let mac = |input: u8| -> Result<[u8; 32], CoreError> {
        let mut hmac = <Hmac<Sha256> as Mac>::new_from_slice(chain)
            .map_err(|_| CoreError::Crypto("Некорректный ключ цепочки".to_owned()))?;
        hmac.update(&[input]);
        Ok(hmac.finalize().into_bytes().into())
    };
    Ok((mac(1)?, mac(2)?))
}

fn root_kdf(root: &[u8; 32], dh: &[u8; 32]) -> Result<([u8; 32], [u8; 32]), CoreError> {
    split(&hkdf(dh, root, b"TuratText.RootKdf.v2", 64)?)
}

fn initial_chains(root: &[u8; 32]) -> Result<([u8; 32], [u8; 32]), CoreError> {
    split(&hkdf(root, &[0u8; 32], b"TuratText.InitialChains.v2", 64)?)
}

fn handshake_root(
    dh1: &[u8; 32],
    dh2: &[u8; 32],
    dh3: &[u8; 32],
    dh4: Option<&[u8; 32]>,
    pq_secret: &[u8; 32],
    sender_user_id: &str,
    recipient_user_id: &str,
    session_id: &str,
) -> Result<[u8; 32], CoreError> {
    let mut input = Vec::with_capacity(160);
    input.extend_from_slice(dh1);
    input.extend_from_slice(dh2);
    input.extend_from_slice(dh3);
    if let Some(value) = dh4 {
        input.extend_from_slice(value);
    }
    input.extend_from_slice(pq_secret);
    let info = format!(
        "TuratText.HybridHandshake.v2\0{sender_user_id}\0{recipient_user_id}\0{session_id}"
    );
    let output = hkdf(&input, &[0u8; 32], info.as_bytes(), 32)?;
    output[..32]
        .try_into()
        .map_err(|_| CoreError::Crypto("Не удалось вывести корневой ключ".to_owned()))
}

fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8], length: usize) -> Result<Vec<u8>, CoreError> {
    let mut output = vec![0u8; length];
    Hkdf::<Sha256>::new(Some(salt), ikm)
        .expand(info, &mut output)
        .map_err(|_| CoreError::Crypto("Не удалось вывести ключ".to_owned()))?;
    Ok(output)
}

fn split(value: &[u8]) -> Result<([u8; 32], [u8; 32]), CoreError> {
    if value.len() < 64 {
        return Err(CoreError::Crypto("Короткий вывод KDF".to_owned()));
    }
    Ok((
        value[..32].try_into().expect("32 байта"),
        value[32..64].try_into().expect("32 байта"),
    ))
}

fn decode32(value: &str) -> Result<[u8; 32], CoreError> {
    STANDARD
        .decode(value)?
        .try_into()
        .map_err(|_| CoreError::Crypto("Некорректный ключ храповика".to_owned()))
}

fn skipped_id(public_key: &str, number: i32) -> String {
    format!("{public_key}#{number}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prekeys::{ClaimedPrekeyBundle, PrekeyState};

    fn bundle_for(identity: &StoredIdentity, state: &PrekeyState) -> ClaimedPrekeyBundle {
        let publication = state.publication(identity).expect("публикация");
        assert!(crate::prekeys::verify_publication(&publication));
        ClaimedPrekeyBundle {
            user_id: identity.public.user_id.clone(),
            device_id: identity.public.device_id.clone(),
            identity: publication.identity.clone(),
            signed_prekey: publication.signed_prekey.clone(),
            one_time_prekey: publication.one_time_prekeys.first().cloned(),
        }
    }

    #[test]
    fn session_survives_a_full_conversation() {
        let alice = StoredIdentity::create().expect("личность");
        let bob = StoredIdentity::create().expect("личность");
        let alice_state = PrekeyState::create().expect("предключи");
        let mut bob_state = PrekeyState::create().expect("предключи");

        let (mut alice_session, envelope) =
            initiate(&alice, &alice_state, &bundle_for(&bob, &bob_state), "привет".as_bytes()).expect("init");
        let (mut bob_session, plaintext) = accept(&bob, &mut bob_state, &envelope).expect("accept");
        assert_eq!(plaintext, "привет".as_bytes());

        // Ответ разворачивает храповик, а следом идёт встречный поток.
        let reply = bob_session.encrypt(&bob, "и тебе".as_bytes()).expect("шифрование");
        assert_eq!(
            alice_session.decrypt(&alice, &reply).expect("расшифровка"),
            "и тебе".as_bytes()
        );
        for index in 0..5 {
            let text = format!("сообщение {index}");
            let message = alice_session
                .encrypt(&alice, text.as_bytes())
                .expect("шифрование");
            assert_eq!(
                bob_session.decrypt(&bob, &message).expect("расшифровка"),
                text.as_bytes()
            );
        }
        let _ = alice_state;
    }

    #[test]
    fn out_of_order_delivery_still_decrypts() {
        let alice = StoredIdentity::create().expect("личность");
        let bob = StoredIdentity::create().expect("личность");
        let alice_state = PrekeyState::create().expect("предключи");
        let mut bob_state = PrekeyState::create().expect("предключи");
        let (mut alice_session, envelope) =
            initiate(&alice, &alice_state, &bundle_for(&bob, &bob_state), b"1").expect("init");
        let (mut bob_session, _) = accept(&bob, &mut bob_state, &envelope).expect("accept");

        let second = alice_session.encrypt(&alice, b"2").expect("шифрование");
        let third = alice_session.encrypt(&alice, b"3").expect("шифрование");
        assert_eq!(bob_session.decrypt(&bob, &third).expect("третье"), b"3");
        assert_eq!(bob_session.decrypt(&bob, &second).expect("второе"), b"2");
    }

    #[test]
    fn a_tampered_message_is_rejected() {
        let alice = StoredIdentity::create().expect("личность");
        let bob = StoredIdentity::create().expect("личность");
        let alice_state = PrekeyState::create().expect("предключи");
        let mut bob_state = PrekeyState::create().expect("предключи");
        let (mut alice_session, envelope) =
            initiate(&alice, &alice_state, &bundle_for(&bob, &bob_state), b"1").expect("init");
        let (mut bob_session, _) = accept(&bob, &mut bob_state, &envelope).expect("accept");

        let mut message = alice_session.encrypt(&alice, "перевод 100".as_bytes()).expect("шифр");
        message.message_number += 1;
        assert!(bob_session.decrypt(&bob, &message).is_err());
    }
}
