use serde::{Deserialize, Serialize};

pub const DEFAULT_BOOTSTRAP_URL: &str = "https://turattext.rplacefree.store";
pub const DEFAULT_NODE_ID: &str =
    "ttn1-c6fcc7ef3f4693c7359d5ce47bd413bb516429bec7b118ca04b8da2fd3254a97";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicIdentity {
    pub version: i32,
    pub user_id: String,
    pub identity_algorithm: String,
    pub identity_public_key: String,
    pub device_id: String,
    pub device_algorithm: String,
    pub device_public_key: String,
    pub created_at_unix_milliseconds: i64,
    pub device_certificate: String,
    pub is_authority: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub username: String,
    pub display_name: String,
    pub about: String,
    pub avatar_base64: Option<String>,
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            username: String::new(),
            display_name: String::new(),
            about: String::new(),
            avatar_base64: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Contact {
    pub user_id: String,
    pub display_name: String,
    pub username: Option<String>,
    pub about: Option<String>,
    pub avatar_base64: Option<String>,
    pub added_at_unix_milliseconds: i64,
    pub fingerprint_verified: bool,
    pub pending_approval: bool,
    pub last_seen_unix_milliseconds: Option<i64>,
    /// Закреплённые чаты Telegram всегда стоят выше остальных.
    #[serde(default)]
    pub pinned: bool,
    /// Чат «без звука»: счётчик рисуется серым и не поднимает уведомление.
    #[serde(default)]
    pub muted: bool,
    /// Несохранённый черновик — Telegram показывает его в списке чатов.
    #[serde(default)]
    pub draft: String,
    /// Отметка «непрочитано», поставленная вручную.
    #[serde(default)]
    pub manual_unread: bool,
}

/// Что именно лежит во вложении: от этого зависит, рисует клиент превью, плеер или карточку файла.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaKind {
    Image,
    Video,
    Audio,
    File,
}

impl Default for MediaKind {
    fn default() -> Self {
        Self::File
    }
}

impl MediaKind {
    /// Клиент присылает свой `kind`, но если не прислал — тип выводится из MIME.
    pub fn from_mime(mime: &str) -> Self {
        let mime = mime.to_ascii_lowercase();
        if mime.starts_with("image/") {
            Self::Image
        } else if mime.starts_with("video/") {
            Self::Video
        } else if mime.starts_with("audio/") {
            Self::Audio
        } else {
            Self::File
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub attachment_id: String,
    pub file_name: String,
    pub mime_type: String,
    pub size: u64,
    pub local_path: String,
    /// Категория вложения: картинка, видео, аудио или обычный файл.
    #[serde(default)]
    pub kind: MediaKind,
    /// Размер картинки или кадра видео — клиент резервирует место в пузыре до загрузки.
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
    /// Длительность видео и аудио в миллисекундах.
    #[serde(default)]
    pub duration_milliseconds: i64,
    /// Крошечный JPEG-кадр в base64: показывается мгновенно, пока грузится оригинал.
    #[serde(default)]
    pub thumbnail_base64: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub event_id: String,
    pub conversation_id: String,
    pub sender_user_id: String,
    pub text: String,
    pub created_at_unix_milliseconds: i64,
    pub outgoing: bool,
    pub edited: bool,
    pub deleted: bool,
    pub reactions: Vec<String>,
    pub delivered: bool,
    pub read: bool,
    #[serde(default)]
    pub pinned: bool,
    pub attachment: Option<Attachment>,
    /// Ответ на сообщение: id цитируемого события в том же диалоге.
    #[serde(default)]
    pub reply_to_event_id: Option<String>,
    /// Имя автора оригинала для пересланных сообщений.
    #[serde(default)]
    pub forwarded_from: Option<String>,
    /// Служебная отметка группы: «Алиса добавила Боба». Клиент рисует её по центру
    /// ленты, и с ней нельзя ничего сделать — ни ответить, ни переслать.
    #[serde(default)]
    pub service: bool,
    /// Кто какую реакцию поставил. В группе реакций от разных людей много, и снятие
    /// своей не должно убирать чужую; `reactions` выводится из этого списка.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reaction_marks: Vec<ReactionMark>,
    /// Имя автора для ленты группы. Заполняется только в снимке и не хранится.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReactionMark {
    pub user_id: String,
    pub reaction: String,
}

/// Роль в группе. Порядок значим: сравнение ролей отвечает на вопрос «кто старше».
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GroupRole {
    Member,
    Admin,
    Owner,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMember {
    pub user_id: String,
    /// Имя, под которым участника добавили. Если он есть в контактах, клиент видит
    /// своё имя контакта, а это — запасное.
    pub display_name: String,
    pub role: GroupRole,
    pub added_by: String,
    pub added_at_unix_milliseconds: i64,
}

/// Что разрешено рядовым участникам. Администраторы и владелец могут всё это всегда.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupPermissions {
    #[serde(default)]
    pub members_can_invite: bool,
    #[serde(default)]
    pub members_can_edit_info: bool,
}

/// Полное состояние группы. Уходит участникам целиком при каждом изменении, а номер
/// `epoch` строго растёт: получатель проверяет разницу с тем, что знает сам.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupState {
    pub version: i32,
    pub group_id: String,
    pub epoch: i64,
    pub name: String,
    pub about: String,
    pub avatar_base64: Option<String>,
    pub created_by: String,
    pub created_at_unix_milliseconds: i64,
    pub members: Vec<GroupMember>,
    pub permissions: GroupPermissions,
    pub updated_by: String,
    pub updated_at_unix_milliseconds: i64,
}

/// Группа в локальном хранилище: состояние плюс то, что знает только это устройство.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupRecord {
    pub state: GroupState,
    /// Приглашение ещё не принято: писать в группу нельзя, квитанции не уходят.
    #[serde(default)]
    pub pending_invite: bool,
    #[serde(default)]
    pub invited_by: Option<String>,
    /// Пользователь вышел или его исключили: история остаётся, писать нельзя.
    #[serde(default)]
    pub left: bool,
    /// Чат удалён из списка. Запись остаётся надгробием: старые изменения группы,
    /// ещё летящие по сети, не должны воскрешать её как новое приглашение.
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub pinned: bool,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub draft: String,
    #[serde(default)]
    pub manual_unread: bool,
    /// Участники, от которых пришло хоть одно подписанное событие этой группы. Список
    /// из приглашения — лишь заявление пригласившего, а это — подтверждение.
    #[serde(default)]
    pub confirmed_members: Vec<String>,
    #[serde(default)]
    pub joined_at_unix_milliseconds: i64,
}

