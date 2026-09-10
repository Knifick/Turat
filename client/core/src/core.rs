use std::{fs, path::Path};

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use argon2::Argon2;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand_core::{OsRng, RngCore};
use serde_json::json;

use crate::{
    CoreError,
    identity::{StoredIdentity, conversation_id},
    models::{
        Attachment, Chat, Command, Contact, Message, MetadataProtection, Profile, Response,
        SearchHit, Snapshot,
    },
    network::{Network, expected_node_for},
    store::Store,
};

/// Опрос «последней активности» контактов: реже, чем сама синхронизация.
const PRESENCE_POLL_INTERVAL_MILLISECONDS: i64 = 60_000;
const PRESENCE_POLL_CONTACTS: usize = 50;

pub struct AppCore {
    store: Store,
    identity: StoredIdentity,
    network: Network,
    selected_contact: Option<String>,
    online: bool,
    status: String,
    search_query: String,
    /// Опрос «последней активности» стоит запроса на контакт: фоновая синхронизация делает его
    /// заметно реже, чем всё остальное.
    last_presence_poll_unix_milliseconds: i64,
}

impl AppCore {
    pub fn open(app_dir: &Path, vault_key: [u8; 32]) -> Result<Self, CoreError> {
        let store = Store::open(app_dir, vault_key)?;
        let identity = store.load_or_create_identity()?;
        let selected_contact = store.selected_contact()?;
        Ok(Self {
            store,
            identity,
            network: Network::new()?,
            selected_contact,
            online: false,
            status: "Локальное хранилище готово".to_owned(),
            search_query: String::new(),
            last_presence_poll_unix_milliseconds: 0,
        })
    }

    pub fn invoke(&mut self, request: &str) -> String {
        let result = serde_json::from_str::<Command>(request)
            .map_err(CoreError::from)
            .and_then(|command| self.execute(command));
        let response = match result {
            Ok(value) => Response {
                ok: true,
                snapshot: Some(
                    self.snapshot()
                        .unwrap_or_else(|error| self.error_snapshot(error.to_string())),
                ),
                value,
                error: None,
            },
            Err(error) => {
                self.status = error.to_string();
                Response {
                    ok: false,
                    snapshot: self.snapshot().ok(),
                    value: None,
                    error: Some(error.to_string()),
                }
            }
        };
        serde_json::to_string(&response).unwrap_or_else(|_| {
            "{\"ok\":false,\"error\":\"Не удалось сериализовать ответ ядра\"}".to_owned()
        })
    }

