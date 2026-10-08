//! Учётная запись: пароль, ключ восстановления и зашифрованный «сейф» на Node.
//!
//! Аккаунт не привязан ни к телефону, ни к почте. Его держат три секрета, и все они живут
//! только у пользователя:
//!
//! * **ключ аккаунта** — 32 случайных байта. Им зашифрованы сейф (ключ личности и всё, что нужно
//!   новому устройству) и снимок данных для синхронизации;
//! * **пароль** — из него Argon2id выводит ключ, которым ключ аккаунта завёрнут на Node, и
//!   отдельный «ключ входа». Node хранит только SHA-256 ключа входа: пароль он не видит и проверить
//!   его может лишь онлайн, с ограничением числа попыток;
//! * **ключ восстановления** — 144 случайных бита (плюс контрольная сумма от опечаток), второй
//!   независимый способ развернуть ключ аккаунта. Анонимный: в нём нет ничего, кроме случайности.
//!
//! Логин — это username. Node ищет аккаунт по хешу логина и по хешу ключа восстановления, не зная
//! ни того, ни другого, ни UserID владельца.

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::{Aead, Payload}};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::{
    Engine as _,
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
};
use hkdf::Hkdf;
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

use crate::CoreError;

/// Argon2id: 64 МиБ памяти и три прохода. На телефоне это около секунды — один раз при входе,
/// а для перебора пароля на видеокарте такая память в разы дороже обычного хеша.
const ARGON_MEMORY_KIB: u32 = 64 * 1024;
const ARGON_PASSES: u32 = 3;
const RECOVERY_RANDOM_BYTES: usize = 18;
const RECOVERY_CHECK_BYTES: usize = 2;
const CROCKFORD: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
pub const MIN_PASSWORD_CHARS: usize = 8;

/// Что Node хранит об аккаунте. Копия лежит и внутри сейфа: любое устройство аккаунта может
/// перенести его на другой Node, не спрашивая пароль заново.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerCredentials {
    pub login_lookup: String,
    pub recovery_lookup: String,
    pub password_salt: String,
    pub password_verifier: String,
    pub recovery_verifier: String,
    pub access_verifier: String,
    pub password_wrapped_key: String,
    pub recovery_wrapped_key: String,
}

/// Содержимое сейфа.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VaultContent {
    pub version: i32,
    pub user_id: String,
    /// Ключ личности в PKCS#8 (Base64): им новое устройство подписывает свой сертификат.
    pub identity_private_key: String,
    /// Пропуск к записи аккаунта на Node; Node знает только его хеш.
    pub access_token: String,
    pub created_at_unix_milliseconds: i64,
    /// Логин аккаунта: нужен устройству, восстановившему доступ по ключу.
    #[serde(default)]
    pub username: String,
    pub credentials: ServerCredentials,
}

/// Аккаунт на этом устройстве. Лежит в зашифрованной локальной базе вместе с ключами личности.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccount {
    pub account_id: String,
    /// Node, на котором лежит сейф. После смены Node аккаунт переносится туда же.
    pub base_url: String,
    pub username: String,
    /// Ключ аккаунта (Base64): им шифруются снимки и перешифровывается сейф.
    pub account_key: String,
    pub access_token: String,
    pub vault_version: i64,
    /// Версия снимка на Node, уже слитая с локальными данными или выложенная отсюда.
    pub snapshot_version: i64,
    pub snapshot_uploaded_at_unix_milliseconds: i64,
    /// Номер журнала изменений, до которого всё уже вошло в выложенный снимок.
    pub snapshot_seq: i64,
    /// Номер журнала, до которого изменения уже разосланы другим устройствам.
    pub sync_seq: i64,
    /// Устройства аккаунта, о которых это устройство уже знает: новому нужен свежий снимок.
    #[serde(default)]
    pub known_devices: Vec<String>,
    /// Ключ восстановления, который ещё надо показать пользователю; после подтверждения стирается.
    #[serde(default)]
    pub pending_recovery_key: Option<String>,
    /// Username занят на текущем Node — пользователь должен выбрать другой.
    #[serde(default)]
    pub username_conflict: bool,
    /// Снимок надо выложить при ближайшей синхронизации, не дожидаясь расписания.
    #[serde(default)]
    pub snapshot_requested: bool,
    #[serde(default)]
    pub last_snapshot_check_unix_milliseconds: i64,
}

impl LocalAccount {
    pub fn key(&self) -> Result<[u8; 32], CoreError> {
        STANDARD
            .decode(&self.account_key)
            .ok()
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or_else(|| CoreError::Crypto("Повреждён ключ аккаунта".to_owned()))
    }
}

