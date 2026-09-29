//! Каналы: публикации администраторов для подписчиков, как в Telegram.
//!
//! Канал устроен так же, как группа: общего ключа и сервера, знающего состав, нет. Каждый
//! пост подписывается один раз и уходит каждому подписчику по его личному double ratchet.
//! Отличия от группы:
//!
//! - в подписанном состоянии канала ([`ChannelState`]) перечислены только администраторы
//!   с их правами; подписчиков знают лишь администраторы, а подписчики друг друга не видят;
//! - подписаться можно по ссылке-приглашению: в ней ChannelID и UserID администратора,
//!   которому уходит запрос; он и присылает состояние канала и последние посты;
//! - подписчики друг другу писать не могут, поэтому их комментарии разносит администратор,
//!   пересылая исходное подписанное событие ([`KIND_CHANNEL_RELAY`]): подпись комментатора
//!   проверяет каждый получатель, подделать чужой комментарий пересылающий не может;
//! - просмотры и реакции уходят автору поста, а он периодически рассылает всем итоговые
//!   счётчики ([`KIND_CHANNEL_STATS`]).
//!
//! Изменения состояния проверяются так же строго, как у группы: по правам автора в
//! прежнем состоянии ([`validate_transition`]).

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};

use super::{AppCore, random_hex, set_reaction_mark, short_id};
use crate::{
    CoreError,
    models::{
        ChannelAdmin, ChannelAdminRights, ChannelAdminView, ChannelPostInfo, ChannelRecord,
        ChannelRole, ChannelSettings, ChannelState, ChannelSubscriber, ChannelSubscriberView,
        ChannelView, Message, ReactionCount, SubscriberStatus,
    },
    protocol::{
        ChannelCommentDeletePayload, ChannelCommentPayload, ChannelNoticePayload,
        ChannelPostStats, ChannelRelayPayload, ChannelRosterPayload, ChannelStatePayload,
        ChannelStatsPayload, ChannelSubscribePayload, ChannelViewsPayload, EditPayload,
        KIND_ATTACHMENT, KIND_CHANNEL_COMMENT, KIND_CHANNEL_COMMENT_DELETE, KIND_CHANNEL_INVITE,
        KIND_CHANNEL_LEAVE, KIND_CHANNEL_RELAY, KIND_CHANNEL_REMOVED, KIND_CHANNEL_ROSTER,
        KIND_CHANNEL_STATE, KIND_CHANNEL_STATS, KIND_CHANNEL_SUBSCRIBE, KIND_CHANNEL_VIEWS,
        KIND_DELETE, KIND_EDIT, KIND_REACTION, KIND_TEXT, PROTOCOL_VERSION, ReactionPayload,
        RelayedEvent, SignedProtocolEvent, TargetPayload, WireIdentity, is_channel_id,
        is_group_id, is_user_id,
    },
    store::{PendingChannelState, StoredChannelPost},
};

pub const CHANNEL_STATE_VERSION: i32 = 1;
/// Каждый пост шифруется под каждого подписчика отдельно. Тысяча — предел, при котором
/// рассылка ещё укладывается в несколько циклов фоновой синхронизации.
pub const MAX_CHANNEL_SUBSCRIBERS: usize = 1000;
pub const MAX_CHANNEL_ADMINS: usize = 50;
const MAX_CHANNEL_NAME_CHARS: usize = 64;
const MAX_CHANNEL_ABOUT_CHARS: usize = 255;
const MAX_ADMIN_NAME_CHARS: usize = 64;
const MAX_ADMIN_TITLE_CHARS: usize = 16;
const MAX_COMMENT_CHARS: usize = 4096;
const PENDING_STATE_TTL_MILLISECONDS: i64 = 7 * 86_400_000;
/// Сколько последних постов получает новый подписчик.
const HISTORY_POSTS: usize = 30;
/// Пересылка режется на куски: конверт Node не резиновый.
const RELAY_CHUNK_BYTES: usize = 40 * 1024;
const ROSTER_CHUNK: usize = 150;
/// Счётчики рассылаются не чаще раза в две минуты: каждая рассылка — событие на подписчика.
const STATS_INTERVAL_MILLISECONDS: i64 = 120_000;
const STATS_POSTS: usize = 100;
const MAX_LISTED_SUBSCRIBERS: usize = 500;

/// Что изменилось в состоянии канала.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChannelChange {
    Renamed(String),
    AboutChanged,
    AvatarChanged,
    SettingsChanged,
    DiscussionChanged(Option<String>),
    AdminAdded(String),
    AdminRemoved(String),
    AdminEdited(String),
    Resigned,
    OwnershipTransferred(String),
    Closed,
}

fn denied(message: &str) -> CoreError {
    CoreError::InvalidInput(message.to_owned())
}

pub fn admin<'a>(state: &'a ChannelState, user_id: &str) -> Option<&'a ChannelAdmin> {
    state.admins.iter().find(|value| value.user_id == user_id)
}

/// Права пользователя в канале; у владельца — все. `None` — он не администратор.
pub fn rights_of(state: &ChannelState, user_id: &str) -> Option<ChannelAdminRights> {
    admin(state, user_id).map(|value| match value.role {
        ChannelRole::Owner => ChannelAdminRights::ALL,
        ChannelRole::Admin => value.rights,
    })
}

fn owner_of(state: &ChannelState) -> Option<&str> {
    state
        .admins
        .iter()
        .find(|value| value.role == ChannelRole::Owner)
        .map(|value| value.user_id.as_str())
}

/// Ветка комментариев поста хранится как отдельный диалог.
pub fn thread_id(channel_id: &str, post_event_id: &str) -> String {
    format!("{channel_id}/{post_event_id}")
}

/// Канал и пост ветки комментариев.
pub fn split_thread(conversation: &str) -> Option<(&str, &str)> {
    conversation
        .split_once('/')
        .filter(|(channel, _)| is_channel_id(channel))
}

/// Канал, к которому относится диалог: сам канал или ветка комментариев в нём.
pub fn channel_of_conversation(conversation: &str) -> Option<&str> {
    if is_channel_id(conversation) {
        Some(conversation)
    } else {
        split_thread(conversation).map(|(channel, _)| channel)
    }
}

pub fn invite_link(channel_id: &str, via: &str) -> String {
    format!("turat://channel/{channel_id}?via={via}")
}

/// Разбирает ссылку-приглашение. Понимает и полную ссылку, и пару «ChannelID UserID».
pub fn parse_invite_link(link: &str) -> Result<(String, String), CoreError> {
    let tokens: Vec<&str> = link
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '-'))
        .filter(|token| !token.is_empty())
        .collect();
    let channel = tokens.iter().find(|token| is_channel_id(token));
    let via = tokens.iter().find(|token| is_user_id(token));
    match (channel, via) {
        (Some(channel), Some(via)) => Ok(((*channel).to_owned(), (*via).to_owned())),
        _ => Err(denied(
            "Ссылка на канал должна выглядеть так: turat://channel/ttch1-…?via=tt1-…",
        )),
    }
}

fn state_hash(state: &ChannelState) -> Vec<u8> {
    Sha256::digest(serde_json::to_vec(state).unwrap_or_default()).to_vec()
}

fn truncate(value: &str, limit: usize) -> String {
    value.trim().chars().take(limit).collect()
}

/// Более поздняя запись списка подписчиков побеждает; при равном времени — более строгий
/// статус, чтобы все администраторы сошлись к одному и тому же.
fn newer(incoming: &ChannelSubscriber, existing: Option<&ChannelSubscriber>) -> bool {
    match existing {
        None => true,
        Some(current) => {
            (incoming.updated_at_unix_milliseconds, incoming.status)
                > (current.updated_at_unix_milliseconds, current.status)
        }
    }
}

/// Форма состояния без учёта того, кто и что менял.
pub fn validate_state(state: &ChannelState) -> Result<(), CoreError> {
    if state.version != CHANNEL_STATE_VERSION {
        return Err(denied("Неподдерживаемая версия канала"));
    }
    if !is_channel_id(&state.channel_id) || state.epoch < 1 {
        return Err(denied("Повреждено состояние канала"));
    }
    let name_length = state.name.trim().chars().count();
    if name_length == 0 || name_length > MAX_CHANNEL_NAME_CHARS {
        return Err(denied("Название канала: 1–64 символа"));
    }
    if state.about.chars().count() > MAX_CHANNEL_ABOUT_CHARS {
        return Err(denied("Описание канала: максимум 255 символов"));
    }
    if state
        .avatar_base64
        .as_ref()
        .is_some_and(|value| value.len() > super::groups::MAX_GROUP_AVATAR_BASE64)
    {
        return Err(denied("Фото канала слишком большое"));
    }
    if !is_user_id(&state.created_by) || !is_user_id(&state.updated_by) {
        return Err(denied("Повреждено состояние канала"));
    }
    if state.admins.len() > MAX_CHANNEL_ADMINS {
        return Err(denied("В канале может быть не больше 50 администраторов"));
    }
    if let Some(group) = &state.settings.discussion_group_id
        && !is_group_id(group)
    {
        return Err(denied("Повреждена ссылка на группу обсуждения"));
    }
    if state.settings.discussion_group_name.chars().count() > MAX_CHANNEL_NAME_CHARS {
        return Err(denied("Повреждена ссылка на группу обсуждения"));
    }
    let mut seen = HashSet::new();
    for value in &state.admins {
        if !is_user_id(&value.user_id)
            || !is_user_id(&value.added_by)
            || value.display_name.chars().count() > MAX_ADMIN_NAME_CHARS
            || value.title.chars().count() > MAX_ADMIN_TITLE_CHARS
            || !seen.insert(value.user_id.as_str())
        {
            return Err(denied("Повреждён список администраторов канала"));
        }
        if value.role == ChannelRole::Owner && value.rights != ChannelAdminRights::ALL {
            return Err(denied("У владельца канала есть все права"));
        }
    }
    let owners = state
        .admins
        .iter()
        .filter(|value| value.role == ChannelRole::Owner)
        .count();
    if owners != 1 {
        return Err(denied("У канала должен быть ровно один владелец"));
    }
    Ok(())
}

/// Проверяет, что `actor` имел право превратить `old` в `new`, и возвращает, что изменилось.
/// Права берутся из прежнего состояния.
pub fn validate_transition(
    old: &ChannelState,
    new: &ChannelState,
    actor: &str,
    allow_equal_epoch: bool,
) -> Result<Vec<ChannelChange>, CoreError> {
    validate_state(new)?;
    if new.channel_id != old.channel_id
        || new.created_by != old.created_by
        || new.created_at_unix_milliseconds != old.created_at_unix_milliseconds
    {
        return Err(denied("Нельзя подменить происхождение канала"));
    }
    if new.epoch < old.epoch || (new.epoch == old.epoch && !allow_equal_epoch) {
        return Err(denied("Устаревшее состояние канала"));
    }
    if new.updated_by != actor {
        return Err(denied("Изменение подписано не его автором"));
    }
    if old.closed {
        return Err(denied("Канал удалён"));
    }
    let actor_admin = admin(old, actor).ok_or_else(|| denied("Менять канал могут только администраторы"))?;
    let is_owner = actor_admin.role == ChannelRole::Owner;
    let rights = rights_of(old, actor).unwrap_or_default();
    let mut changes = Vec::new();

    let old_owner = owner_of(old).map(str::to_owned);
    let new_owner = owner_of(new).map(str::to_owned);
    let transferred = old_owner != new_owner;
    if transferred {
        if !is_owner {
            return Err(denied("Передать владение может только владелец"));
        }
        let Some(heir) = new_owner.as_deref() else {
            return Err(denied("У канала должен быть ровно один владелец"));
        };
        if admin(old, heir).is_none() {
            return Err(denied("Владение передаётся только администратору"));
        }
        if admin(new, actor).is_none_or(|value| value.role != ChannelRole::Admin) {
            return Err(denied("Прежний владелец остаётся администратором"));
        }
        changes.push(ChannelChange::OwnershipTransferred(heir.to_owned()));
    }

    let old_admins: HashMap<&str, &ChannelAdmin> = old
        .admins
        .iter()
        .map(|value| (value.user_id.as_str(), value))
        .collect();
    let new_ids: HashSet<&str> = new.admins.iter().map(|value| value.user_id.as_str()).collect();

    for value in &new.admins {
        if old_admins.contains_key(value.user_id.as_str()) {
            continue;
        }
        if !rights.add_admins {
            return Err(denied("Недостаточно прав, чтобы назначать администраторов"));
        }
        if value.role != ChannelRole::Admin || value.added_by != actor {
            return Err(denied("Новый администратор добавлен с неверной ролью"));
        }
        if !is_owner && !value.rights.within(rights) {
            return Err(denied("Нельзя выдать права, которых нет у вас"));
        }
        changes.push(ChannelChange::AdminAdded(value.user_id.clone()));
    }

    let mut resigned = false;
    for value in &old.admins {
        if new_ids.contains(value.user_id.as_str()) {
            continue;
        }
        if value.user_id == actor {
            if is_owner {
                return Err(denied("Владелец не может уйти, не передав канал"));
            }
            resigned = true;
            continue;
        }
        if value.role == ChannelRole::Owner {
            return Err(denied("Владельца нельзя разжаловать"));
        }
        if !rights.add_admins || !(is_owner || value.added_by == actor) {
            return Err(denied("Разжаловать можно только назначенных вами администраторов"));
        }
        changes.push(ChannelChange::AdminRemoved(value.user_id.clone()));
    }

    for current in &new.admins {
        let Some(previous) = old_admins.get(current.user_id.as_str()) else {
            continue;
        };
        if current.display_name != previous.display_name
            || current.added_by != previous.added_by
            || current.added_at_unix_milliseconds != previous.added_at_unix_milliseconds
        {
            return Err(denied("Нельзя менять данные администратора"));
        }
        // Участники передачи владения меняют роль и права только в её рамках.
        let in_transfer = transferred
            && (Some(current.user_id.as_str()) == new_owner.as_deref()
                || Some(current.user_id.as_str()) == old_owner.as_deref());
        if in_transfer {
            if current.title != previous.title {
                return Err(denied("Подпись администратора меняется отдельно"));
            }
            continue;
        }
        if current.role != previous.role {
            return Err(denied("Роль меняется только передачей владения"));
        }
        if current.rights == previous.rights && current.title == previous.title {
            continue;
        }
        if current.user_id == actor {
            return Err(denied("Свои права администратор не меняет"));
        }
        if !rights.add_admins || !(is_owner || previous.added_by == actor) {
            return Err(denied("Менять права можно только назначенным вами администраторам"));
        }
        if !is_owner && !current.rights.within(rights) {
            return Err(denied("Нельзя выдать права, которых нет у вас"));
        }
        changes.push(ChannelChange::AdminEdited(current.user_id.clone()));
    }

    let mut info_changed = false;
    if new.name.trim() != old.name.trim() {
        changes.push(ChannelChange::Renamed(new.name.trim().to_owned()));
        info_changed = true;
    }
    if new.about != old.about {
        changes.push(ChannelChange::AboutChanged);
        info_changed = true;
    }
    if new.avatar_base64 != old.avatar_base64 {
        changes.push(ChannelChange::AvatarChanged);
        info_changed = true;
    }
    if new.settings.sign_posts != old.settings.sign_posts
        || new.settings.comments_enabled != old.settings.comments_enabled
    {
        changes.push(ChannelChange::SettingsChanged);
        info_changed = true;
    }
    if new.settings.discussion_group_id != old.settings.discussion_group_id
        || new.settings.discussion_group_name != old.settings.discussion_group_name
    {
        changes.push(ChannelChange::DiscussionChanged(new.settings.discussion_group_id.clone()));
        info_changed = true;
    }
    if info_changed && !rights.change_info {
        return Err(denied("Недостаточно прав, чтобы менять данные канала"));
    }
    if new.closed {
        if !is_owner {
            return Err(denied("Удалить канал может только владелец"));
        }
        changes.push(ChannelChange::Closed);
    }
    if resigned {
        if !changes.is_empty() {
            return Err(denied("Уход из администраторов нельзя совмещать с другими изменениями"));
        }
        changes.push(ChannelChange::Resigned);
    }
    if changes.is_empty() {
        return Err(denied("Изменений нет"));
    }
    Ok(changes)
}