/// Участник в карточке группы: имя уже разрешено через контакты.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupMemberView {
    pub user_id: String,
    pub display_name: String,
    pub avatar_base64: Option<String>,
    pub role: GroupRole,
    pub is_self: bool,
    pub is_contact: bool,
    pub confirmed: bool,
    pub added_by_name: String,
}

/// Карточка открытой группы и то, что текущему пользователю в ней разрешено.
/// Права считает ядро: клиент только прячет недоступные кнопки.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GroupView {
    pub group_id: String,
    pub name: String,
    pub about: String,
    pub avatar_base64: Option<String>,
    pub epoch: i64,
    pub created_by: String,
    pub created_at_unix_milliseconds: i64,
    pub my_role: Option<GroupRole>,
    pub pending_invite: bool,
    pub invited_by_name: Option<String>,
    pub left: bool,
    pub permissions: GroupPermissions,
    pub members: Vec<GroupMemberView>,
    pub can_send: bool,
    pub can_invite: bool,
    pub can_edit_info: bool,
    pub can_remove_members: bool,
    pub can_manage_admins: bool,
    pub can_delete_messages: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub bootstrap_url: String,
    pub expected_node_id: Option<String>,
    pub metadata_protection: MetadataProtection,
    /// Публиковать ли «последнюю активность» в directory (как приватность Telegram).
    #[serde(default)]
    pub publish_presence: bool,
    /// Номер последней опубликованной записи в directory: сервер требует роста.
    #[serde(default)]
    pub directory_sequence: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bootstrap_url: DEFAULT_BOOTSTRAP_URL.to_owned(),
            expected_node_id: Some(DEFAULT_NODE_ID.to_owned()),
            metadata_protection: MetadataProtection::Balanced,
            publish_presence: false,
            directory_sequence: 0,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MetadataProtection {
    Fast,
    Balanced,
    High,
}

/// Строка списка чатов: контакт вместе с превью последнего события и счётчиком
/// непрочитанного. Клиенты рисуют список чатов только из этой структуры.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    #[serde(flatten)]
    pub contact: Contact,
    pub preview: String,
    pub last_activity_unix_milliseconds: i64,
    pub has_last_message: bool,
    pub last_message_outgoing: bool,
    pub last_message_delivered: bool,
    pub last_message_read: bool,
    pub unread_count: u32,
    /// Строка — группа: `userId` тогда содержит GroupID (`ttg1-…`).
    pub is_group: bool,
    pub member_count: u32,
    pub group_role: Option<GroupRole>,
    /// Пользователь покинул группу или исключён: писать в неё нельзя.
    pub group_left: bool,
}

