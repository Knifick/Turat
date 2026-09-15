use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
};

use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use argon2::Argon2;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rand_core::{OsRng, RngCore};
use serde_json::json;

mod delivery;

use crate::{
    CoreError,
    blobs::{self, AttachmentManifest},
    identity::{StoredIdentity, conversation_id},
    media::{self, Progress},
    models::{
        Attachment, Chat, Command, Contact, MediaKind, Message, MetadataProtection, Profile,
        Response, SearchHit, Snapshot,
    },
    network::{MailboxWatch, Network, expected_node_for},
    protocol::{
        AttachmentPayload, EditPayload, KIND_ATTACHMENT, KIND_DELETE, KIND_EDIT, KIND_REACTION,
        KIND_RECEIPT_DELIVERY, KIND_RECEIPT_READ, KIND_TEXT, PROTOCOL_VERSION, ReactionPayload,
        TargetPayload,
        TextPayload,
    },
    store::Store,
};

/// Размер в понятном человеку виде: сообщение об отказе должно называть предел так,
/// как его назвал бы сам пользователь, а не в байтах.
fn format_size(bytes: u64) -> String {
    const MEGABYTE: u64 = 1024 * 1024;
    if bytes >= 1024 * MEGABYTE {
        format!("{:.1} ГБ", bytes as f64 / (1024 * MEGABYTE) as f64)
    } else {
        format!("{} МБ", bytes / MEGABYTE)
    }
}

/// Фото и видео клиент сжимает перед отправкой, поэтому исходник может быть крупным:
/// до Node доедет уже сжатое. Настоящий предел всё равно не выше того, что Node объявил
/// в дескрипторе, — хранилище у него конечно.
const MAXIMUM_MEDIA_BYTES: u64 = 512 * 1024 * 1024;
/// Файл уходит как есть, байт в байт, поэтому предел на него заметно строже.
const MAXIMUM_FILE_BYTES: u64 = 100 * 1024 * 1024;

const JOB_RUNNING: u8 = 0;
const JOB_DONE: u8 = 1;
const JOB_FAILED: u8 = 2;

/// Фоновая передача вложения: шифрование при отправке или расшифровка при сохранении.
/// Интерфейс опрашивает её командой `media_job` и рисует понятный прогресс вместо
/// замершего окна.
pub struct MediaJob {
    pub progress: Arc<Progress>,
    state: Arc<AtomicU8>,
    error: Arc<Mutex<String>>,
    /// Заготовка сообщения: заполнена только у задач отправки.
    pending: Option<PendingAttachment>,
    /// Идентификатор вложения у фоновых загрузок входящих файлов.
    attachment_id: Option<String>,
}

/// Всё, что нужно, чтобы после завершения шифрования создать сообщение с вложением.
struct PendingAttachment {
    user_id: String,
    caption: String,
    reply_to_event_id: Option<String>,
    attachment: Attachment,
    /// Манифест выложенного файла: его заполняет фоновый поток, когда выгрузка закончена.
    manifest: Arc<Mutex<Option<AttachmentManifest>>>,
}

impl MediaJob {
    fn state(&self) -> u8 {
        self.state.load(Ordering::Relaxed)
    }

    fn error(&self) -> String {
        self.error.lock().map(|value| value.clone()).unwrap_or_default()
    }

    fn describe(&self, job_id: &str) -> serde_json::Value {
        let state = match self.state() {
            JOB_DONE => "done",
            JOB_FAILED => "failed",
            _ => "running",
        };
        json!({
            "jobId": job_id,
            "state": state,
            "done": self.progress.done.load(Ordering::Relaxed),
            "total": self.progress.total.load(Ordering::Relaxed),
            "error": self.error(),
        })
    }
}

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
    /// Активные фоновые передачи вложений по идентификатору задачи.
    media_jobs: HashMap<String, MediaJob>,
    /// Адрес собственного ящика для фонового ожидания конверта. Наблюдатель работает вне
    /// замка ядра, поэтому ожидание не мешает пользователю отправлять сообщения.
    watch: Arc<Mutex<Option<MailboxWatch>>>,
}