/// Ветка комментариев и её пост: откуда пришла команда пользователя.
enum ChannelItem {
    Post { channel_id: String },
    Comment { channel_id: String, post_event_id: String },
}

fn item_of(message: &Message) -> Option<ChannelItem> {
    if is_channel_id(&message.conversation_id) {
        return Some(ChannelItem::Post {
            channel_id: message.conversation_id.clone(),
        });
    }
    split_thread(&message.conversation_id).map(|(channel, post)| ChannelItem::Comment {
        channel_id: channel.to_owned(),
        post_event_id: post.to_owned(),
    })
}

impl AppCore {
    fn me(&self) -> String {
        self.identity.public.user_id.clone()
    }

    fn own_wire(&self) -> WireIdentity {
        WireIdentity::from(&self.identity.public)
    }

    /// Видимый канал. Надгробие удалённого чата для команд не считается.
    pub(super) fn channel_record(&self, channel_id: &str) -> Result<ChannelRecord, CoreError> {
        self.store
            .channel(channel_id)?
            .filter(|record| !record.hidden)
            .ok_or_else(|| denied("Канал не найден"))
    }

    /// Пользователь — действующий подписчик или администратор канала.
    fn is_active(record: &ChannelRecord) -> bool {
        !record.hidden
            && !record.left
            && !record.removed
            && !record.awaiting_state
            && !record.pending_invite
    }

    /// Права пользователя в канале, если он в нём действующий администратор.
    fn my_rights(&self, record: &ChannelRecord) -> Option<ChannelAdminRights> {
        if !Self::is_active(record) || record.state.closed {
            return None;
        }
        rights_of(&record.state, &self.identity.public.user_id)
    }

    fn ensure_channel_active(&self, channel_id: &str) -> Result<ChannelRecord, CoreError> {
        let record = self.channel_record(channel_id)?;
        if record.awaiting_state {
            return Err(denied("Ждём ответа администратора канала"));
        }
        if record.pending_invite {
            return Err(denied("Сначала примите приглашение в канал"));
        }
        if record.removed {
            return Err(denied("Вас удалили из канала"));
        }
        if record.left {
            return Err(denied("Вы отписались от канала"));
        }
        if record.state.closed {
            return Err(denied("Канал удалён владельцем"));
        }
        Ok(record)
    }

    fn ensure_channel_rights(
        &self,
        channel_id: &str,
        check: impl Fn(&ChannelAdminRights) -> bool,
        message: &str,
    ) -> Result<(ChannelRecord, ChannelAdminRights), CoreError> {
        let record = self.ensure_channel_active(channel_id)?;
        let rights = self
            .my_rights(&record)
            .filter(|rights| check(rights))
            .ok_or_else(|| denied(message))?;
        Ok((record, rights))
    }

    /// Публиковать можно только администратору с правом публикации.
    pub(super) fn ensure_channel_postable(&self, channel_id: &str) -> Result<ChannelRecord, CoreError> {
        self.ensure_channel_rights(
            channel_id,
            |rights| rights.post_messages,
            "Публиковать в канал могут только администраторы",
        )
        .map(|(record, _)| record)
    }

    /// Администраторы, кроме себя.
    fn channel_staff(&self, state: &ChannelState) -> Vec<String> {
        state
            .admins
            .iter()
            .filter(|value| value.user_id != self.identity.public.user_id)
            .map(|value| value.user_id.clone())
            .collect()
    }

    fn active_subscribers(&self, channel_id: &str) -> Result<Vec<ChannelSubscriber>, CoreError> {
        Ok(self
            .store
            .channel_subscribers(channel_id)?
            .into_iter()
            .filter(|value| value.status == SubscriberStatus::Active)
            .collect())
    }

    /// Все, кому уходит пост: администраторы и подписчики. Подписчиков знает только
    /// администратор, у подписчика этот список — одни администраторы.
    fn channel_audience(&self, state: &ChannelState) -> Result<Vec<String>, CoreError> {
        let me = self.me();
        let mut recipients = self.channel_staff(state);
        if admin(state, &me).is_some() {
            let mut seen: HashSet<String> = recipients.iter().cloned().collect();
            for value in self.active_subscribers(&state.channel_id)? {
                if value.user_id != me && seen.insert(value.user_id.clone()) {
                    recipients.push(value.user_id);
                }
            }
        }
        Ok(recipients)
    }

    fn queue_channel_event<T: serde::Serialize>(
        &mut self,
        channel_id: &str,
        recipients: &[String],
        event_id: &str,
        kind: &str,
        payload: &T,
    ) -> Result<SignedProtocolEvent, CoreError> {
        let event = self.sign_event(channel_id, event_id, kind, payload)?;
        for recipient in recipients {
            self.store.retry_now(recipient)?;
            self.store.enqueue_outbox(recipient, &event)?;
        }
        Ok(event)
    }

    fn new_event_id() -> String {
        format!("evt1-{}", random_hex(16))
    }

    pub(super) fn create_channel(
        &mut self,
        name: &str,
        about: &str,
        avatar_base64: Option<String>,
    ) -> Result<String, CoreError> {
        let me = self.me();
        let now = chrono::Utc::now().timestamp_millis();
        let own_name = self.store.profile()?.display_name;
        let state = ChannelState {
            version: CHANNEL_STATE_VERSION,
            channel_id: format!("ttch1-{}", random_hex(32)),
            epoch: 1,
            name: name.trim().to_owned(),
            about: about.trim().to_owned(),
            avatar_base64: avatar_base64.filter(|value| !value.is_empty()),
            created_by: me.clone(),
            created_at_unix_milliseconds: now,
            admins: vec![ChannelAdmin {
                user_id: me.clone(),
                display_name: if own_name.trim().is_empty() {
                    short_id(&me)
                } else {
                    truncate(&own_name, MAX_ADMIN_NAME_CHARS)
                },
                role: ChannelRole::Owner,
                rights: ChannelAdminRights::ALL,
                title: String::new(),
                added_by: me.clone(),
                added_at_unix_milliseconds: now,
            }],
            settings: ChannelSettings {
                sign_posts: false,
                comments_enabled: true,
                discussion_group_id: None,
                discussion_group_name: String::new(),
            },
            closed: false,
            updated_by: me.clone(),
            updated_at_unix_milliseconds: now,
        };
        validate_state(&state)?;
        let channel_id = state.channel_id.clone();
        let name = state.name.clone();
        self.store.save_channel(&ChannelRecord {
            state,
            awaiting_state: false,
            via: None,
            pending_invite: false,
            invited_by: None,
            left: false,
            removed: false,
            hidden: false,
            pinned: false,
            muted: false,
            draft: String::new(),
            manual_unread: false,
            joined_at_unix_milliseconds: now,
            subscriber_count: 0,
            subscriber_count_at_unix_milliseconds: now,
            stats_dirty: false,
            stats_sent_at_unix_milliseconds: 0,
        })?;
        self.save_channel_service(&channel_id, &Self::new_event_id(), &me, now, format!("Вы создали канал «{name}»"), true)?;
        self.selected_contact = Some(channel_id.clone());
        self.selected_thread = None;
        self.store.set_selected_contact(&self.selected_contact)?;
        self.status = "Канал создан. Пригласите подписчиков ссылкой из карточки канала".to_owned();
        Ok(channel_id)
    }

    /// Подписка по ссылке. Запрос уходит администратору из ссылки, а тот присылает
    /// состояние канала и последние посты.
    pub(super) fn subscribe_channel(&mut self, link: &str) -> Result<String, CoreError> {
        let (channel_id, via) = parse_invite_link(link)?;
        let me = self.me();
        if via == me {
            return Err(denied("Это ваша собственная ссылка"));
        }
        let now = chrono::Utc::now().timestamp_millis();
        let record = match self.store.channel(&channel_id)? {
            Some(record) if Self::is_active(&record) && !record.state.closed => {
                self.select_channel(&channel_id)?;
                self.status = "Вы уже подписаны на этот канал".to_owned();
                return Ok(channel_id);
            }
            Some(mut record) => {
                if record.state.closed {
                    return Err(denied("Канал удалён владельцем"));
                }
                record.hidden = false;
                record.left = false;
                record.removed = false;
                record.pending_invite = false;
                record.awaiting_state = record.state.epoch == 0;
                record.via = Some(via.clone());
                record.joined_at_unix_milliseconds = now;
                record
            }
            None => ChannelRecord {
                state: ChannelState {
                    version: CHANNEL_STATE_VERSION,
                    channel_id: channel_id.clone(),
                    epoch: 0,
                    name: "Канал".to_owned(),
                    about: String::new(),
                    avatar_base64: None,
                    created_by: via.clone(),
                    created_at_unix_milliseconds: 0,
                    admins: Vec::new(),
                    settings: ChannelSettings::default(),
                    closed: false,
                    updated_by: via.clone(),
                    updated_at_unix_milliseconds: 0,
                },
                awaiting_state: true,
                via: Some(via.clone()),
                pending_invite: false,
                invited_by: None,
                left: false,
                removed: false,
                hidden: false,
                pinned: false,
                muted: false,
                draft: String::new(),
                manual_unread: false,
                joined_at_unix_milliseconds: now,
                subscriber_count: 0,
                subscriber_count_at_unix_milliseconds: 0,
                stats_dirty: false,
                stats_sent_at_unix_milliseconds: 0,
            },
        };
        self.store.save_channel(&record)?;
        self.send_subscribe(&channel_id, &via)?;
        self.select_channel(&channel_id)?;
        self.deliver_now();
        self.status = if record.awaiting_state {
            "Запрос на подписку отправлен: канал появится, когда ответит администратор".to_owned()
        } else {
            "Вы снова подписаны на канал".to_owned()
        };
        Ok(channel_id)
    }

    fn select_channel(&mut self, channel_id: &str) -> Result<(), CoreError> {
        self.selected_contact = Some(channel_id.to_owned());
        self.selected_thread = None;
        self.store.set_selected_contact(&self.selected_contact)
    }

    fn send_subscribe(&mut self, channel_id: &str, via: &str) -> Result<(), CoreError> {
        let own_name = self.store.profile()?.display_name;
        let me = self.me();
        self.queue_channel_event(
            channel_id,
            &[via.to_owned()],
            &Self::new_event_id(),
            KIND_CHANNEL_SUBSCRIBE,
            &ChannelSubscribePayload {
                version: PROTOCOL_VERSION,
                channel_id: channel_id.to_owned(),
                display_name: if own_name.trim().is_empty() {
                    short_id(&me)
                } else {
                    truncate(&own_name, MAX_ADMIN_NAME_CHARS)
                },
            },
        )?;
        Ok(())
    }

    pub(super) fn accept_channel_invite(&mut self, channel_id: &str) -> Result<(), CoreError> {
        let mut record = self.channel_record(channel_id)?;
        if !record.pending_invite {
            return Ok(());
        }
        if record.state.closed {
            return Err(denied("Канал удалён владельцем"));
        }
        let inviter = record
            .invited_by
            .clone()
            .or_else(|| owner_of(&record.state).map(str::to_owned))
            .ok_or_else(|| denied("Неизвестно, кто пригласил"))?;
        let now = chrono::Utc::now().timestamp_millis();
        record.pending_invite = false;
        record.via = Some(inviter.clone());
        record.joined_at_unix_milliseconds = now;
        self.store.save_channel(&record)?;
        self.send_subscribe(channel_id, &inviter)?;
        let me = self.me();
        self.save_channel_service(channel_id, &Self::new_event_id(), &me, now, "Вы подписались на канал".to_owned(), true)?;
        self.deliver_now();
        self.status = "Вы подписались на канал".to_owned();
        Ok(())
    }

