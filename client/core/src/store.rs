use std::path::{Path, PathBuf};

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use rand_core::{OsRng, RngCore};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    CoreError,
    identity::StoredIdentity,
    models::{Contact, Message, Profile, Settings},
};

pub struct Store {
    connection: Connection,
    key: [u8; 32],
    pub app_dir: PathBuf,
}

impl Store {
    pub fn open(app_dir: &Path, key: [u8; 32]) -> Result<Self, CoreError> {
        std::fs::create_dir_all(app_dir.join("local-first"))?;
        let connection = Connection::open(app_dir.join("local-first").join("turattext-v3.db"))?;
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value BLOB NOT NULL);
             CREATE TABLE IF NOT EXISTS contacts(
               user_id TEXT PRIMARY KEY,
               added_at_ms INTEGER NOT NULL,
               value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS events(
               event_id TEXT PRIMARY KEY,
               conversation_id TEXT NOT NULL,
               created_at_ms INTEGER NOT NULL,
               delivery_state TEXT NOT NULL,
               value BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS ix_events_conversation_time
               ON events(conversation_id, created_at_ms);
             CREATE TABLE IF NOT EXISTS revoked_devices(
               device_id TEXT PRIMARY KEY,
               revoked_at_ms INTEGER NOT NULL
             );",
        )?;
        Ok(Self {
            connection,
            key,
            app_dir: app_dir.to_owned(),
        })
    }

    pub fn load_or_create_identity(&self) -> Result<StoredIdentity, CoreError> {
        if let Some(value) = self.get_meta::<StoredIdentity>("identity")? {
            return Ok(value);
        }
        let value = StoredIdentity::create()?;
        self.set_meta("identity", &value)?;
        Ok(value)
    }

    pub fn replace_identity(&self, identity: &StoredIdentity) -> Result<(), CoreError> {
        self.set_meta("identity", identity)
    }

    pub fn profile(&self) -> Result<Profile, CoreError> {
        Ok(self.get_meta("profile")?.unwrap_or_default())
    }

    pub fn save_profile(&self, profile: &Profile) -> Result<(), CoreError> {
        self.set_meta("profile", profile)
    }

    pub fn settings(&self) -> Result<Settings, CoreError> {
        Ok(self.get_meta("settings")?.unwrap_or_default())
    }

    pub fn save_settings(&self, settings: &Settings) -> Result<(), CoreError> {
        self.set_meta("settings", settings)
    }

    /// В meta может лежать JSON `null` — это «диалог не выбран», а не ошибка формата.
    pub fn selected_contact(&self) -> Result<Option<String>, CoreError> {
        Ok(self.get_meta::<Option<String>>("selected-contact")?.flatten())
    }

    pub fn set_selected_contact(&self, user_id: &Option<String>) -> Result<(), CoreError> {
        self.set_meta("selected-contact", user_id)
    }