impl AppCore {
    pub fn open(app_dir: &Path, vault_key: [u8; 32]) -> Result<Self, CoreError> {
        let store = Store::open(app_dir, vault_key)?;
        let identity = store.load_or_create_identity()?;
        let selected_contact = store.selected_contact()?;
        // Ящик известен ещё до первой синхронизации: ожидание конверта начинается сразу
        // после запуска, а не после первого успешного цикла.
        let watch = store.mailbox()?.as_ref().map(MailboxWatch::of);
        Ok(Self {
            store,
            identity,
            network: Network::new()?,
            selected_contact,
            online: false,
            status: "Локальное хранилище готово".to_owned(),
            search_query: String::new(),
            last_presence_poll_unix_milliseconds: 0,
            media_jobs: HashMap::new(),
            watch: Arc::new(Mutex::new(watch)),
        })
    }

    /// Общий с наблюдателем адрес ящика: пока он пуст, клиенту остаётся обычный опрос.
    pub fn watch_handle(&self) -> Arc<Mutex<Option<MailboxWatch>>> {
        Arc::clone(&self.watch)
    }

    pub(crate) fn publish_watch(&self, mailbox: &crate::mailbox::OwnedMailbox) {
        if let Ok(mut slot) = self.watch.lock() {
            *slot = Some(MailboxWatch::of(mailbox));
        }
    }