    fn execute(&mut self, command: Command) -> Result<Option<serde_json::Value>, CoreError> {
        match command {
            Command::Snapshot => {}
            Command::SelectContact { user_id } => {
                self.selected_contact = user_id;
                self.store.set_selected_contact(&self.selected_contact)?;
            }
            Command::AddContact {
                query,
                display_name,
            } => self.add_contact(&query, display_name)?,
            Command::DeleteContact { user_id } | Command::RejectContact { user_id } => {
                self.store.delete_contact(&user_id)?;
                if self.selected_contact.as_deref() == Some(&user_id) {
                    self.selected_contact = None;
                }
                self.store.set_selected_contact(&self.selected_contact)?;
                self.status = "Диалог удалён".to_owned();
            }
            Command::AcceptContact { user_id } => {
                self.update_contact(&user_id, |c| c.pending_approval = false)?
            }
            Command::VerifyContact { user_id, verified } => {
                self.update_contact(&user_id, |c| c.fingerprint_verified = verified)?
            }
            Command::SendText {
                user_id,
                text,
                reply_to_event_id,
            } => self.send_text(&user_id, &text, None, reply_to_event_id)?,
            Command::ForwardMessages { event_ids, user_id } => {
                self.forward_messages(&event_ids, &user_id)?
            }
            Command::SetChatPinned { user_id, pinned } => {
                self.update_contact(&user_id, |contact| contact.pinned = pinned)?;
                self.status = if pinned {
                    "Чат закреплён".to_owned()
                } else {
                    "Чат откреплён".to_owned()
                };
            }
            Command::SetChatMuted { user_id, muted } => {
                self.update_contact(&user_id, |contact| contact.muted = muted)?;
                self.status = if muted {
                    "Уведомления выключены".to_owned()
                } else {
                    "Уведомления включены".to_owned()
                };
            }
            Command::SaveDraft { user_id, text } => {
                let draft = text.trim().to_owned();
                self.update_contact(&user_id, |contact| contact.draft = draft)?;
            }
            Command::ClearHistory { user_id } => {
                let conversation = conversation_id(&self.identity.public.user_id, &user_id);
                self.store.clear_conversation(&conversation)?;
                self.status = "История диалога очищена".to_owned();
            }
            Command::MarkUnread { user_id } => {
                self.update_contact(&user_id, |contact| contact.manual_unread = true)?;
            }
            Command::Search { query } => self.search_query = query.trim().to_owned(),
            Command::SetPresencePublishing { enabled } => {
                let mut settings = self.store.settings()?;
                settings.publish_presence = enabled;
                self.store.save_settings(&settings)?;
                // Выключение отзывает уже опубликованную запись, а не ждёт её истечения.
                let published = self.publish_presence(if enabled {
                    chrono::Utc::now().timestamp_millis()
                } else {
                    0
                });
                self.status = match (enabled, published) {
                    (true, Ok(())) => "Последняя активность публикуется в directory".to_owned(),
                    (false, Ok(())) => "Последняя активность скрыта и отозвана".to_owned(),
                    (true, Err(error)) => format!("Настройка сохранена. Directory: {error}"),
                    (false, Err(error)) => {
                        format!("Публикация выключена, но запись не отозвана: {error}")
                    }
                };
            }
            Command::EditMessage { event_id, text } => {
                if text.trim().is_empty() {
                    return Err(CoreError::InvalidInput("Сообщение пустое".to_owned()));
                }
                let mut message = self.require_message(&event_id)?;
                if !message.outgoing || message.deleted {
                    return Err(CoreError::InvalidInput(
                        "Сообщение нельзя изменить".to_owned(),
                    ));
                }
                message.text = text.trim().to_owned();
                message.edited = true;
                message.delivered = false;
                self.store.save_message(&message)?;
                self.status = "Изменение сохранено в outbox".to_owned();
            }
            Command::DeleteMessages { event_ids } => {
                for event_id in event_ids {
                    let mut message = self.require_message(&event_id)?;
                    if message.outgoing && !message.deleted {
                        message.deleted = true;
                        message.text.clear();
                        message.attachment = None;
                        message.delivered = false;
                        self.store.save_message(&message)?;
                    }
                }
            }
            Command::React {
                event_ids,
                reaction,
            } => {
                const ALLOWED: &[&str] = &["❤", "🔥", "👌", "😱", "😭", "🤨", "👍", "💔"];
                if !ALLOWED.contains(&reaction.as_str()) {
                    return Err(CoreError::InvalidInput("Неизвестная реакция".to_owned()));
                }
                for event_id in event_ids {
                    let mut message = self.require_message(&event_id)?;
                    if !message.deleted {
                        if message.reactions.contains(&reaction) {
                            message.reactions.retain(|v| v != &reaction);
                        } else {
                            message.reactions.push(reaction.clone());
                        }
                        message.delivered = false;
                        self.store.save_message(&message)?;
                    }
                }
            }
            Command::MarkRead { user_id } => {
                let conversation = conversation_id(&self.identity.public.user_id, &user_id);
                for mut message in self.store.messages(&conversation)? {
                    if !message.outgoing && !message.read {
                        message.read = true;
                        self.store.save_message(&message)?;
                    }
                }
                if let Some(contact) = self.store.contact(&user_id)? {
                    if contact.manual_unread {
                        self.update_contact(&user_id, |value| value.manual_unread = false)?;
                    }
                }
            }
            Command::SaveProfile {
                username,
                display_name,
                about,
                avatar_base64,
            } => {
                if display_name.trim().is_empty() || display_name.chars().count() > 64 {
                    return Err(CoreError::InvalidInput(
                        "Видимое имя: 1–64 символа".to_owned(),
                    ));
                }
                if about.chars().count() > 200 {
                    return Err(CoreError::InvalidInput(
                        "Раздел «О себе»: максимум 200 символов".to_owned(),
                    ));
                }
                if !username.trim().is_empty() {
                    crate::network::normalize_username(&username)?;
                }
                let profile = Profile {
                    username: username.trim().trim_start_matches('@').to_ascii_lowercase(),
                    display_name: display_name.trim().to_owned(),
                    about: about.trim().to_owned(),
                    avatar_base64,
                };
                self.store.save_profile(&profile)?;
                self.status = "Профиль сохранён локально".to_owned();
            }
            Command::PublishProfile => {
                let profile = self.store.profile()?;
                if profile.display_name.trim().is_empty() {
                    return Err(CoreError::InvalidInput(
                        "Сначала заполните видимое имя".to_owned(),
                    ));
                }
                self.status = if self.publish_directory(&profile)? {
                    "Профиль опубликован: вас найдут по username".to_owned()
                } else {
                    "Публиковать профиль может только корневое устройство".to_owned()
                };
            }
            Command::Connect { bootstrap_url } => {
                let expected = expected_node_for(&bootstrap_url);
                self.network.forget_descriptor(&bootstrap_url);
                let descriptor = match self.network.descriptor(&bootstrap_url, expected) {
                    Ok(value) => value,
                    Err(error) => {
                        self.online = false;
                        return Err(error);
                    }
                };
                let mut settings = self.store.settings()?;
                settings.bootstrap_url = descriptor.base_url.clone();
                settings.expected_node_id = Some(descriptor.node_id);
                self.store.save_settings(&settings)?;
                self.online = true;
                self.status = format!("Подключено: {}", descriptor.name);
            }
            // Синхронизация идёт в фоне сама, поэтому обрыв связи — это состояние, а не ошибка
            // команды: клиенту незачем показывать диалог каждые несколько секунд.
            Command::Sync => {
                if let Err(error) = self.sync() {
                    self.online = false;
                    self.status = format!("Нет связи с Node: {error}");
                }
            }
            Command::CycleMetadataProtection => {
                let mut settings = self.store.settings()?;
                settings.metadata_protection = match settings.metadata_protection {
                    MetadataProtection::Fast => MetadataProtection::Balanced,
                    MetadataProtection::Balanced => MetadataProtection::High,
                    MetadataProtection::High => MetadataProtection::Fast,
                };
                self.store.save_settings(&settings)?;
                self.status = "Режим защиты метаданных изменён".to_owned();
            }
            Command::AttachFile {
                user_id,
                path,
                mime_type,
                caption,
                reply_to_event_id,
            } => {
                let bytes = fs::read(&path)?;
                if bytes.len() > 32 * 1024 * 1024 {
                    return Err(CoreError::InvalidInput("Файл больше 32 МБ".to_owned()));
                }
                let attachment_id = format!("att1-{}", random_hex(16));
                let encrypted = self.store.encrypt_bytes(&bytes)?;
                let directory = self.store.app_dir.join("local-first").join("attachments");
                fs::create_dir_all(&directory)?;
                let target = directory.join(format!("{attachment_id}.bin"));
                fs::write(&target, encrypted)?;
                let name = Path::new(&path)
                    .file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or("attachment")
                    .to_owned();
                let attachment = Attachment {
                    attachment_id,
                    file_name: name,
                    mime_type,
                    size: bytes.len() as u64,
                    local_path: target.to_string_lossy().into_owned(),
                };
                self.send_text(
                    &user_id,
                    caption.as_deref().unwrap_or(""),
                    Some(attachment),
                    reply_to_event_id,
                )?;
            }
            Command::ExportAttachment {
                event_id,
                destination_path,
            } => {
                let message = self.require_message(&event_id)?;
                let attachment = message
                    .attachment
                    .ok_or_else(|| CoreError::InvalidInput("В сообщении нет файла".to_owned()))?;
                let value = self
                    .store
                    .decrypt_bytes(&fs::read(attachment.local_path)?)?;
                fs::write(destination_path, value)?;
                self.status = "Файл сохранён".to_owned();
            }
            Command::CreateBackup { path, passphrase } => {
                ensure_passphrase(&passphrase)?;
                write_secret_file(
                    &path,
                    b"TTBACKUP3",
                    &passphrase,
                    &serde_json::to_vec(&self.store.export_plain()?)?,
                )?;
                self.status = "Зашифрованная резервная копия создана".to_owned();
            }
            Command::RestoreBackup { path, passphrase } => {
                let value: serde_json::Value =
                    serde_json::from_slice(&read_secret_file(&path, b"TTBACKUP3", &passphrase)?)?;
                self.store.import_plain(&value)?;
                self.identity = self.store.load_or_create_identity()?;
                self.status = "Резервная копия восстановлена".to_owned();
            }
            Command::CreateDeviceLink { path, passphrase } => {
                ensure_passphrase(&passphrase)?;
                let linked = self.identity.linked_device()?;
                write_secret_file(
                    &path,
                    b"TTLINKV3",
                    &passphrase,
                    &serde_json::to_vec(&linked)?,
                )?;
                self.status = "Пакет привязки устройства создан".to_owned();
            }
            Command::ImportDeviceLink { path, passphrase } => {
                let linked: StoredIdentity =
                    serde_json::from_slice(&read_secret_file(&path, b"TTLINKV3", &passphrase)?)?;
                self.store.replace_identity(&linked)?;
                self.identity = linked;
                self.status = "Устройство привязано".to_owned();
            }
            Command::ExportPortable { path } => {
                let pending = self.store.pending_messages()?;
                let body = serde_json::to_vec(
                    &json!({"version":3,"sender":self.identity.public,"events":pending}),
                )?;
                let signature = self.identity.sign_device(&body)?;
                fs::write(
                    path,
                    serde_json::to_vec_pretty(
                        &json!({"body":STANDARD.encode(body),"signature":signature}),
                    )?,
                )?;
                self.status = "Переносимый пакет создан".to_owned();
            }
            Command::ImportPortable { path } => {
                let outer: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
                let body = STANDARD.decode(
                    outer["body"]
                        .as_str()
                        .ok_or_else(|| CoreError::InvalidInput("Повреждён пакет".to_owned()))?,
                )?;
                let value: serde_json::Value = serde_json::from_slice(&body)?;
                let events: Vec<Message> = serde_json::from_value(value["events"].clone())?;
                for message in events {
                    if !message.outgoing {
                        self.store.save_message(&message)?;
                    }
                }
                self.status = "Переносимый пакет импортирован".to_owned();
            }
            Command::ExportDiscovery { path } => {
                let settings = self.store.settings()?;
                let body = json!({"version":3,"userId":self.identity.public.user_id,"bootstrapUrl":settings.bootstrap_url,"createdAt":chrono::Utc::now().timestamp_millis()});
                fs::write(path, serde_json::to_vec_pretty(&body)?)?;
                self.status = "Discovery bundle создан".to_owned();
            }
            Command::ImportDiscovery { path } => {
                let value: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
                let url = value["bootstrapUrl"].as_str().ok_or_else(|| {
                    CoreError::InvalidInput("Повреждён discovery bundle".to_owned())
                })?;
                self.network.descriptor(url, expected_node_for(url))?;
                let mut settings = self.store.settings()?;
                settings.bootstrap_url = url.to_owned();
                self.store.save_settings(&settings)?;
                self.status = "Сеть импортирована и проверена".to_owned();
            }
            Command::RevokeDevice { device_id } => {
                if !self.identity.is_authority() {
                    return Err(CoreError::InvalidInput(
                        "Отзыв доступен только корневому устройству".to_owned(),
                    ));
                }
                if device_id == self.identity.public.device_id {
                    return Err(CoreError::InvalidInput(
                        "Нельзя отозвать текущее устройство".to_owned(),
                    ));
                }
                self.store.revoke_device(&device_id)?;
                self.status = "Устройство добавлено в подписанный список отзыва".to_owned();
            }
        }
        Ok(None)
    }