    /// Приглашение контактов: каждый получает состояние канала как запрос и решает сам.
    pub(super) fn invite_to_channel(&mut self, channel_id: &str, user_ids: &[String]) -> Result<(), CoreError> {
        let (record, _) = self.ensure_channel_rights(
            channel_id,
            |rights| rights.invite_users,
            "Недостаточно прав, чтобы приглашать подписчиков",
        )?;
        let me = self.me();
        let mut invited = Vec::new();
        for user_id in user_ids {
            if user_id == &me || invited.contains(user_id) || admin(&record.state, user_id).is_some() {
                continue;
            }
            self.store
                .contact(user_id)?
                .filter(|contact| !contact.pending_approval && is_user_id(&contact.user_id))
                .ok_or_else(|| denied("Приглашать можно только принятые контакты"))?;
            if let Some(entry) = self.store.channel_subscriber(channel_id, user_id)? {
                if entry.status == SubscriberStatus::Active {
                    continue;
                }
                if entry.status == SubscriberStatus::Banned {
                    return Err(denied("Этот пользователь заблокирован в канале"));
                }
            }
            invited.push(user_id.clone());
        }
        if invited.is_empty() {
            return Err(denied("Эти пользователи уже подписаны"));
        }
        self.queue_channel_event(
            channel_id,
            &invited,
            &Self::new_event_id(),
            KIND_CHANNEL_INVITE,
            &ChannelStatePayload {
                version: PROTOCOL_VERSION,
                state: record.state.clone(),
            },
        )?;
        self.deliver_now();
        self.status = format!("Приглашения отправлены: {}", invited.len());
        Ok(())
    }

    pub(super) fn update_channel_info(
        &mut self,
        channel_id: &str,
        name: &str,
        about: &str,
        avatar_base64: Option<String>,
    ) -> Result<(), CoreError> {
        let name = name.trim().to_owned();
        let about = about.trim().to_owned();
        let avatar = avatar_base64.filter(|value| !value.is_empty());
        self.commit_channel_state(channel_id, move |state, _| {
            state.name = name;
            state.about = about;
            state.avatar_base64 = avatar;
            Ok(())
        })?;
        self.status = "Данные канала обновлены".to_owned();
        Ok(())
    }

    pub(super) fn set_channel_settings(
        &mut self,
        channel_id: &str,
        sign_posts: bool,
        comments_enabled: bool,
    ) -> Result<(), CoreError> {
        self.commit_channel_state(channel_id, move |state, _| {
            state.settings.sign_posts = sign_posts;
            state.settings.comments_enabled = comments_enabled;
            Ok(())
        })?;
        self.status = "Настройки канала сохранены".to_owned();
        Ok(())
    }

    /// Привязка группы обсуждения. Привязать можно только группу, в которой состоишь сам:
    /// посты туда пересылает публикующий администратор.
    pub(super) fn link_discussion_group(
        &mut self,
        channel_id: &str,
        group_id: Option<String>,
    ) -> Result<(), CoreError> {
        let name = match &group_id {
            Some(group) => {
                if !is_group_id(group) {
                    return Err(denied("Это не группа"));
                }
                truncate(&self.ensure_group_writable(group)?.state.name, MAX_CHANNEL_NAME_CHARS)
            }
            None => String::new(),
        };
        let linked = group_id.is_some();
        self.commit_channel_state(channel_id, move |state, _| {
            state.settings.discussion_group_id = group_id;
            state.settings.discussion_group_name = name;
            Ok(())
        })?;
        self.status = if linked {
            "Группа обсуждения привязана".to_owned()
        } else {
            "Группа обсуждения отвязана".to_owned()
        };
        Ok(())
    }

    /// Назначение администратора или правка его прав. Кандидат — принятый контакт или
    /// действующий подписчик: у обоих уже есть проверенный адрес.
    pub(super) fn set_channel_admin(
        &mut self,
        channel_id: &str,
        user_id: &str,
        rights: ChannelAdminRights,
        title: &str,
    ) -> Result<(), CoreError> {
        let me = self.me();
        if user_id == me {
            return Err(denied("Свои права администратор не меняет"));
        }
        if !is_user_id(user_id) {
            return Err(denied("Неверный UserID"));
        }
        let title = truncate(title, MAX_ADMIN_TITLE_CHARS);
        let record = self.ensure_channel_active(channel_id)?;
        let existing = admin(&record.state, user_id).is_some();
        let display_name = if existing {
            String::new()
        } else {
            let contact = self
                .store
                .contact(user_id)?
                .filter(|contact| !contact.pending_approval)
                .map(|contact| contact.display_name);
            let subscriber = self
                .store
                .channel_subscriber(channel_id, user_id)?
                .filter(|entry| entry.status == SubscriberStatus::Active)
                .map(|entry| entry.display_name);
            truncate(
                &contact.or(subscriber).ok_or_else(|| {
                    denied("Администратором можно назначить подписчика канала или принятый контакт")
                })?,
                MAX_ADMIN_NAME_CHARS,
            )
        };
        let target = user_id.to_owned();
        let now = chrono::Utc::now().timestamp_millis();
        let changes = self.commit_channel_state(channel_id, move |state, me| {
            match state.admins.iter_mut().find(|value| value.user_id == target) {
                Some(value) => {
                    if value.role == ChannelRole::Owner {
                        return Err(denied("Права владельца не меняются"));
                    }
                    value.rights = rights;
                    value.title = title;
                }
                None => {
                    if state.admins.len() >= MAX_CHANNEL_ADMINS {
                        return Err(denied("В канале может быть не больше 50 администраторов"));
                    }
                    state.admins.push(ChannelAdmin {
                        user_id: target,
                        display_name,
                        role: ChannelRole::Admin,
                        rights,
                        title,
                        added_by: me.to_owned(),
                        added_at_unix_milliseconds: now,
                    });
                }
            }
            Ok(())
        })?;
        if changes.contains(&ChannelChange::AdminAdded(user_id.to_owned())) {
            // Новому администратору нужен список подписчиков, иначе его посты никому
            // не уйдут, и последние посты — чтобы он видел канал целиком.
            self.send_full_roster(channel_id, user_id)?;
            self.send_history(channel_id, user_id)?;
            self.deliver_now();
            self.status = "Администратор назначен".to_owned();
        } else {
            self.status = "Права администратора изменены".to_owned();
        }
        Ok(())
    }

    pub(super) fn remove_channel_admin(&mut self, channel_id: &str, user_id: &str) -> Result<(), CoreError> {
        let me = self.me();
        if user_id == me {
            return Err(denied("Чтобы сложить полномочия, отпишитесь от канала"));
        }
        let target = user_id.to_owned();
        let record = self.ensure_channel_active(channel_id)?;
        let name = admin(&record.state, user_id)
            .map(|value| value.display_name.clone())
            .ok_or_else(|| denied("Администратор не найден"))?;
        self.commit_channel_state(channel_id, move |state, _| {
            state.admins.retain(|value| value.user_id != target);
            Ok(())
        })?;
        // Разжалованный остаётся подписчиком, как в Telegram.
        let now = chrono::Utc::now().timestamp_millis();
        let entry = ChannelSubscriber {
            user_id: user_id.to_owned(),
            display_name: name,
            status: SubscriberStatus::Active,
            subscribed_at_unix_milliseconds: now,
            updated_at_unix_milliseconds: now,
            updated_by: me,
        };
        self.store.save_channel_subscriber(channel_id, &entry)?;
        self.share_roster(channel_id, &[entry])?;
        self.deliver_now();
        self.status = "Администратор разжалован".to_owned();
        Ok(())
    }

    pub(super) fn transfer_channel_ownership(&mut self, channel_id: &str, user_id: &str) -> Result<(), CoreError> {
        let target = user_id.to_owned();
        self.commit_channel_state(channel_id, move |state, me| {
            if target == me {
                return Err(denied("Вы уже владелец"));
            }
            if admin(state, me).is_none_or(|value| value.role != ChannelRole::Owner) {
                return Err(denied("Передать владение может только владелец"));
            }
            let value = state
                .admins
                .iter_mut()
                .find(|value| value.user_id == target)
                .ok_or_else(|| denied("Сначала назначьте этого пользователя администратором"))?;
            value.role = ChannelRole::Owner;
            value.rights = ChannelAdminRights::ALL;
            if let Some(own) = state.admins.iter_mut().find(|value| value.user_id == me) {
                own.role = ChannelRole::Admin;
                own.rights = ChannelAdminRights::ALL;
            }
            Ok(())
        })?;
        self.status = "Владение каналом передано".to_owned();
        Ok(())
    }

    pub(super) fn close_channel(&mut self, channel_id: &str) -> Result<(), CoreError> {
        self.commit_channel_state(channel_id, |state, me| {
            if admin(state, me).is_none_or(|value| value.role != ChannelRole::Owner) {
                return Err(denied("Удалить канал может только владелец"));
            }
            state.closed = true;
            Ok(())
        })?;
        self.status = "Канал удалён у всех подписчиков".to_owned();
        Ok(())
    }

    pub(super) fn remove_channel_subscriber(
        &mut self,
        channel_id: &str,
        user_id: &str,
        ban: bool,
    ) -> Result<(), CoreError> {
        let (record, _) = self.ensure_channel_rights(
            channel_id,
            |rights| rights.ban_users,
            "Недостаточно прав, чтобы удалять подписчиков",
        )?;
        if admin(&record.state, user_id).is_some() {
            return Err(denied("Сначала разжалуйте администратора"));
        }
        let me = self.me();
        let now = chrono::Utc::now().timestamp_millis();
        let existing = self.store.channel_subscriber(channel_id, user_id)?;
        let display_name = existing
            .as_ref()
            .map(|entry| entry.display_name.clone())
            .or_else(|| self.store.contact(user_id).ok().flatten().map(|contact| contact.display_name))
            .ok_or_else(|| denied("Подписчик не найден"))?;
        let entry = ChannelSubscriber {
            user_id: user_id.to_owned(),
            display_name: truncate(&display_name, MAX_ADMIN_NAME_CHARS),
            status: if ban { SubscriberStatus::Banned } else { SubscriberStatus::Left },
            subscribed_at_unix_milliseconds: existing
                .as_ref()
                .map_or(now, |entry| entry.subscribed_at_unix_milliseconds),
            updated_at_unix_milliseconds: now.max(
                existing
                    .as_ref()
                    .map_or(0, |entry| entry.updated_at_unix_milliseconds + 1),
            ),
            updated_by: me,
        };
        self.store.save_channel_subscriber(channel_id, &entry)?;
        if existing.is_some_and(|entry| entry.status == SubscriberStatus::Active) {
            self.queue_channel_event(
                channel_id,
                &[user_id.to_owned()],
                &Self::new_event_id(),
                KIND_CHANNEL_REMOVED,
                &ChannelNoticePayload {
                    version: PROTOCOL_VERSION,
                    channel_id: channel_id.to_owned(),
                },
            )?;
        }
        self.share_roster(channel_id, &[entry])?;
        self.mark_stats_dirty(channel_id)?;
        self.deliver_now();
        self.status = if ban {
            "Подписчик заблокирован".to_owned()
        } else {
            "Подписчик удалён".to_owned()
        };
        Ok(())
    }

    pub(super) fn unban_channel_subscriber(&mut self, channel_id: &str, user_id: &str) -> Result<(), CoreError> {
        self.ensure_channel_rights(
            channel_id,
            |rights| rights.ban_users,
            "Недостаточно прав, чтобы разблокировать подписчиков",
        )?;
        let mut entry = self
            .store
            .channel_subscriber(channel_id, user_id)?
            .filter(|entry| entry.status == SubscriberStatus::Banned)
            .ok_or_else(|| denied("Пользователь не заблокирован"))?;
        let now = chrono::Utc::now().timestamp_millis();
        entry.status = SubscriberStatus::Left;
        entry.updated_at_unix_milliseconds = now.max(entry.updated_at_unix_milliseconds + 1);
        entry.updated_by = self.me();
        self.store.save_channel_subscriber(channel_id, &entry)?;
        self.share_roster(channel_id, &[entry])?;
        self.deliver_now();
        self.status = "Пользователь разблокирован: он может подписаться снова".to_owned();
        Ok(())
    }

    /// Отписка. Администратор заодно слагает полномочия; владелец должен сначала передать
    /// канал или удалить его. `forget` ещё и убирает канал из списка с историей.
    pub(super) fn leave_channel(&mut self, channel_id: &str, forget: bool) -> Result<(), CoreError> {
        let me = self.me();
        let record = self.channel_record(channel_id)?;
        let active = Self::is_active(&record) && !record.state.closed;
        if active {
            match admin(&record.state, &me).map(|value| value.role) {
                Some(ChannelRole::Owner) => {
                    return Err(denied(
                        "Владелец не может отписаться: передайте канал другому администратору или удалите его",
                    ));
                }
                Some(ChannelRole::Admin) => {
                    self.commit_channel_state(channel_id, |state, me| {
                        state.admins.retain(|value| value.user_id != me);
                        Ok(())
                    })?;
                }
                None => {}
            }
            let record = self.channel_record(channel_id)?;
            let staff = self.channel_staff(&record.state);
            self.queue_channel_event(
                channel_id,
                &staff,
                &Self::new_event_id(),
                KIND_CHANNEL_LEAVE,
                &ChannelNoticePayload {
                    version: PROTOCOL_VERSION,
                    channel_id: channel_id.to_owned(),
                },
            )?;
            self.deliver_now();
        } else if record.awaiting_state
            && let Some(via) = record.via.clone()
        {
            // Запрос мог дойти: пусть администратор не держит нас в подписчиках.
            self.queue_channel_event(
                channel_id,
                &[via],
                &Self::new_event_id(),
                KIND_CHANNEL_LEAVE,
                &ChannelNoticePayload {
                    version: PROTOCOL_VERSION,
                    channel_id: channel_id.to_owned(),
                },
            )?;
            self.deliver_now();
        }
        let mut record = self.channel_record(channel_id)?;
        record.left = true;
        record.pending_invite = false;
        record.awaiting_state = false;
        self.store.clear_channel_subscribers(channel_id)?;
        if forget {
            record.hidden = true;
            record.draft.clear();
            self.store.save_channel(&record)?;
            self.store.clear_conversation(channel_id)?;
            self.store.clear_conversations_with_prefix(&format!("{channel_id}/"))?;
            self.store.forget_channel_data(channel_id)?;
            if self.selected_contact.as_deref() == Some(channel_id) {
                self.selected_contact = None;
                self.selected_thread = None;
                self.store.set_selected_contact(&None)?;
            }
            self.status = "Канал удалён из списка".to_owned();
        } else {
            self.store.save_channel(&record)?;
            self.status = "Вы отписались от канала".to_owned();
        }
        Ok(())
    }

