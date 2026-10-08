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

mod calls;
mod channels;
mod delivery;
mod groups;

use crate::{
    CoreError,
    blobs::{self, AttachmentManifest},
    identity::{StoredIdentity, conversation_id},
    media::{self, Progress},
    models::{
        Attachment, Chat, Command, Contact, GroupPermissions, GroupRecord, MediaKind, Message,
        MetadataProtection, Profile, ReactionMark, Response, SearchHit, Snapshot,
    },
    network::{MailboxWatch, Network, expected_node_for},
    protocol::{
        AttachmentPayload, EditPayload, KIND_ATTACHMENT, KIND_DELETE, KIND_EDIT, KIND_REACTION,
        KIND_RECEIPT_DELIVERY, KIND_RECEIPT_READ, KIND_TEXT, PROTOCOL_VERSION, ReactionPayload,
        TargetPayload, TextPayload, is_channel_id, is_group_id,
    },
    store::Store,
};

/// Реакции, которые понимают все клиенты. Чужую, пришедшую по сети, ядро просто не применит.
pub(super) const ALLOWED_REACTIONS: &[&str] = &["❤", "🔥", "👌", "😱", "😭", "🤨", "👍", "💔"];

/// Флаги строки списка чатов — одинаковые у личного диалога и группы.
struct ChatFlags {
    pinned: bool,
    muted: bool,
    draft: String,
    manual_unread: bool,
}

