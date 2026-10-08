//! Криптография голосового звонка.
//!
//! Ключи звонка не зависят ни от Node, ни от ретранслятора. Каждая сторона создаёт
//! одноразовую X25519-пару и отправляет открытый ключ внутри подписанного события по уже
//! существующему double ratchet: подменить его не может никто, кроме владельца ключей
//! устройства собеседника. Общий секрет пропускается через HKDF вместе со всем
//! «протоколом» звонка (ID звонка, оба UserID, оба одноразовых ключа), так что ключи
//! одного звонка бесполезны для любого другого.
//!
//! Каждое направление шифруется своим ключом AES-256-GCM. Nonce — номер направления и
//! номер пакета: он никогда не повторяется под одним ключом. Повтор старого пакета
//! отсекается окном из 128 последних номеров.

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead, aead::Payload};
use hkdf::Hkdf;
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

use crate::CoreError;

/// Одноразовая пара ключей стороны звонка.
pub struct Ephemeral {
    secret: StaticSecret,
    pub public: [u8; 32],
}

impl Ephemeral {
    pub fn generate() -> Self {
        let secret = StaticSecret::random_from_rng(OsRng);
        let public = PublicKey::from(&secret).to_bytes();
        Self { secret, public }
    }
}

/// Ключи звонка для одной стороны: чем шифровать своё и чем открывать чужое.
pub struct CallKeys {
    send: Aes256Gcm,
    receive: Aes256Gcm,
    send_direction: u32,
    receive_direction: u32,
    /// Код проверки: одинаковый у обоих, если между ними никого нет.
    pub safety_code: Vec<&'static str>,
}

/// Эмодзи кода проверки: 64 узнаваемых значка, по 6 бит на каждый.
const SAFETY_EMOJI: [&str; 64] = [
    "🐶", "🐱", "🦊", "🐻", "🐼", "🐨", "🐯", "🦁", "🐮", "🐷", "🐸", "🐵", "🐔", "🐧", "🐦", "🦆",
    "🦉", "🐴", "🦄", "🐝", "🦋", "🐌", "🐞", "🐢", "🐙", "🦀", "🐬", "🐳", "🦈", "🐊", "🦒", "🐘",
    "🌵", "🌲", "🍀", "🍁", "🍄", "🌻", "🌙", "⭐", "🔥", "🌈", "❄", "🍎", "🍋", "🍉", "🍇", "🍒",
    "🥕", "🌽", "🍕", "🍩", "🎈", "🎁", "🎸", "🚀", "⚓", "⏰", "🔑", "📌", "🎯", "🧲", "💎", "🔔",
];

impl CallKeys {
    /// `caller` — сторона, начавшая звонок. Обе стороны получают одни и те же два ключа,
    /// но зеркально.
    #[allow(clippy::too_many_arguments)]
    pub fn derive(
        own: &Ephemeral,
        peer_public: &[u8; 32],
        caller: bool,
        call_id: &str,
        caller_user_id: &str,
        callee_user_id: &str,
    ) -> Result<Self, CoreError> {
        let shared = own.secret.diffie_hellman(&PublicKey::from(*peer_public));
        if !shared.was_contributory() {
            return Err(CoreError::Crypto("Негодный ключ собеседника".to_owned()));
        }
        let (caller_public, callee_public) = if caller {
            (own.public, *peer_public)
        } else {
            (*peer_public, own.public)
        };
        let mut transcript = Sha256::new();
        transcript.update(b"TuratText.Call.v1\0");
        for part in [call_id.as_bytes(), caller_user_id.as_bytes(), callee_user_id.as_bytes()] {
            transcript.update((part.len() as u32).to_be_bytes());
            transcript.update(part);
        }
        transcript.update(caller_public);
        transcript.update(callee_public);
        let salt = transcript.finalize();
        let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared.as_bytes());
        let mut forward = [0u8; 32];
        let mut backward = [0u8; 32];
        let mut code = [0u8; 4];
        hkdf.expand(b"caller->callee", &mut forward)
            .and_then(|_| hkdf.expand(b"callee->caller", &mut backward))
            .and_then(|_| hkdf.expand(b"safety-code", &mut code))
            .map_err(|_| CoreError::Crypto("HKDF звонка".to_owned()))?;
        let bits = u32::from_be_bytes(code);
        let safety_code = (0..4)
            .map(|index| SAFETY_EMOJI[((bits >> (26 - index * 6)) & 0x3f) as usize])
            .collect();
        let (send_key, receive_key) = if caller { (forward, backward) } else { (backward, forward) };
        let keys = Self {
            send: Aes256Gcm::new_from_slice(&send_key).expect("32-byte key"),
            receive: Aes256Gcm::new_from_slice(&receive_key).expect("32-byte key"),
            send_direction: u32::from(!caller),
            receive_direction: u32::from(caller),
            safety_code,
        };
        forward.zeroize();
        backward.zeroize();
        Ok(keys)
    }

    /// Пакет на отправку: номер в открытую и шифротекст. `header` (комната и сторона)
    /// входит в подпись: пакет нельзя перекинуть в чужую комнату.
    pub fn seal(&self, header: &[u8], sequence: u64, plaintext: &[u8]) -> Vec<u8> {
        let aad = [header, &sequence.to_be_bytes()].concat();
        let ciphertext = self
            .send
            .encrypt(
                &nonce(self.send_direction, sequence),
                Payload { msg: plaintext, aad: &aad },
            )
            .expect("AES-GCM encrypt");
        let mut packet = Vec::with_capacity(8 + ciphertext.len());
        packet.extend_from_slice(&sequence.to_be_bytes());
        packet.extend_from_slice(&ciphertext);
        packet
    }

    /// Открытый пакет и его номер; `None` — чужой, испорченный или поддельный пакет.
    pub fn open(&self, header: &[u8], packet: &[u8]) -> Option<(u64, Vec<u8>)> {
        if packet.len() < 8 + 16 {
            return None;
        }
        let sequence = u64::from_be_bytes(packet[..8].try_into().ok()?);
        let aad = [header, &sequence.to_be_bytes()].concat();
        let plaintext = self
            .receive
            .decrypt(
                &nonce(self.receive_direction, sequence),
                Payload { msg: &packet[8..], aad: &aad },
            )
            .ok()?;
        Some((sequence, plaintext))
    }
}