    fn add_contact(&mut self, query: &str, display_name: Option<String>) -> Result<(), CoreError> {
        let query = query.trim();
        if query.is_empty() {
            return Err(CoreError::InvalidInput(
                "Введите @username или UserID".to_owned(),
            ));
        }
        let settings = self.store.settings()?;
        let descriptor = self.network.descriptor(
            &settings.bootstrap_url,
            settings.expected_node_id.as_deref(),
        )?;
        self.online = true;
        let user_id = if query.starts_with("tt1-") {
            query.to_owned()
        } else {
            self.network.resolve_username(&descriptor, query)?
        };
        if user_id == self.identity.public.user_id {
            return Err(CoreError::InvalidInput(
                "Нельзя добавить самого себя".to_owned(),
            ));
        }
        let profile = self.network.profile(&descriptor, &user_id).ok().flatten();
        let display = display_name
            .filter(|v| !v.trim().is_empty())
            .or_else(|| profile.as_ref().map(|v| v.display_name.clone()))
            .unwrap_or_else(|| short_id(&user_id));
        let contact = Contact {
            user_id: user_id.clone(),
            display_name: display,
            username: (!query.starts_with("tt1-"))
                .then(|| query.trim_start_matches('@').to_owned()),
            about: profile.as_ref().map(|v| v.about.clone()),
            avatar_base64: profile.and_then(|v| v.avatar_base64),
            added_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
            fingerprint_verified: false,
            pending_approval: false,
            last_seen_unix_milliseconds: None,
            pinned: false,
            muted: false,
            draft: String::new(),
            manual_unread: false,
        };
        self.store.save_contact(&contact)?;
        self.selected_contact = Some(user_id);
        self.store.set_selected_contact(&self.selected_contact)?;
        self.status = "Защищённый диалог добавлен".to_owned();
        Ok(())
    }

