//! Вложения: файл шифруется отдельным ключом и по чанкам уезжает в blob-хранилище Node,
//! а ключ уходит собеседнику внутри уже зашифрованного сообщения. Node хранит только
//! шифротекст и не может ни прочитать файл, ни связать его с диалогом.

use std::{
    fs::File,
    io::{BufWriter, Write},
    path::Path,
};

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    CoreError,
    media::{self, MediaReader, Progress},
    models::MediaKind,
    network::Network,
    prekeys::random_token,
};

/// 512 КиБ открытого текста: с тегом и nonce чанк остаётся в пределах лимита Node (1 МиБ).
const CHUNK_LENGTH: usize = 512 * 1024;
const NONCE_SIZE: usize = 12;
const TAG_SIZE: usize = 16;
const BLOB_LIFETIME_DAYS: i64 = 14;

/// Сколько байт увидит Node: к каждому чанку добавляются nonce и тег, поэтому шифротекст
/// чуть больше файла. Предел Node задан именно для шифротекста, и сверять надо с ним.
pub fn ciphertext_size(plaintext_size: u64) -> u64 {
    let chunks = plaintext_size.div_ceil(CHUNK_LENGTH as u64);
    plaintext_size + chunks * (NONCE_SIZE + TAG_SIZE) as u64
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentBlobReference {
    pub node_id: String,
    pub base_url: String,
    pub object_id: String,
    pub read_capability: String,
    pub expires_at_unix_milliseconds: i64,
}

/// Всё, что нужно получателю, чтобы собрать файл обратно: ключ, разбивка и адреса чанков.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentManifest {
    pub version: i32,
    pub attachment_id: String,
    pub file_name: String,
    pub mime_type: String,
    pub plaintext_size: u64,
    pub chunk_size: i32,
    pub chunk_count: i32,
    pub file_key: String,
    pub plaintext_sha256: String,
    pub ciphertext_chunk_sha256: Vec<String>,
    pub blobs: Vec<AttachmentBlobReference>,
    /// Подсказки для интерфейса: без них получатель не знает, рисовать плеер или карточку файла.
    #[serde(default)]
    pub kind: MediaKind,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    #[serde(default)]
    pub duration_milliseconds: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail_base64: Option<String>,
}

/// Описание файла, который отправляем: берётся из уже зашифрованного локального вложения.
pub struct UploadRequest<'a> {
    pub local_path: &'a Path,
    pub file_name: &'a str,
    pub mime_type: &'a str,
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    pub duration_milliseconds: i64,
    pub thumbnail_base64: Option<String>,
}

/// Шифрует локальное вложение файловым ключом и выкладывает чанки на Node.
pub fn upload(
    network: &Network,
    node_id: &str,
    base_url: &str,
    vault_key: &[u8; 32],
    request: &UploadRequest<'_>,
    progress: &Progress,
) -> Result<AttachmentManifest, CoreError> {
    let mut reader = MediaReader::open(vault_key, request.local_path)?;
    let plaintext_size = reader.length();
    if plaintext_size == 0 {
        return Err(CoreError::InvalidInput("Вложение пустое".to_owned()));
    }
    progress.total.store(plaintext_size, std::sync::atomic::Ordering::Relaxed);

    let attachment_id = format!("att1-{}", random_token(18));
    let mut file_key = [0u8; 32];
    OsRng.fill_bytes(&mut file_key);
    let cipher = Aes256Gcm::new_from_slice(&file_key)
        .map_err(|_| CoreError::Crypto("Некорректный ключ вложения".to_owned()))?;

    let chunk_count = plaintext_size.div_ceil(CHUNK_LENGTH as u64) as usize;
    let object_id = format!("blob1-{}", random_token(18));
    let read_capability = random_token(32);
    let write_capability = random_token(32);
    let expires_at = chrono::Utc::now() + chrono::Duration::days(BLOB_LIFETIME_DAYS);
    let ciphertext_size = ciphertext_size(plaintext_size);
    network.blob_register(
        base_url,
        &object_id,
        &read_capability,
        &write_capability,
        ciphertext_size,
        (CHUNK_LENGTH + NONCE_SIZE + TAG_SIZE) as i32,
        expires_at,
    )?;

    let mut digests = Vec::with_capacity(chunk_count);
    let mut whole = Sha256::new();
    let mut buffer = vec![0u8; CHUNK_LENGTH];
    let mut offset = 0u64;
    let mut index = 0i32;
    while offset < plaintext_size {
        let wanted = CHUNK_LENGTH.min((plaintext_size - offset) as usize);
        let read = reader.read_at(offset, &mut buffer[..wanted])?;
        if read != wanted {
            return Err(CoreError::Crypto("Вложение обрезано".to_owned()));
        }
        whole.update(&buffer[..wanted]);
        let mut nonce = [0u8; NONCE_SIZE];
        OsRng.fill_bytes(&mut nonce);
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                Payload {
                    msg: &buffer[..wanted],
                    aad: &chunk_aad(&attachment_id, index),
                },
            )
            .map_err(|_| CoreError::Crypto("Не удалось зашифровать чанк".to_owned()))?;
        let mut packed = Vec::with_capacity(NONCE_SIZE + sealed.len());
        packed.extend_from_slice(&nonce);
        packed.extend_from_slice(&sealed);
        let digest = hex::encode(Sha256::digest(&packed));
        network.blob_put_chunk(base_url, &object_id, index, &write_capability, &digest, packed)?;
        digests.push(digest);
        offset += wanted as u64;
        index += 1;
        progress.done.store(offset, std::sync::atomic::Ordering::Relaxed);
    }

    Ok(AttachmentManifest {
        version: 2,
        attachment_id,
        file_name: request.file_name.to_owned(),
        mime_type: request.mime_type.to_owned(),
        plaintext_size,
        chunk_size: CHUNK_LENGTH as i32,
        chunk_count: chunk_count as i32,
        file_key: STANDARD.encode(file_key),
        plaintext_sha256: hex::encode(whole.finalize()),
        ciphertext_chunk_sha256: digests,
        blobs: vec![AttachmentBlobReference {
            node_id: node_id.to_owned(),
            base_url: base_url.trim_end_matches('/').to_owned(),
            object_id,
            read_capability,
            expires_at_unix_milliseconds: expires_at.timestamp_millis(),
        }],
        kind: request.kind,
        width: request.width,
        height: request.height,
        duration_milliseconds: request.duration_milliseconds,
        thumbnail_base64: request.thumbnail_base64.clone(),
    })
}