fn nonce(direction: u32, sequence: u64) -> Nonce<aes_gcm::aead::consts::U12> {
    let mut value = [0u8; 12];
    value[..4].copy_from_slice(&direction.to_be_bytes());
    value[4..].copy_from_slice(&sequence.to_be_bytes());
    Nonce::clone_from_slice(&value)
}

/// Окно защиты от повтора: принимает каждый номер ровно один раз, в пределах 128
/// последних. Старше окна — отказ: голосу такие пакеты всё равно уже не нужны.
#[derive(Default)]
pub struct ReplayWindow {
    highest: Option<u64>,
    seen: u128,
}

impl ReplayWindow {
    pub fn accept(&mut self, sequence: u64) -> bool {
        let Some(highest) = self.highest else {
            self.highest = Some(sequence);
            self.seen = 1;
            return true;
        };
        if sequence > highest {
            let shift = sequence - highest;
            self.seen = if shift >= 128 { 0 } else { self.seen << shift };
            self.seen |= 1;
            self.highest = Some(sequence);
            return true;
        }
        let age = highest - sequence;
        if age >= 128 {
            return false;
        }
        let bit = 1u128 << age;
        if self.seen & bit != 0 {
            return false;
        }
        self.seen |= bit;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (CallKeys, CallKeys) {
        let caller = Ephemeral::generate();
        let callee = Ephemeral::generate();
        let a = CallKeys::derive(&caller, &callee.public, true, "call1", "tt1-a", "tt1-b").unwrap();
        let b = CallKeys::derive(&callee, &caller.public, false, "call1", "tt1-a", "tt1-b").unwrap();
        (a, b)
    }

    #[test]
    fn both_sides_agree_and_directions_differ() {
        let (caller, callee) = pair();
        assert_eq!(caller.safety_code, callee.safety_code);
        assert_eq!(caller.safety_code.len(), 4);
        let packet = caller.seal(b"room", 7, b"hello");
        assert_eq!(callee.open(b"room", &packet), Some((7, b"hello".to_vec())));
        // Свой же пакет обратно не открывается: у направлений разные ключи.
        assert_eq!(caller.open(b"room", &packet), None);
        let reply = callee.seal(b"room", 7, b"hi");
        assert_eq!(caller.open(b"room", &reply), Some((7, b"hi".to_vec())));
    }

    #[test]
    fn tampering_and_foreign_rooms_are_rejected() {
        let (caller, callee) = pair();
        let mut packet = caller.seal(b"room", 1, b"secret");
        assert_eq!(callee.open(b"other", &packet), None);
        packet[10] ^= 1;
        assert_eq!(callee.open(b"room", &packet), None);
        // Другой звонок — другие ключи, даже с теми же людьми.
        let (stranger, _) = pair();
        let foreign = stranger.seal(b"room", 1, b"secret");
        assert_eq!(callee.open(b"room", &foreign), None);
    }

    #[test]
    fn a_man_in_the_middle_changes_the_safety_code() {
        let caller = Ephemeral::generate();
        let callee = Ephemeral::generate();
        let mallory = Ephemeral::generate();
        let a = CallKeys::derive(&caller, &mallory.public, true, "c", "tt1-a", "tt1-b").unwrap();
        let b = CallKeys::derive(&callee, &mallory.public, false, "c", "tt1-a", "tt1-b").unwrap();
        assert_ne!(a.safety_code, b.safety_code);
    }

    #[test]
    fn replay_window_accepts_each_number_once() {
        let mut window = ReplayWindow::default();
        assert!(window.accept(10));
        assert!(!window.accept(10));
        assert!(window.accept(8));
        assert!(window.accept(12));
        assert!(!window.accept(8));
        assert!(window.accept(300));
        assert!(!window.accept(12), "старше окна");
        assert!(window.accept(299));
    }
}