    fn update_contact(
        &mut self,
        user_id: &str,
        update: impl FnOnce(&mut Contact),
    ) -> Result<(), CoreError> {
        let mut contact = self
            .store
            .contact(user_id)?
            .ok_or_else(|| CoreError::InvalidInput("Контакт не найден".to_owned()))?;
        update(&mut contact);
        self.store.save_contact(&contact)
    }

    fn send_text(
        &mut self,
        user_id: &str,
        text: &str,
        attachment: Option<Attachment>,
        reply_to_event_id: Option<String>,
    ) -> Result<(), CoreError> {
        if text.trim().is_empty() && attachment.is_none() {
            return Err(CoreError::InvalidInput("Сообщение пустое".to_owned()));
        }
        if text.len() > 64 * 1024 {
            return Err(CoreError::InvalidInput(
                "Сообщение слишком большое".to_owned(),
            ));
        }
        let contact = self
            .store
            .contact(user_id)?
            .ok_or_else(|| CoreError::InvalidInput("Контакт не найден".to_owned()))?;
        if contact.pending_approval && attachment.is_some() {
            return Err(CoreError::InvalidInput(
                "Файлы доступны после принятия контакта".to_owned(),
            ));
        }
        let message = Message {
            event_id: format!("evt1-{}", random_hex(16)),
            conversation_id: conversation_id(&self.identity.public.user_id, user_id),
            sender_user_id: self.identity.public.user_id.clone(),
            text: text.trim().to_owned(),
            created_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
            outgoing: true,
            edited: false,
            deleted: false,
            reactions: Vec::new(),
            delivered: false,
            read: true,
            attachment,
            reply_to_event_id,
            forwarded_from: None,
        };
        self.store.save_message(&message)?;
        if !contact.draft.is_empty() {
            self.update_contact(user_id, |value| value.draft.clear())?;
        }
        self.status = "Сообщение сохранено в outbox".to_owned();
        Ok(())
    }