    pub fn contacts(&self) -> Result<Vec<Contact>, CoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT value FROM contacts ORDER BY added_at_ms DESC")?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|value| self.decrypt_json(&value?)).collect()
    }

    pub fn contact(&self, user_id: &str) -> Result<Option<Contact>, CoreError> {
        let encrypted = self
            .connection
            .query_row(
                "SELECT value FROM contacts WHERE user_id=?1",
                [user_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        encrypted.map(|value| self.decrypt_json(&value)).transpose()
    }

    pub fn save_contact(&self, contact: &Contact) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT INTO contacts(user_id,added_at_ms,value) VALUES(?1,?2,?3)
             ON CONFLICT(user_id) DO UPDATE SET added_at_ms=excluded.added_at_ms,value=excluded.value",
            params![contact.user_id, contact.added_at_unix_milliseconds, self.encrypt_json(contact)?],
        )?;
        Ok(())
    }

    pub fn delete_contact(&self, user_id: &str) -> Result<(), CoreError> {
        if let Some(contact) = self.contact(user_id)? {
            let conversation = crate::identity::conversation_id(
                &self.load_or_create_identity()?.public.user_id,
                &contact.user_id,
            );
            let transaction = self.connection.unchecked_transaction()?;
            transaction.execute(
                "DELETE FROM events WHERE conversation_id=?1",
                [&conversation],
            )?;
            transaction.execute("DELETE FROM contacts WHERE user_id=?1", [user_id])?;
            transaction.commit()?;
        }
        Ok(())
    }

    pub fn save_message(&self, message: &Message) -> Result<(), CoreError> {
        let state = if message.delivered {
            "delivered"
        } else {
            "pending"
        };
        self.connection.execute(
            "INSERT INTO events(event_id,conversation_id,created_at_ms,delivery_state,value)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(event_id) DO UPDATE SET delivery_state=excluded.delivery_state,value=excluded.value",
            params![
                message.event_id,
                message.conversation_id,
                message.created_at_unix_milliseconds,
                state,
                self.encrypt_json(message)?
            ],
        )?;
        Ok(())
    }

    pub fn message(&self, event_id: &str) -> Result<Option<Message>, CoreError> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM events WHERE event_id=?1",
                [event_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        value.map(|bytes| self.decrypt_json(&bytes)).transpose()
    }

    pub fn messages(&self, conversation_id: &str) -> Result<Vec<Message>, CoreError> {
        let mut statement = self.connection.prepare(
            "SELECT value FROM events WHERE conversation_id=?1 ORDER BY created_at_ms,event_id LIMIT 10000"
        )?;
        let rows = statement.query_map([conversation_id], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|value| self.decrypt_json(&value?)).collect()
    }

    /// Последнее событие диалога и число непрочитанных входящих. `mark_read`
    /// помечает диалог целиком, поэтому непрочитанные всегда образуют хвост
    /// истории и обратный проход останавливается на первом прочитанном входящем.
    pub fn conversation_preview(
        &self,
        conversation_id: &str,
    ) -> Result<(Option<Message>, u32), CoreError> {
        let mut statement = self.connection.prepare(
            "SELECT value FROM events WHERE conversation_id=?1
             ORDER BY created_at_ms DESC,event_id DESC LIMIT 200",
        )?;
        let mut rows = statement.query([conversation_id])?;
        let mut last: Option<Message> = None;
        let mut unread = 0u32;
        while let Some(row) = rows.next()? {
            let message: Message = self.decrypt_json(&row.get::<_, Vec<u8>>(0)?)?;
            let incoming = !message.outgoing;
            let read = message.read;
            if last.is_none() {
                last = Some(message);
            }
            if incoming {
                if read {
                    break;
                }
                unread += 1;
            }
        }
        Ok((last, unread))
    }

    /// Очистка истории диалога с сохранением самого контакта.
    pub fn clear_conversation(&self, conversation_id: &str) -> Result<(), CoreError> {
        self.connection
            .execute("DELETE FROM events WHERE conversation_id=?1", [conversation_id])?;
        Ok(())
    }

    /// Поиск по тексту во всех диалогах — источник глобального поиска клиента.
    pub fn search_messages(&self, needle: &str, limit: usize) -> Result<Vec<Message>, CoreError> {
        let needle = needle.to_lowercase();
        let mut statement = self
            .connection
            .prepare("SELECT value FROM events ORDER BY created_at_ms DESC LIMIT 5000")?;
        let mut rows = statement.query([])?;
        let mut found = Vec::new();
        while let Some(row) = rows.next()? {
            let message: Message = self.decrypt_json(&row.get::<_, Vec<u8>>(0)?)?;
            if !message.deleted && message.text.to_lowercase().contains(&needle) {
                found.push(message);
                if found.len() >= limit {
                    break;
                }
            }
        }
        Ok(found)
    }

    pub fn pending_messages(&self) -> Result<Vec<Message>, CoreError> {
        let mut statement = self.connection.prepare(
            "SELECT value FROM events WHERE delivery_state='pending' ORDER BY created_at_ms LIMIT 1000"
        )?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|value| self.decrypt_json(&value?)).collect()
    }

    #[allow(dead_code)]
    pub fn mark_delivered(&self, event_id: &str) -> Result<(), CoreError> {
        if let Some(mut message) = self.message(event_id)? {
            message.delivered = true;
            self.save_message(&message)?;
        }
        Ok(())
    }

    pub fn revoke_device(&self, device_id: &str) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT OR REPLACE INTO revoked_devices(device_id,revoked_at_ms) VALUES(?1,?2)",
            params![device_id, chrono::Utc::now().timestamp_millis()],
        )?;
        Ok(())
    }

    pub fn export_plain(&self) -> Result<serde_json::Value, CoreError> {
        Ok(serde_json::json!({
            "version": 3,
            "identity": self.load_or_create_identity()?,
            "profile": self.profile()?,
            "settings": self.settings()?,
            "contacts": self.contacts()?,
            "events": self.all_messages()?
        }))
    }

    pub fn import_plain(&self, value: &serde_json::Value) -> Result<(), CoreError> {
        if value.get("version").and_then(|v| v.as_i64()) != Some(3) {
            return Err(CoreError::InvalidInput(
                "Неподдерживаемая версия резервной копии".to_owned(),
            ));
        }
        let transaction = self.connection.unchecked_transaction()?;
        transaction.execute("DELETE FROM contacts", [])?;
        transaction.execute("DELETE FROM events", [])?;
        transaction.commit()?;
        self.replace_identity(&serde_json::from_value(value["identity"].clone())?)?;
        self.save_profile(&serde_json::from_value(value["profile"].clone())?)?;
        self.save_settings(&serde_json::from_value(value["settings"].clone())?)?;
        for contact in serde_json::from_value::<Vec<Contact>>(value["contacts"].clone())? {
            self.save_contact(&contact)?;
        }
        for message in serde_json::from_value::<Vec<Message>>(value["events"].clone())? {
            self.save_message(&message)?;
        }
        Ok(())
    }

    fn all_messages(&self) -> Result<Vec<Message>, CoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT value FROM events ORDER BY created_at_ms")?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|value| self.decrypt_json(&value?)).collect()
    }

    fn get_meta<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>, CoreError> {
        let value = self
            .connection
            .query_row("SELECT value FROM meta WHERE key=?1", [key], |row| {
                row.get::<_, Vec<u8>>(0)
            })
            .optional()?;
        value.map(|bytes| self.decrypt_json(&bytes)).transpose()
    }

    fn set_meta<T: Serialize + ?Sized>(&self, key: &str, value: &T) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT INTO meta(key,value) VALUES(?1,?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, self.encrypt_json(value)?],
        )?;
        Ok(())
    }

    pub fn encrypt_bytes(&self, value: &[u8]) -> Result<Vec<u8>, CoreError> {
        let cipher = Aes256Gcm::new_from_slice(&self.key).expect("32-byte key");
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let mut result = nonce_bytes.to_vec();
        result.extend(
            cipher
                .encrypt(Nonce::from_slice(&nonce_bytes), value)
                .map_err(|_| {
                    CoreError::Crypto("Не удалось зашифровать локальные данные".to_owned())
                })?,
        );
        Ok(result)
    }

    pub fn decrypt_bytes(&self, value: &[u8]) -> Result<Vec<u8>, CoreError> {
        if value.len() < 28 {
            return Err(CoreError::Crypto(
                "Повреждены зашифрованные локальные данные".to_owned(),
            ));
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key).expect("32-byte key");
        cipher
            .decrypt(Nonce::from_slice(&value[..12]), &value[12..])
            .map_err(|_| {
                CoreError::Crypto("Неверный ключ или повреждены локальные данные".to_owned())
            })
    }

    fn encrypt_json<T: Serialize + ?Sized>(&self, value: &T) -> Result<Vec<u8>, CoreError> {
        self.encrypt_bytes(&serde_json::to_vec(value)?)
    }

    fn decrypt_json<T: DeserializeOwned>(&self, value: &[u8]) -> Result<T, CoreError> {
        Ok(serde_json::from_slice(&self.decrypt_bytes(value)?)?)
    }
}