    pub fn invoke(&mut self, request: &str) -> String {
        let parsed = serde_json::from_str::<Command>(request).map_err(CoreError::from);
        // Опрос прогресса идёт несколько раз в секунду: гнать в ответ весь снимок с историей
        // и аватарами ради двух чисел незачем.
        let quiet = matches!(
            parsed,
            Ok(Command::MediaJob { .. }) | Ok(Command::AttachmentSource { .. })
        );
        let result = parsed.and_then(|command| self.execute(command));
        let response = match result {
            Ok(value) => Response {
                ok: true,
                snapshot: if quiet {
                    None
                } else {
                    Some(
                        self.snapshot()
                            .unwrap_or_else(|error| self.error_snapshot(error.to_string())),
                    )
                },
                value,
                error: None,
            },
            Err(error) => {
                if !quiet {
                    self.status = error.to_string();
                }
                Response {
                    ok: false,
                    snapshot: if quiet { None } else { self.snapshot().ok() },
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
                self.update_contact(&user_id, |c| c.pending_approval = false)?;
                // Принятие диалога подтверждается квитанциями на всё уже полученное.
                // Вместе с ними собеседник получает наш личный обратный адрес — до этого
                // момента он мог писать только короткий текст в публичный ящик.
                let conversation = conversation_id(&self.identity.public.user_id, &user_id);
                let incoming: Vec<String> = self
                    .store
                    .messages(&conversation)?
                    .into_iter()
                    .rev()
                    .filter(|message| !message.outgoing && !message.deleted)
                    .take(20)
                    .map(|message| message.event_id)
                    .collect();
                for target_event_id in incoming {
                    self.queue_event(
                        &user_id,
                        &format!("evt1-{}", random_hex(16)),
                        KIND_RECEIPT_DELIVERY,
                        &TargetPayload {
                            version: PROTOCOL_VERSION,
                            target_event_id,
                        },
                    )?;
                }
                self.deliver_now();
                self.status = "Диалог принят".to_owned();
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
                self.store.save_message(&message)?;
                if let Some(user_id) = self.peer_of(&message)? {
                    self.queue_event(
                        &user_id,
                        &format!("evt1-{}", random_hex(16)),
                        KIND_EDIT,
                        &EditPayload {
                            version: PROTOCOL_VERSION,
                            target_event_id: message.event_id.clone(),
                            text: message.text.clone(),
                        },
                    )?;
                }
                self.deliver_now();
                self.status = "Изменение отправлено".to_owned();
            }
            Command::DeleteMessages { event_ids } => {
                for event_id in event_ids {
                    let mut message = self.require_message(&event_id)?;
                    if message.outgoing && !message.deleted {
                        message.deleted = true;
                        message.text.clear();
                        message.attachment = None;
                        self.store.save_message(&message)?;
                        if let Some(user_id) = self.peer_of(&message)? {
                            self.queue_event(
                                &user_id,
                                &format!("evt1-{}", random_hex(16)),
                                KIND_DELETE,
                                &TargetPayload {
                                    version: PROTOCOL_VERSION,
                                    target_event_id: message.event_id.clone(),
                                },
                            )?;
                        }
                    }
                }
                self.deliver_now();
            }
            Command::SetMessagePinned { event_id, pinned } => {
                let mut message = self.require_message(&event_id)?;
                if message.deleted {
                    return Err(CoreError::InvalidInput(
                        "Удалённое сообщение нельзя закрепить".to_owned(),
                    ));
                }
                message.pinned = pinned;
                self.store.save_message(&message)?;
                self.status = if pinned {
                    "Сообщение закреплено".to_owned()
                } else {
                    "Сообщение откреплено".to_owned()
                };
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
                        let active = !message.reactions.contains(&reaction);
                        if active {
                            message.reactions.push(reaction.clone());
                        } else {
                            message.reactions.retain(|v| v != &reaction);
                        }
                        self.store.save_message(&message)?;
                        if let Some(user_id) = self.peer_of(&message)? {
                            self.queue_event(
                                &user_id,
                                &format!("evt1-{}", random_hex(16)),
                                KIND_REACTION,
                                &ReactionPayload {
                                    version: PROTOCOL_VERSION,
                                    target_event_id: message.event_id.clone(),
                                    reaction: reaction.clone(),
                                    active,
                                },
                            )?;
                        }
                    }
                }
                self.deliver_now();
            }
            Command::MarkRead { user_id } => {
                let conversation = conversation_id(&self.identity.public.user_id, &user_id);
                let accepted = self
                    .store
                    .contact(&user_id)?
                    .is_some_and(|contact| !contact.pending_approval);
                let mut receipts = Vec::new();
                for mut message in self.store.messages(&conversation)? {
                    if !message.outgoing && !message.read {
                        message.read = true;
                        self.store.save_message(&message)?;
                        receipts.push(message.event_id.clone());
                    }
                }
                // Квитанция о прочтении уходит только по принятому диалогу: неотвеченный
                // запрос не должен подтверждать отправителю, что его читают.
                if accepted {
                    for target in receipts {
                        self.queue_event(
                            &user_id,
                            &format!("evt1-{}", random_hex(16)),
                            KIND_RECEIPT_READ,
                            &TargetPayload {
                                version: PROTOCOL_VERSION,
                                target_event_id: target,
                            },
                        )?;
                    }
                    self.deliver_now();
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
                kind,
                width,
                height,
                duration_milliseconds,
                thumbnail_base64,
            } => {
                let (attachment, progress) = self.prepare_attachment(
                    &path,
                    &mime_type,
                    kind,
                    width,
                    height,
                    duration_milliseconds,
                    thumbnail_base64,
                )?;
                media::encrypt_file(
                    self.store.vault_key(),
                    Path::new(&path),
                    Path::new(&attachment.local_path),
                    &progress,
                )?;
                let manifest = self.try_upload_attachment(&attachment, &progress);
                self.send_text(
                    &user_id,
                    caption.as_deref().unwrap_or(""),
                    Some((attachment, manifest)),
                    reply_to_event_id,
                )?;
            }
            Command::StartAttachment {
                user_id,
                path,
                mime_type,
                caption,
                reply_to_event_id,
                kind,
                width,
                height,
                duration_milliseconds,
                thumbnail_base64,
            } => {
                // Контакт проверяется сразу: незачем шифровать 300 МБ, чтобы потом отказать.
                let contact = self
                    .store
                    .contact(&user_id)?
                    .ok_or_else(|| CoreError::InvalidInput("Контакт не найден".to_owned()))?;
                if contact.pending_approval {
                    return Err(CoreError::InvalidInput(
                        "Файлы доступны после принятия контакта".to_owned(),
                    ));
                }
                let (attachment, progress) = self.prepare_attachment(
                    &path,
                    &mime_type,
                    kind,
                    width,
                    height,
                    duration_milliseconds,
                    thumbnail_base64,
                )?;
                let source = PathBuf::from(&path);
                let target = PathBuf::from(&attachment.local_path);
                let key = *self.store.vault_key();
                // Адрес Node узнаём заранее, но офлайн это не приговор: файл зашифруется
                // локально, а выгрузка произойдёт при первой же связи.
                let node = self.require_node().ok();
                let slot: Arc<Mutex<Option<AttachmentManifest>>> = Arc::new(Mutex::new(None));
                let worker_slot = Arc::clone(&slot);
                let upload = attachment.clone();
                let job_id = self.spawn_media_job(
                    progress,
                    Some(PendingAttachment {
                        user_id,
                        caption: caption.unwrap_or_default(),
                        reply_to_event_id,
                        attachment,
                        manifest: slot,
                    }),
                    move |progress| {
                        media::encrypt_file(&key, &source, &target, progress)?;
                        let Some(node) = node else {
                            return Ok(());
                        };
                        let manifest = delivery::upload_attachment(
                            &Network::new()?,
                            &node,
                            &key,
                            &upload,
                            progress,
                        )?;
                        if let Ok(mut value) = worker_slot.lock() {
                            *value = Some(manifest);
                        }
                        Ok(())
                    },
                );
                self.status = "Файл готовится к отправке".to_owned();
                return Ok(Some(json!({ "jobId": job_id })));
            }
            Command::FinishAttachment { job_id } => {
                let job = self
                    .media_jobs
                    .remove(&job_id)
                    .ok_or_else(|| CoreError::InvalidInput("Передача не найдена".to_owned()))?;
                if job.state() == JOB_RUNNING {
                    self.media_jobs.insert(job_id, job);
                    return Err(CoreError::InvalidInput("Файл ещё готовится".to_owned()));
                }
                let failed = job.state() == JOB_FAILED;
                let error = job.error();
                let pending = job
                    .pending
                    .ok_or_else(|| CoreError::InvalidInput("Это не отправка файла".to_owned()))?;
                if failed {
                    fs::remove_file(&pending.attachment.local_path).ok();
                    return Err(CoreError::InvalidInput(error));
                }
                let manifest = pending.manifest.lock().ok().and_then(|value| value.clone());
                self.send_text(
                    &pending.user_id,
                    &pending.caption,
                    Some((pending.attachment, manifest)),
                    pending.reply_to_event_id,
                )?;
            }
            Command::StartExportAttachment {
                event_id,
                destination_path,
            } => {
                let attachment = self.require_attachment(&event_id)?;
                let source = PathBuf::from(attachment.local_path);
                let target = PathBuf::from(destination_path);
                let key = *self.store.vault_key();
                let progress = Arc::new(Progress::default());
                progress.total.store(attachment.size, Ordering::Relaxed);
                let job_id = self.spawn_media_job(progress, None, move |progress| {
                    media::decrypt_to_file(&key, &source, &target, progress).map(|_| ())
                });
                return Ok(Some(json!({ "jobId": job_id })));
            }
            Command::MediaJob { job_id } => {
                let job = self
                    .media_jobs
                    .get(&job_id)
                    .ok_or_else(|| CoreError::InvalidInput("Передача не найдена".to_owned()))?;
                let value = job.describe(&job_id);
                // Задача без заготовки сообщения (сохранение файла) больше никому не нужна.
                if job.state() != JOB_RUNNING && job.pending.is_none() {
                    self.media_jobs.remove(&job_id);
                }
                return Ok(Some(value));
            }
            Command::CancelMediaJob { job_id } => {
                if let Some(job) = self.media_jobs.remove(&job_id) {
                    job.progress.cancel();
                    if let Some(pending) = job.pending {
                        fs::remove_file(&pending.attachment.local_path).ok();
                    }
                    self.status = "Передача отменена".to_owned();
                }
            }
            Command::AttachmentSource { event_id } => {
                let attachment = self.require_attachment(&event_id)?;
                return Ok(Some(json!({
                    "path": attachment.local_path,
                    "size": attachment.size,
                    "mimeType": attachment.mime_type,
                    "fileName": attachment.file_name,
                    "width": attachment.width,
                    "height": attachment.height,
                    "durationMilliseconds": attachment.duration_milliseconds,
                })));
            }
            Command::ExportAttachment {
                event_id,
                destination_path,
            } => {
                let attachment = self.require_attachment(&event_id)?;
                media::decrypt_to_file(
                    self.store.vault_key(),
                    Path::new(&attachment.local_path),
                    Path::new(&destination_path),
                    &Progress::default(),
                )?;
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
        self.selected_contact = Some(user_id.clone());
        self.store.set_selected_contact(&self.selected_contact)?;
        // Адрес забираем сразу: лучше честно сказать, что человек ещё не заходил в сеть,
        // чем молча положить первое сообщение в очередь.
        self.status = match self.network.routing(&descriptor, &user_id) {
            Ok(Some(routing)) => {
                self.store.save_peer_routing(&routing)?;
                "Защищённый диалог добавлен".to_owned()
            }
            _ => "Диалог добавлен, но собеседник ещё ни разу не выходил в сеть".to_owned(),
        };
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
        attachment: Option<(Attachment, Option<AttachmentManifest>)>,
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
        let (attachment, manifest) = match attachment {
            Some((value, manifest)) => (Some(value), manifest),
            None => (None, None),
        };
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
            // «Прочитано» у своего сообщения ставит квитанция собеседника, а не мы сами.
            read: false,
            pinned: false,
            attachment,
            reply_to_event_id: reply_to_event_id.clone(),
            forwarded_from: None,
        };
        self.store.save_message(&message)?;
        match (manifest, message.attachment.clone()) {
            // Файл ещё не в хранилище: выгрузим его при первой связи и тогда же
            // соберём событие — сообщение в истории уже есть.
            (None, Some(attachment)) => {
                self.store.save_pending_upload(&crate::store::PendingUpload {
                    event_id: message.event_id.clone(),
                    user_id: user_id.to_owned(),
                    caption: message.text.clone(),
                    reply_to_event_id,
                    forwarded_from: None,
                    attachment,
                })?;
            }
            (Some(manifest), _) => {
                self.store
                    .save_event_manifest(&message.event_id, &manifest)?;
                self.queue_event(
                    user_id,
                    &message.event_id,
                    KIND_ATTACHMENT,
                    &AttachmentPayload {
                        version: PROTOCOL_VERSION,
                        caption: message.text.clone(),
                        manifest,
                        reply_to_event_id,
                        forwarded_from: None,
                    },
                )?;
            }
            (None, None) => self.queue_event(
                user_id,
                &message.event_id,
                KIND_TEXT,
                &TextPayload {
                    version: PROTOCOL_VERSION,
                    text: message.text.clone(),
                    reply_to_event_id,
                    forwarded_from: None,
                },
            )?,
        }
        if !contact.draft.is_empty() {
            self.update_contact(user_id, |value| value.draft.clear())?;
        }
        self.deliver_now();
        Ok(())
    }

    /// Попытка отправить прямо сейчас: пользователю незачем ждать очередной цикл
    /// синхронизации. Нет связи — событие останется в очереди и уйдёт позже.
    fn deliver_now(&mut self) {
        // Известно, что связи нет — не задерживаем интерфейс на таймауте запроса:
        // фоновая синхронизация всё равно попробует ещё раз через несколько секунд.
        if !self.online {
            self.status = "Сообщение в очереди: нет связи с Node".to_owned();
            return;
        }
        let Ok(settings) = self.store.settings() else {
            return;
        };
        let Ok(node) = self.network.descriptor(
            &settings.bootstrap_url,
            settings.expected_node_id.as_deref(),
        ) else {
            self.status = "Сообщение в очереди: нет связи с Node".to_owned();
            return;
        };
        if self.ensure_transport(&node).is_err() {
            self.status = "Сообщение в очереди: адрес ещё не опубликован".to_owned();
            return;
        }
        match self.flush_outbox(&node) {
            Ok(count) if count > 0 => self.status = "Отправлено".to_owned(),
            Ok(_) => {}
            Err(error) => self.status = format!("Сообщение в очереди: {error}"),
        }
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
            // Файл пересылается ссылкой на тот же объект в хранилище: выкладывать
            // его заново незачем, ключ и без того есть у обеих сторон.
            let manifest = match &source.attachment {
                Some(_) => self.store.event_manifest(event_id)?,
                None => None,
            };
            if source.attachment.is_some() && manifest.is_none() {
                return Err(CoreError::InvalidInput(
                    "Этот файл больше нельзя переслать: срок его хранения истёк".to_owned(),
                ));
            }
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
                read: false,
                pinned: false,
                attachment: source.attachment.clone(),
                reply_to_event_id: None,
                forwarded_from: Some(author.clone()),
            };
            self.store.save_message(&message)?;
            match manifest {
                Some(manifest) => {
                    self.store
                        .save_event_manifest(&message.event_id, &manifest)?;
                    self.queue_event(
                        user_id,
                        &message.event_id,
                        KIND_ATTACHMENT,
                        &AttachmentPayload {
                            version: PROTOCOL_VERSION,
                            caption: message.text.clone(),
                            manifest,
                            reply_to_event_id: None,
                            forwarded_from: Some(author),
                        },
                    )?;
                }
                None => self.queue_event(
                    user_id,
                    &message.event_id,
                    KIND_TEXT,
                    &TextPayload {
                        version: PROTOCOL_VERSION,
                        text: message.text.clone(),
                        reply_to_event_id: None,
                        forwarded_from: Some(author),
                    },
                )?,
            }
        }
        self.selected_contact = Some(user_id.to_owned());
        self.store.set_selected_contact(&self.selected_contact)?;
        self.deliver_now();
        self.status = "Сообщения пересланы".to_owned();
        Ok(())
    }

    /// Собеседник события: сообщения хранятся по диалогу, а адресуется отправка человеку.
    fn peer_of(&self, message: &Message) -> Result<Option<String>, CoreError> {
        Ok(self.store.contacts()?.into_iter().find_map(|contact| {
            (conversation_id(&self.identity.public.user_id, &contact.user_id)
                == message.conversation_id)
                .then_some(contact.user_id)
        }))
    }

    /// Выгрузка вложения «по возможности»: офлайн она просто откладывается.
    fn try_upload_attachment(
        &mut self,
        attachment: &Attachment,
        progress: &Progress,
    ) -> Option<AttachmentManifest> {
        let node = self.require_node().ok()?;
        self.ensure_transport(&node).ok()?;
        delivery::upload_attachment(
            &self.network,
            &node,
            self.store.vault_key(),
            attachment,
            progress,
        )
        .ok()
    }

    /// Проверенный дескриптор текущего Node.
    fn require_node(&mut self) -> Result<crate::network::NodeDescriptor, CoreError> {
        let settings = self.store.settings()?;
        self.network.descriptor(
            &settings.bootstrap_url,
            settings.expected_node_id.as_deref(),
        )
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

        // Порядок важен: сначала свой адрес и предключи, иначе писать нам будет некуда;
        // затем приём, и только потом отправка — ответ уходит уже с новым обратным адресом.
        self.ensure_transport(&descriptor)?;
        let download_failure = self.settle_downloads();
        self.start_pending_downloads();
        let received = self.fetch_inbox(&descriptor)?;
        self.flush_outbox(&descriptor)?;

        self.store.prune_seen_events()?;
        let pending = self.store.outbox_length()?;
        // Сорвавшаяся загрузка файла важнее бодрого «синхронизировано»: иначе
        // пользователь видит в переписке вложение, которое не открывается.
        if let Some(error) = download_failure {
            self.status = format!("Файл не скачан: {error}");
            return Ok(());
        }
        self.status = match (received, pending) {
            (0, 0) => format!("Синхронизировано с {}", descriptor.name),
            (0, pending) => format!("{}: в очереди {} событий", descriptor.name, pending),
            (received, 0) => format!("Получено сообщений: {received}"),
            (received, pending) => {
                format!("Получено: {received}, в очереди: {pending}")
            }
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

    /// Предел на вложение: что разрешает Node, но не выше потолка для этого вида вложения.
    /// Берётся из уже проверенного дескриптора, без обращения к сети, — отказ должен
    /// прийти до того, как пользователь дождётся шифрования сотен мегабайт.
    fn attachment_limit(&self, kind: MediaKind) -> u64 {
        let own = match kind {
            MediaKind::File => MAXIMUM_FILE_BYTES,
            _ => MAXIMUM_MEDIA_BYTES,
        };
        let node = self
            .store
            .settings()
            .ok()
            .and_then(|settings| self.network.known_descriptor(&settings.bootstrap_url));
        match node {
            Some(descriptor) if descriptor.max_blob_bytes > 0 => {
                (descriptor.max_blob_bytes as u64).min(own)
            }
            _ => own,
        }
    }

    /// Готовит запись вложения и целевой файл до начала шифрования: клиенту нужно знать
    /// имя, размер и превью сразу, ещё до того как большой файл будет обработан.
    #[allow(clippy::too_many_arguments)]
    fn prepare_attachment(
        &self,
        path: &str,
        mime_type: &str,
        kind: Option<MediaKind>,
        width: u32,
        height: u32,
        duration_milliseconds: i64,
        thumbnail_base64: Option<String>,
    ) -> Result<(Attachment, Arc<Progress>), CoreError> {
        let size = fs::metadata(path)?.len();
        if size == 0 {
            return Err(CoreError::InvalidInput("Файл пуст".to_owned()));
        }
        let resolved = kind.unwrap_or_else(|| MediaKind::from_mime(mime_type));
        let limit = self.attachment_limit(resolved);
        // Сверяем размер шифротекста, а не файла: предел Node задан для того, что он хранит.
        if blobs::ciphertext_size(size) > limit {
            return Err(CoreError::InvalidInput(match resolved {
                MediaKind::File => format!("Файл больше {}", format_size(limit)),
                _ => format!("Медиа больше {}", format_size(limit)),
            }));
        }
        let attachment_id = format!("att1-{}", random_hex(16));
        let directory = self.store.app_dir.join("local-first").join("attachments");
        fs::create_dir_all(&directory)?;
        let target = directory.join(format!("{attachment_id}.bin"));
        let attachment = Attachment {
            attachment_id,
            file_name: Path::new(path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("attachment")
                .to_owned(),
            mime_type: mime_type.to_owned(),
            size,
            local_path: target.to_string_lossy().into_owned(),
            kind: kind.unwrap_or_else(|| MediaKind::from_mime(mime_type)),
            width,
            height,
            duration_milliseconds,
            thumbnail_base64,
        };
        let progress = Arc::new(Progress::default());
        progress.total.store(size, Ordering::Relaxed);
        Ok((attachment, progress))
    }

    /// Запускает работу с файлом в отдельном потоке: ядро под мьютексом остаётся свободным,
    /// поэтому интерфейс продолжает отвечать и опрашивать прогресс.
    fn spawn_media_job(
        &mut self,
        progress: Arc<Progress>,
        pending: Option<PendingAttachment>,
        work: impl FnOnce(&Progress) -> Result<(), CoreError> + Send + 'static,
    ) -> String {
        let job_id = format!("job1-{}", random_hex(8));
        let state = Arc::new(AtomicU8::new(JOB_RUNNING));
        let error = Arc::new(Mutex::new(String::new()));
        let worker_progress = Arc::clone(&progress);
        let worker_state = Arc::clone(&state);
        let worker_error = Arc::clone(&error);
        std::thread::spawn(move || {
            let outcome = work(&worker_progress);
            match outcome {
                Ok(()) => worker_state.store(JOB_DONE, Ordering::Relaxed),
                Err(failure) => {
                    if let Ok(mut slot) = worker_error.lock() {
                        *slot = failure.to_string();
                    }
                    worker_state.store(JOB_FAILED, Ordering::Relaxed);
                }
            }
        });
        self.media_jobs.insert(
            job_id.clone(),
            MediaJob {
                progress,
                state,
                error,
                pending,
                attachment_id: None,
            },
        );
        job_id
    }

    /// Фоновая загрузка входящего вложения: та же машинерия, что и у отправки,
    /// но результат никуда не превращается — файл просто появляется на диске.
    fn spawn_download_job(
        &mut self,
        progress: Arc<Progress>,
        attachment_id: String,
        work: impl FnOnce(&Progress) -> Result<(), CoreError> + Send + 'static,
    ) {
        let job_id = self.spawn_media_job(progress, None, work);
        if let Some(job) = self.media_jobs.get_mut(&job_id) {
            job.attachment_id = Some(attachment_id);
        }
    }

    fn require_attachment(&self, event_id: &str) -> Result<Attachment, CoreError> {
        self.require_message(event_id)?
            .attachment
            .ok_or_else(|| CoreError::InvalidInput("В сообщении нет файла".to_owned()))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn open_core(root: &Path) -> AppCore {
        let core = AppCore::open(root, [13u8; 32]).expect("ядро открывается");
        // Тесты работают только с локальным хранилищем: адрес Node заведомо мёртвый,
        // чтобы ни один прогон не постучался в настоящую сеть.
        core.store
            .save_settings(&crate::models::Settings {
                bootstrap_url: "http://127.0.0.1:9".to_owned(),
                expected_node_id: None,
                ..Default::default()
            })
            .expect("настройки сохраняются");
        // Сеть в тестах недоступна, поэтому контакт кладётся в хранилище напрямую.
        core.store
            .save_contact(&Contact {
                user_id: "tt1-0123456789abcdef0123456789abcdef".to_owned(),
                display_name: "Тест".to_owned(),
                username: None,
                about: None,
                avatar_base64: None,
                added_at_unix_milliseconds: 1,
                fingerprint_verified: false,
                pending_approval: false,
                last_seen_unix_milliseconds: None,
                pinned: false,
                muted: false,
                draft: String::new(),
                manual_unread: false,
            })
            .expect("контакт сохраняется");
        core
    }

    /// Отправка большого вложения идёт фоновой задачей: команда возвращает `jobId`
    /// мгновенно, прогресс опрашивается отдельно, и только потом появляется сообщение.
    #[test]
    fn attachment_job_reports_progress_and_creates_message() {
        let root = std::env::temp_dir().join(format!("turat-attach-{}", uuid::Uuid::new_v4()));
        let mut core = open_core(&root);
        let contact_id = "tt1-0123456789abcdef0123456789abcdef";

        let source = root.join("clip.mp4");
        fs::write(&source, vec![42u8; 700 * 1024]).expect("исходный файл");
        let request = json!({
            "command": "start_attachment",
            "user_id": contact_id,
            "path": source.to_string_lossy(),
            "mime_type": "video/mp4",
            "caption": "Проверка",
            "kind": "video",
            "width": 1280,
            "height": 720,
            "duration_milliseconds": 4200,
            "thumbnail_base64": "AAAA",
        })
        .to_string();
        let started: serde_json::Value = serde_json::from_str(&core.invoke(&request)).unwrap();
        assert_eq!(started["ok"], true, "{started}");
        let job_id = started["value"]["jobId"].as_str().unwrap().to_owned();

        let mut state = String::new();
        for _ in 0..600 {
            let polled: serde_json::Value = serde_json::from_str(
                &core.invoke(&format!(r#"{{"command":"media_job","job_id":"{job_id}"}}"#)),
            )
            .unwrap();
            // Опрос прогресса нарочно не тащит за собой снимок всего состояния.
            assert!(polled["snapshot"].is_null());
            assert_eq!(polled["value"]["total"], 700 * 1024);
            state = polled["value"]["state"].as_str().unwrap().to_owned();
            if state != "running" {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(state, "done");

        let finished: serde_json::Value = serde_json::from_str(
            &core.invoke(&format!(r#"{{"command":"finish_attachment","job_id":"{job_id}"}}"#)),
        )
        .unwrap();
        assert_eq!(finished["ok"], true, "{finished}");

        let selected: serde_json::Value = serde_json::from_str(&core.invoke(&format!(
            r#"{{"command":"select_contact","user_id":"{contact_id}"}}"#
        )))
        .unwrap();
        let message = &selected["snapshot"]["messages"][0];
        assert_eq!(message["text"], "Проверка");
        assert_eq!(message["attachment"]["kind"], "video");
        assert_eq!(message["attachment"]["durationMilliseconds"], 4200);
        assert_eq!(message["attachment"]["size"], 700 * 1024);
        let event_id = message["eventId"].as_str().unwrap().to_owned();

        // Потоковый читатель отдаёт ровно те же байты, что были отправлены, без полной расшифровки.
        let source_info: serde_json::Value = serde_json::from_str(
            &core.invoke(&format!(r#"{{"command":"attachment_source","event_id":"{event_id}"}}"#)),
        )
        .unwrap();
        let stored = source_info["value"]["path"].as_str().unwrap().to_owned();
        let mut reader = media::MediaReader::open(&[13u8; 32], Path::new(&stored)).unwrap();
        assert_eq!(reader.length(), 700 * 1024);
        let mut window = [0u8; 32];
        assert_eq!(reader.read_at(600 * 1024, &mut window).unwrap(), 32);
        assert!(window.iter().all(|value| *value == 42));

        drop(reader);
        drop(core);
        fs::remove_dir_all(root).ok();
    }

    /// Отмена убирает задачу и не оставляет полуготовый файл в хранилище вложений.
    #[test]
    fn cancelled_attachment_leaves_nothing_behind() {
        let root = std::env::temp_dir().join(format!("turat-cancel-{}", uuid::Uuid::new_v4()));
        let mut core = open_core(&root);
        let source = root.join("big.bin");
        fs::write(&source, vec![7u8; 8 * 1024 * 1024]).expect("исходный файл");
        let started: serde_json::Value = serde_json::from_str(
            &core.invoke(
                &json!({
                    "command": "start_attachment",
                    "user_id": "tt1-0123456789abcdef0123456789abcdef",
                    "path": source.to_string_lossy(),
                    "mime_type": "application/octet-stream",
                    "caption": null,
                })
                .to_string(),
            ),
        )
        .unwrap();
        let job_id = started["value"]["jobId"].as_str().unwrap().to_owned();
        let cancelled: serde_json::Value = serde_json::from_str(
            &core.invoke(&format!(r#"{{"command":"cancel_media_job","job_id":"{job_id}"}}"#)),
        )
        .unwrap();
        assert_eq!(cancelled["ok"], true, "{cancelled}");
        let missing: serde_json::Value = serde_json::from_str(
            &core.invoke(&format!(r#"{{"command":"media_job","job_id":"{job_id}"}}"#)),
        )
        .unwrap();
        assert_eq!(missing["ok"], false);
        assert!(core.store.messages("любой").unwrap_or_default().is_empty());

        drop(core);
        fs::remove_dir_all(root).ok();
    }
}