    /// Прочтение канала: авторам уходят просмотры их постов.
    pub(super) fn mark_channel_read(&mut self, channel_id: &str) -> Result<(), CoreError> {
        let record = self.channel_record(channel_id)?;
        let active = Self::is_active(&record) && !record.state.closed;
        let me = self.me();
        let mut by_author: HashMap<String, Vec<String>> = HashMap::new();
        for mut message in self.store.messages(channel_id)? {
            if message.outgoing || message.read {
                continue;
            }
            message.read = true;
            self.store.save_message(&message)?;
            if message.service || message.deleted || message.sender_user_id == me {
                continue;
            }
            by_author
                .entry(message.sender_user_id.clone())
                .or_default()
                .push(message.event_id.clone());
        }
        if active && !by_author.is_empty() {
            for (author, posts) in by_author {
                if admin(&record.state, &author).is_none() {
                    continue;
                }
                for chunk in posts.chunks(200) {
                    self.queue_channel_event(
                        channel_id,
                        std::slice::from_ref(&author),
                        &Self::new_event_id(),
                        KIND_CHANNEL_VIEWS,
                        &ChannelViewsPayload {
                            version: PROTOCOL_VERSION,
                            post_event_ids: chunk.to_vec(),
                        },
                    )?;
                }
            }
            self.deliver_now();
        }
        if record.manual_unread {
            let mut record = self.channel_record(channel_id)?;
            record.manual_unread = false;
            self.store.save_channel(&record)?;
        }
        Ok(())
    }

    pub(super) fn open_comments(&mut self, post_event_id: Option<String>) -> Result<(), CoreError> {
        let Some(post_event_id) = post_event_id else {
            self.selected_thread = None;
            return Ok(());
        };
        let post = self.require_message(&post_event_id)?;
        if !is_channel_id(&post.conversation_id) || post.service {
            return Err(denied("Комментарии есть только у постов канала"));
        }
        self.channel_record(&post.conversation_id)?;
        self.selected_contact = Some(post.conversation_id.clone());
        self.store.set_selected_contact(&self.selected_contact)?;
        self.selected_thread = Some(post_event_id);
        Ok(())
    }

    /// Кому подписчик отдаёт свой голос по посту: автору, если тот ещё администратор,
    /// иначе владельцу.
    fn relayer_for(&self, state: &ChannelState, post_author: &str) -> Option<String> {
        if post_author != self.identity.public.user_id && admin(state, post_author).is_some() {
            return Some(post_author.to_owned());
        }
        owner_of(state)
            .filter(|owner| *owner != self.identity.public.user_id)
            .map(str::to_owned)
    }

    pub(super) fn send_comment(
        &mut self,
        post_event_id: &str,
        text: &str,
        reply_to_event_id: Option<String>,
    ) -> Result<(), CoreError> {
        let text = text.trim();
        if text.is_empty() {
            return Err(denied("Комментарий пустой"));
        }
        if text.chars().count() > MAX_COMMENT_CHARS {
            return Err(denied("Комментарий: максимум 4096 символов"));
        }
        let post = self.require_message(post_event_id)?;
        if !is_channel_id(&post.conversation_id) || post.service || post.deleted {
            return Err(denied("Комментировать можно только пост канала"));
        }
        let channel_id = post.conversation_id.clone();
        let record = self.ensure_channel_active(&channel_id)?;
        if !record.state.settings.comments_enabled {
            return Err(denied("Комментарии в этом канале выключены"));
        }
        let me = self.me();
        let thread = thread_id(&channel_id, post_event_id);
        let reply_to = reply_to_event_id.filter(|id| {
            self.store
                .message(id)
                .ok()
                .flatten()
                .is_some_and(|message| message.conversation_id == thread)
        });
        let own_name = self.store.profile()?.display_name;
        let payload = ChannelCommentPayload {
            version: PROTOCOL_VERSION,
            post_event_id: post_event_id.to_owned(),
            text: text.to_owned(),
            reply_to_event_id: reply_to.clone(),
            author_name: if own_name.trim().is_empty() {
                short_id(&me)
            } else {
                truncate(&own_name, MAX_ADMIN_NAME_CHARS)
            },
        };
        let event_id = Self::new_event_id();
        // Администратор знает всех и рассылает сам; подписчик отдаёт комментарий тому,
        // кто его разнесёт.
        let recipients = if admin(&record.state, &me).is_some() {
            self.channel_audience(&record.state)?
        } else {
            self.relayer_for(&record.state, &post.sender_user_id)
                .into_iter()
                .collect()
        };
        self.queue_channel_event(&channel_id, &recipients, &event_id, KIND_CHANNEL_COMMENT, &payload)?;
        self.store.save_message(&Message {
            event_id,
            conversation_id: thread,
            sender_user_id: me,
            text: text.to_owned(),
            created_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
            outgoing: true,
            edited: false,
            deleted: false,
            reactions: Vec::new(),
            delivered: recipients.is_empty(),
            read: true,
            pinned: false,
            attachment: None,
            reply_to_event_id: reply_to,
            forwarded_from: None,
            service: false,
            reaction_marks: Vec::new(),
            sender_name: None,
            channel_post: None,
        })?;
        if post.outgoing {
            self.mark_stats_dirty(&channel_id)?;
        }
        self.deliver_now();
        Ok(())
    }

    /// Пост ушёл в очередь: разослать его всем и сохранить подписанный оригинал для
    /// новых подписчиков. Вызывается из общего пути отправки.
    pub(super) fn queue_channel_post<T: serde::Serialize>(
        &mut self,
        channel_id: &str,
        event_id: &str,
        kind: &str,
        payload: &T,
    ) -> Result<(), CoreError> {
        let record = self.ensure_channel_postable(channel_id)?;
        let recipients = self.channel_audience(&record.state)?;
        let event = self.queue_channel_event(channel_id, &recipients, event_id, kind, payload)?;
        if recipients.is_empty() {
            self.store.mark_delivered(event_id)?;
        }
        self.store.save_channel_post(
            channel_id,
            event.created_at_unix_milliseconds,
            &StoredChannelPost {
                post: RelayedEvent {
                    identity: self.own_wire(),
                    event,
                },
                edit: None,
            },
        )?;
        // Привязанная группа получает пост как пересланный — если автор в ней состоит.
        if let Some(group) = record.state.settings.discussion_group_id.clone()
            && self.ensure_group_writable(&group).is_ok()
            && let Err(error) = self.forward_into(std::slice::from_ref(&event_id.to_owned()), &group)
        {
            self.status = format!("Пост не переслан в обсуждение: {error}");
        }
        Ok(())
    }

    /// Правка поста: автором или администратором с правом редактировать чужие.
    pub(super) fn edit_channel_post(&mut self, mut message: Message, text: &str) -> Result<(), CoreError> {
        let channel_id = message.conversation_id.clone();
        if message.deleted || message.service {
            return Err(denied("Пост нельзя изменить"));
        }
        let (record, rights) = self.ensure_channel_rights(
            &channel_id,
            |rights| rights.post_messages || rights.edit_messages,
            "Редактировать посты могут только администраторы",
        )?;
        if !message.outgoing && !rights.edit_messages {
            return Err(denied("Недостаточно прав, чтобы редактировать чужие посты"));
        }
        message.text = text.trim().to_owned();
        message.edited = true;
        self.store.save_message(&message)?;
        let recipients = self.channel_audience(&record.state)?;
        let event = self.queue_channel_event(
            &channel_id,
            &recipients,
            &Self::new_event_id(),
            KIND_EDIT,
            &EditPayload {
                version: PROTOCOL_VERSION,
                target_event_id: message.event_id.clone(),
                text: message.text.clone(),
            },
        )?;
        if let Some(mut stored) = self.store.channel_post(&message.event_id)? {
            stored.edit = Some(RelayedEvent {
                identity: self.own_wire(),
                event,
            });
            self.store.save_channel_post(&channel_id, message.created_at_unix_milliseconds, &stored)?;
        }
        self.deliver_now();
        self.status = "Пост изменён".to_owned();
        Ok(())
    }

    /// Удаление поста или комментария по команде пользователя.
    pub(super) fn delete_channel_item(&mut self, message: Message) -> Result<(), CoreError> {
        let me = self.me();
        match item_of(&message) {
            Some(ChannelItem::Post { channel_id }) => {
                if message.deleted || message.service {
                    return Ok(());
                }
                let record = self.channel_record(&channel_id)?;
                let rights = self.my_rights(&record);
                let allowed = match rights {
                    Some(rights) => message.outgoing || rights.delete_messages,
                    None => false,
                };
                if !allowed {
                    // Подписчик может убрать пост только у себя.
                    self.store.delete_message(&message.event_id)?;
                    return Ok(());
                }
                self.tombstone_post(&channel_id, &message.event_id)?;
                let recipients = self.channel_audience(&record.state)?;
                self.queue_channel_event(
                    &channel_id,
                    &recipients,
                    &Self::new_event_id(),
                    KIND_DELETE,
                    &TargetPayload {
                        version: PROTOCOL_VERSION,
                        target_event_id: message.event_id.clone(),
                    },
                )?;
            }
            Some(ChannelItem::Comment { channel_id, post_event_id }) => {
                let record = self.channel_record(&channel_id)?;
                let rights = self.my_rights(&record);
                let mine = message.sender_user_id == me;
                if !mine && !rights.is_some_and(|rights| rights.delete_messages) {
                    return Err(denied("Недостаточно прав, чтобы удалять чужие комментарии"));
                }
                self.store.delete_message(&message.event_id)?;
                if Self::is_active(&record) && !record.state.closed {
                    let recipients = if rights.is_some() {
                        self.channel_audience(&record.state)?
                    } else {
                        let author = self
                            .store
                            .message(&post_event_id)?
                            .map(|post| post.sender_user_id)
                            .unwrap_or_default();
                        self.relayer_for(&record.state, &author).into_iter().collect()
                    };
                    self.queue_channel_event(
                        &channel_id,
                        &recipients,
                        &Self::new_event_id(),
                        KIND_CHANNEL_COMMENT_DELETE,
                        &ChannelCommentDeletePayload {
                            version: PROTOCOL_VERSION,
                            post_event_id,
                            target_event_id: message.event_id.clone(),
                        },
                    )?;
                }
            }
            None => {}
        }
        Ok(())
    }

    fn tombstone_post(&mut self, channel_id: &str, event_id: &str) -> Result<(), CoreError> {
        if let Some(mut message) = self.store.message(event_id)? {
            message.deleted = true;
            message.text.clear();
            message.attachment = None;
            message.reaction_marks.clear();
            message.reactions.clear();
            self.store.save_message(&message)?;
        }
        self.store.delete_channel_post(event_id)?;
        self.store.clear_conversation(&thread_id(channel_id, event_id))?;
        if self.selected_thread.as_deref() == Some(event_id) {
            self.selected_thread = None;
        }
        Ok(())
    }

    /// Реакция на пост уходит только автору: он считает и рассылает итог.
    pub(super) fn react_channel_post(&mut self, mut message: Message, reaction: &str) -> Result<(), CoreError> {
        let channel_id = message.conversation_id.clone();
        if !is_channel_id(&channel_id) || message.deleted || message.service {
            return Ok(());
        }
        let record = self.ensure_channel_active(&channel_id)?;
        let me = self.me();
        let active = set_reaction_mark(&mut message, &me, reaction, None);
        self.store.save_message(&message)?;
        if message.outgoing || message.sender_user_id == me {
            self.store.set_channel_reaction(&message.event_id, &me, reaction, active)?;
            self.mark_stats_dirty(&channel_id)?;
            return Ok(());
        }
        if admin(&record.state, &message.sender_user_id).is_some() {
            self.queue_channel_event(
                &channel_id,
                std::slice::from_ref(&message.sender_user_id),
                &Self::new_event_id(),
                KIND_REACTION,
                &ReactionPayload {
                    version: PROTOCOL_VERSION,
                    target_event_id: message.event_id.clone(),
                    reaction: reaction.to_owned(),
                    active,
                },
            )?;
        }
        Ok(())
    }

    fn mark_stats_dirty(&mut self, channel_id: &str) -> Result<(), CoreError> {
        if let Some(mut record) = self.store.channel(channel_id)?
            && !record.stats_dirty
        {
            record.stats_dirty = true;
            self.store.save_channel(&record)?;
        }
        Ok(())
    }

    /// Локальное изменение канала: та же проверка, что у получателей, затем рассылка
    /// нового состояния администраторам (прежним и новым) и подписчикам.
    fn commit_channel_state(
        &mut self,
        channel_id: &str,
        mutate: impl FnOnce(&mut ChannelState, &str) -> Result<(), CoreError>,
    ) -> Result<Vec<ChannelChange>, CoreError> {
        let me = self.me();
        let mut record = self.ensure_channel_active(channel_id)?;
        if admin(&record.state, &me).is_none() {
            return Err(denied("Менять канал могут только администраторы"));
        }
        let old = record.state.clone();
        let mut new = old.clone();
        mutate(&mut new, &me)?;
        let now = chrono::Utc::now().timestamp_millis();
        new.epoch = old.epoch + 1;
        new.updated_by = me.clone();
        new.updated_at_unix_milliseconds = now.max(old.updated_at_unix_milliseconds);
        let changes = validate_transition(&old, &new, &me, false)?;

        let mut recipients = self.channel_audience(&old)?;
        for value in self.channel_staff(&new) {
            if !recipients.contains(&value) {
                recipients.push(value);
            }
        }
        let event_id = Self::new_event_id();
        self.queue_channel_event(
            channel_id,
            &recipients,
            &event_id,
            KIND_CHANNEL_STATE,
            &ChannelStatePayload {
                version: PROTOCOL_VERSION,
                state: new.clone(),
            },
        )?;
        record.state = new;
        self.store.save_channel(&record)?;
        if admin(&record.state, &me).is_none() {
            self.store.clear_channel_subscribers(channel_id)?;
        }
        self.record_channel_changes(&record, &event_id, &me, now, &changes)?;
        self.deliver_now();
        Ok(changes)
    }