/// Реакция конкретного участника группы. Возвращает, стоит ли она теперь.
pub(super) fn set_reaction_mark(
    message: &mut Message,
    user_id: &str,
    reaction: &str,
    active: Option<bool>,
) -> bool {
    let present = message
        .reaction_marks
        .iter()
        .any(|mark| mark.user_id == user_id && mark.reaction == reaction);
    let active = active.unwrap_or(!present);
    message
        .reaction_marks
        .retain(|mark| !(mark.user_id == user_id && mark.reaction == reaction));
    if active {
        message.reaction_marks.push(ReactionMark {
            user_id: user_id.to_owned(),
            reaction: reaction.to_owned(),
        });
    }
    message.reactions.clear();
    for mark in &message.reaction_marks {
        if !message.reactions.contains(&mark.reaction) {
            message.reactions.push(mark.reaction.clone());
        }
    }
    active
}

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
    /// Открытая ветка комментариев поста выбранного канала.
    selected_thread: Option<String>,
    /// Текущий звонок. Свой замок: аудиопотоки и интерфейс читают его, не дожидаясь ядра.
    pub(crate) call_slot: crate::calls::CallSlot,
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
            selected_thread: None,
            call_slot: Default::default(),
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
        // Звонок мог закончиться, пока ядро было занято: сначала доделываем его сигналы.
        self.settle_calls();
        match command {
            Command::Snapshot => {}
            Command::SelectContact { user_id } => {
                if user_id != self.selected_contact {
                    self.selected_thread = None;
                }
                self.selected_contact = user_id;
                self.store.set_selected_contact(&self.selected_contact)?;
            }
            Command::AddContact {
                query,
                display_name,
            } => self.add_contact(&query, display_name)?,
            // Удаление чата группы — это выход из неё; отказ от приглашения — тоже.
            Command::DeleteContact { user_id } | Command::RejectContact { user_id }
                if is_group_id(&user_id) =>
            {
                self.leave_group(&user_id, true)?
            }
            Command::AcceptContact { user_id } if is_group_id(&user_id) => {
                self.accept_group_invite(&user_id)?
            }
            Command::MarkRead { user_id } if is_group_id(&user_id) => {
                self.mark_group_read(&user_id)?
            }
            // Удаление канала из списка — отписка; отказ от приглашения — тоже.
            Command::DeleteContact { user_id } | Command::RejectContact { user_id }
                if is_channel_id(&user_id) =>
            {
                self.leave_channel(&user_id, true)?
            }
            Command::AcceptContact { user_id } if is_channel_id(&user_id) => {
                self.accept_channel_invite(&user_id)?
            }
            Command::MarkRead { user_id } if is_channel_id(&user_id) => {
                self.mark_channel_read(&user_id)?
            }
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
                self.update_chat(&user_id, |flags| flags.pinned = pinned)?;
                self.status = if pinned {
                    "Чат закреплён".to_owned()
                } else {
                    "Чат откреплён".to_owned()
                };
            }
            Command::SetChatMuted { user_id, muted } => {
                self.update_chat(&user_id, |flags| flags.muted = muted)?;
                self.status = if muted {
                    "Уведомления выключены".to_owned()
                } else {
                    "Уведомления включены".to_owned()
                };
            }
            Command::SaveDraft { user_id, text } => {
                let draft = text.trim().to_owned();
                self.update_chat(&user_id, |flags| flags.draft = draft)?;
            }
            Command::ClearHistory { user_id } => {
                let conversation = self.chat_conversation(&user_id);
                self.store.clear_conversation(&conversation)?;
                if is_channel_id(&user_id) {
                    self.store
                        .clear_conversations_with_prefix(&format!("{user_id}/"))?;
                    self.selected_thread = None;
                }
                self.status = "История диалога очищена".to_owned();
            }
            Command::MarkUnread { user_id } => {
                self.update_chat(&user_id, |flags| flags.manual_unread = true)?;
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
                if is_channel_id(&message.conversation_id) {
                    self.edit_channel_post(message, &text)?;
                    return Ok(None);
                }
                if !message.outgoing || message.deleted || message.service {
                    return Err(CoreError::InvalidInput(
                        "Сообщение нельзя изменить".to_owned(),
                    ));
                }
                let chat = self.chat_of(&message)?;
                if let Some(group) = chat.as_deref().filter(|id| is_group_id(id)) {
                    self.ensure_group_writable(group)?;
                }
                message.text = text.trim().to_owned();
                message.edited = true;
                self.store.save_message(&message)?;
                if let Some(chat) = chat {
                    self.queue_for_chat(
                        &chat,
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
                let me = self.identity.public.user_id.clone();
                for event_id in event_ids {
                    let mut message = self.require_message(&event_id)?;
                    if channels::channel_of_conversation(&message.conversation_id).is_some() {
                        self.delete_channel_item(message)?;
                        continue;
                    }
                    if message.deleted || message.service {
                        continue;
                    }
                    let chat = self.chat_of(&message)?;
                    let in_group = chat.as_deref().is_some_and(is_group_id);
                    let writable = match chat.as_deref() {
                        Some(group) if in_group => self.ensure_group_writable(group).ok(),
                        _ => None,
                    };
                    // Чужое сообщение в группе удаляет только старший по роли — и у всех сразу.
                    let moderated = !message.outgoing
                        && writable.as_ref().is_some_and(|record| {
                            groups::may_moderate(&record.state, &me, &message.sender_user_id)
                        });
                    if !message.outgoing && !moderated {
                        continue;
                    }
                    message.deleted = true;
                    message.text.clear();
                    message.attachment = None;
                    message.reaction_marks.clear();
                    message.reactions.clear();
                    self.store.save_message(&message)?;
                    if let Some(chat) = chat
                        && (!in_group || writable.is_some())
                    {
                        self.queue_for_chat(
                            &chat,
                            &format!("evt1-{}", random_hex(16)),
                            KIND_DELETE,
                            &TargetPayload {
                                version: PROTOCOL_VERSION,
                                target_event_id: message.event_id.clone(),
                            },
                        )?;
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
                if !ALLOWED_REACTIONS.contains(&reaction.as_str()) {
                    return Err(CoreError::InvalidInput("Неизвестная реакция".to_owned()));
                }
                let me = self.identity.public.user_id.clone();
                for event_id in event_ids {
                    let mut message = self.require_message(&event_id)?;
                    if channels::channel_of_conversation(&message.conversation_id).is_some() {
                        self.react_channel_post(message, &reaction)?;
                        continue;
                    }
                    let chat = self.chat_of(&message)?;
                    let group = chat.as_deref().filter(|id| is_group_id(id));
                    if group.is_some_and(|group| self.ensure_group_writable(group).is_err()) {
                        continue;
                    }
                    if !message.deleted && !message.service {
                        let active = if group.is_some() {
                            set_reaction_mark(&mut message, &me, &reaction, None)
                        } else {
                            let active = !message.reactions.contains(&reaction);
                            if active {
                                message.reactions.push(reaction.clone());
                            } else {
                                message.reactions.retain(|v| v != &reaction);
                            }
                            active
                        };
                        self.store.save_message(&message)?;
                        if let Some(chat) = chat {
                            self.queue_for_chat(
                                &chat,
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
                // Получатель проверяется сразу: незачем шифровать 300 МБ, чтобы потом отказать.
                self.ensure_chat_writable(&user_id, true)?;
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
            Command::CreateGroup {
                name,
                about,
                avatar_base64,
                member_ids,
            } => {
                let group_id = self.create_group(&name, &about, avatar_base64, &member_ids)?;
                return Ok(Some(json!({ "groupId": group_id })));
            }
            Command::AddGroupMembers { group_id, user_ids } => {
                self.add_group_members(&group_id, &user_ids)?
            }
            Command::RemoveGroupMember { group_id, user_id } => {
                self.remove_group_member(&group_id, &user_id)?
            }
            Command::SetGroupRole {
                group_id,
                user_id,
                role,
            } => self.set_group_role(&group_id, &user_id, role)?,
            Command::TransferGroupOwnership { group_id, user_id } => {
                self.transfer_group_ownership(&group_id, &user_id)?
            }
            Command::UpdateGroupInfo {
                group_id,
                name,
                about,
                avatar_base64,
            } => self.update_group_info(&group_id, &name, &about, avatar_base64)?,
            Command::SetGroupPermissions {
                group_id,
                members_can_invite,
                members_can_edit_info,
            } => self.set_group_permissions(
                &group_id,
                GroupPermissions {
                    members_can_invite,
                    members_can_edit_info,
                },
            )?,
            Command::LeaveGroup { group_id } => self.leave_group(&group_id, false)?,
            Command::CreateChannel {
                name,
                about,
                avatar_base64,
            } => {
                let channel_id = self.create_channel(&name, &about, avatar_base64)?;
                return Ok(Some(json!({ "channelId": channel_id })));
            }
            Command::SubscribeChannel { link } => {
                let channel_id = self.subscribe_channel(&link)?;
                return Ok(Some(json!({ "channelId": channel_id })));
            }
            Command::UpdateChannelInfo {
                channel_id,
                name,
                about,
                avatar_base64,
            } => self.update_channel_info(&channel_id, &name, &about, avatar_base64)?,
            Command::SetChannelSettings {
                channel_id,
                sign_posts,
                comments_enabled,
            } => self.set_channel_settings(&channel_id, sign_posts, comments_enabled)?,
            Command::LinkDiscussionGroup {
                channel_id,
                group_id,
            } => self.link_discussion_group(&channel_id, group_id)?,
            Command::InviteToChannel {
                channel_id,
                user_ids,
            } => self.invite_to_channel(&channel_id, &user_ids)?,
            Command::SetChannelAdmin {
                channel_id,
                user_id,
                rights,
                title,
            } => self.set_channel_admin(&channel_id, &user_id, rights, &title)?,
            Command::RemoveChannelAdmin {
                channel_id,
                user_id,
            } => self.remove_channel_admin(&channel_id, &user_id)?,
            Command::TransferChannelOwnership {
                channel_id,
                user_id,
            } => self.transfer_channel_ownership(&channel_id, &user_id)?,
            Command::RemoveChannelSubscriber {
                channel_id,
                user_id,
                ban,
            } => self.remove_channel_subscriber(&channel_id, &user_id, ban)?,
            Command::UnbanChannelSubscriber {
                channel_id,
                user_id,
            } => self.unban_channel_subscriber(&channel_id, &user_id)?,
            Command::LeaveChannel { channel_id } => self.leave_channel(&channel_id, false)?,
            Command::CloseChannel { channel_id } => self.close_channel(&channel_id)?,
            Command::OpenComments { post_event_id } => self.open_comments(post_event_id)?,
            Command::SendComment {
                post_event_id,
                text,
                reply_to_event_id,
            } => self.send_comment(&post_event_id, &text, reply_to_event_id)?,
            Command::StartCall { user_id } => {
                let call_id = self.start_call(&user_id)?;
                return Ok(Some(json!({ "callId": call_id })));
            }
            Command::AcceptCall => self.accept_call()?,
            Command::SettleCalls => {}
        }
        Ok(None)
    }

    /// Диалог, в котором хранятся сообщения чата: у группы это сам GroupID.
    fn chat_conversation(&self, chat_id: &str) -> String {
        if is_group_id(chat_id) || is_channel_id(chat_id) {
            chat_id.to_owned()
        } else {
            conversation_id(&self.identity.public.user_id, chat_id)
        }
    }

    fn update_chat(
        &mut self,
        chat_id: &str,
        update: impl FnOnce(&mut ChatFlags),
    ) -> Result<(), CoreError> {
        if is_channel_id(chat_id) {
            let mut record = self.channel_record(chat_id)?;
            let mut flags = ChatFlags {
                pinned: record.pinned,
                muted: record.muted,
                draft: std::mem::take(&mut record.draft),
                manual_unread: record.manual_unread,
            };
            update(&mut flags);
            record.pinned = flags.pinned;
            record.muted = flags.muted;
            record.draft = flags.draft;
            record.manual_unread = flags.manual_unread;
            return self.store.save_channel(&record);
        }
        if is_group_id(chat_id) {
            let mut record = self.group_record(chat_id)?;
            let mut flags = ChatFlags {
                pinned: record.pinned,
                muted: record.muted,
                draft: std::mem::take(&mut record.draft),
                manual_unread: record.manual_unread,
            };
            update(&mut flags);
            record.pinned = flags.pinned;
            record.muted = flags.muted;
            record.draft = flags.draft;
            record.manual_unread = flags.manual_unread;
            return self.store.save_group(&record);
        }
        self.update_contact(chat_id, |contact| {
            let mut flags = ChatFlags {
                pinned: contact.pinned,
                muted: contact.muted,
                draft: std::mem::take(&mut contact.draft),
                manual_unread: contact.manual_unread,
            };
            update(&mut flags);
            contact.pinned = flags.pinned;
            contact.muted = flags.muted;
            contact.draft = flags.draft;
            contact.manual_unread = flags.manual_unread;
        })
    }

    /// Можно ли сейчас писать в чат. В непринятый личный диалог — только текст.
    fn ensure_chat_writable(&self, chat_id: &str, attachment: bool) -> Result<(), CoreError> {
        if is_channel_id(chat_id) {
            return self.ensure_channel_postable(chat_id).map(|_| ());
        }
        if is_group_id(chat_id) {
            return self.ensure_group_writable(chat_id).map(|_| ());
        }
        let contact = self
            .store
            .contact(chat_id)?
            .ok_or_else(|| CoreError::InvalidInput("Контакт не найден".to_owned()))?;
        if attachment && contact.pending_approval {
            return Err(CoreError::InvalidInput(
                "Файлы доступны после принятия контакта".to_owned(),
            ));
        }
        Ok(())
    }

    fn chat_draft(&self, chat_id: &str) -> Result<String, CoreError> {
        if is_channel_id(chat_id) {
            return Ok(self.channel_record(chat_id)?.draft);
        }
        if is_group_id(chat_id) {
            return Ok(self.group_record(chat_id)?.draft);
        }
        Ok(self
            .store
            .contact(chat_id)?
            .map(|contact| contact.draft)
            .unwrap_or_default())
    }

    /// Событие чата: собеседнику лично или всем участникам группы.
    pub(super) fn queue_for_chat<T: serde::Serialize>(
        &mut self,
        chat_id: &str,
        event_id: &str,
        kind: &str,
        payload: &T,
    ) -> Result<(), CoreError> {
        if is_channel_id(chat_id) {
            return self.queue_channel_post(chat_id, event_id, kind, payload);
        }
        if is_group_id(chat_id) {
            let record = self.ensure_group_writable(chat_id)?;
            let recipients = self.group_recipients(&record.state);
            return self.queue_group_event(chat_id, &recipients, event_id, kind, payload);
        }
        self.queue_event(chat_id, event_id, kind, payload)
    }

    /// Имя автора сообщения для подписи «переслано от» и ленты группы.
    fn sender_name_of(&self, message: &Message) -> Result<String, CoreError> {
        // Пост канала пересылается от имени канала, комментарий — от имени автора.
        if is_channel_id(&message.conversation_id) {
            return Ok(self
                .store
                .channel(&message.conversation_id)?
                .map(|record| record.state.name)
                .unwrap_or_else(|| "Канал".to_owned()));
        }
        if channels::split_thread(&message.conversation_id).is_some() {
            return Ok(message
                .sender_name
                .clone()
                .unwrap_or_else(|| short_id(&message.sender_user_id)));
        }
        if is_group_id(&message.conversation_id) {
            let record = self.store.group(&message.conversation_id)?;
            return Ok(self.member_display_name(
                record.as_ref().map(|record| &record.state),
                &message.sender_user_id,
            ));
        }
        Ok(self
            .store
            .contact(&message.sender_user_id)?
            .map(|value| value.display_name)
            .unwrap_or_else(|| short_id(&message.sender_user_id)))
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
        self.ensure_chat_writable(user_id, attachment.is_some())?;
        let draft = self.chat_draft(user_id)?;
        let (attachment, manifest) = match attachment {
            Some((value, manifest)) => (Some(value), manifest),
            None => (None, None),
        };
        let message = Message {
            event_id: format!("evt1-{}", random_hex(16)),
            conversation_id: self.chat_conversation(user_id),
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
            service: false,
            reaction_marks: Vec::new(),
            sender_name: None,
            channel_post: None,
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
                self.queue_for_chat(
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
            (None, None) => self.queue_for_chat(
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
        if !draft.is_empty() {
            self.update_chat(user_id, |flags| flags.draft.clear())?;
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
        // Одна пачка: в большую группу остальное дошлёт фоновая синхронизация, а
        // интерфейс не ждёт, пока сообщение зашифруется под сотню участников.
        match self.flush_outbox_limited(&node, 50) {
            Ok(count) if count > 0 => self.status = "Отправлено".to_owned(),
            Ok(_) => {}
            Err(error) => self.status = format!("Сообщение в очереди: {error}"),
        }
    }

    /// Пересылка сообщений в другой диалог с сохранением автора оригинала.
    fn forward_messages(&mut self, event_ids: &[String], user_id: &str) -> Result<(), CoreError> {
        self.forward_into(event_ids, user_id)?;
        self.selected_contact = Some(user_id.to_owned());
        self.selected_thread = None;
        self.store.set_selected_contact(&self.selected_contact)?;
        self.deliver_now();
        self.status = "Сообщения пересланы".to_owned();
        Ok(())
    }

    /// Пересылка без смены открытого чата: ей же пост канала уходит в группу обсуждения.
    fn forward_into(&mut self, event_ids: &[String], user_id: &str) -> Result<(), CoreError> {
        if is_channel_id(user_id) {
            self.ensure_channel_postable(user_id)?;
        } else if is_group_id(user_id) {
            self.ensure_group_writable(user_id)?;
        } else {
            let target = self
                .store
                .contact(user_id)?
                .ok_or_else(|| CoreError::InvalidInput("Контакт не найден".to_owned()))?;
            if target.pending_approval {
                return Err(CoreError::InvalidInput(
                    "Переслать можно только в принятый диалог".to_owned(),
                ));
            }
        }
        let own_name = self.store.profile()?.display_name;
        let conversation = self.chat_conversation(user_id);
        for event_id in event_ids {
            let source = self.require_message(event_id)?;
            if source.deleted || source.service {
                continue;
            }
            let author = match source.forwarded_from.clone() {
                Some(value) => value,
                None if is_channel_id(&source.conversation_id) => self.sender_name_of(&source)?,
                None if source.outgoing => own_name.clone(),
                None => self.sender_name_of(&source)?,
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
                service: false,
                reaction_marks: Vec::new(),
                sender_name: None,
                channel_post: None,
            };
            self.store.save_message(&message)?;
            match manifest {
                Some(manifest) => {
                    self.store
                        .save_event_manifest(&message.event_id, &manifest)?;
                    self.queue_for_chat(
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
                None => self.queue_for_chat(
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
        Ok(())
    }

    /// Чат события: сообщения хранятся по диалогу, а адресуется отправка человеку или группе.
    fn chat_of(&self, message: &Message) -> Result<Option<String>, CoreError> {
        if is_channel_id(&message.conversation_id) {
            return Ok(self
                .store
                .channel(&message.conversation_id)?
                .filter(|record| !record.hidden)
                .map(|record| record.state.channel_id));
        }
        if is_group_id(&message.conversation_id) {
            return Ok(self
                .store
                .group(&message.conversation_id)?
                .filter(|record| !record.hidden)
                .map(|record| record.state.group_id));
        }
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
        self.settle_calls();
        if let Err(error) = self.broadcast_channel_stats() {
            self.status = format!("Счётчики канала не разосланы: {error}");
        }
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
        let mut group = None;
        let mut channel = None;
        let mut comments = Vec::new();
        let messages = match self.selected_contact.as_deref() {
            Some(channel_id) if is_channel_id(channel_id) => {
                match self.store.channel(channel_id)?.filter(|record| !record.hidden) {
                    Some(record) => {
                        let view = self.channel_view(&record)?;
                        if let Some(post) = &view.thread_post_event_id {
                            comments = self.channel_comments(&record, post)?;
                        }
                        channel = Some(view);
                        self.channel_feed(&record)?
                    }
                    None => Vec::new(),
                }
            }
            Some(group_id) if is_group_id(group_id) => {
                match self.store.group(group_id)?.filter(|record| !record.hidden) {
                    Some(record) => {
                        let mut messages = self.store.messages(group_id)?;
                        let mut names: std::collections::HashMap<String, String> =
                            std::collections::HashMap::new();
                        for message in &mut messages {
                            if message.outgoing || message.service {
                                continue;
                            }
                            let name = names
                                .entry(message.sender_user_id.clone())
                                .or_insert_with(|| {
                                    self.member_display_name(
                                        Some(&record.state),
                                        &message.sender_user_id,
                                    )
                                })
                                .clone();
                            message.sender_name = Some(name);
                        }
                        group = Some(self.group_view(&record)?);
                        messages
                    }
                    None => Vec::new(),
                }
            }
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
            group,
            channel,
            comments,
        })
    }

    /// Глобальный поиск Telegram: сообщения из всех диалогов сразу.
    fn search_results(&self) -> Result<Vec<SearchHit>, CoreError> {
        if self.search_query.chars().count() < 2 {
            return Ok(Vec::new());
        }
        let contacts = self.store.contacts()?;
        let groups: std::collections::HashMap<String, GroupRecord> = self
            .store
            .groups()?
            .into_iter()
            .filter(|record| !record.hidden)
            .map(|record| (record.state.group_id.clone(), record))
            .collect();
        let channels: std::collections::HashMap<String, crate::models::ChannelRecord> = self
            .store
            .channels()?
            .into_iter()
            .filter(|record| !record.hidden)
            .map(|record| (record.state.channel_id.clone(), record))
            .collect();
        let mut hits = Vec::new();
        for message in self.store.search_messages(&self.search_query, 60)? {
            if message.service {
                continue;
            }
            if let Some(channel_id) = channels::channel_of_conversation(&message.conversation_id) {
                if let Some(record) = channels.get(channel_id) {
                    hits.push(SearchHit {
                        event_id: message.event_id,
                        user_id: record.state.channel_id.clone(),
                        display_name: record.state.name.clone(),
                        avatar_base64: record.state.avatar_base64.clone(),
                        text: message.text,
                        created_at_unix_milliseconds: message.created_at_unix_milliseconds,
                        outgoing: message.outgoing,
                    });
                }
                continue;
            }
            if is_group_id(&message.conversation_id) {
                if let Some(record) = groups.get(&message.conversation_id) {
                    hits.push(SearchHit {
                        event_id: message.event_id,
                        user_id: record.state.group_id.clone(),
                        display_name: record.state.name.clone(),
                        avatar_base64: record.state.avatar_base64.clone(),
                        text: message.text,
                        created_at_unix_milliseconds: message.created_at_unix_milliseconds,
                        outgoing: message.outgoing,
                    });
                }
                continue;
            }
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
                    is_group: false,
                    member_count: 0,
                    group_role: None,
                    group_left: false,
                    is_channel: false,
                    channel_role: None,
                    channel_can_post: false,
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
                    is_group: false,
                    member_count: 0,
                    group_role: None,
                    group_left: false,
                    is_channel: false,
                    channel_role: None,
                    channel_can_post: false,
                },
            });
        }
        let me = self.identity.public.user_id.clone();
        for record in self.store.groups()? {
            if record.hidden {
                continue;
            }
            let group_id = record.state.group_id.clone();
            let (last, unread_count) = self.store.conversation_preview(&group_id)?;
            let contact = Contact {
                user_id: group_id,
                display_name: record.state.name.clone(),
                username: None,
                about: (!record.state.about.is_empty()).then(|| record.state.about.clone()),
                avatar_base64: record.state.avatar_base64.clone(),
                added_at_unix_milliseconds: record.joined_at_unix_milliseconds,
                fingerprint_verified: false,
                pending_approval: record.pending_invite,
                last_seen_unix_milliseconds: None,
                pinned: record.pinned,
                muted: record.muted,
                draft: record.draft.clone(),
                manual_unread: record.manual_unread,
            };
            let group_role = if record.left {
                None
            } else {
                groups::role_of(&record.state, &me)
            };
            let member_count = record.state.members.len() as u32;
            chats.push(match last {
                Some(message) => {
                    // В группе превью подписано автором, как в Telegram.
                    let preview = if message.service || message.deleted {
                        preview_of(&message)
                    } else if message.outgoing {
                        format!("Вы: {}", preview_of(&message))
                    } else {
                        format!(
                            "{}: {}",
                            self.member_display_name(Some(&record.state), &message.sender_user_id),
                            preview_of(&message)
                        )
                    };
                    Chat {
                        preview,
                        last_activity_unix_milliseconds: message.created_at_unix_milliseconds,
                        has_last_message: true,
                        last_message_outgoing: message.outgoing && !message.service,
                        last_message_delivered: message.delivered,
                        last_message_read: message.read,
                        unread_count,
                        contact,
                        is_group: true,
                        member_count,
                        group_role,
                        group_left: record.left,
                        is_channel: false,
                        channel_role: None,
                        channel_can_post: false,
                    }
                }
                None => Chat {
                    preview: String::new(),
                    last_activity_unix_milliseconds: record.joined_at_unix_milliseconds,
                    has_last_message: false,
                    last_message_outgoing: false,
                    last_message_delivered: false,
                    last_message_read: false,
                    unread_count: 0,
                    contact,
                    is_group: true,
                    member_count,
                    group_role,
                    group_left: record.left,
                    is_channel: false,
                    channel_role: None,
                    channel_can_post: false,
                },
            });
        }
        for record in self.store.channels()? {
            if record.hidden {
                continue;
            }
            chats.push(self.channel_chat(&record)?);
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
            group: None,
            channel: None,
            comments: Vec::new(),
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

    fn member_id(index: u8) -> String {
        format!("tt1-{}", hex::encode([index; 32]))
    }

    fn add_accepted_contact(core: &AppCore, user_id: &str, name: &str) {
        core.store
            .save_contact(&Contact {
                user_id: user_id.to_owned(),
                display_name: name.to_owned(),
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
    }

    fn call(core: &mut AppCore, command: serde_json::Value) -> serde_json::Value {
        serde_json::from_str(&core.invoke(&command.to_string())).expect("ответ ядра")
    }

    /// Полный локальный цикл группы: создание, роли, рассылка каждому участнику, выход.
    #[test]
    fn group_lifecycle_fans_out_to_every_member() {
        let root = std::env::temp_dir().join(format!("turat-group-{}", uuid::Uuid::new_v4()));
        let mut core = open_core(&root);
        call(&mut core, json!({"command":"save_profile","username":"","display_name":"Я","about":"","avatar_base64":null}));
        let alice = member_id(2);
        let bob = member_id(3);
        add_accepted_contact(&core, &alice, "Алиса");
        add_accepted_contact(&core, &bob, "Боб");

        let created = call(&mut core, json!({
            "command": "create_group",
            "name": "Команда",
            "member_ids": [alice, bob, "tt1-0123456789abcdef0123456789abcdef"],
        }));
        // Третий id — контакт с неверным форматом: группа из него не соберётся.
        assert_eq!(created["ok"], false, "{created}");

        let created = call(&mut core, json!({
            "command": "create_group",
            "name": "Команда",
            "member_ids": [alice, bob],
        }));
        assert_eq!(created["ok"], true, "{created}");
        let group_id = created["value"]["groupId"].as_str().unwrap().to_owned();
        assert_eq!(created["snapshot"]["selectedContactId"], group_id);
        let chat = created["snapshot"]["chats"]
            .as_array()
            .unwrap()
            .iter()
            .find(|chat| chat["userId"] == group_id)
            .cloned()
            .expect("группа в списке чатов");
        assert_eq!(chat["isGroup"], true);
        assert_eq!(chat["memberCount"], 3);
        assert_eq!(created["snapshot"]["group"]["myRole"], "owner");
        assert_eq!(created["snapshot"]["group"]["canManageAdmins"], true);
        assert_eq!(created["snapshot"]["messages"][0]["service"], true);
        assert_eq!(core.store.outbox_length().unwrap(), 2, "состояние каждому участнику");

        let sent = call(&mut core, json!({"command":"send_text","user_id":group_id,"text":"Привет всем"}));
        assert_eq!(sent["ok"], true, "{sent}");
        assert_eq!(core.store.outbox_length().unwrap(), 4);
        assert_eq!(sent["snapshot"]["chats"][0]["preview"], "Вы: Привет всем");

        let promoted = call(&mut core, json!({"command":"set_group_role","group_id":group_id,"user_id":alice,"role":"admin"}));
        assert_eq!(promoted["ok"], true, "{promoted}");
        let members = promoted["snapshot"]["group"]["members"].as_array().unwrap().clone();
        assert_eq!(members[0]["role"], "owner");
        assert_eq!(members[1]["role"], "admin");
        assert_eq!(members[1]["displayName"], "Алиса");

        let owner_role = call(&mut core, json!({"command":"set_group_role","group_id":group_id,"user_id":bob,"role":"owner"}));
        assert_eq!(owner_role["ok"], false);

        let removed = call(&mut core, json!({"command":"remove_group_member","group_id":group_id,"user_id":bob}));
        assert_eq!(removed["ok"], true, "{removed}");
        // Исключённый тоже получает новое состояние — он должен узнать, что его исключили.
        assert_eq!(core.store.outbox_length().unwrap(), 8);
        assert_eq!(removed["snapshot"]["group"]["members"].as_array().unwrap().len(), 2);

        let left = call(&mut core, json!({"command":"leave_group","group_id":group_id}));
        assert_eq!(left["ok"], true, "{left}");
        assert_eq!(left["snapshot"]["group"]["left"], true);
        assert_eq!(left["snapshot"]["group"]["canSend"], false);
        let record = core.store.group(&group_id).unwrap().unwrap();
        assert_eq!(groups::role_of(&record.state, &alice), Some(crate::models::GroupRole::Owner));

        let refused = call(&mut core, json!({"command":"send_text","user_id":group_id,"text":"после выхода"}));
        assert_eq!(refused["ok"], false);

        let deleted = call(&mut core, json!({"command":"delete_contact","user_id":group_id}));
        assert_eq!(deleted["ok"], true, "{deleted}");
        assert!(deleted["snapshot"]["chats"].as_array().unwrap().iter().all(|chat| chat["userId"] != group_id));

        drop(core);
        fs::remove_dir_all(root).ok();
    }

    fn remote_event(
        sender: &str,
        group_id: &str,
        kind: &str,
        payload: &impl serde::Serialize,
    ) -> crate::protocol::SignedProtocolEvent {
        crate::protocol::SignedProtocolEvent {
            version: PROTOCOL_VERSION,
            event_id: format!("evt1-{}", random_hex(16)),
            conversation_id: group_id.to_owned(),
            sender_user_id: sender.to_owned(),
            sender_device_id: "ttd1-test".to_owned(),
            device_sequence: 1,
            kind: kind.to_owned(),
            created_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
            payload: STANDARD.encode(serde_json::to_vec(payload).unwrap()),
            signature: String::new(),
        }
    }

    /// Приглашение, самовольное повышение, сообщение от чужака и исключение — со стороны
    /// получателя. Подписи здесь не проверяются: это делает приём конверта раньше.
    #[test]
    fn a_member_only_accepts_authorised_group_changes() {
        use crate::models::{GroupMember, GroupRole, GroupState};
        use crate::protocol::{GroupStatePayload, KIND_GROUP_STATE};

        let root = std::env::temp_dir().join(format!("turat-invite-{}", uuid::Uuid::new_v4()));
        let mut core = open_core(&root);
        let me = core.identity.public.user_id.clone();
        let owner = member_id(5);
        let other = member_id(6);
        let group_id = format!("ttg1-{}", hex::encode([4u8; 32]));
        let member = |user_id: &str, role| GroupMember {
            user_id: user_id.to_owned(),
            display_name: "Кто-то".to_owned(),
            role,
            added_by: owner.clone(),
            added_at_unix_milliseconds: 1,
        };
        let mut state = GroupState {
            version: groups::GROUP_STATE_VERSION,
            group_id: group_id.clone(),
            epoch: 1,
            name: "Чужая группа".to_owned(),
            about: String::new(),
            avatar_base64: None,
            created_by: owner.clone(),
            created_at_unix_milliseconds: 1,
            members: vec![
                member(&owner, GroupRole::Owner),
                member(&me, GroupRole::Member),
                member(&other, GroupRole::Member),
            ],
            permissions: Default::default(),
            updated_by: owner.clone(),
            updated_at_unix_milliseconds: 1,
        };
        let payload = |state: &GroupState| GroupStatePayload { version: PROTOCOL_VERSION, state: state.clone() };

        assert!(core.apply_group_event(&remote_event(&owner, &group_id, KIND_GROUP_STATE, &payload(&state))).unwrap());
        let record = core.store.group(&group_id).unwrap().unwrap();
        assert!(record.pending_invite);
        let refused = call(&mut core, json!({"command":"send_text","user_id":group_id,"text":"до принятия"}));
        assert_eq!(refused["ok"], false);

        // Рядовой участник выдаёт себе администратора — изменение не применяется.
        let mut seized = state.clone();
        seized.epoch = 2;
        seized.updated_by = other.clone();
        seized.members[2].role = GroupRole::Admin;
        assert!(!core.apply_group_event(&remote_event(&other, &group_id, KIND_GROUP_STATE, &payload(&seized))).unwrap());
        assert_eq!(core.store.group(&group_id).unwrap().unwrap().state.epoch, 1);

        let accepted = call(&mut core, json!({"command":"accept_contact","user_id":group_id}));
        assert_eq!(accepted["ok"], true, "{accepted}");
        assert_eq!(core.store.outbox_length().unwrap(), 2, "group.joined обоим участникам");

        // Сообщение от того, кто в группе не состоит, в ленту не попадает.
        let stranger = member_id(9);
        let text = TextPayload { version: PROTOCOL_VERSION, text: "спам".to_owned(), reply_to_event_id: None, forwarded_from: None };
        assert!(!core.apply_group_event(&remote_event(&stranger, &group_id, KIND_TEXT, &text)).unwrap());
        let text = TextPayload { text: "свои".to_owned(), ..text };
        assert!(core.apply_group_event(&remote_event(&other, &group_id, KIND_TEXT, &text)).unwrap());

        // Изменение через одно (epoch 3 без 2) ждёт, пока догонит предыдущее.
        let mut promoted = state.clone();
        promoted.epoch = 2;
        promoted.members[2].role = GroupRole::Admin;
        let mut removed_by_admin = promoted.clone();
        removed_by_admin.epoch = 3;
        removed_by_admin.updated_by = other.clone();
        removed_by_admin.members.retain(|value| value.user_id != me);
        assert!(!core.apply_group_event(&remote_event(&other, &group_id, KIND_GROUP_STATE, &payload(&removed_by_admin))).unwrap());
        assert_eq!(core.store.group(&group_id).unwrap().unwrap().state.epoch, 1);
        assert!(core.apply_group_event(&remote_event(&owner, &group_id, KIND_GROUP_STATE, &payload(&promoted))).unwrap());
        let record = core.store.group(&group_id).unwrap().unwrap();
        assert_eq!(record.state.epoch, 3, "отложенное изменение применилось следом");
        assert!(record.left);

        let messages = core.store.messages(&group_id).unwrap();
        assert!(messages.iter().any(|message| message.service && message.text.contains("исключил(а) вас")));
        state.epoch = 4;
        assert!(!core.apply_group_event(&remote_event(&other, &group_id, KIND_TEXT, &TextPayload { text: "после исключения".to_owned(), version: PROTOCOL_VERSION, reply_to_event_id: None, forwarded_from: None })).unwrap());

        drop(core);
        fs::remove_dir_all(root).ok();
    }

    /// Перекладывает очередь `from` в ядра получателей, как это сделала бы сеть, и
    /// возвращает виды доставленных событий по получателям.
    fn pump(from: &mut AppCore, peers: &mut [&mut AppCore]) -> Vec<(String, String)> {
        let sender = crate::protocol::WireIdentity::from(&from.identity.public);
        let jobs = from.store.due_outbox(10_000).unwrap();
        let mut delivered = Vec::new();
        for job in jobs {
            from.store.complete_outbox(&job.job_id).unwrap();
            let Some(peer) = peers
                .iter_mut()
                .find(|peer| peer.identity.public.user_id == job.user_id)
            else {
                continue;
            };
            assert!(job.event.verify(&sender), "подпись события {}", job.event.kind);
            peer.store.mark_seen(&job.event.event_id).unwrap();
            peer.apply_channel_event(&job.event, &sender).unwrap();
            delivered.push((job.user_id.clone(), job.event.kind.clone()));
        }
        delivered
    }

    fn named_core(root: &Path, name: &str) -> AppCore {
        let mut core = open_core(root);
        call(&mut core, json!({"command":"save_profile","username":"","display_name":name,"about":"","avatar_base64":null}));
        core
    }

    /// Канал целиком: подписка по ссылке, пост, просмотры, реакции, комментарии через
    /// администратора, счётчики, права и удаление подписчика.
    #[test]
    fn channel_lifecycle_between_owner_and_subscribers() {
        let base = std::env::temp_dir().join(format!("turat-channel-{}", uuid::Uuid::new_v4()));
        let mut owner = named_core(&base.join("owner"), "Владелец");
        let mut bob = named_core(&base.join("bob"), "Боб");
        let mut carol = named_core(&base.join("carol"), "Кэрол");
        let bob_id = bob.identity.public.user_id.clone();
        let carol_id = carol.identity.public.user_id.clone();

        let created = call(&mut owner, json!({"command":"create_channel","name":"Новости","about":"Главное за день"}));
        assert_eq!(created["ok"], true, "{created}");
        let channel_id = created["value"]["channelId"].as_str().unwrap().to_owned();
        let view = &created["snapshot"]["channel"];
        assert_eq!(view["myRole"], "owner");
        assert_eq!(view["canPost"], true);
        let link = view["inviteLink"].as_str().unwrap().to_owned();
        let chat = created["snapshot"]["chats"].as_array().unwrap().iter()
            .find(|chat| chat["userId"] == channel_id).cloned().unwrap();
        assert_eq!(chat["isChannel"], true);
        assert_eq!(chat["channelCanPost"], true);

        // Пост до подписки: его новичок получит историей.
        let early = call(&mut owner, json!({"command":"send_text","user_id":channel_id,"text":"Первый пост"}));
        assert_eq!(early["ok"], true, "{early}");

        for subscriber in [&mut bob, &mut carol] {
            let subscribed = call(subscriber, json!({"command":"subscribe_channel","link":link}));
            assert_eq!(subscribed["ok"], true, "{subscribed}");
            assert_eq!(subscribed["snapshot"]["channel"]["awaitingState"], true);
            assert_eq!(subscribed["snapshot"]["channel"]["canPost"], false);
            pump(subscriber, &mut [&mut owner]);
        }
        let sent = pump(&mut owner, &mut [&mut bob, &mut carol]);
        assert!(sent.contains(&(bob_id.clone(), "channel.state".to_owned())), "{sent:?}");
        assert!(sent.contains(&(carol_id.clone(), "channel.relay".to_owned())), "{sent:?}");

        let bob_view = call(&mut bob, json!({"command":"select_contact","user_id":channel_id}));
        assert_eq!(bob_view["snapshot"]["channel"]["awaitingState"], false, "{bob_view}");
        assert_eq!(bob_view["snapshot"]["channel"]["name"], "Новости");
        // Число подписчиков Боб узнал в момент своей подписки — Кэрол пришла позже.
        assert_eq!(bob_view["snapshot"]["channel"]["subscriberCount"], 1);
        assert!(bob_view["snapshot"]["channel"]["subscribers"].as_array().unwrap().is_empty(),
            "подписчик не видит других подписчиков");
        let texts: Vec<String> = bob_view["snapshot"]["messages"].as_array().unwrap().iter()
            .filter(|message| message["service"] == false)
            .map(|message| message["text"].as_str().unwrap().to_owned()).collect();
        assert_eq!(texts, vec!["Первый пост"]);

        let owner_view = call(&mut owner, json!({"command":"select_contact","user_id":channel_id}));
        assert_eq!(owner_view["snapshot"]["channel"]["subscriberCount"], 2);
        assert_eq!(owner_view["snapshot"]["channel"]["subscribers"].as_array().unwrap().len(), 2);

        // Подписчик публиковать не может — ни у себя, ни подделкой события.
        let refused = call(&mut bob, json!({"command":"send_text","user_id":channel_id,"text":"спам"}));
        assert_eq!(refused["ok"], false);
        let forged = bob.sign_event(&channel_id, "evt1-forged", KIND_TEXT,
            &TextPayload { version: PROTOCOL_VERSION, text: "спам".to_owned(), reply_to_event_id: None, forwarded_from: None }).unwrap();
        let bob_wire = crate::protocol::WireIdentity::from(&bob.identity.public);
        assert!(!carol.apply_channel_event(&forged, &bob_wire).unwrap());

        // Новый пост, просмотр и реакция возвращаются автору.
        call(&mut owner, json!({"command":"send_text","user_id":channel_id,"text":"Второй пост"}));
        pump(&mut owner, &mut [&mut bob, &mut carol]);
        let bob_feed = call(&mut bob, json!({"command":"mark_read","user_id":channel_id}));
        let post = bob_feed["snapshot"]["messages"].as_array().unwrap().iter()
            .find(|message| message["text"] == "Второй пост").cloned().unwrap();
        let post_id = post["eventId"].as_str().unwrap().to_owned();
        call(&mut bob, json!({"command":"react","event_ids":[post_id],"reaction":"🔥"}));
        let returned = pump(&mut bob, &mut [&mut owner]);
        assert!(returned.iter().any(|(_, kind)| kind == "channel.views"), "{returned:?}");
        assert!(returned.iter().any(|(_, kind)| kind == "message.reaction"), "{returned:?}");
        let owner_feed = call(&mut owner, json!({"command":"snapshot"}));
        let own_post = owner_feed["snapshot"]["messages"].as_array().unwrap().iter()
            .find(|message| message["eventId"] == post_id).cloned().unwrap();
        assert_eq!(own_post["channelPost"]["views"], 1);
        assert_eq!(own_post["channelPost"]["reactions"][0]["reaction"], "🔥");
        assert_eq!(own_post["channelPost"]["reactions"][0]["count"], 1);

        // Комментарий подписчика уходит автору, а тот разносит его остальным.
        let commented = call(&mut bob, json!({"command":"send_comment","post_event_id":post_id,"text":"Отличный пост"}));
        assert_eq!(commented["ok"], true, "{commented}");
        let to_owner = pump(&mut bob, &mut [&mut owner, &mut carol]);
        assert_eq!(to_owner.len(), 1, "комментарий идёт только администратору: {to_owner:?}");
        let relayed = pump(&mut owner, &mut [&mut bob, &mut carol]);
        assert!(relayed.contains(&(carol_id.clone(), "channel.relay".to_owned())), "{relayed:?}");
        assert!(!relayed.iter().any(|(user, _)| user == &bob_id), "автору комментарий не возвращается");
        let thread = call(&mut carol, json!({"command":"open_comments","post_event_id":post_id}));
        assert_eq!(thread["ok"], true, "{thread}");
        let comments = thread["snapshot"]["comments"].as_array().unwrap();
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0]["text"], "Отличный пост");
        assert_eq!(comments[0]["senderName"], "Боб");
        assert_eq!(thread["snapshot"]["channel"]["threadPostEventId"], post_id);

        // Счётчики автора доходят до подписчиков.
        owner.broadcast_channel_stats().unwrap();
        pump(&mut owner, &mut [&mut bob, &mut carol]);
        let carol_feed = call(&mut carol, json!({"command":"open_comments","post_event_id":null}));
        let seen = carol_feed["snapshot"]["messages"].as_array().unwrap().iter()
            .find(|message| message["eventId"] == post_id).cloned().unwrap();
        assert_eq!(seen["channelPost"]["views"], 1);
        assert_eq!(seen["channelPost"]["comments"], 1);
        assert_eq!(seen["channelPost"]["reactions"][0]["count"], 1);
        assert_eq!(carol_feed["snapshot"]["channel"]["subscriberCount"], 2);

        // Администратор с правом публикации: его пост принимают подписчики.
        let appointed = call(&mut owner, json!({"command":"set_channel_admin","channel_id":channel_id,"user_id":bob_id,
            "rights":{"postMessages":true},"title":"Редактор"}));
        assert_eq!(appointed["ok"], true, "{appointed}");
        let appointment = pump(&mut owner, &mut [&mut bob, &mut carol]);
        assert!(appointment.contains(&(bob_id.clone(), "channel.roster".to_owned())), "{appointment:?}");
        let bob_admin = call(&mut bob, json!({"command":"select_contact","user_id":channel_id}));
        assert_eq!(bob_admin["snapshot"]["channel"]["myRole"], "admin");
        assert_eq!(bob_admin["snapshot"]["channel"]["canPost"], true);
        assert_eq!(bob_admin["snapshot"]["channel"]["canBan"], false);
        let bob_post = call(&mut bob, json!({"command":"send_text","user_id":channel_id,"text":"От редактора"}));
        assert_eq!(bob_post["ok"], true, "{bob_post}");
        pump(&mut bob, &mut [&mut owner, &mut carol]);
        let carol_texts = carol.store.messages(&channel_id).unwrap();
        assert!(carol_texts.iter().any(|message| message.text == "От редактора"));
        // Без права блокировки чужого подписчика не удалить.
        let kick = call(&mut bob, json!({"command":"remove_channel_subscriber","channel_id":channel_id,"user_id":carol_id}));
        assert_eq!(kick["ok"], false);

        // Владелец удаляет подписчика: тот узнаёт об этом и больше ничего не получает.
        let removed = call(&mut owner, json!({"command":"remove_channel_subscriber","channel_id":channel_id,"user_id":carol_id,"ban":true}));
        assert_eq!(removed["ok"], true, "{removed}");
        pump(&mut owner, &mut [&mut bob, &mut carol]);
        let carol_view = call(&mut carol, json!({"command":"select_contact","user_id":channel_id}));
        assert_eq!(carol_view["snapshot"]["channel"]["removed"], true);
        call(&mut owner, json!({"command":"send_text","user_id":channel_id,"text":"Без Кэрол"}));
        let last = pump(&mut owner, &mut [&mut bob, &mut carol]);
        assert!(!last.iter().any(|(user, _)| user == &carol_id), "{last:?}");

        // Владелец не может просто отписаться, а после удаления канала публиковать нельзя.
        let owner_leave = call(&mut owner, json!({"command":"leave_channel","channel_id":channel_id}));
        assert_eq!(owner_leave["ok"], false);
        let closed = call(&mut owner, json!({"command":"close_channel","channel_id":channel_id}));
        assert_eq!(closed["ok"], true, "{closed}");
        pump(&mut owner, &mut [&mut bob, &mut carol]);
        let bob_closed = call(&mut bob, json!({"command":"select_contact","user_id":channel_id}));
        assert_eq!(bob_closed["snapshot"]["channel"]["closed"], true);
        assert_eq!(bob_closed["snapshot"]["channel"]["canPost"], false);

        drop(owner);
        drop(bob);
        drop(carol);
        fs::remove_dir_all(base).ok();
    }

    /// Приглашение из контактов приходит запросом; принятие — это подписка.
    #[test]
    fn a_channel_invite_waits_for_consent() {
        let base = std::env::temp_dir().join(format!("turat-channel-invite-{}", uuid::Uuid::new_v4()));
        let mut owner = named_core(&base.join("owner"), "Владелец");
        let mut bob = named_core(&base.join("bob"), "Боб");
        let bob_id = bob.identity.public.user_id.clone();
        add_accepted_contact(&owner, &bob_id, "Боб");
        let created = call(&mut owner, json!({"command":"create_channel","name":"Клуб"}));
        let channel_id = created["value"]["channelId"].as_str().unwrap().to_owned();
        let invited = call(&mut owner, json!({"command":"invite_to_channel","channel_id":channel_id,"user_ids":[bob_id]}));
        assert_eq!(invited["ok"], true, "{invited}");
        pump(&mut owner, &mut [&mut bob]);

        let pending = call(&mut bob, json!({"command":"select_contact","user_id":channel_id}));
        assert_eq!(pending["snapshot"]["channel"]["pendingInvite"], true);
        assert_eq!(pending["snapshot"]["channel"]["invitedByName"], "Владелец");
        call(&mut owner, json!({"command":"send_text","user_id":channel_id,"text":"до согласия"}));
        assert!(pump(&mut owner, &mut [&mut bob]).is_empty(), "до принятия постов нет");

        let accepted = call(&mut bob, json!({"command":"accept_contact","user_id":channel_id}));
        assert_eq!(accepted["ok"], true, "{accepted}");
        pump(&mut bob, &mut [&mut owner]);
        pump(&mut owner, &mut [&mut bob]);
        let texts: Vec<String> = bob.store.messages(&channel_id).unwrap().into_iter()
            .filter(|message| !message.service).map(|message| message.text).collect();
        assert_eq!(texts, vec!["до согласия"], "история пришла после подписки");

        let left = call(&mut bob, json!({"command":"delete_contact","user_id":channel_id}));
        assert_eq!(left["ok"], true, "{left}");
        pump(&mut bob, &mut [&mut owner]);
        let owner_view = call(&mut owner, json!({"command":"select_contact","user_id":channel_id}));
        assert_eq!(owner_view["snapshot"]["channel"]["subscriberCount"], 0);

        drop(owner);
        drop(bob);
        fs::remove_dir_all(base).ok();
    }

    /// Привязанная группа обсуждения получает посты как пересланные от имени канала.
    #[test]
    fn posts_reach_the_linked_discussion_group() {
        let root = std::env::temp_dir().join(format!("turat-discussion-{}", uuid::Uuid::new_v4()));
        let mut core = named_core(&root, "Автор");
        let alice = member_id(2);
        add_accepted_contact(&core, &alice, "Алиса");
        let group = call(&mut core, json!({"command":"create_group","name":"Обсуждение","member_ids":[alice]}));
        let group_id = group["value"]["groupId"].as_str().unwrap().to_owned();
        let channel = call(&mut core, json!({"command":"create_channel","name":"Блог"}));
        let channel_id = channel["value"]["channelId"].as_str().unwrap().to_owned();

        let linked = call(&mut core, json!({"command":"link_discussion_group","channel_id":channel_id,"group_id":group_id}));
        assert_eq!(linked["ok"], true, "{linked}");
        assert_eq!(linked["snapshot"]["channel"]["settings"]["discussionGroupName"], "Обсуждение");
        assert_eq!(linked["snapshot"]["channel"]["discussionJoined"], true);

        let posted = call(&mut core, json!({"command":"send_text","user_id":channel_id,"text":"Новая заметка"}));
        assert_eq!(posted["ok"], true, "{posted}");
        assert_eq!(posted["snapshot"]["selectedContactId"], channel_id, "публикация не уводит из канала");
        let forwarded = core.store.messages(&group_id).unwrap().into_iter()
            .find(|message| message.text == "Новая заметка").expect("пост в группе");
        assert_eq!(forwarded.forwarded_from.as_deref(), Some("Блог"));

        let unlinked = call(&mut core, json!({"command":"link_discussion_group","channel_id":channel_id,"group_id":null}));
        assert_eq!(unlinked["ok"], true, "{unlinked}");
        assert!(unlinked["snapshot"]["channel"]["settings"]["discussionGroupId"].is_null());

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
