use std::path::{Path, PathBuf};

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use rand_core::{OsRng, RngCore};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Serialize, de::DeserializeOwned};

use crate::{
    CoreError,
    blobs::AttachmentManifest,
    identity::StoredIdentity,
    mailbox::{MailboxGrant, OwnedMailbox},
    models::{Contact, GroupRecord, Message, Profile, Settings},
    prekeys::PrekeyState,
    protocol::SignedProtocolEvent,
    ratchet::RatchetSession,
    routing::{SignedDeviceList, SignedRoutingDescriptor},
};

/// Вложение, ждущее выгрузки в хранилище. Сообщение уже видно в истории,
/// событие для собеседника соберётся, когда файл окажется на Node.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingUpload {
    pub event_id: String,
    pub user_id: String,
    pub caption: String,
    pub reply_to_event_id: Option<String>,
    pub forwarded_from: Option<String>,
    pub attachment: crate::models::Attachment,
}

/// Изменение группы, пришедшее раньше предыдущих: его нельзя проверить, пока не
/// известно состояние, на котором оно построено. Лежит, пока не догонят остальные.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingGroupState {
    pub event_id: String,
    pub actor: String,
    pub created_at_unix_milliseconds: i64,
    pub state: crate::models::GroupState,
}

/// Событие, ждущее отправки. Шифрование откладывается до самой доставки: пока
/// сообщение лежит в очереди, адрес собеседника может смениться.
pub struct OutboxJob {
    pub job_id: String,
    pub user_id: String,
    pub attempts: i64,
    pub event: SignedProtocolEvent,
}

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
             );
             CREATE TABLE IF NOT EXISTS ratchet_sessions(
               session_id TEXT PRIMARY KEY,
               peer_device_id TEXT NOT NULL,
               updated_at_ms INTEGER NOT NULL,
               value BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS ix_sessions_device
               ON ratchet_sessions(peer_device_id, updated_at_ms);
             CREATE TABLE IF NOT EXISTS peer_routing(
               user_id TEXT PRIMARY KEY,
               sequence_number INTEGER NOT NULL,
               value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS mailbox_grants(
               user_id TEXT NOT NULL,
               device_id TEXT NOT NULL,
               mailbox_id TEXT NOT NULL,
               expires_at_ms INTEGER NOT NULL,
               value BLOB NOT NULL,
               PRIMARY KEY(user_id, device_id, mailbox_id)
             );
             CREATE TABLE IF NOT EXISTS outbox(
               job_id TEXT PRIMARY KEY,
               user_id TEXT NOT NULL,
               event_id TEXT NOT NULL,
               created_at_ms INTEGER NOT NULL,
               attempts INTEGER NOT NULL DEFAULT 0,
               next_attempt_ms INTEGER NOT NULL DEFAULT 0,
               value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS pending_uploads(
               event_id TEXT PRIMARY KEY,
               created_at_ms INTEGER NOT NULL,
               value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS event_manifests(
               event_id TEXT PRIMARY KEY,
               value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS pending_blobs(
               event_id TEXT PRIMARY KEY,
               value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS seen_events(
               event_id TEXT PRIMARY KEY,
               seen_at_ms INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS groups(
               group_id TEXT PRIMARY KEY,
               value BLOB NOT NULL
             );
             CREATE TABLE IF NOT EXISTS group_pending_states(
               event_id TEXT PRIMARY KEY,
               group_id TEXT NOT NULL,
               epoch INTEGER NOT NULL,
               received_at_ms INTEGER NOT NULL,
               value BLOB NOT NULL
             );
             CREATE INDEX IF NOT EXISTS ix_group_pending_states
               ON group_pending_states(group_id, epoch);",
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
            let contact_devices: Vec<String> = self
                .peer_routing(user_id)?
                .map(|routing| {
                    routing
                        .descriptor
                        .devices
                        .iter()
                        .map(|entry| entry.identity.device_id.clone())
                        .collect()
                })
                .unwrap_or_default();
            let transaction = self.connection.unchecked_transaction()?;
            transaction.execute(
                "DELETE FROM events WHERE conversation_id=?1",
                [&conversation],
            )?;
            transaction.execute("DELETE FROM contacts WHERE user_id=?1", [user_id])?;
            // Вместе с диалогом уходят и ключи сессий с его устройствами: удалённая
            // переписка не должна оставлять за собой рабочий канал.
            transaction.execute("DELETE FROM outbox WHERE user_id=?1", [user_id])?;
            transaction.execute("DELETE FROM peer_routing WHERE user_id=?1", [user_id])?;
            transaction.execute("DELETE FROM mailbox_grants WHERE user_id=?1", [user_id])?;
            for device in contact_devices {
                transaction.execute(
                    "DELETE FROM ratchet_sessions WHERE peer_device_id=?1",
                    [&device],
                )?;
            }
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
            // Служебная отметка группы не считается непрочитанным и не прерывает счёт.
            let incoming = !message.outgoing && !message.service;
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

    pub fn mark_delivered(&self, event_id: &str) -> Result<(), CoreError> {
        if let Some(mut message) = self.message(event_id)? {
            message.delivered = true;
            self.save_message(&message)?;
        }
        Ok(())
    }

    /// Транспортное состояние живёт в той же зашифрованной базе, что и переписка:
    /// ключи сессий не должны оставаться в открытых файлах рядом с ней.
    pub fn prekey_state(&self) -> Result<Option<PrekeyState>, CoreError> {
        self.get_meta("prekeys")
    }

    pub fn save_prekey_state(&self, value: &PrekeyState) -> Result<(), CoreError> {
        self.set_meta("prekeys", value)
    }

    pub fn mailbox(&self) -> Result<Option<OwnedMailbox>, CoreError> {
        self.get_meta("mailbox")
    }

    pub fn save_mailbox(&self, value: &OwnedMailbox) -> Result<(), CoreError> {
        self.set_meta("mailbox", value)
    }

    pub fn device_list(&self) -> Result<Option<SignedDeviceList>, CoreError> {
        self.get_meta("device-list")
    }

    pub fn save_device_list(&self, value: &SignedDeviceList) -> Result<(), CoreError> {
        self.set_meta("device-list", value)
    }

    pub fn own_routing(&self) -> Result<Option<SignedRoutingDescriptor>, CoreError> {
        self.get_meta("own-routing")
    }

    pub fn save_own_routing(&self, value: &SignedRoutingDescriptor) -> Result<(), CoreError> {
        self.set_meta("own-routing", value)
    }

    /// Номер события устройства строго растёт: по нему получатель отбрасывает повторы.
    pub fn next_device_sequence(&self) -> Result<i64, CoreError> {
        let next = self.get_meta::<i64>("device-sequence")?.unwrap_or(0) + 1;
        self.set_meta("device-sequence", &next)?;
        Ok(next)
    }

    /// Номер уже опубликованной связки предключей: пока он совпадает с локальным,
    /// повторно дёргать Node незачем.
    pub fn published_prekey_sequence(&self) -> Result<i64, CoreError> {
        Ok(self.get_meta("prekeys-published")?.unwrap_or(0))
    }

    pub fn save_published_prekey_sequence(&self, value: i64) -> Result<(), CoreError> {
        self.set_meta("prekeys-published", &value)
    }

    pub fn routing_sequence(&self) -> Result<i64, CoreError> {
        Ok(self.get_meta("routing-sequence")?.unwrap_or(0))
    }

    pub fn save_routing_sequence(&self, value: i64) -> Result<(), CoreError> {
        self.set_meta("routing-sequence", &value)
    }

    /// Вложения, которые ещё не скачаны: сообщение уже в истории, а файл догружается
    /// фоном. Запись живёт, пока файл не окажется на диске.
    pub fn pending_blobs(&self) -> Result<Vec<(String, AttachmentManifest)>, CoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT event_id,value FROM pending_blobs LIMIT 50")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.map(|value| {
            let (event_id, encrypted) = value?;
            Ok((event_id, self.decrypt_json(&encrypted)?))
        })
        .collect()
    }

    pub fn save_pending_blob(
        &self,
        event_id: &str,
        manifest: &AttachmentManifest,
    ) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT INTO pending_blobs(event_id,value) VALUES(?1,?2)
             ON CONFLICT(event_id) DO UPDATE SET value=excluded.value",
            params![event_id, self.encrypt_json(manifest)?],
        )?;
        Ok(())
    }

    /// Файл, который уже лежит в сообщении, но ещё не выложен в хранилище: так
    /// вложение можно отправить офлайн — выгрузка случится при первой же связи.
    pub fn pending_uploads(&self) -> Result<Vec<PendingUpload>, CoreError> {
        let mut statement = self
            .connection
            .prepare("SELECT value FROM pending_uploads ORDER BY created_at_ms LIMIT 8")?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|value| self.decrypt_json(&value?)).collect()
    }

    pub fn save_pending_upload(&self, value: &PendingUpload) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT INTO pending_uploads(event_id,created_at_ms,value) VALUES(?1,?2,?3)
             ON CONFLICT(event_id) DO UPDATE SET value=excluded.value",
            params![
                value.event_id,
                chrono::Utc::now().timestamp_millis(),
                self.encrypt_json(value)?
            ],
        )?;
        Ok(())
    }

    pub fn clear_pending_upload(&self, event_id: &str) -> Result<(), CoreError> {
        self.connection
            .execute("DELETE FROM pending_uploads WHERE event_id=?1", [event_id])?;
        Ok(())
    }

    /// Манифест отправленного или полученного вложения. Нужен, чтобы переслать файл
    /// дальше, не выгружая его в хранилище заново.
    pub fn event_manifest(&self, event_id: &str) -> Result<Option<AttachmentManifest>, CoreError> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM event_manifests WHERE event_id=?1",
                [event_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        value.map(|bytes| self.decrypt_json(&bytes)).transpose()
    }

    pub fn save_event_manifest(
        &self,
        event_id: &str,
        manifest: &AttachmentManifest,
    ) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT INTO event_manifests(event_id,value) VALUES(?1,?2)
             ON CONFLICT(event_id) DO UPDATE SET value=excluded.value",
            params![event_id, self.encrypt_json(manifest)?],
        )?;
        Ok(())
    }

    pub fn clear_pending_blob(&self, event_id: &str) -> Result<(), CoreError> {
        self.connection
            .execute("DELETE FROM pending_blobs WHERE event_id=?1", [event_id])?;
        Ok(())
    }

    pub fn session(&self, session_id: &str) -> Result<Option<RatchetSession>, CoreError> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM ratchet_sessions WHERE session_id=?1",
                [session_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        value.map(|bytes| self.decrypt_json(&bytes)).transpose()
    }

    /// Самая свежая сессия с устройством: старые остаются, чтобы дочитать
    /// сообщения, которые ушли до пересоздания сессии.
    pub fn session_for_device(
        &self,
        peer_device_id: &str,
    ) -> Result<Option<RatchetSession>, CoreError> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM ratchet_sessions WHERE peer_device_id=?1
                 ORDER BY updated_at_ms DESC LIMIT 1",
                [peer_device_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        value.map(|bytes| self.decrypt_json(&bytes)).transpose()
    }

    pub fn save_session(&self, session: &RatchetSession) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT INTO ratchet_sessions(session_id,peer_device_id,updated_at_ms,value)
             VALUES(?1,?2,?3,?4)
             ON CONFLICT(session_id) DO UPDATE SET updated_at_ms=excluded.updated_at_ms,value=excluded.value",
            params![
                session.session_id,
                session.peer_device_id,
                chrono::Utc::now().timestamp_millis(),
                self.encrypt_json(session)?
            ],
        )?;
        Ok(())
    }

    pub fn peer_routing(
        &self,
        user_id: &str,
    ) -> Result<Option<SignedRoutingDescriptor>, CoreError> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM peer_routing WHERE user_id=?1",
                [user_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        value.map(|bytes| self.decrypt_json(&bytes)).transpose()
    }

    /// Адрес принимается только с номером выше сохранённого: так Node не сможет
    /// «откатить» собеседника на старый набор устройств.
    pub fn save_peer_routing(
        &self,
        routing: &SignedRoutingDescriptor,
    ) -> Result<bool, CoreError> {
        if let Some(existing) = self.peer_routing(&routing.descriptor.user_id)?
            && existing.descriptor.sequence >= routing.descriptor.sequence
        {
            return Ok(false);
        }
        self.connection.execute(
            "INSERT INTO peer_routing(user_id,sequence_number,value) VALUES(?1,?2,?3)
             ON CONFLICT(user_id) DO UPDATE SET sequence_number=excluded.sequence_number,value=excluded.value",
            params![
                routing.descriptor.user_id,
                routing.descriptor.sequence,
                self.encrypt_json(routing)?
            ],
        )?;
        Ok(true)
    }

    pub fn grants(&self, user_id: &str, device_id: &str) -> Result<Vec<MailboxGrant>, CoreError> {
        let mut statement = self.connection.prepare(
            "SELECT value FROM mailbox_grants WHERE user_id=?1 AND device_id=?2 AND expires_at_ms>?3",
        )?;
        let rows = statement.query_map(
            params![user_id, device_id, chrono::Utc::now().timestamp_millis()],
            |row| row.get::<_, Vec<u8>>(0),
        )?;
        rows.map(|value| self.decrypt_json(&value?)).collect()
    }

    pub fn save_grant(
        &self,
        user_id: &str,
        device_id: &str,
        grant: &MailboxGrant,
    ) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT INTO mailbox_grants(user_id,device_id,mailbox_id,expires_at_ms,value)
             VALUES(?1,?2,?3,?4,?5)
             ON CONFLICT(user_id,device_id,mailbox_id)
             DO UPDATE SET expires_at_ms=excluded.expires_at_ms,value=excluded.value",
            params![
                user_id,
                device_id,
                grant.mailbox_id,
                grant.expires_at_unix_milliseconds,
                self.encrypt_json(grant)?
            ],
        )?;
        Ok(())
    }

    pub fn enqueue_outbox(
        &self,
        user_id: &str,
        event: &SignedProtocolEvent,
    ) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT INTO outbox(job_id,user_id,event_id,created_at_ms,attempts,next_attempt_ms,value)
             VALUES(?1,?2,?3,?4,0,0,?5)",
            params![
                format!("job1-{}", crate::prekeys::random_token(16)),
                user_id,
                event.event_id,
                event.created_at_unix_milliseconds,
                self.encrypt_json(event)?
            ],
        )?;
        Ok(())
    }

    /// Список уже принятых событий нужен только на время жизни конверта на Node:
    /// хранить его вечно — значит бесконечно растить базу ради защиты от повторов.
    pub fn prune_seen_events(&self) -> Result<(), CoreError> {
        self.connection.execute(
            "DELETE FROM seen_events WHERE seen_at_ms < ?1",
            [chrono::Utc::now().timestamp_millis() - 30 * 86_400_000],
        )?;
        Ok(())
    }

    /// Новое действие в диалоге снимает паузу с отложенных задач: пользователь
    /// не должен ждать конца выдержки, если он только что снова написал.
    pub fn retry_now(&self, user_id: &str) -> Result<(), CoreError> {
        self.connection.execute(
            "UPDATE outbox SET next_attempt_ms=0 WHERE user_id=?1",
            [user_id],
        )?;
        Ok(())
    }

    pub fn due_outbox(&self, limit: usize) -> Result<Vec<OutboxJob>, CoreError> {
        let mut statement = self.connection.prepare(
            "SELECT job_id,user_id,attempts,value FROM outbox
             WHERE next_attempt_ms<=?1 ORDER BY created_at_ms LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![chrono::Utc::now().timestamp_millis(), limit as i64],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )?;
        rows.map(|value| {
            let (job_id, user_id, attempts, encrypted) = value?;
            Ok(OutboxJob {
                job_id,
                user_id,
                attempts,
                event: self.decrypt_json(&encrypted)?,
            })
        })
        .collect()
    }

    pub fn outbox_length(&self) -> Result<i64, CoreError> {
        Ok(self
            .connection
            .query_row("SELECT COUNT(*) FROM outbox", [], |row| row.get(0))?)
    }

    pub fn complete_outbox(&self, job_id: &str) -> Result<(), CoreError> {
        self.connection
            .execute("DELETE FROM outbox WHERE job_id=?1", [job_id])?;
        Ok(())
    }

    /// Неудачная попытка отодвигается по нарастающей: пока Node недоступен,
    /// клиент не должен колотиться в него каждую секунду.
    pub fn defer_outbox(&self, job_id: &str, attempts: i64) -> Result<(), CoreError> {
        let delay = (30_000i64 * (1 << attempts.min(6))).min(3_600_000);
        self.connection.execute(
            "UPDATE outbox SET attempts=?2, next_attempt_ms=?3 WHERE job_id=?1",
            params![
                job_id,
                attempts + 1,
                chrono::Utc::now().timestamp_millis() + delay
            ],
        )?;
        Ok(())
    }

    /// Возвращает `true`, если событие видим впервые: Node гарантирует доставку
    /// «хотя бы один раз», поэтому дубликаты приходят штатно.
    pub fn mark_seen(&self, event_id: &str) -> Result<bool, CoreError> {
        let inserted = self.connection.execute(
            "INSERT OR IGNORE INTO seen_events(event_id,seen_at_ms) VALUES(?1,?2)",
            params![event_id, chrono::Utc::now().timestamp_millis()],
        )?;
        Ok(inserted == 1)
    }

    pub fn groups(&self) -> Result<Vec<GroupRecord>, CoreError> {
        let mut statement = self.connection.prepare("SELECT value FROM groups")?;
        let rows = statement.query_map([], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|value| self.decrypt_json(&value?)).collect()
    }

    pub fn group(&self, group_id: &str) -> Result<Option<GroupRecord>, CoreError> {
        let value = self
            .connection
            .query_row(
                "SELECT value FROM groups WHERE group_id=?1",
                [group_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .optional()?;
        value.map(|bytes| self.decrypt_json(&bytes)).transpose()
    }

    pub fn save_group(&self, record: &GroupRecord) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT INTO groups(group_id,value) VALUES(?1,?2)
             ON CONFLICT(group_id) DO UPDATE SET value=excluded.value",
            params![record.state.group_id, self.encrypt_json(record)?],
        )?;
        Ok(())
    }

    pub fn save_pending_group_state(&self, value: &PendingGroupState) -> Result<(), CoreError> {
        self.connection.execute(
            "INSERT OR IGNORE INTO group_pending_states(event_id,group_id,epoch,received_at_ms,value)
             VALUES(?1,?2,?3,?4,?5)",
            params![
                value.event_id,
                value.state.group_id,
                value.state.epoch,
                chrono::Utc::now().timestamp_millis(),
                self.encrypt_json(value)?
            ],
        )?;
        Ok(())
    }

    pub fn pending_group_states(&self, group_id: &str) -> Result<Vec<PendingGroupState>, CoreError> {
        let mut statement = self.connection.prepare(
            "SELECT value FROM group_pending_states WHERE group_id=?1 ORDER BY epoch LIMIT 200",
        )?;
        let rows = statement.query_map([group_id], |row| row.get::<_, Vec<u8>>(0))?;
        rows.map(|value| self.decrypt_json(&value?)).collect()
    }

    pub fn delete_pending_group_state(&self, event_id: &str) -> Result<(), CoreError> {
        self.connection.execute(
            "DELETE FROM group_pending_states WHERE event_id=?1",
            [event_id],
        )?;
        Ok(())
    }

    pub fn clear_pending_group_states(&self, group_id: &str) -> Result<(), CoreError> {
        self.connection.execute(
            "DELETE FROM group_pending_states WHERE group_id=?1",
            [group_id],
        )?;
        Ok(())
    }

    /// Недостающее звено может так и не прийти: неделя — срок жизни конверта на Node.
    pub fn prune_pending_group_states(&self, older_than_ms: i64) -> Result<(), CoreError> {
        self.connection.execute(
            "DELETE FROM group_pending_states WHERE received_at_ms < ?1",
            [older_than_ms],
        )?;
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
            "groups": self.groups()?,
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
        transaction.execute("DELETE FROM groups", [])?;
        transaction.execute("DELETE FROM group_pending_states", [])?;
        transaction.commit()?;
        self.replace_identity(&serde_json::from_value(value["identity"].clone())?)?;
        self.save_profile(&serde_json::from_value(value["profile"].clone())?)?;
        self.save_settings(&serde_json::from_value(value["settings"].clone())?)?;
        for contact in serde_json::from_value::<Vec<Contact>>(value["contacts"].clone())? {
            self.save_contact(&contact)?;
        }
        // Копии, снятые до появления групп, поля `groups` не содержат.
        if let Some(groups) = value.get("groups").filter(|groups| groups.is_array()) {
            for group in serde_json::from_value::<Vec<GroupRecord>>(groups.clone())? {
                self.save_group(&group)?;
            }
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

    /// Ключ хранилища нужен фоновым потокам вложений: они шифруют файл без доступа к базе.
    pub fn vault_key(&self) -> &[u8; 32] {
        &self.key
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