/// Имя устройства в списке сеансов: «Android · Pixel 8», «Windows · DESKTOP-1».
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviceName {
    pub name: String,
    pub added_at_unix_milliseconds: i64,
}

/// Ключи из пароля: «ключ входа» уходит на Node, ключ обёртки остаётся на устройстве.
pub struct PasswordKeys {
    pub auth: String,
    pub wrap: [u8; 32],
}

impl Drop for PasswordKeys {
    fn drop(&mut self) {
        self.wrap.zeroize();
    }
}

/// Ключи из ключа восстановления.
pub struct RecoveryKeys {
    pub lookup: String,
    pub auth: String,
    pub wrap: [u8; 32],
}

impl Drop for RecoveryKeys {
    fn drop(&mut self) {
        self.wrap.zeroize();
    }
}

pub fn check_password(password: &str) -> Result<(), CoreError> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(CoreError::InvalidInput(format!(
            "Пароль должен содержать минимум {MIN_PASSWORD_CHARS} символов"
        )));
    }
    if password.chars().count() > 256 {
        return Err(CoreError::InvalidInput("Слишком длинный пароль".to_owned()));
    }
    Ok(())
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut value = [0u8; N];
    OsRng.fill_bytes(&mut value);
    value
}

pub fn random_salt() -> String {
    STANDARD.encode(random_bytes::<16>())
}

/// Случайный пропуск: 32 байта в Base64URL.
pub fn random_token() -> String {
    URL_SAFE_NO_PAD.encode(random_bytes::<32>())
}

/// Хеш, под которым Node хранит пропуск или ключ входа.
pub fn verifier(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// Поиск аккаунта по логину. Логин — нормализованный username.
pub fn login_lookup(username: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"turat.account.login.v1\0");
    hasher.update(username.as_bytes());
    hex::encode(hasher.finalize())
}

pub fn derive_password(password: &str, salt_base64: &str) -> Result<PasswordKeys, CoreError> {
    let salt = STANDARD
        .decode(salt_base64)
        .map_err(|_| CoreError::Crypto("Повреждена соль пароля".to_owned()))?;
    let params = Params::new(ARGON_MEMORY_KIB, ARGON_PASSES, 1, Some(32))
        .map_err(|error| CoreError::Crypto(error.to_string()))?;
    let mut master = [0u8; 32];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), &salt, &mut master)
        .map_err(|error| CoreError::Crypto(error.to_string()))?;
    let keys = PasswordKeys {
        auth: URL_SAFE_NO_PAD.encode(expand(&master, b"turat.account.password.auth")),
        wrap: expand(&master, b"turat.account.password.wrap"),
    };
    master.zeroize();
    Ok(keys)
}

pub fn derive_recovery(raw: &[u8]) -> RecoveryKeys {
    let mut lookup = Sha256::new();
    lookup.update(b"turat.account.recovery.lookup.v1\0");
    lookup.update(raw);
    RecoveryKeys {
        lookup: hex::encode(lookup.finalize()),
        auth: URL_SAFE_NO_PAD.encode(expand(raw, b"turat.account.recovery.auth")),
        wrap: expand(raw, b"turat.account.recovery.wrap"),
    }
}

/// Ключ, которым шифруется снимок данных: отдельный от ключа сейфа.
pub fn snapshot_key(account_key: &[u8; 32]) -> [u8; 32] {
    expand(account_key, b"turat.account.snapshot")
}

fn expand(secret: &[u8], info: &[u8]) -> [u8; 32] {
    let mut output = [0u8; 32];
    Hkdf::<Sha256>::new(None, secret)
        .expand(info, &mut output)
        .expect("32 bytes is a valid HKDF length");
    output
}

/// Новый ключ восстановления: 144 случайных бита и 16 бит контрольной суммы.
pub fn new_recovery_key() -> Vec<u8> {
    let random = random_bytes::<RECOVERY_RANDOM_BYTES>();
    let mut raw = random.to_vec();
    raw.extend_from_slice(&recovery_check(&random));
    raw
}

fn recovery_check(random: &[u8]) -> [u8; RECOVERY_CHECK_BYTES] {
    let mut hasher = Sha256::new();
    hasher.update(b"turat.account.recovery.check\0");
    hasher.update(random);
    let digest = hasher.finalize();
    [digest[0], digest[1]]
}

/// `ABCD-EFGH-…`: восемь групп по четыре символа без букв, которые путаются с цифрами.
pub fn format_recovery_key(raw: &[u8]) -> String {
    let mut bits: u64 = 0;
    let mut count = 0;
    let mut text = String::new();
    for &byte in raw {
        bits = (bits << 8) | u64::from(byte);
        count += 8;
        while count >= 5 {
            count -= 5;
            text.push(CROCKFORD[((bits >> count) & 31) as usize] as char);
        }
    }
    if count > 0 {
        text.push(CROCKFORD[((bits << (5 - count)) & 31) as usize] as char);
    }
    text.as_bytes()
        .chunks(4)
        .map(|chunk| std::str::from_utf8(chunk).expect("ASCII"))
        .collect::<Vec<_>>()
        .join("-")
}