    /// Пересылка сообщений в другой диалог с сохранением автора оригинала.
    fn forward_messages(&mut self, event_ids: &[String], user_id: &str) -> Result<(), CoreError> {
        let target = self
            .store
            .contact(user_id)?
            .ok_or_else(|| CoreError::InvalidInput("Контакт не найден".to_owned()))?;
        if target.pending_approval {
            return Err(CoreError::InvalidInput(
                "Переслать можно только в принятый диалог".to_owned(),
            ));
        }
        let own_name = self.store.profile()?.display_name;
        let conversation = conversation_id(&self.identity.public.user_id, user_id);
        for event_id in event_ids {
            let source = self.require_message(event_id)?;
            if source.deleted {
                continue;
            }
            let author = match source.forwarded_from.clone() {
                Some(value) => value,
                None if source.outgoing => own_name.clone(),
                None => self
                    .store
                    .contact(&source.sender_user_id)?
                    .map(|value| value.display_name)
                    .unwrap_or_else(|| short_id(&source.sender_user_id)),
            };
            let message = Message {
                event_id: format!("evt1-{}", random_hex(16)),
                conversation_id: conversation.clone(),
                sender_user_id: self.identity.public.user_id.clone(),
                text: source.text.clone(),
                created_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
                outgoing: true,
                edited: false,
                deleted: false,
                reactions: Vec::new(),
                delivered: false,
                read: true,
                attachment: source.attachment.clone(),
                reply_to_event_id: None,
                forwarded_from: Some(author),
            };
            self.store.save_message(&message)?;
        }
        self.selected_contact = Some(user_id.to_owned());
        self.store.set_selected_contact(&self.selected_contact)?;
        self.status = "Сообщения пересланы".to_owned();
        Ok(())
    }

