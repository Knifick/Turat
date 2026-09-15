//! Потоковый контейнер вложений.
//!
//! Раньше вложение шифровалось одним блоком AES-GCM: чтобы показать картинку или включить
//! видео, файл приходилось расшифровать целиком в память. Для 300-мегабайтного ролика это
//! и рывок интерфейса, и риск OOM на телефоне. Здесь тот же AES-GCM, но по чанкам: каждый
//! чанк шифруется отдельно и адресуется по индексу, поэтому плеер читает ровно те 256 КиБ,
//! которые ему нужны прямо сейчас, а перемотка не требует чтения всего файла.
//!
//! Раскладка файла:
//! ```text
//! magic       8   b"TTMEDIA1"
//! version     1   = 1
//! chunk_size  4   u32 LE, размер чанка открытого текста
//! plain_len   8   u64 LE, длина открытого текста целиком
//! nonce_prefix 8  случайный префикс nonce для этого файла
//! ---------------- 29 байт заголовка, дальше подряд идут чанки
//! chunk[i]        min(chunk_size, plain_len - i * chunk_size) + 16 байт тега
//! ```
//! Nonce чанка — `nonce_prefix || (i as u32 BE)`, AAD — весь заголовок: подменить или
//! обрезать файл, не сломав расшифровку, нельзя.

use std::{
    fs::File,
    io::{BufWriter, Read, Seek, SeekFrom, Write},
    path::Path,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use rand_core::{OsRng, RngCore};

use crate::CoreError;

const MAGIC: &[u8; 8] = b"TTMEDIA1";
const VERSION: u8 = 1;
const TAG_LENGTH: usize = 16;
pub const HEADER_LENGTH: usize = 29;
/// 256 КиБ — компромисс между гранулярностью перемотки и накладными расходами на теги.
pub const CHUNK_LENGTH: usize = 256 * 1024;

/// Счётчик прогресса, за которым следит интерфейс, пока фоновый поток шифрует или
/// расшифровывает файл.
#[derive(Debug, Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancelled: AtomicBool,
}

impl Progress {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    fn check(&self) -> Result<(), CoreError> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(CoreError::InvalidInput("Передача отменена".to_owned()));
        }
        Ok(())
    }
}

fn cipher(key: &[u8; 32]) -> Aes256Gcm {
    Aes256Gcm::new_from_slice(key).expect("32-byte key")
}

fn chunk_nonce(prefix: &[u8; 8], index: u32) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[..8].copy_from_slice(prefix);
    nonce[8..].copy_from_slice(&index.to_be_bytes());
    nonce
}

fn build_header(plain_len: u64, nonce_prefix: &[u8; 8]) -> [u8; HEADER_LENGTH] {
    let mut header = [0u8; HEADER_LENGTH];
    header[..8].copy_from_slice(MAGIC);
    header[8] = VERSION;
    header[9..13].copy_from_slice(&(CHUNK_LENGTH as u32).to_le_bytes());
    header[13..21].copy_from_slice(&plain_len.to_le_bytes());
    header[21..29].copy_from_slice(nonce_prefix);
    header
}

/// Шифрует файл в потоковый контейнер, обновляя `progress` после каждого чанка.
pub fn encrypt_file(
    key: &[u8; 32],
    source: &Path,
    destination: &Path,
    progress: &Progress,
) -> Result<u64, CoreError> {
    let mut input = File::open(source)?;
    let plain_len = input.metadata()?.len();
    progress.total.store(plain_len, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);

    let mut nonce_prefix = [0u8; 8];
    OsRng.fill_bytes(&mut nonce_prefix);
    let header = build_header(plain_len, &nonce_prefix);

    let cipher = cipher(key);
    let mut output = BufWriter::new(File::create(destination)?);
    output.write_all(&header)?;

    let mut buffer = vec![0u8; CHUNK_LENGTH];
    let mut index: u32 = 0;
    let mut written: u64 = 0;
    while written < plain_len {
        progress.check()?;
        let wanted = CHUNK_LENGTH.min((plain_len - written) as usize);
        input.read_exact(&mut buffer[..wanted])?;
        let sealed = cipher
            .encrypt(
                Nonce::from_slice(&chunk_nonce(&nonce_prefix, index)),
                Payload {
                    msg: &buffer[..wanted],
                    aad: &header,
                },
            )
            .map_err(|_| CoreError::Crypto("Не удалось зашифровать вложение".to_owned()))?;
        output.write_all(&sealed)?;
        written += wanted as u64;
        index += 1;
        progress.done.store(written, Ordering::Relaxed);
    }
    output.flush()?;
    Ok(plain_len)
}