/// Разбор ключа, введённого человеком: регистр, пробелы и дефисы не важны, O читается как 0,
/// I и L — как 1. Опечатку ловит контрольная сумма ещё до обращения к Node.
pub fn parse_recovery_key(text: &str) -> Result<Vec<u8>, CoreError> {
    let invalid = || CoreError::InvalidInput("Ключ восстановления введён с ошибкой — проверьте символы".to_owned());
    let mut bits: u64 = 0;
    let mut count = 0;
    let mut raw = Vec::new();
    for symbol in text.chars().filter(|c| !c.is_whitespace() && *c != '-') {
        let symbol = match symbol.to_ascii_uppercase() {
            'O' => '0',
            'I' | 'L' => '1',
            other => other,
        };
        let value = CROCKFORD.iter().position(|&c| c as char == symbol).ok_or_else(invalid)? as u64;
        bits = (bits << 5) | value;
        count += 5;
        if count >= 8 {
            count -= 8;
            raw.push(((bits >> count) & 0xff) as u8);
        }
    }
    if raw.len() != RECOVERY_RANDOM_BYTES + RECOVERY_CHECK_BYTES {
        return Err(invalid());
    }
    let (random, check) = raw.split_at(RECOVERY_RANDOM_BYTES);
    if recovery_check(random) != check {
        return Err(invalid());
    }
    Ok(raw)
}

/// AES-256-GCM со случайным nonce; `purpose` входит в подпись шифротекста, так что сейф нельзя
/// подсунуть вместо снимка или обёртки ключа.
pub fn seal(key: &[u8; 32], purpose: &[u8], plaintext: &[u8]) -> Result<Vec<u8>, CoreError> {
    let nonce = random_bytes::<12>();
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    let mut result = nonce.to_vec();
    result.extend(
        cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: purpose })
            .map_err(|_| CoreError::Crypto("Не удалось зашифровать данные аккаунта".to_owned()))?,
    );
    Ok(result)
}

pub fn open(key: &[u8; 32], purpose: &[u8], sealed: &[u8]) -> Result<Vec<u8>, CoreError> {
    if sealed.len() < 28 {
        return Err(CoreError::Crypto("Повреждены данные аккаунта".to_owned()));
    }
    let cipher = Aes256Gcm::new_from_slice(key).expect("32-byte key");
    cipher
        .decrypt(Nonce::from_slice(&sealed[..12]), Payload { msg: &sealed[12..], aad: purpose })
        .map_err(|_| CoreError::Crypto("Не удалось расшифровать данные аккаунта".to_owned()))
}

pub const PURPOSE_PASSWORD_WRAP: &[u8] = b"turat.account.key.password.v1";
pub const PURPOSE_RECOVERY_WRAP: &[u8] = b"turat.account.key.recovery.v1";
pub const PURPOSE_VAULT: &[u8] = b"turat.account.vault.v1";
pub const PURPOSE_SNAPSHOT: &[u8] = b"turat.account.snapshot.v1";

pub fn wrap_key(wrap: &[u8; 32], purpose: &[u8], account_key: &[u8; 32]) -> Result<String, CoreError> {
    Ok(STANDARD.encode(seal(wrap, purpose, account_key)?))
}

pub fn unwrap_key(wrap: &[u8; 32], purpose: &[u8], wrapped: &str) -> Result<[u8; 32], CoreError> {
    let sealed = STANDARD
        .decode(wrapped)
        .map_err(|_| CoreError::Crypto("Повреждён ключ аккаунта".to_owned()))?;
    let mut key = open(wrap, purpose, &sealed)?;
    let result: [u8; 32] = key
        .as_slice()
        .try_into()
        .map_err(|_| CoreError::Crypto("Повреждён ключ аккаунта".to_owned()))?;
    key.zeroize();
    Ok(result)
}

pub fn seal_vault(account_key: &[u8; 32], content: &VaultContent) -> Result<String, CoreError> {
    Ok(STANDARD.encode(seal(account_key, PURPOSE_VAULT, &serde_json::to_vec(content)?)?))
}

pub fn open_vault(account_key: &[u8; 32], vault: &str) -> Result<VaultContent, CoreError> {
    let sealed = STANDARD
        .decode(vault)
        .map_err(|_| CoreError::Crypto("Повреждён сейф аккаунта".to_owned()))?;
    Ok(serde_json::from_slice(&open(account_key, PURPOSE_VAULT, &sealed)?)?)
}