    /// Событие канала от другого пользователя. `sender` — проверенная личность отправителя.
    /// Возвращает `true`, если в ленте что-то появилось или изменилось.
    pub(super) fn apply_channel_event(
        &mut self,
        event: &SignedProtocolEvent,
        sender: &WireIdentity,
    ) -> Result<bool, CoreError> {
        let channel_id = event.conversation_id.clone();
        match event.kind.as_str() {
            KIND_CHANNEL_STATE | KIND_CHANNEL_INVITE => {
                let payload: ChannelStatePayload = event.decode_payload()?;
                if payload.state.channel_id != channel_id {
                    return Err(CoreError::Crypto("Состояние относится к другому каналу".to_owned()));
                }
                if event.kind == KIND_CHANNEL_INVITE {
                    self.apply_channel_invite(event, payload.state)
                } else {
                    self.apply_remote_channel_state(
                        &event.event_id,
                        &event.sender_user_id,
                        event.created_at_unix_milliseconds,
                        payload.state,
                    )
                }
            }
            KIND_CHANNEL_SUBSCRIBE => self.handle_subscribe(event),
            KIND_CHANNEL_LEAVE => self.handle_leave(event),
            KIND_CHANNEL_REMOVED => self.handle_removed(event),
            KIND_CHANNEL_ROSTER => self.apply_roster(event),
            KIND_CHANNEL_RELAY => self.apply_relay(event),
            KIND_CHANNEL_VIEWS => self.apply_views(event),
            KIND_CHANNEL_STATS => self.apply_stats(event),
            _ => self.apply_channel_content(event, sender, None),
        }
    }

    /// Канал, в котором пользователь сейчас действует, — для входящих событий.
    fn live_channel(&self, channel_id: &str) -> Result<Option<ChannelRecord>, CoreError> {
        Ok(self
            .store
            .channel(channel_id)?
            .filter(Self::is_active))
    }