/// Результат глобального поиска по всем диалогам.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHit {
    pub event_id: String,
    pub user_id: String,
    pub display_name: String,
    pub avatar_base64: Option<String>,
    pub text: String,
    pub created_at_unix_milliseconds: i64,
    pub outgoing: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub identity: PublicIdentity,
    pub profile: Profile,
    pub chats: Vec<Chat>,
    pub selected_contact_id: Option<String>,
    pub messages: Vec<Message>,
    pub settings: Settings,
    pub online: bool,
    pub status_message: String,
    pub onboarding_required: bool,
    pub search_query: String,
    pub search_results: Vec<SearchHit>,
    /// Карточка выбранной группы; у личного диалога пусто.
    pub group: Option<GroupView>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum Command {
    Snapshot,
    SelectContact {
        user_id: Option<String>,
    },
    AddContact {
        query: String,
        display_name: Option<String>,
    },
    DeleteContact {
        user_id: String,
    },
    AcceptContact {
        user_id: String,
    },
    RejectContact {
        user_id: String,
    },
    VerifyContact {
        user_id: String,
        verified: bool,
    },
    SendText {
        user_id: String,
        text: String,
        #[serde(default)]
        reply_to_event_id: Option<String>,
    },
    ForwardMessages {
        event_ids: Vec<String>,
        user_id: String,
    },
    SetChatPinned {
        user_id: String,
        pinned: bool,
    },
    SetChatMuted {
        user_id: String,
        muted: bool,
    },
    SaveDraft {
        user_id: String,
        text: String,
    },
    ClearHistory {
        user_id: String,
    },
    MarkUnread {
        user_id: String,
    },
    Search {
        query: String,
    },
    SetPresencePublishing {
        enabled: bool,
    },
    EditMessage {
        event_id: String,
        text: String,
    },
    DeleteMessages {
        event_ids: Vec<String>,
    },
    SetMessagePinned {
        event_id: String,
        pinned: bool,
    },
    React {
        event_ids: Vec<String>,
        reaction: String,
    },
    MarkRead {
        user_id: String,
    },
    SaveProfile {
        username: String,
        display_name: String,
        about: String,
        avatar_base64: Option<String>,
    },
    /// Явная публикация профиля и username в directory — только по действию пользователя.
    PublishProfile,
    Connect {
        bootstrap_url: String,
    },
    Sync,
    CycleMetadataProtection,
    AttachFile {
        user_id: String,
        path: String,
        mime_type: String,
        caption: Option<String>,
        #[serde(default)]
        reply_to_event_id: Option<String>,
        #[serde(default)]
        kind: Option<MediaKind>,
        #[serde(default)]
        width: u32,
        #[serde(default)]
        height: u32,
        #[serde(default)]
        duration_milliseconds: i64,
        #[serde(default)]
        thumbnail_base64: Option<String>,
    },
    /// Запускает шифрование вложения в фоне и сразу возвращает `jobId`: интерфейс рисует
    /// прогресс, а не замирает на большом видео.
    StartAttachment {
        user_id: String,
        path: String,
        mime_type: String,
        caption: Option<String>,
        #[serde(default)]
        reply_to_event_id: Option<String>,
        #[serde(default)]
        kind: Option<MediaKind>,
        #[serde(default)]
        width: u32,
        #[serde(default)]
        height: u32,
        #[serde(default)]
        duration_milliseconds: i64,
        #[serde(default)]
        thumbnail_base64: Option<String>,
    },
    /// Превращает завершённую фоновую задачу в сообщение с вложением.
    FinishAttachment {
        job_id: String,
    },
    /// Сохранение вложения на диск в фоне — с тем же прогрессом, что и отправка.
    StartExportAttachment {
        event_id: String,
        destination_path: String,
    },
    /// Состояние фоновой задачи: сколько байт готово, завершена ли, была ли ошибка.
    MediaJob {
        job_id: String,
    },
    CancelMediaJob {
        job_id: String,
    },
    /// Путь к зашифрованному вложению — клиент открывает его потоковым читателем.
    AttachmentSource {
        event_id: String,
    },
    ExportAttachment {
        event_id: String,
        destination_path: String,
    },
    CreateBackup {
        path: String,
        passphrase: String,
    },
    RestoreBackup {
        path: String,
        passphrase: String,
    },
    CreateDeviceLink {
        path: String,
        passphrase: String,
    },
    ImportDeviceLink {
        path: String,
        passphrase: String,
    },
    ExportPortable {
        path: String,
    },
    ImportPortable {
        path: String,
    },
    ExportDiscovery {
        path: String,
    },
    ImportDiscovery {
        path: String,
    },
    RevokeDevice {
        device_id: String,
    },
    /// Новая группа из принятых контактов. Возвращает `groupId`.
    CreateGroup {
        name: String,
        #[serde(default)]
        about: String,
        #[serde(default)]
        avatar_base64: Option<String>,
        #[serde(default)]
        member_ids: Vec<String>,
    },
    AddGroupMembers {
        group_id: String,
        user_ids: Vec<String>,
    },
    RemoveGroupMember {
        group_id: String,
        user_id: String,
    },
    /// Назначение или снятие администратора; владельца так не сменить.
    SetGroupRole {
        group_id: String,
        user_id: String,
        role: GroupRole,
    },
    TransferGroupOwnership {
        group_id: String,
        user_id: String,
    },
    UpdateGroupInfo {
        group_id: String,
        name: String,
        #[serde(default)]
        about: String,
        #[serde(default)]
        avatar_base64: Option<String>,
    },
    SetGroupPermissions {
        group_id: String,
        members_can_invite: bool,
        members_can_edit_info: bool,
    },
    /// Выход из группы с сохранением истории. Удаление чата группы — `delete_contact`.
    LeaveGroup {
        group_id: String,
    },
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<Snapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}