    fn sync(&mut self) -> Result<(), CoreError> {
        let settings = self.store.settings()?;
        let descriptor = self.network.descriptor(
            &settings.bootstrap_url,
            settings.expected_node_id.as_deref(),
        )?;
        self.online = true;
        let now = chrono::Utc::now().timestamp_millis();
        let profile = self.store.profile()?;
        if settings.directory_sequence == 0 && !profile.display_name.trim().is_empty() {
            let _ = self.publish_directory(&profile);
        }
        if now - self.last_presence_poll_unix_milliseconds >= PRESENCE_POLL_INTERVAL_MILLISECONDS {
            self.last_presence_poll_unix_milliseconds = now;
            if settings.publish_presence {
                let _ = self.publish_presence(now);
            }
            for mut contact in self.store.contacts()?.into_iter().take(PRESENCE_POLL_CONTACTS) {
                if let Ok(Some(seen)) = self.network.presence(&descriptor, &contact.user_id) {
                    if contact.last_seen_unix_milliseconds != Some(seen) {
                        contact.last_seen_unix_milliseconds = Some(seen);
                        self.store.save_contact(&contact)?;
                    }
                }
            }
        }
        let pending = self.store.pending_messages()?.len();
        self.status = if pending == 0 {
            format!("Синхронизировано с {}", descriptor.name)
        } else {
            format!("{}: в outbox {} событий", descriptor.name, pending)
        };
        Ok(())
    }

    /// Публикация профиля и username: без неё собеседники не найдут пользователя.
    /// Доступна только корневому устройству — подпись даёт identity-ключ.
    fn publish_directory(&mut self, profile: &Profile) -> Result<bool, CoreError> {
        if !self.identity.is_authority() {
            return Ok(false);
        }
        let mut settings = self.store.settings()?;
        let descriptor = self.network.descriptor(
            &settings.bootstrap_url,
            settings.expected_node_id.as_deref(),
        )?;
        let now = chrono::Utc::now().timestamp_millis();
        let sequence = (settings.directory_sequence + 1).max(now);
        let expires = now + 30 * 24 * 60 * 60 * 1000;
        let user_id = self.identity.public.user_id.clone();
        let identity_key = self.identity.public.identity_public_key.clone();

        let claim = serde_json::to_string(&json!({
            "version": 1,
            "userId": user_id,
            "sequence": sequence,
            "displayName": profile.display_name,
            "about": profile.about,
            "avatarBase64": profile.avatar_base64,
            "expiresAtUnixMilliseconds": expires,
        }))?;
        let signature = self.identity.sign_identity(claim.as_bytes())?;
        self.network.publish_profile(
            &descriptor,
            &user_id,
            &identity_key,
            sequence,
            &claim,
            &signature,
            expires,
        )?;

        if !profile.username.is_empty() {
            let username = crate::network::normalize_username(&profile.username)?;
            let claim = serde_json::to_string(&json!({
                "version": 2,
                "username": username,
                "userId": user_id,
                "sequence": sequence,
                "expiresAtUnixMilliseconds": expires,
            }))?;
            let signature = self.identity.sign_identity(claim.as_bytes())?;
            self.network.publish_username(
                &descriptor,
                &username,
                &user_id,
                &identity_key,
                sequence,
                &claim,
                &signature,
                expires,
            )?;
        }

        settings.directory_sequence = sequence;
        self.store.save_settings(&settings)?;
        self.online = true;
        Ok(true)
    }