/// Скачивает чанки, проверяет их хеши и складывает файл в локальный шифрованный контейнер.
pub fn download(
    network: &Network,
    manifest: &AttachmentManifest,
    vault_key: &[u8; 32],
    destination: &Path,
    progress: &Progress,
) -> Result<(), CoreError> {
    let file_key: [u8; 32] = STANDARD
        .decode(&manifest.file_key)?
        .try_into()
        .map_err(|_| CoreError::Crypto("Некорректный ключ вложения".to_owned()))?;
    let cipher = Aes256Gcm::new_from_slice(&file_key)
        .map_err(|_| CoreError::Crypto("Некорректный ключ вложения".to_owned()))?;
    let reference = manifest
        .blobs
        .first()
        .ok_or_else(|| CoreError::InvalidInput("У вложения нет адреса".to_owned()))?;
    progress
        .total
        .store(manifest.plaintext_size, std::sync::atomic::Ordering::Relaxed);

    // Папка вложений появляется только при первой отправке, а входящий файл может
    // прийти и раньше — создаём её сами.
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Собираем открытый текст во временный файл: держать видео целиком в памяти телефон не готов.
    let temporary = destination.with_extension("plain.tmp");
    let mut whole = Sha256::new();
    let mut written = 0u64;
    {
        let mut output = BufWriter::new(File::create(&temporary)?);
        for index in 0..manifest.chunk_count {
            let packed = network.blob_get_chunk(
                &reference.base_url,
                &reference.object_id,
                index,
                &reference.read_capability,
            )?;
            if manifest
                .ciphertext_chunk_sha256
                .get(index as usize)
                .is_none_or(|expected| *expected != hex::encode(Sha256::digest(&packed)))
            {
                let _ = std::fs::remove_file(&temporary);
                return Err(CoreError::Crypto("Чанк вложения подменён".to_owned()));
            }
            if packed.len() < NONCE_SIZE + TAG_SIZE {
                let _ = std::fs::remove_file(&temporary);
                return Err(CoreError::Crypto("Чанк вложения обрезан".to_owned()));
            }
            let plaintext = cipher
                .decrypt(
                    Nonce::from_slice(&packed[..NONCE_SIZE]),
                    Payload {
                        msg: &packed[NONCE_SIZE..],
                        aad: &chunk_aad(&manifest.attachment_id, index),
                    },
                )
                .map_err(|_| CoreError::Crypto("Чанк вложения не расшифровывается".to_owned()))?;
            whole.update(&plaintext);
            output.write_all(&plaintext)?;
            written += plaintext.len() as u64;
            progress.done.store(written, std::sync::atomic::Ordering::Relaxed);
        }
        output.flush()?;
    }
    if written != manifest.plaintext_size || hex::encode(whole.finalize()) != manifest.plaintext_sha256
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(CoreError::Crypto("Вложение собрано неверно".to_owned()));
    }
    let result = media::encrypt_file(vault_key, &temporary, destination, progress);
    let _ = std::fs::remove_file(&temporary);
    result.map(|_| ())
}

fn chunk_aad(attachment_id: &str, index: i32) -> Vec<u8> {
    let mut result = attachment_id.as_bytes().to_vec();
    result.extend_from_slice(&index.to_be_bytes());
    result
}