/// Читатель вложения с произвольным доступом: расшифровывает только запрошенные чанки.
/// Файлы старого формата (единый блок AES-GCM) поддерживаются и просто держатся в памяти —
/// их размер был ограничен 32 МБ.
pub struct MediaReader {
    key: [u8; 32],
    file: Option<File>,
    header: [u8; HEADER_LENGTH],
    nonce_prefix: [u8; 8],
    chunk_length: usize,
    length: u64,
    cached_index: Option<u32>,
    cached_chunk: Vec<u8>,
    legacy: Option<Vec<u8>>,
}

impl MediaReader {
    pub fn open(key: &[u8; 32], path: &Path) -> Result<Self, CoreError> {
        let mut file = File::open(path)?;
        let file_length = file.metadata()?.len();
        let mut header = [0u8; HEADER_LENGTH];
        if file_length < HEADER_LENGTH as u64 || file.read_exact(&mut header).is_err() {
            return Self::open_legacy(key, path);
        }
        if &header[..8] != MAGIC || header[8] != VERSION {
            return Self::open_legacy(key, path);
        }
        let chunk_length = u32::from_le_bytes(header[9..13].try_into().expect("4 байта")) as usize;
        if chunk_length == 0 || chunk_length > 8 * 1024 * 1024 {
            return Err(CoreError::Crypto("Повреждён заголовок вложения".to_owned()));
        }
        let length = u64::from_le_bytes(header[13..21].try_into().expect("8 байт"));
        let mut nonce_prefix = [0u8; 8];
        nonce_prefix.copy_from_slice(&header[21..29]);
        Ok(Self {
            key: *key,
            file: Some(file),
            header,
            nonce_prefix,
            chunk_length,
            length,
            cached_index: None,
            cached_chunk: Vec::new(),
            legacy: None,
        })
    }

    fn open_legacy(key: &[u8; 32], path: &Path) -> Result<Self, CoreError> {
        let raw = std::fs::read(path)?;
        if raw.len() < 28 {
            return Err(CoreError::Crypto("Повреждено вложение".to_owned()));
        }
        let plain = cipher(key)
            .decrypt(Nonce::from_slice(&raw[..12]), &raw[12..])
            .map_err(|_| CoreError::Crypto("Не удалось расшифровать вложение".to_owned()))?;
        Ok(Self {
            key: *key,
            file: None,
            header: [0u8; HEADER_LENGTH],
            nonce_prefix: [0u8; 8],
            chunk_length: CHUNK_LENGTH,
            length: plain.len() as u64,
            cached_index: None,
            cached_chunk: Vec::new(),
            legacy: Some(plain),
        })
    }

    pub fn length(&self) -> u64 {
        self.length
    }

    fn load_chunk(&mut self, index: u32) -> Result<(), CoreError> {
        if self.cached_index == Some(index) {
            return Ok(());
        }
        let start = index as u64 * self.chunk_length as u64;
        let plain_length = self.chunk_length.min((self.length - start) as usize);
        let file = self
            .file
            .as_mut()
            .ok_or_else(|| CoreError::Crypto("Вложение недоступно".to_owned()))?;
        let offset = HEADER_LENGTH as u64
            + index as u64 * (self.chunk_length + TAG_LENGTH) as u64;
        file.seek(SeekFrom::Start(offset))?;
        let mut sealed = vec![0u8; plain_length + TAG_LENGTH];
        file.read_exact(&mut sealed)?;
        let plain = cipher(&self.key)
            .decrypt(
                Nonce::from_slice(&chunk_nonce(&self.nonce_prefix, index)),
                Payload {
                    msg: &sealed,
                    aad: &self.header,
                },
            )
            .map_err(|_| CoreError::Crypto("Не удалось расшифровать вложение".to_owned()))?;
        self.cached_chunk = plain;
        self.cached_index = Some(index);
        Ok(())
    }