    /// Подписанная отметка «был(а) в сети» — публикуется только по согласию.
    fn publish_presence(&mut self, last_seen: i64) -> Result<(), CoreError> {
        if !self.identity.is_authority() {
            return Ok(());
        }
        let mut settings = self.store.settings()?;
        let descriptor = self.network.descriptor(
            &settings.bootstrap_url,
            settings.expected_node_id.as_deref(),
        )?;
        let now = chrono::Utc::now().timestamp_millis();
        let sequence = (settings.directory_sequence + 1).max(now);
        let expires = now + 7 * 24 * 60 * 60 * 1000;
        let user_id = self.identity.public.user_id.clone();
        let claim = serde_json::to_string(&json!({
            "version": 1,
            "userId": user_id,
            "lastSeenUnixMilliseconds": last_seen,
            "expiresAtUnixMilliseconds": expires,
        }))?;
        let signature = self.identity.sign_identity(claim.as_bytes())?;
        self.network.publish_presence(
            &descriptor,
            &user_id,
            &self.identity.public.identity_public_key.clone(),
            sequence,
            &claim,
            &signature,
            expires,
        )?;
        settings.directory_sequence = sequence;
        self.store.save_settings(&settings)?;
        Ok(())
    }

    fn require_message(&self, event_id: &str) -> Result<Message, CoreError> {
        self.store
            .message(event_id)?
            .ok_or_else(|| CoreError::InvalidInput("Сообщение не найдено".to_owned()))
    }

    fn snapshot(&self) -> Result<Snapshot, CoreError> {
        let profile = self.store.profile()?;
        let chats = self.chats()?;
        let messages = match self.selected_contact.as_deref() {
            Some(user_id) => self
                .store
                .messages(&conversation_id(&self.identity.public.user_id, user_id))?,
            None => Vec::new(),
        };
        Ok(Snapshot {
            identity: self.identity.public.clone(),
            onboarding_required: profile.display_name.is_empty(),
            profile,
            chats,
            selected_contact_id: self.selected_contact.clone(),
            messages,
            settings: self.store.settings()?,
            online: self.online,
            status_message: self.status.clone(),
            search_query: self.search_query.clone(),
            search_results: self.search_results()?,
        })
    }

    /// Глобальный поиск Telegram: сообщения из всех диалогов сразу.
    fn search_results(&self) -> Result<Vec<SearchHit>, CoreError> {
        if self.search_query.chars().count() < 2 {
            return Ok(Vec::new());
        }
        let contacts = self.store.contacts()?;
        let mut hits = Vec::new();
        for message in self.store.search_messages(&self.search_query, 60)? {
            let contact = contacts.iter().find(|contact| {
                conversation_id(&self.identity.public.user_id, &contact.user_id)
                    == message.conversation_id
            });
            let Some(contact) = contact else { continue };
            hits.push(SearchHit {
                event_id: message.event_id,
                user_id: contact.user_id.clone(),
                display_name: contact.display_name.clone(),
                avatar_base64: contact.avatar_base64.clone(),
                text: message.text,
                created_at_unix_milliseconds: message.created_at_unix_milliseconds,
                outgoing: message.outgoing,
            });
        }
        Ok(hits)
    }