    fn apply_channel_invite(
        &mut self,
        event: &SignedProtocolEvent,
        state: ChannelState,
    ) -> Result<bool, CoreError> {
        let actor = event.sender_user_id.clone();
        if let Some(existing) = self.store.channel(&state.channel_id)?
            && (Self::is_active(&existing) || existing.awaiting_state)
        {
            return Ok(false);
        }
        if let Err(error) = validate_state(&state) {
            self.status = format!("Приглашение в канал отклонено: {error}");
            return Ok(false);
        }
        if state.closed || !rights_of(&state, &actor).is_some_and(|rights| rights.invite_users) {
            return Ok(false);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let inviter = self.channel_member_name(&state, &actor);
        let channel_id = state.channel_id.clone();
        let name = state.name.clone();
        let previous = self.store.channel(&channel_id)?;
        let mut record = previous.unwrap_or(ChannelRecord {
            state: state.clone(),
            awaiting_state: false,
            via: None,
            pending_invite: true,
            invited_by: None,
            left: false,
            removed: false,
            hidden: false,
            pinned: false,
            muted: false,
            draft: String::new(),
            manual_unread: false,
            joined_at_unix_milliseconds: now,
            subscriber_count: 0,
            subscriber_count_at_unix_milliseconds: 0,
            stats_dirty: false,
            stats_sent_at_unix_milliseconds: 0,
        });
        if record.state.epoch <= state.epoch {
            record.state = state;
        }
        record.pending_invite = true;
        record.invited_by = Some(actor.clone());
        record.left = false;
        record.removed = false;
        record.hidden = false;
        record.joined_at_unix_milliseconds = now;
        self.store.save_channel(&record)?;
        self.save_channel_service(
            &channel_id,
            &event.event_id,
            &actor,
            event.created_at_unix_milliseconds,
            format!("{inviter} приглашает вас в канал «{name}»"),
            false,
        )?;
        Ok(true)
    }

    fn apply_remote_channel_state(
        &mut self,
        event_id: &str,
        actor: &str,
        created_at: i64,
        state: ChannelState,
    ) -> Result<bool, CoreError> {
        let me = self.me();
        let now = chrono::Utc::now().timestamp_millis();
        let Some(mut record) = self.store.channel(&state.channel_id)? else {
            // Незнакомый канал интересен, только если нас в нём назначили администратором.
            if admin(&state, &me).is_none()
                || admin(&state, actor).is_none()
                || validate_state(&state).is_err()
                || state.closed
            {
                return Ok(false);
            }
            let channel_id = state.channel_id.clone();
            let name = state.name.clone();
            let appointer = self.channel_member_name(&state, actor);
            self.store.save_channel(&ChannelRecord {
                state,
                awaiting_state: false,
                via: Some(actor.to_owned()),
                pending_invite: false,
                invited_by: None,
                left: false,
                removed: false,
                hidden: false,
                pinned: false,
                muted: false,
                draft: String::new(),
                manual_unread: false,
                joined_at_unix_milliseconds: now,
                subscriber_count: 0,
                subscriber_count_at_unix_milliseconds: 0,
                stats_dirty: false,
                stats_sent_at_unix_milliseconds: 0,
            })?;
            self.save_channel_service(
                &channel_id,
                event_id,
                actor,
                created_at,
                format!("{appointer} назначил(а) вас администратором канала «{name}»"),
                false,
            )?;
            return Ok(true);
        };
        if record.hidden || record.left || record.removed {
            return Ok(false);
        }
        if record.awaiting_state || record.state.epoch == 0 {
            // Ответ на подписку по ссылке: состояние присылает администратор с правом
            // приглашать — сверить его пока не с чем.
            if validate_state(&state).is_err()
                || !rights_of(&state, actor).is_some_and(|rights| rights.invite_users)
            {
                self.status = "Канал прислал повреждённое состояние".to_owned();
                return Ok(false);
            }
            let channel_id = state.channel_id.clone();
            let name = state.name.clone();
            let closed = state.closed;
            record.state = state;
            record.awaiting_state = false;
            record.via = Some(actor.to_owned());
            self.store.save_channel(&record)?;
            self.save_channel_service(
                &channel_id,
                event_id,
                actor,
                created_at,
                if closed {
                    format!("Канал «{name}» удалён владельцем")
                } else {
                    format!("Вы подписались на канал «{name}»")
                },
                true,
            )?;
            self.retry_pending_channel_states(&channel_id)?;
            return Ok(true);
        }
        if state.epoch < record.state.epoch || state == record.state {
            return Ok(false);
        }
        let equal = state.epoch == record.state.epoch;
        if equal && state_hash(&state) <= state_hash(&record.state) {
            return Ok(false);
        }
        let changes = match validate_transition(&record.state, &state, actor, equal) {
            Ok(changes) => changes,
            Err(_) if !equal && state.epoch > record.state.epoch + 1 => {
                self.store.save_pending_channel_state(&PendingChannelState {
                    event_id: event_id.to_owned(),
                    actor: actor.to_owned(),
                    created_at_unix_milliseconds: created_at,
                    state,
                })?;
                return Ok(false);
            }
            Err(error) => {
                self.status = format!("Изменение канала отклонено: {error}");
                return Ok(false);
            }
        };
        let channel_id = state.channel_id.clone();
        self.commit_remote_channel_state(&mut record, state, event_id, actor, created_at, &changes)?;
        self.retry_pending_channel_states(&channel_id)?;
        Ok(true)
    }

    fn commit_remote_channel_state(
        &mut self,
        record: &mut ChannelRecord,
        state: ChannelState,
        event_id: &str,
        actor: &str,
        created_at: i64,
        changes: &[ChannelChange],
    ) -> Result<(), CoreError> {
        record.state = state;
        self.store.save_channel(record)?;
        if admin(&record.state, &self.identity.public.user_id).is_none() {
            // Бывшему администратору список подписчиков больше не положен.
            self.store.clear_channel_subscribers(&record.state.channel_id)?;
        }
        self.record_channel_changes(record, event_id, actor, created_at, changes)
    }

    fn retry_pending_channel_states(&mut self, channel_id: &str) -> Result<(), CoreError> {
        let expired_before = chrono::Utc::now().timestamp_millis() - PENDING_STATE_TTL_MILLISECONDS;
        self.store.prune_pending_channel_states(expired_before)?;
        loop {
            let Some(mut record) = self.store.channel(channel_id)? else {
                return Ok(());
            };
            if !Self::is_active(&record) && !record.pending_invite {
                self.store.clear_pending_channel_states(channel_id)?;
                return Ok(());
            }
            let mut progressed = false;
            for pending in self.store.pending_channel_states(channel_id)? {
                if pending.state.epoch <= record.state.epoch {
                    self.store.delete_pending_channel_state(&pending.event_id)?;
                    continue;
                }
                match validate_transition(&record.state, &pending.state, &pending.actor, false) {
                    Ok(changes) => {
                        self.store.delete_pending_channel_state(&pending.event_id)?;
                        self.commit_remote_channel_state(
                            &mut record,
                            pending.state,
                            &pending.event_id,
                            &pending.actor,
                            pending.created_at_unix_milliseconds,
                            &changes,
                        )?;
                        progressed = true;
                        break;
                    }
                    Err(_) if pending.state.epoch == record.state.epoch + 1 => {
                        self.store.delete_pending_channel_state(&pending.event_id)?;
                    }
                    Err(_) => {}
                }
            }
            if !progressed {
                return Ok(());
            }
        }
    }

    /// Служебные отметки. Подписчику интересно немногое: название, фото, закрытие канала
    /// и то, что касается его самого.
    fn record_channel_changes(
        &mut self,
        record: &ChannelRecord,
        event_id: &str,
        actor: &str,
        created_at: i64,
        changes: &[ChannelChange],
    ) -> Result<(), CoreError> {
        let me = self.me();
        let by_me = actor == me;
        for (index, change) in changes.iter().enumerate() {
            let (text, notable) = match change {
                ChannelChange::Renamed(name) => (format!("Канал переименован в «{name}»"), false),
                ChannelChange::AvatarChanged => ("Фото канала обновлено".to_owned(), false),
                ChannelChange::Closed => ("Канал удалён владельцем".to_owned(), !by_me),
                ChannelChange::AdminAdded(id) if id == &me => {
                    ("Вас назначили администратором канала".to_owned(), true)
                }
                ChannelChange::AdminRemoved(id) if id == &me => {
                    ("Вы больше не администратор канала".to_owned(), true)
                }
                ChannelChange::AdminEdited(id) if id == &me => {
                    ("Ваши права администратора изменены".to_owned(), false)
                }
                ChannelChange::OwnershipTransferred(id) if id == &me => {
                    ("Вы стали владельцем канала".to_owned(), true)
                }
                ChannelChange::AdminAdded(id) if by_me => (
                    format!("Вы назначили {} администратором", self.channel_member_name(&record.state, id)),
                    false,
                ),
                ChannelChange::OwnershipTransferred(id) if by_me => (
                    format!("Вы передали канал: владелец теперь {}", self.channel_member_name(&record.state, id)),
                    false,
                ),
                ChannelChange::Resigned if by_me => ("Вы сложили полномочия администратора".to_owned(), false),
                _ => continue,
            };
            let id = if index == 0 {
                event_id.to_owned()
            } else {
                format!("{event_id}.{index}")
            };
            self.save_channel_service(&record.state.channel_id, &id, actor, created_at, text, !notable)?;
        }
        Ok(())
    }

    fn save_channel_service(
        &mut self,
        channel_id: &str,
        event_id: &str,
        actor: &str,
        created_at: i64,
        text: String,
        read: bool,
    ) -> Result<(), CoreError> {
        let outgoing = actor == self.identity.public.user_id;
        self.store.save_message(&Message {
            event_id: event_id.to_owned(),
            conversation_id: channel_id.to_owned(),
            sender_user_id: actor.to_owned(),
            text,
            created_at_unix_milliseconds: created_at,
            outgoing,
            edited: false,
            deleted: false,
            reactions: Vec::new(),
            delivered: true,
            read: read || outgoing,
            pinned: false,
            attachment: None,
            reply_to_event_id: None,
            forwarded_from: None,
            service: true,
            reaction_marks: Vec::new(),
            sender_name: None,
            channel_post: None,
        })
    }

    /// Запрос на подписку: его обрабатывает администратор с правом приглашать.
    fn handle_subscribe(&mut self, event: &SignedProtocolEvent) -> Result<bool, CoreError> {
        let payload: ChannelSubscribePayload = event.decode_payload()?;
        let channel_id = event.conversation_id.clone();
        let user = event.sender_user_id.clone();
        if payload.channel_id != channel_id {
            return Ok(false);
        }
        let Some(record) = self.live_channel(&channel_id)? else {
            return Ok(false);
        };
        if !self.my_rights(&record).is_some_and(|rights| rights.invite_users)
            || admin(&record.state, &user).is_some()
        {
            return Ok(false);
        }
        let existing = self.store.channel_subscriber(&channel_id, &user)?;
        if existing
            .as_ref()
            .is_some_and(|entry| entry.status == SubscriberStatus::Banned)
        {
            return Ok(false);
        }
        let already = existing
            .as_ref()
            .is_some_and(|entry| entry.status == SubscriberStatus::Active);
        if !already && self.active_subscribers(&channel_id)?.len() >= MAX_CHANNEL_SUBSCRIBERS {
            self.status = "В канале уже максимум подписчиков".to_owned();
            return Ok(false);
        }
        let now = chrono::Utc::now().timestamp_millis();
        let entry = ChannelSubscriber {
            user_id: user.clone(),
            display_name: truncate(&payload.display_name, MAX_ADMIN_NAME_CHARS),
            status: SubscriberStatus::Active,
            subscribed_at_unix_milliseconds: existing
                .as_ref()
                .filter(|_| already)
                .map_or(now, |entry| entry.subscribed_at_unix_milliseconds),
            updated_at_unix_milliseconds: now.max(
                existing
                    .as_ref()
                    .map_or(0, |entry| entry.updated_at_unix_milliseconds + 1),
            ),
            updated_by: user.clone(),
        };
        self.store.save_channel_subscriber(&channel_id, &entry)?;
        self.queue_channel_event(
            &channel_id,
            std::slice::from_ref(&user),
            &Self::new_event_id(),
            KIND_CHANNEL_STATE,
            &ChannelStatePayload {
                version: PROTOCOL_VERSION,
                state: record.state.clone(),
            },
        )?;
        self.send_history(&channel_id, &user)?;
        self.send_stats(&channel_id, std::slice::from_ref(&user))?;
        self.share_roster(&channel_id, &[entry])?;
        self.mark_stats_dirty(&channel_id)?;
        Ok(false)
    }

    fn handle_leave(&mut self, event: &SignedProtocolEvent) -> Result<bool, CoreError> {
        let channel_id = event.conversation_id.clone();
        let Some(record) = self.live_channel(&channel_id)? else {
            return Ok(false);
        };
        if self.my_rights(&record).is_none() {
            return Ok(false);
        }
        let Some(mut entry) = self.store.channel_subscriber(&channel_id, &event.sender_user_id)? else {
            return Ok(false);
        };
        if entry.status != SubscriberStatus::Active {
            return Ok(false);
        }
        entry.status = SubscriberStatus::Left;
        entry.updated_at_unix_milliseconds = event
            .created_at_unix_milliseconds
            .max(entry.updated_at_unix_milliseconds + 1);
        entry.updated_by = event.sender_user_id.clone();
        self.store.save_channel_subscriber(&channel_id, &entry)?;
        self.mark_stats_dirty(&channel_id)?;
        Ok(false)
    }

    fn handle_removed(&mut self, event: &SignedProtocolEvent) -> Result<bool, CoreError> {
        let channel_id = event.conversation_id.clone();
        let Some(mut record) = self.live_channel(&channel_id)? else {
            return Ok(false);
        };
        if !rights_of(&record.state, &event.sender_user_id).is_some_and(|rights| rights.ban_users)
            || admin(&record.state, &self.identity.public.user_id).is_some()
        {
            return Ok(false);
        }
        record.removed = true;
        record.left = true;
        self.store.save_channel(&record)?;
        let actor = event.sender_user_id.clone();
        self.save_channel_service(
            &channel_id,
            &event.event_id,
            &actor,
            event.created_at_unix_milliseconds,
            "Администратор удалил вас из подписчиков канала".to_owned(),
            false,
        )?;
        Ok(true)
    }

    fn apply_roster(&mut self, event: &SignedProtocolEvent) -> Result<bool, CoreError> {
        let payload: ChannelRosterPayload = event.decode_payload()?;
        let channel_id = event.conversation_id.clone();
        if payload.channel_id != channel_id {
            return Ok(false);
        }
        let Some(record) = self.live_channel(&channel_id)? else {
            return Ok(false);
        };
        let Some(sender_rights) = rights_of(&record.state, &event.sender_user_id) else {
            return Ok(false);
        };
        if self.my_rights(&record).is_none() {
            return Ok(false);
        }
        let mut changed = false;
        for entry in payload.entries.into_iter().take(ROSTER_CHUNK * 10) {
            if !is_user_id(&entry.user_id)
                || !is_user_id(&entry.updated_by)
                || entry.display_name.chars().count() > MAX_ADMIN_NAME_CHARS
                || entry.user_id == self.identity.public.user_id
            {
                continue;
            }
            // Удалить или заблокировать чужого подписчика вправе только тот, у кого есть
            // право блокировки; отписку сам подписчик присылает всем администраторам.
            let by_other = entry.updated_by != entry.user_id;
            if entry.status != SubscriberStatus::Active && by_other && !sender_rights.ban_users {
                continue;
            }
            let existing = self.store.channel_subscriber(&channel_id, &entry.user_id)?;
            if newer(&entry, existing.as_ref()) {
                self.store.save_channel_subscriber(&channel_id, &entry)?;
                changed = true;
            }
        }
        if changed {
            self.mark_stats_dirty(&channel_id)?;
        }
        Ok(false)
    }

    /// Пересланные администратором чужие события: история, комментарии подписчиков.
    fn apply_relay(&mut self, event: &SignedProtocolEvent) -> Result<bool, CoreError> {
        let payload: ChannelRelayPayload = event.decode_payload()?;
        let channel_id = event.conversation_id.clone();
        if payload.channel_id != channel_id {
            return Ok(false);
        }
        let Some(record) = self.live_channel(&channel_id)? else {
            return Ok(false);
        };
        let Some(relayer_rights) = rights_of(&record.state, &event.sender_user_id) else {
            return Ok(false);
        };
        let mut applied = false;
        for relayed in payload.events.into_iter().take(200) {
            let inner = &relayed.event;
            if inner.conversation_id != channel_id
                || !matches!(
                    inner.kind.as_str(),
                    KIND_TEXT | KIND_ATTACHMENT | KIND_EDIT | KIND_CHANNEL_COMMENT | KIND_CHANNEL_COMMENT_DELETE
                )
                || !inner.verify(&relayed.identity)
            {
                continue;
            }
            if inner.sender_user_id == self.identity.public.user_id
                || !self.store.mark_seen(&inner.event_id)?
            {
                continue;
            }
            if self.apply_channel_content(inner, &relayed.identity, Some(relayer_rights))? {
                applied = true;
            }
        }
        Ok(applied)
    }

    /// Пост, правка, удаление, реакция или комментарий. `relayed_by` — права
    /// переславшего администратора, если событие пришло не от автора напрямую.
    fn apply_channel_content(
        &mut self,
        event: &SignedProtocolEvent,
        identity: &WireIdentity,
        relayed_by: Option<ChannelAdminRights>,
    ) -> Result<bool, CoreError> {
        let channel_id = event.conversation_id.clone();
        let Some(record) = self.live_channel(&channel_id)? else {
            return Ok(false);
        };
        let sender = event.sender_user_id.clone();
        let sender_rights = rights_of(&record.state, &sender);
        let find_post = |core: &Self, event_id: &str| -> Result<Option<Message>, CoreError> {
            Ok(core
                .store
                .message(event_id)?
                .filter(|message| message.conversation_id == channel_id && !message.service))
        };
        match event.kind.as_str() {
            KIND_TEXT | KIND_ATTACHMENT => {
                // История от администратора с правом публикации принимается и от бывших
                // авторов: подпись всё равно их.
                let allowed = sender_rights.is_some_and(|rights| rights.post_messages)
                    || relayed_by.is_some_and(|rights| rights.post_messages);
                if record.state.closed || !allowed {
                    return Ok(false);
                }
                if self.store.message(&event.event_id)?.is_some() {
                    return Ok(false);
                }
                let applied = self.apply_message_event(event, &channel_id, None)?;
                if applied {
                    self.store.save_channel_post(
                        &channel_id,
                        event.created_at_unix_milliseconds,
                        &StoredChannelPost {
                            post: RelayedEvent {
                                identity: identity.clone(),
                                event: event.clone(),
                            },
                            edit: None,
                        },
                    )?;
                }
                Ok(applied)
            }
            KIND_EDIT => {
                let payload: EditPayload = event.decode_payload()?;
                let Some(mut message) = find_post(self, &payload.target_event_id)? else {
                    return Ok(false);
                };
                let own = message.sender_user_id == sender;
                let allowed = (own && (sender_rights.is_some() || relayed_by.is_some()))
                    || sender_rights.is_some_and(|rights| rights.edit_messages);
                if message.deleted || !allowed {
                    return Ok(false);
                }
                message.text = payload.text;
                message.edited = true;
                self.store.save_message(&message)?;
                if let Some(mut stored) = self.store.channel_post(&message.event_id)? {
                    stored.edit = Some(RelayedEvent {
                        identity: identity.clone(),
                        event: event.clone(),
                    });
                    self.store
                        .save_channel_post(&channel_id, message.created_at_unix_milliseconds, &stored)?;
                }
                Ok(true)
            }
            KIND_DELETE => {
                let payload: TargetPayload = event.decode_payload()?;
                let Some(message) = find_post(self, &payload.target_event_id)? else {
                    return Ok(false);
                };
                let allowed = (message.sender_user_id == sender && sender_rights.is_some())
                    || sender_rights.is_some_and(|rights| rights.delete_messages);
                if !allowed {
                    return Ok(false);
                }
                self.tombstone_post(&channel_id, &message.event_id)?;
                Ok(true)
            }
            KIND_REACTION => {
                let payload: ReactionPayload = event.decode_payload()?;
                if !super::ALLOWED_REACTIONS.contains(&payload.reaction.as_str()) {
                    return Ok(false);
                }
                let Some(message) = find_post(self, &payload.target_event_id)? else {
                    return Ok(false);
                };
                if !message.outgoing || message.deleted || !self.is_audience(&record, &sender)? {
                    return Ok(false);
                }
                if self
                    .store
                    .set_channel_reaction(&message.event_id, &sender, &payload.reaction, payload.active)?
                {
                    self.mark_stats_dirty(&channel_id)?;
                }
                Ok(false)
            }
            KIND_CHANNEL_COMMENT => {
                let payload: ChannelCommentPayload = event.decode_payload()?;
                let text = payload.text.trim();
                if text.is_empty() || text.chars().count() > MAX_COMMENT_CHARS {
                    return Ok(false);
                }
                let Some(post) = find_post(self, &payload.post_event_id)? else {
                    return Ok(false);
                };
                if post.deleted {
                    return Ok(false);
                }
                let thread = thread_id(&channel_id, &post.event_id);
                if self.store.message(&event.event_id)?.is_some() {
                    return Ok(false);
                }
                if relayed_by.is_none() && sender_rights.is_none() {
                    // Подписчик просит разнести его комментарий.
                    if self.my_rights(&record).is_none()
                        || !record.state.settings.comments_enabled
                        || !self.is_audience(&record, &sender)?
                    {
                        return Ok(false);
                    }
                    let audience: Vec<String> = self
                        .channel_audience(&record.state)?
                        .into_iter()
                        .filter(|value| value != &sender)
                        .collect();
                    self.relay(&channel_id, &audience, vec![RelayedEvent {
                        identity: identity.clone(),
                        event: event.clone(),
                    }])?;
                }
                let reply_to = payload.reply_to_event_id.filter(|id| {
                    self.store
                        .message(id)
                        .ok()
                        .flatten()
                        .is_some_and(|message| message.conversation_id == thread)
                });
                self.store.save_message(&Message {
                    event_id: event.event_id.clone(),
                    conversation_id: thread,
                    sender_user_id: sender,
                    text: text.to_owned(),
                    created_at_unix_milliseconds: event.created_at_unix_milliseconds,
                    outgoing: false,
                    edited: false,
                    deleted: false,
                    reactions: Vec::new(),
                    delivered: true,
                    read: true,
                    pinned: false,
                    attachment: None,
                    reply_to_event_id: reply_to,
                    forwarded_from: None,
                    service: false,
                    reaction_marks: Vec::new(),
                    sender_name: Some(truncate(&payload.author_name, MAX_ADMIN_NAME_CHARS)),
                    channel_post: None,
                })?;
                if post.outgoing {
                    self.mark_stats_dirty(&channel_id)?;
                }
                Ok(true)
            }
            KIND_CHANNEL_COMMENT_DELETE => {
                let payload: ChannelCommentDeletePayload = event.decode_payload()?;
                let thread = thread_id(&channel_id, &payload.post_event_id);
                let Some(comment) = self
                    .store
                    .message(&payload.target_event_id)?
                    .filter(|message| message.conversation_id == thread)
                else {
                    return Ok(false);
                };
                let own = comment.sender_user_id == sender;
                let moderator = sender_rights.is_some_and(|rights| rights.delete_messages);
                if !own && !moderator {
                    return Ok(false);
                }
                if own && sender_rights.is_none() && relayed_by.is_none() {
                    if self.my_rights(&record).is_none() {
                        return Ok(false);
                    }
                    let audience: Vec<String> = self
                        .channel_audience(&record.state)?
                        .into_iter()
                        .filter(|value| value != &sender)
                        .collect();
                    self.relay(&channel_id, &audience, vec![RelayedEvent {
                        identity: identity.clone(),
                        event: event.clone(),
                    }])?;
                }
                self.store.delete_message(&comment.event_id)?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Подписчик или администратор канала — с точки зрения администратора.
    fn is_audience(&self, record: &ChannelRecord, user_id: &str) -> Result<bool, CoreError> {
        if admin(&record.state, user_id).is_some() {
            return Ok(true);
        }
        Ok(self
            .store
            .channel_subscriber(&record.state.channel_id, user_id)?
            .is_some_and(|entry| entry.status == SubscriberStatus::Active))
    }

    fn apply_views(&mut self, event: &SignedProtocolEvent) -> Result<bool, CoreError> {
        let payload: ChannelViewsPayload = event.decode_payload()?;
        let channel_id = event.conversation_id.clone();
        let Some(record) = self.live_channel(&channel_id)? else {
            return Ok(false);
        };
        if !self.is_audience(&record, &event.sender_user_id)? {
            return Ok(false);
        }
        let mut changed = false;
        for post_id in payload.post_event_ids.iter().take(200) {
            let own = self.store.message(post_id)?.is_some_and(|message| {
                message.conversation_id == channel_id && message.outgoing && !message.service
            });
            if own && self.store.add_channel_view(post_id, &event.sender_user_id)? {
                changed = true;
            }
        }
        if changed {
            self.mark_stats_dirty(&channel_id)?;
        }
        Ok(false)
    }

    fn apply_stats(&mut self, event: &SignedProtocolEvent) -> Result<bool, CoreError> {
        let payload: ChannelStatsPayload = event.decode_payload()?;
        let channel_id = event.conversation_id.clone();
        let Some(mut record) = self.live_channel(&channel_id)? else {
            return Ok(false);
        };
        if admin(&record.state, &event.sender_user_id).is_none() {
            return Ok(false);
        }
        if event.created_at_unix_milliseconds > record.subscriber_count_at_unix_milliseconds {
            record.subscriber_count = payload.subscriber_count.min(MAX_CHANNEL_SUBSCRIBERS as u32);
            record.subscriber_count_at_unix_milliseconds = event.created_at_unix_milliseconds;
            self.store.save_channel(&record)?;
        }
        for mut stats in payload.posts.into_iter().take(STATS_POSTS * 2) {
            let authored = self.store.message(&stats.event_id)?.is_some_and(|message| {
                message.conversation_id == channel_id && message.sender_user_id == event.sender_user_id
            });
            if !authored {
                continue;
            }
            stats
                .reactions
                .retain(|value| super::ALLOWED_REACTIONS.contains(&value.reaction.as_str()));
            for value in &mut stats.reactions {
                value.mine = false;
            }
            self.store.save_channel_post_stats(&stats)?;
        }
        Ok(false)
    }

    fn relay(&mut self, channel_id: &str, recipients: &[String], events: Vec<RelayedEvent>) -> Result<(), CoreError> {
        if recipients.is_empty() || events.is_empty() {
            return Ok(());
        }
        let mut chunk: Vec<RelayedEvent> = Vec::new();
        let mut size = 0;
        for value in events {
            let length = serde_json::to_vec(&value)?.len();
            if !chunk.is_empty() && size + length > RELAY_CHUNK_BYTES {
                self.send_relay_chunk(channel_id, recipients, std::mem::take(&mut chunk))?;
                size = 0;
            }
            size += length;
            chunk.push(value);
        }
        self.send_relay_chunk(channel_id, recipients, chunk)
    }

    fn send_relay_chunk(
        &mut self,
        channel_id: &str,
        recipients: &[String],
        events: Vec<RelayedEvent>,
    ) -> Result<(), CoreError> {
        if events.is_empty() {
            return Ok(());
        }
        self.queue_channel_event(
            channel_id,
            recipients,
            &Self::new_event_id(),
            KIND_CHANNEL_RELAY,
            &ChannelRelayPayload {
                version: PROTOCOL_VERSION,
                channel_id: channel_id.to_owned(),
                events,
            },
        )?;
        Ok(())
    }

    /// Последние посты с правками — новому подписчику или администратору.
    fn send_history(&mut self, channel_id: &str, user_id: &str) -> Result<(), CoreError> {
        let mut events = Vec::new();
        for stored in self.store.recent_channel_posts(channel_id, HISTORY_POSTS)? {
            let alive = self
                .store
                .message(&stored.post.event.event_id)?
                .is_some_and(|message| !message.deleted);
            if !alive {
                continue;
            }
            events.push(stored.post);
            if let Some(edit) = stored.edit {
                events.push(edit);
            }
        }
        self.relay(channel_id, &[user_id.to_owned()], events)
    }

    /// Изменения списка подписчиков — остальным администраторам.
    fn share_roster(&mut self, channel_id: &str, entries: &[ChannelSubscriber]) -> Result<(), CoreError> {
        let record = self.channel_record(channel_id)?;
        let staff = self.channel_staff(&record.state);
        if staff.is_empty() || entries.is_empty() {
            return Ok(());
        }
        for chunk in entries.chunks(ROSTER_CHUNK) {
            self.queue_channel_event(
                channel_id,
                &staff,
                &Self::new_event_id(),
                KIND_CHANNEL_ROSTER,
                &ChannelRosterPayload {
                    version: PROTOCOL_VERSION,
                    channel_id: channel_id.to_owned(),
                    entries: chunk.to_vec(),
                },
            )?;
        }
        Ok(())
    }

    fn send_full_roster(&mut self, channel_id: &str, user_id: &str) -> Result<(), CoreError> {
        let entries = self.store.channel_subscribers(channel_id)?;
        for chunk in entries.chunks(ROSTER_CHUNK) {
            self.queue_channel_event(
                channel_id,
                &[user_id.to_owned()],
                &Self::new_event_id(),
                KIND_CHANNEL_ROSTER,
                &ChannelRosterPayload {
                    version: PROTOCOL_VERSION,
                    channel_id: channel_id.to_owned(),
                    entries: chunk.to_vec(),
                },
            )?;
        }
        Ok(())
    }

    /// Счётчики своих постов в том виде, в каком их видит автор.
    fn own_stats(&self, channel_id: &str) -> Result<ChannelStatsPayload, CoreError> {
        let mut posts: Vec<Message> = self
            .store
            .messages(channel_id)?
            .into_iter()
            .filter(|message| message.outgoing && !message.service && !message.deleted)
            .collect();
        let skip = posts.len().saturating_sub(STATS_POSTS);
        posts.drain(..skip);
        let mut stats = Vec::new();
        for post in posts {
            stats.push(ChannelPostStats {
                views: self.store.channel_view_count(&post.event_id)?,
                comments: self.store.count_messages(&thread_id(channel_id, &post.event_id))?,
                reactions: self
                    .store
                    .channel_reaction_counts(&post.event_id)?
                    .into_iter()
                    .map(|(reaction, count)| ReactionCount {
                        reaction,
                        count,
                        mine: false,
                    })
                    .collect(),
                event_id: post.event_id,
            });
        }
        Ok(ChannelStatsPayload {
            version: PROTOCOL_VERSION,
            subscriber_count: self.active_subscribers(channel_id)?.len() as u32,
            posts: stats,
        })
    }

    fn send_stats(&mut self, channel_id: &str, recipients: &[String]) -> Result<(), CoreError> {
        if recipients.is_empty() {
            return Ok(());
        }
        let payload = self.own_stats(channel_id)?;
        self.queue_channel_event(channel_id, recipients, &Self::new_event_id(), KIND_CHANNEL_STATS, &payload)?;
        Ok(())
    }

    /// Фоновая рассылка счётчиков: у каждого канала, где пользователь — администратор
    /// и с прошлого раза что-то изменилось.
    pub(super) fn broadcast_channel_stats(&mut self) -> Result<(), CoreError> {
        let now = chrono::Utc::now().timestamp_millis();
        for mut record in self.store.channels()? {
            if !record.stats_dirty
                || now - record.stats_sent_at_unix_milliseconds < STATS_INTERVAL_MILLISECONDS
                || self.my_rights(&record).is_none()
            {
                continue;
            }
            let channel_id = record.state.channel_id.clone();
            let audience = self.channel_audience(&record.state)?;
            self.send_stats(&channel_id, &audience)?;
            record.stats_dirty = false;
            record.stats_sent_at_unix_milliseconds = now;
            self.store.save_channel(&record)?;
        }
        Ok(())
    }

    /// Имя администратора или подписчика: своё имя контакта, иначе объявленное.
    pub(super) fn channel_member_name(&self, state: &ChannelState, user_id: &str) -> String {
        if user_id == self.identity.public.user_id {
            let own = self.store.profile().map(|value| value.display_name).unwrap_or_default();
            return if own.trim().is_empty() { "Вы".to_owned() } else { own };
        }
        if let Ok(Some(contact)) = self.store.contact(user_id) {
            return contact.display_name;
        }
        if let Some(value) = admin(state, user_id).filter(|value| !value.display_name.trim().is_empty()) {
            return value.display_name.clone();
        }
        if let Ok(Some(entry)) = self.store.channel_subscriber(&state.channel_id, user_id)
            && !entry.display_name.trim().is_empty()
        {
            return entry.display_name;
        }
        short_id(user_id)
    }

    /// Счётчики поста для ленты: у своих — живые, у чужих — из рассылки автора.
    pub(super) fn channel_post_info(&self, channel_id: &str, message: &Message) -> Result<ChannelPostInfo, CoreError> {
        let me = &self.identity.public.user_id;
        let local_comments = self.store.count_messages(&thread_id(channel_id, &message.event_id))?;
        let mine: Vec<&str> = message
            .reaction_marks
            .iter()
            .filter(|mark| &mark.user_id == me)
            .map(|mark| mark.reaction.as_str())
            .collect();
        let (views, comments, mut reactions) = if message.outgoing {
            (
                self.store.channel_view_count(&message.event_id)?,
                local_comments,
                self.store
                    .channel_reaction_counts(&message.event_id)?
                    .into_iter()
                    .map(|(reaction, count)| ReactionCount { reaction, count, mine: false })
                    .collect::<Vec<_>>(),
            )
        } else {
            let stats = self.store.channel_post_stats(&message.event_id)?;
            let views = stats.as_ref().map_or(0, |value| value.views).max(u32::from(message.read));
            let comments = stats.as_ref().map_or(0, |value| value.comments).max(local_comments);
            let mut reactions = stats.map(|value| value.reactions).unwrap_or_default();
            // Своя реакция могла ещё не дойти до автора: считаем её сами.
            for reaction in &mine {
                if !reactions.iter().any(|value| value.reaction == *reaction) {
                    reactions.push(ReactionCount {
                        reaction: (*reaction).to_owned(),
                        count: 1,
                        mine: true,
                    });
                }
            }
            (views, comments, reactions)
        };
        for value in &mut reactions {
            if mine.contains(&value.reaction.as_str()) {
                value.mine = true;
                value.count = value.count.max(1);
            }
        }
        reactions.retain(|value| value.count > 0);
        Ok(ChannelPostInfo {
            views,
            comments,
            reactions,
        })
    }

    /// Лента канала: подписи авторов и счётчики.
    pub(super) fn channel_feed(&self, record: &ChannelRecord) -> Result<Vec<Message>, CoreError> {
        let channel_id = &record.state.channel_id;
        let mut messages = self.store.messages(channel_id)?;
        for message in &mut messages {
            if message.service {
                continue;
            }
            if record.state.settings.sign_posts {
                let name = self.channel_member_name(&record.state, &message.sender_user_id);
                let title = admin(&record.state, &message.sender_user_id)
                    .map(|value| value.title.clone())
                    .unwrap_or_default();
                message.sender_name = Some(if title.is_empty() {
                    name
                } else {
                    format!("{name} · {title}")
                });
            }
            if !message.deleted {
                message.channel_post = Some(self.channel_post_info(channel_id, message)?);
            }
        }
        Ok(messages)
    }

    /// Комментарии открытой ветки с именами авторов.
    pub(super) fn channel_comments(&self, record: &ChannelRecord, post_event_id: &str) -> Result<Vec<Message>, CoreError> {
        let mut comments = self
            .store
            .messages(&thread_id(&record.state.channel_id, post_event_id))?;
        for comment in &mut comments {
            if comment.outgoing {
                comment.sender_name = None;
                continue;
            }
            let declared = comment.sender_name.clone();
            let mut name = match self.store.contact(&comment.sender_user_id)? {
                Some(contact) => contact.display_name,
                None => admin(&record.state, &comment.sender_user_id)
                    .map(|value| value.display_name.clone())
                    .or(declared)
                    .filter(|value| !value.trim().is_empty())
                    .unwrap_or_else(|| short_id(&comment.sender_user_id)),
            };
            if admin(&record.state, &comment.sender_user_id).is_some() {
                name.push_str(" · админ");
            }
            comment.sender_name = Some(name);
        }
        Ok(comments)
    }

    /// Строка списка чатов для канала.
    pub(super) fn channel_chat(&self, record: &ChannelRecord) -> Result<crate::models::Chat, CoreError> {
        let me = self.me();
        let channel_id = record.state.channel_id.clone();
        let (last, unread_count) = self.store.conversation_preview(&channel_id)?;
        let active = Self::is_active(record) && !record.state.closed;
        let role = if active { admin(&record.state, &me).map(|value| value.role) } else { None };
        let subscriber_count = if role.is_some() {
            self.active_subscribers(&channel_id)?.len() as u32
        } else {
            record.subscriber_count
        };
        let contact = crate::models::Contact {
            user_id: channel_id,
            display_name: if record.awaiting_state {
                "Канал · ждём ответа администратора".to_owned()
            } else {
                record.state.name.clone()
            },
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
        let can_post = active && rights_of(&record.state, &me).is_some_and(|rights| rights.post_messages);
        let left = record.left || record.removed || record.state.closed;
        Ok(match last {
            Some(message) => crate::models::Chat {
                preview: super::preview_of(&message),
                last_activity_unix_milliseconds: message.created_at_unix_milliseconds,
                has_last_message: true,
                // Галочек у постов канала нет, как в Telegram.
                last_message_outgoing: false,
                last_message_delivered: message.delivered,
                last_message_read: message.read,
                unread_count,
                contact,
                is_group: false,
                member_count: subscriber_count,
                group_role: None,
                group_left: left,
                is_channel: true,
                channel_role: role,
                channel_can_post: can_post,
            },
            None => crate::models::Chat {
                preview: String::new(),
                last_activity_unix_milliseconds: record.joined_at_unix_milliseconds,
                has_last_message: false,
                last_message_outgoing: false,
                last_message_delivered: false,
                last_message_read: false,
                unread_count: 0,
                contact,
                is_group: false,
                member_count: subscriber_count,
                group_role: None,
                group_left: left,
                is_channel: true,
                channel_role: role,
                channel_can_post: can_post,
            },
        })
    }

    pub(super) fn channel_view(&self, record: &ChannelRecord) -> Result<ChannelView, CoreError> {
        let me = self.me();
        let contacts: HashMap<String, crate::models::Contact> = self
            .store
            .contacts()?
            .into_iter()
            .map(|contact| (contact.user_id.clone(), contact))
            .collect();
        let own = self.store.profile()?;
        let name_of = |user_id: &str| -> String {
            if user_id == me {
                return "Вы".to_owned();
            }
            contacts
                .get(user_id)
                .map(|contact| contact.display_name.clone())
                .or_else(|| admin(&record.state, user_id).map(|value| value.display_name.clone()))
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| short_id(user_id))
        };
        let active = Self::is_active(record) && !record.state.closed;
        let my_admin = if active { admin(&record.state, &me) } else { None };
        let my_role = my_admin.map(|value| value.role);
        let rights = if active {
            rights_of(&record.state, &me).unwrap_or_default()
        } else {
            ChannelAdminRights::default()
        };
        let is_owner = my_role == Some(ChannelRole::Owner);
        let mut admins: Vec<ChannelAdminView> = record
            .state
            .admins
            .iter()
            .map(|value| {
                let is_self = value.user_id == me;
                ChannelAdminView {
                    user_id: value.user_id.clone(),
                    display_name: if is_self && !own.display_name.trim().is_empty() {
                        own.display_name.clone()
                    } else {
                        name_of(&value.user_id)
                    },
                    avatar_base64: if is_self {
                        own.avatar_base64.clone()
                    } else {
                        contacts.get(&value.user_id).and_then(|contact| contact.avatar_base64.clone())
                    },
                    role: value.role,
                    rights: match value.role {
                        ChannelRole::Owner => ChannelAdminRights::ALL,
                        ChannelRole::Admin => value.rights,
                    },
                    title: value.title.clone(),
                    is_self,
                    added_by_name: name_of(&value.added_by),
                    can_edit: !is_self
                        && value.role != ChannelRole::Owner
                        && rights.add_admins
                        && (is_owner || value.added_by == me),
                }
            })
            .collect();
        admins.sort_by(|left, right| {
            right
                .role
                .cmp(&left.role)
                .then_with(|| right.is_self.cmp(&left.is_self))
                .then_with(|| left.display_name.to_lowercase().cmp(&right.display_name.to_lowercase()))
        });
        let mut subscribers = Vec::new();
        let mut subscriber_count = record.subscriber_count;
        if my_role.is_some() {
            let roster = self.store.channel_subscribers(&record.state.channel_id)?;
            subscriber_count = roster
                .iter()
                .filter(|entry| entry.status == SubscriberStatus::Active)
                .count() as u32;
            for entry in roster {
                if entry.status == SubscriberStatus::Left {
                    continue;
                }
                let contact = contacts.get(&entry.user_id);
                subscribers.push(ChannelSubscriberView {
                    display_name: contact
                        .map(|contact| contact.display_name.clone())
                        .unwrap_or_else(|| entry.display_name.clone()),
                    avatar_base64: contact.and_then(|contact| contact.avatar_base64.clone()),
                    is_contact: contact.is_some_and(|contact| !contact.pending_approval),
                    banned: entry.status == SubscriberStatus::Banned,
                    subscribed_at_unix_milliseconds: entry.subscribed_at_unix_milliseconds,
                    user_id: entry.user_id,
                });
            }
            subscribers.sort_by(|left, right| {
                left.banned
                    .cmp(&right.banned)
                    .then_with(|| left.display_name.to_lowercase().cmp(&right.display_name.to_lowercase()))
            });
            subscribers.truncate(MAX_LISTED_SUBSCRIBERS);
        }
        let discussion_joined = record
            .state
            .settings
            .discussion_group_id
            .as_deref()
            .is_some_and(|group| self.ensure_group_writable(group).is_ok());
        let thread = self
            .selected_thread
            .clone()
            .filter(|_| self.selected_contact.as_deref() == Some(record.state.channel_id.as_str()));
        Ok(ChannelView {
            channel_id: record.state.channel_id.clone(),
            name: record.state.name.clone(),
            about: record.state.about.clone(),
            avatar_base64: record.state.avatar_base64.clone(),
            epoch: record.state.epoch,
            created_at_unix_milliseconds: record.state.created_at_unix_milliseconds,
            my_role,
            my_rights: rights,
            my_title: my_admin.map(|value| value.title.clone()).unwrap_or_default(),
            awaiting_state: record.awaiting_state,
            pending_invite: record.pending_invite,
            invited_by_name: record.invited_by.as_deref().map(|id| self.channel_member_name(&record.state, id)),
            left: record.left,
            removed: record.removed,
            closed: record.state.closed,
            settings: record.state.settings.clone(),
            discussion_joined,
            subscriber_count,
            admins,
            subscribers,
            invite_link: (active && rights.invite_users).then(|| invite_link(&record.state.channel_id, &me)),
            can_post: active && rights.post_messages,
            can_edit_info: active && rights.change_info,
            can_invite: active && rights.invite_users,
            can_ban: active && rights.ban_users,
            can_add_admins: active && rights.add_admins,
            can_delete_messages: active && rights.delete_messages,
            can_edit_messages: active && rights.edit_messages,
            can_comment: active && record.state.settings.comments_enabled,
            can_react: active,
            thread_post_event_id: thread,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(index: u8) -> String {
        format!("tt1-{}", hex::encode([index; 32]))
    }

    fn channel_admin(index: u8, role: ChannelRole, rights: ChannelAdminRights, added_by: u8) -> ChannelAdmin {
        ChannelAdmin {
            user_id: user(index),
            display_name: format!("Админ {index}"),
            role,
            rights,
            title: String::new(),
            added_by: user(added_by),
            added_at_unix_milliseconds: 1,
        }
    }

    fn poster() -> ChannelAdminRights {
        ChannelAdminRights {
            post_messages: true,
            ..Default::default()
        }
    }

    fn manager() -> ChannelAdminRights {
        ChannelAdminRights {
            post_messages: true,
            add_admins: true,
            ..Default::default()
        }
    }

    /// Владелец 1, администратор 2 (может назначать), редактор 3 (только посты, назначен 2).
    fn base() -> ChannelState {
        ChannelState {
            version: CHANNEL_STATE_VERSION,
            channel_id: format!("ttch1-{}", hex::encode([9u8; 32])),
            epoch: 1,
            name: "Новости".to_owned(),
            about: String::new(),
            avatar_base64: None,
            created_by: user(1),
            created_at_unix_milliseconds: 1,
            admins: vec![
                channel_admin(1, ChannelRole::Owner, ChannelAdminRights::ALL, 1),
                channel_admin(2, ChannelRole::Admin, manager(), 1),
                channel_admin(3, ChannelRole::Admin, poster(), 2),
            ],
            settings: ChannelSettings::default(),
            closed: false,
            updated_by: user(1),
            updated_at_unix_milliseconds: 1,
        }
    }

    fn next(old: &ChannelState, actor: &str, change: impl FnOnce(&mut ChannelState)) -> ChannelState {
        let mut new = old.clone();
        change(&mut new);
        new.epoch = old.epoch + 1;
        new.updated_by = actor.to_owned();
        new
    }

    #[test]
    fn only_admins_with_rights_change_info() {
        let old = base();
        let by_poster = next(&old, &user(3), |state| state.name = "Взлом".to_owned());
        assert!(validate_transition(&old, &by_poster, &user(3), false).is_err());
        let by_outsider = next(&old, &user(7), |state| state.name = "Взлом".to_owned());
        assert!(validate_transition(&old, &by_outsider, &user(7), false).is_err());
        let by_owner = next(&old, &user(1), |state| state.name = "Главное".to_owned());
        assert_eq!(
            validate_transition(&old, &by_owner, &user(1), false).unwrap(),
            vec![ChannelChange::Renamed("Главное".to_owned())]
        );
        let settings = next(&old, &user(3), |state| state.settings.comments_enabled = true);
        assert!(validate_transition(&old, &settings, &user(3), false).is_err());
    }

    #[test]
    fn an_admin_grants_only_rights_they_have() {
        let old = base();
        let modest = next(&old, &user(2), |state| {
            state.admins.push(channel_admin(4, ChannelRole::Admin, poster(), 2))
        });
        assert_eq!(
            validate_transition(&old, &modest, &user(2), false).unwrap(),
            vec![ChannelChange::AdminAdded(user(4))]
        );
        let greedy = next(&old, &user(2), |state| {
            state.admins.push(channel_admin(4, ChannelRole::Admin, ChannelAdminRights::ALL, 2))
        });
        assert!(validate_transition(&old, &greedy, &user(2), false).is_err());
        let without_right = next(&old, &user(3), |state| {
            state.admins.push(channel_admin(4, ChannelRole::Admin, poster(), 3))
        });
        assert!(validate_transition(&old, &without_right, &user(3), false).is_err());
    }

    #[test]
    fn admins_manage_only_those_they_appointed() {
        let old = base();
        let demote_own = next(&old, &user(2), |state| state.admins.retain(|a| a.user_id != user(3)));
        assert_eq!(
            validate_transition(&old, &demote_own, &user(2), false).unwrap(),
            vec![ChannelChange::AdminRemoved(user(3))]
        );
        let mut foreign = base();
        foreign.admins[2].added_by = user(1);
        let demote_foreign = next(&foreign, &user(2), |state| state.admins.retain(|a| a.user_id != user(3)));
        assert!(validate_transition(&foreign, &demote_foreign, &user(2), false).is_err());
        let demote_owner = next(&old, &user(2), |state| state.admins.retain(|a| a.user_id != user(1)));
        assert!(validate_transition(&old, &demote_owner, &user(2), false).is_err());
        let self_promotion = next(&old, &user(3), |state| state.admins[2].rights = ChannelAdminRights::ALL);
        assert!(validate_transition(&old, &self_promotion, &user(3), false).is_err());
    }

    #[test]
    fn ownership_moves_only_from_the_owner_to_an_admin() {
        let old = base();
        let transfer = next(&old, &user(1), |state| {
            state.admins[0].role = ChannelRole::Admin;
            state.admins[1].role = ChannelRole::Owner;
            state.admins[1].rights = ChannelAdminRights::ALL;
        });
        assert_eq!(
            validate_transition(&old, &transfer, &user(1), false).unwrap(),
            vec![ChannelChange::OwnershipTransferred(user(2))]
        );
        let seized = next(&old, &user(2), |state| {
            state.admins[0].role = ChannelRole::Admin;
            state.admins[1].role = ChannelRole::Owner;
            state.admins[1].rights = ChannelAdminRights::ALL;
        });
        assert!(validate_transition(&old, &seized, &user(2), false).is_err());
        let owner_leaves = next(&old, &user(1), |state| {
            state.admins.remove(0);
        });
        assert!(validate_transition(&old, &owner_leaves, &user(1), false).is_err());
    }

    #[test]
    fn resigning_is_allowed_alone() {
        let old = base();
        let resign = next(&old, &user(3), |state| state.admins.retain(|a| a.user_id != user(3)));
        assert_eq!(
            validate_transition(&old, &resign, &user(3), false).unwrap(),
            vec![ChannelChange::Resigned]
        );
        let sneaky = next(&old, &user(2), |state| {
            state.admins.retain(|a| a.user_id != user(2));
            state.name = "Прощай".to_owned();
        });
        assert!(validate_transition(&old, &sneaky, &user(2), false).is_err());
    }

    #[test]
    fn only_the_owner_closes_and_a_closed_channel_is_final() {
        let old = base();
        let by_admin = next(&old, &user(2), |state| state.closed = true);
        assert!(validate_transition(&old, &by_admin, &user(2), false).is_err());
        let closed = next(&old, &user(1), |state| state.closed = true);
        assert_eq!(
            validate_transition(&old, &closed, &user(1), false).unwrap(),
            vec![ChannelChange::Closed]
        );
        let reopened = next(&closed, &user(1), |state| state.closed = false);
        assert!(validate_transition(&closed, &reopened, &user(1), false).is_err());
    }

    #[test]
    fn history_and_signature_cannot_be_forged() {
        let old = base();
        let rollback = ChannelState { epoch: 1, ..next(&old, &user(1), |s| s.name = "X".to_owned()) };
        assert!(validate_transition(&old, &rollback, &user(1), false).is_err());
        let forged = next(&old, &user(1), |state| state.name = "X".to_owned());
        assert!(validate_transition(&old, &forged, &user(2), false).is_err());
        let recreated = next(&old, &user(1), |state| state.created_by = user(2));
        assert!(validate_transition(&old, &recreated, &user(1), false).is_err());
        let owner_without_rights = next(&old, &user(1), |state| state.admins[0].rights = poster());
        assert!(validate_transition(&old, &owner_without_rights, &user(1), false).is_err());
    }

    #[test]
    fn invite_links_round_trip() {
        let channel = format!("ttch1-{}", hex::encode([9u8; 32]));
        let link = invite_link(&channel, &user(2));
        assert_eq!(parse_invite_link(&link).unwrap(), (channel.clone(), user(2)));
        assert_eq!(
            parse_invite_link(&format!("  {channel} {} ", user(2))).unwrap(),
            (channel.clone(), user(2))
        );
        assert!(parse_invite_link(&channel).is_err());
        assert_eq!(split_thread(&thread_id(&channel, "evt1-ab")), Some((channel.as_str(), "evt1-ab")));
        assert_eq!(channel_of_conversation(&channel), Some(channel.as_str()));
        assert_eq!(split_thread("tt1-x/evt1-ab"), None);
        // Личный диалог — тоже `ttc1-` и 64 hex-символа: с каналом его путать нельзя.
        let direct = crate::identity::conversation_id(&user(1), &user(2));
        assert!(!is_channel_id(&direct));
        assert_eq!(channel_of_conversation(&direct), None);
    }

    #[test]
    fn roster_converges_to_the_latest_entry() {
        let entry = |status, at| ChannelSubscriber {
            user_id: user(5),
            display_name: "Подписчик".to_owned(),
            status,
            subscribed_at_unix_milliseconds: 1,
            updated_at_unix_milliseconds: at,
            updated_by: user(5),
        };
        let active = entry(SubscriberStatus::Active, 10);
        assert!(newer(&active, None));
        assert!(!newer(&entry(SubscriberStatus::Active, 5), Some(&active)));
        assert!(newer(&entry(SubscriberStatus::Left, 11), Some(&active)));
        assert!(newer(&entry(SubscriberStatus::Banned, 10), Some(&active)));
        assert!(!newer(&active, Some(&entry(SubscriberStatus::Banned, 10))));
    }
}