    /// Читает до `output.len()` байт начиная с `offset`; возвращает сколько прочитано
    /// (0 — конец файла). Чтение может пересекать границы чанков.
    pub fn read_at(&mut self, offset: u64, output: &mut [u8]) -> Result<usize, CoreError> {
        if offset >= self.length || output.is_empty() {
            return Ok(0);
        }
        let wanted = output.len().min((self.length - offset) as usize);
        if let Some(plain) = &self.legacy {
            let start = offset as usize;
            output[..wanted].copy_from_slice(&plain[start..start + wanted]);
            return Ok(wanted);
        }
        let mut filled = 0usize;
        while filled < wanted {
            let position = offset + filled as u64;
            let index = (position / self.chunk_length as u64) as u32;
            let inside = (position % self.chunk_length as u64) as usize;
            self.load_chunk(index)?;
            let available = self.cached_chunk.len() - inside;
            let take = available.min(wanted - filled);
            output[filled..filled + take].copy_from_slice(&self.cached_chunk[inside..inside + take]);
            filled += take;
        }
        Ok(filled)
    }

    /// Полное чтение — для небольших вложений и проверок в тестах.
    #[allow(dead_code)]
    pub fn read_all(&mut self) -> Result<Vec<u8>, CoreError> {
        let mut output = vec![0u8; self.length as usize];
        let mut filled = 0usize;
        while filled < output.len() {
            let read = self.read_at(filled as u64, &mut output[filled..])?;
            if read == 0 {
                break;
            }
            filled += read;
        }
        output.truncate(filled);
        Ok(output)
    }
}

/// Расшифровывает вложение в обычный файл (сохранение «как есть»), обновляя прогресс.
pub fn decrypt_to_file(
    key: &[u8; 32],
    source: &Path,
    destination: &Path,
    progress: &Progress,
) -> Result<u64, CoreError> {
    let mut reader = MediaReader::open(key, source)?;
    let length = reader.length();
    progress.total.store(length, Ordering::Relaxed);
    progress.done.store(0, Ordering::Relaxed);
    let mut output = BufWriter::new(File::create(destination)?);
    let mut buffer = vec![0u8; CHUNK_LENGTH];
    let mut written = 0u64;
    while written < length {
        progress.check()?;
        let read = reader.read_at(written, &mut buffer)?;
        if read == 0 {
            break;
        }
        output.write_all(&buffer[..read])?;
        written += read as u64;
        progress.done.store(written, Ordering::Relaxed);
    }
    output.flush()?;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temporary(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("turat-media-{name}-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn round_trip_reads_any_range() {
        let key = [3u8; 32];
        let source = temporary("src");
        // Больше двух чанков, чтобы проверить чтение через границы.
        let plain: Vec<u8> = (0..(CHUNK_LENGTH * 2 + 12345))
            .map(|value| (value % 251) as u8)
            .collect();
        std::fs::write(&source, &plain).unwrap();
        let encrypted = temporary("enc");
        let progress = Progress::default();
        let length = encrypt_file(&key, &source, &encrypted, &progress).unwrap();
        assert_eq!(length, plain.len() as u64);
        assert_eq!(progress.done.load(Ordering::Relaxed), length);

        let mut reader = MediaReader::open(&key, &encrypted).unwrap();
        assert_eq!(reader.length(), plain.len() as u64);
        for offset in [0usize, 1, CHUNK_LENGTH - 5, CHUNK_LENGTH, CHUNK_LENGTH * 2 + 1] {
            let mut buffer = vec![0u8; 4096];
            let read = reader.read_at(offset as u64, &mut buffer).unwrap();
            assert_eq!(&buffer[..read], &plain[offset..offset + read]);
        }
        assert_eq!(reader.read_all().unwrap(), plain);

        let restored = temporary("out");
        decrypt_to_file(&key, &encrypted, &restored, &Progress::default()).unwrap();
        assert_eq!(std::fs::read(&restored).unwrap(), plain);

        for path in [source, encrypted, restored] {
            std::fs::remove_file(path).ok();
        }
    }

    #[test]
    fn wrong_key_is_rejected() {
        let source = temporary("src2");
        std::fs::write(&source, b"turat").unwrap();
        let encrypted = temporary("enc2");
        encrypt_file(&[1u8; 32], &source, &encrypted, &Progress::default()).unwrap();
        let mut reader = MediaReader::open(&[2u8; 32], &encrypted).unwrap();
        assert!(reader.read_all().is_err());
        std::fs::remove_file(source).ok();
        std::fs::remove_file(encrypted).ok();
    }
}