    /// Список чатов в порядке последней активности — так его рисует клиент.
    fn chats(&self) -> Result<Vec<Chat>, CoreError> {
        let mut chats = Vec::new();
        for contact in self.store.contacts()? {
            let conversation = conversation_id(&self.identity.public.user_id, &contact.user_id);
            let (last, unread_count) = self.store.conversation_preview(&conversation)?;
            chats.push(match last {
                Some(message) => Chat {
                    preview: preview_of(&message),
                    last_activity_unix_milliseconds: message.created_at_unix_milliseconds,
                    has_last_message: true,
                    last_message_outgoing: message.outgoing,
                    last_message_delivered: message.delivered,
                    last_message_read: message.read,
                    unread_count,
                    contact,
                },
                None => Chat {
                    preview: String::new(),
                    last_activity_unix_milliseconds: contact.added_at_unix_milliseconds,
                    has_last_message: false,
                    last_message_outgoing: false,
                    last_message_delivered: false,
                    last_message_read: false,
                    unread_count: 0,
                    contact,
                },
            });
        }
        chats.sort_by(|left, right| {
            right
                .contact
                .pinned
                .cmp(&left.contact.pinned)
                .then_with(|| {
                    right
                        .last_activity_unix_milliseconds
                        .cmp(&left.last_activity_unix_milliseconds)
                })
        });
        Ok(chats)
    }

    fn error_snapshot(&self, error: String) -> Snapshot {
        Snapshot {
            identity: self.identity.public.clone(),
            profile: Profile::default(),
            chats: Vec::new(),
            selected_contact_id: None,
            messages: Vec::new(),
            settings: Default::default(),
            online: false,
            status_message: error,
            onboarding_required: true,
            search_query: String::new(),
            search_results: Vec::new(),
        }
    }
}

/// Однострочное превью последнего события для списка чатов.
fn preview_of(message: &Message) -> String {
    let text = if message.deleted {
        "Сообщение удалено".to_owned()
    } else {
        match &message.attachment {
            Some(attachment) if message.text.trim().is_empty() => {
                format!("\u{1f4ce} {}", attachment.file_name)
            }
            Some(attachment) => format!("\u{1f4ce} {} · {}", attachment.file_name, message.text),
            None => message.text.clone(),
        }
    };
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(160)
        .collect()
}

fn random_hex(bytes: usize) -> String {
    let mut value = vec![0u8; bytes];
    OsRng.fill_bytes(&mut value);
    hex::encode(value)
}

fn short_id(value: &str) -> String {
    if value.len() <= 18 {
        value.to_owned()
    } else {
        format!("{}…{}", &value[..10], &value[value.len() - 6..])
    }
}

fn ensure_passphrase(value: &str) -> Result<(), CoreError> {
    if value.chars().count() < 8 {
        Err(CoreError::InvalidInput(
            "Пароль должен содержать минимум 8 символов".to_owned(),
        ))
    } else {
        Ok(())
    }
}

fn write_secret_file(
    path: &str,
    magic: &[u8],
    passphrase: &str,
    plaintext: &[u8],
) -> Result<(), CoreError> {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), &salt, &mut key)
        .map_err(|e| CoreError::Crypto(e.to_string()))?;
    let cipher = Aes256Gcm::new_from_slice(&key).expect("32-byte key");
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| CoreError::Crypto("Не удалось зашифровать пакет".to_owned()))?;
    let mut result = magic.to_vec();
    result.extend_from_slice(&salt);
    result.extend_from_slice(&nonce);
    result.extend(ciphertext);
    key.fill(0);
    fs::write(path, result)?;
    Ok(())
}

fn read_secret_file(path: &str, magic: &[u8], passphrase: &str) -> Result<Vec<u8>, CoreError> {
    let value = fs::read(path)?;
    let header = magic.len();
    if value.len() < header + 44 || &value[..header] != magic {
        return Err(CoreError::InvalidInput("Неверный формат пакета".to_owned()));
    }
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(passphrase.as_bytes(), &value[header..header + 16], &mut key)
        .map_err(|e| CoreError::Crypto(e.to_string()))?;
    let cipher = Aes256Gcm::new_from_slice(&key).expect("32-byte key");
    let result = cipher
        .decrypt(
            Nonce::from_slice(&value[header + 16..header + 28]),
            &value[header + 28..],
        )
        .map_err(|_| CoreError::Crypto("Неверный пароль или повреждён пакет".to_owned()));
    key.fill(0);
    result
}