/// Снимок: JSON сжимается, затем шифруется ключом снимка.
pub fn seal_snapshot(account_key: &[u8; 32], json: &[u8]) -> Result<Vec<u8>, CoreError> {
    let compressed = miniz_oxide::deflate::compress_to_vec(json, 6);
    seal(&snapshot_key(account_key), PURPOSE_SNAPSHOT, &compressed)
}

pub fn open_snapshot(account_key: &[u8; 32], sealed: &[u8]) -> Result<Vec<u8>, CoreError> {
    let compressed = open(&snapshot_key(account_key), PURPOSE_SNAPSHOT, sealed)?;
    miniz_oxide::inflate::decompress_to_vec_with_limit(&compressed, 512 * 1024 * 1024)
        .map_err(|_| CoreError::Crypto("Повреждён снимок данных аккаунта".to_owned()))
}

/// Доказательство работы при создании аккаунта — та же защита от массовой регистрации, что и у
/// почтовых ящиков.
pub fn account_proof(login_lookup: &str, recovery_lookup: &str, bits: i32) -> String {
    if bits <= 0 {
        return String::new();
    }
    let mut nonce: u64 = 0;
    loop {
        let candidate = nonce.to_string();
        let digest = Sha256::digest(
            format!("turat.account.v1:{login_lookup}:{recovery_lookup}:{candidate}").as_bytes(),
        );
        if leading_zero_bits(&digest) >= bits {
            return candidate;
        }
        nonce += 1;
    }
}

fn leading_zero_bits(value: &[u8]) -> i32 {
    let mut zeros = 0;
    for &byte in value {
        if byte == 0 {
            zeros += 8;
            continue;
        }
        zeros += byte.leading_zeros() as i32;
        break;
    }
    zeros
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_keys_survive_human_typing() {
        let raw = new_recovery_key();
        let text = format_recovery_key(&raw);
        assert_eq!(text.len(), 39, "{text}");
        assert_eq!(text.matches('-').count(), 7);
        assert_eq!(parse_recovery_key(&text).unwrap(), raw);
        let sloppy = text.to_lowercase().replace('-', " ").replace('0', "o").replace('1', "l");
        assert_eq!(parse_recovery_key(&sloppy).unwrap(), raw);
    }

    #[test]
    fn a_typo_in_the_recovery_key_is_caught_locally() {
        let raw = new_recovery_key();
        let mut text: Vec<char> = format_recovery_key(&raw).chars().collect();
        text[5] = if text[5] == 'A' { 'B' } else { 'A' };
        let typed: String = text.into_iter().collect();
        assert!(parse_recovery_key(&typed).is_err());
        assert!(parse_recovery_key("ABCD-EFGH").is_err());
    }

    #[test]
    fn the_account_key_unwraps_only_with_the_right_password() {
        let salt = random_salt();
        let account_key = random_bytes::<32>();
        let keys = derive_password("правильный пароль", &salt).unwrap();
        let wrapped = wrap_key(&keys.wrap, PURPOSE_PASSWORD_WRAP, &account_key).unwrap();
        let again = derive_password("правильный пароль", &salt).unwrap();
        assert_eq!(again.auth, keys.auth);
        assert_eq!(unwrap_key(&again.wrap, PURPOSE_PASSWORD_WRAP, &wrapped).unwrap(), account_key);
        let wrong = derive_password("неправильный пароль", &salt).unwrap();
        assert_ne!(wrong.auth, keys.auth);
        assert!(unwrap_key(&wrong.wrap, PURPOSE_PASSWORD_WRAP, &wrapped).is_err());
        // Обёртка пароля не открывается как обёртка ключа восстановления.
        assert!(unwrap_key(&keys.wrap, PURPOSE_RECOVERY_WRAP, &wrapped).is_err());
    }

    #[test]
    fn snapshots_round_trip_compressed() {
        let account_key = random_bytes::<32>();
        let json = serde_json::to_vec(&serde_json::json!({"text": "привет ".repeat(5000)})).unwrap();
        let sealed = seal_snapshot(&account_key, &json).unwrap();
        assert!(sealed.len() < json.len() / 10, "сжатие работает");
        assert_eq!(open_snapshot(&account_key, &sealed).unwrap(), json);
        assert!(open_snapshot(&random_bytes::<32>(), &sealed).is_err());
    }

    #[test]
    fn proof_of_work_meets_the_requested_difficulty() {
        let nonce = account_proof("a", "b", 8);
        let digest = Sha256::digest(format!("turat.account.v1:a:b:{nonce}").as_bytes());
        assert!(leading_zero_bits(&digest) >= 8);
    }
}
