//! Групповые чаты поверх попарных E2EE-каналов.
//!
//! У группы нет общего ключа и нет сервера, который знал бы её состав. Каждое событие
//! группы подписывается один раз (`conversationId` = GroupID) и уходит каждому участнику
//! по уже существующему double ratchet — ровно так же, как личное сообщение. Отсюда два
//! свойства: исключённый участник просто перестаёт получать события (новые сообщения
//! не зашифрованы ничем, что у него осталось), а Node видит лишь обычные конверты.
//!
//! Состав, роли и права живут в [`GroupState`]. Любое изменение — новое полное состояние
//! с `epoch + 1`, подписанное автором. Получатель не верит состоянию на слово: он сравнивает
//! его со своим и пропускает только разницу, на которую у автора были права в прежнем
//! состоянии ([`validate_transition`]). Изменение, пришедшее раньше предыдущих, ждёт их.

use std::collections::{HashMap, HashSet};

use sha2::{Digest, Sha256};

use super::{AppCore, random_hex, short_id};
use crate::{
    CoreError,
    models::{
        GroupMember, GroupMemberView, GroupPermissions, GroupRecord, GroupRole, GroupState,
        GroupView, Message,
    },
    protocol::{
        GroupNoticePayload, GroupStatePayload, KIND_GROUP_ACK, KIND_GROUP_JOINED,
        KIND_GROUP_STATE, PROTOCOL_VERSION, SignedProtocolEvent, is_group_id, is_user_id,
    },
    store::PendingGroupState,
};

pub const GROUP_STATE_VERSION: i32 = 1;
/// Каждое сообщение шифруется под каждого участника отдельно: сотня — разумный потолок
/// для такой схемы, дальше рассылка заметно тормозит отправку.
pub const MAX_GROUP_MEMBERS: usize = 100;
const MAX_GROUP_NAME_CHARS: usize = 64;
const MAX_GROUP_ABOUT_CHARS: usize = 255;
const MAX_MEMBER_NAME_CHARS: usize = 64;
/// Состояние с фото должно пролезать в публичный ящик участника (64 КБ на конверт), пока
/// тот ещё не прислал личный обратный адрес. 32 тысячи символов base64 — это ~24 КБ JPEG.
pub const MAX_GROUP_AVATAR_BASE64: usize = 32_000;
const PENDING_STATE_TTL_MILLISECONDS: i64 = 7 * 86_400_000;

/// Что именно изменилось: из этого собираются служебные отметки в ленте.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupChange {
    Renamed(String),
    AboutChanged,
    AvatarChanged,
    PermissionsChanged,
    Added(Vec<String>),
    Removed(Vec<String>),
    Left,
    RoleChanged(String, GroupRole),
    OwnershipTransferred(String),
}

fn denied(message: &str) -> CoreError {
    CoreError::InvalidInput(message.to_owned())
}

pub fn member<'a>(state: &'a GroupState, user_id: &str) -> Option<&'a GroupMember> {
    state.members.iter().find(|value| value.user_id == user_id)
}

pub fn role_of(state: &GroupState, user_id: &str) -> Option<GroupRole> {
    member(state, user_id).map(|value| value.role)
}

fn owner_of(state: &GroupState) -> Option<&str> {
    state
        .members
        .iter()
        .find(|value| value.role == GroupRole::Owner)
        .map(|value| value.user_id.as_str())
}

/// Может ли `actor` удалить чужое сообщение: только старший по роли.
pub fn may_moderate(state: &GroupState, actor: &str, author: &str) -> bool {
    let Some(actor_role) = role_of(state, actor) else {
        return false;
    };
    let author_role = role_of(state, author).unwrap_or(GroupRole::Member);
    actor_role >= GroupRole::Admin && actor_role > author_role
}

/// Детерминированный порядок для двух разных состояний с одинаковым номером: все
/// участники выберут одно и то же, в каком бы порядке ни пришли события.
fn state_hash(state: &GroupState) -> Vec<u8> {
    Sha256::digest(serde_json::to_vec(state).unwrap_or_default()).to_vec()
}

/// Форма состояния без учёта того, кто и что менял.
pub fn validate_state(state: &GroupState) -> Result<(), CoreError> {
    if state.version != GROUP_STATE_VERSION {
        return Err(denied("Неподдерживаемая версия группы"));
    }
    if !is_group_id(&state.group_id) || state.epoch < 1 {
        return Err(denied("Повреждено состояние группы"));
    }
    let name_length = state.name.trim().chars().count();
    if name_length == 0 || name_length > MAX_GROUP_NAME_CHARS {
        return Err(denied("Название группы: 1–64 символа"));
    }
    if state.about.chars().count() > MAX_GROUP_ABOUT_CHARS {
        return Err(denied("Описание группы: максимум 255 символов"));
    }
    if state
        .avatar_base64
        .as_ref()
        .is_some_and(|value| value.len() > MAX_GROUP_AVATAR_BASE64)
    {
        return Err(denied("Фото группы слишком большое"));
    }
    if state.members.len() > MAX_GROUP_MEMBERS {
        return Err(denied("В группе может быть не больше 100 участников"));
    }
    if !is_user_id(&state.created_by) || !is_user_id(&state.updated_by) {
        return Err(denied("Повреждено состояние группы"));
    }
    let mut seen = HashSet::new();
    for value in &state.members {
        if !is_user_id(&value.user_id)
            || !is_user_id(&value.added_by)
            || value.display_name.chars().count() > MAX_MEMBER_NAME_CHARS
            || !seen.insert(value.user_id.as_str())
        {
            return Err(denied("Повреждён список участников группы"));
        }
    }
    let owners = state
        .members
        .iter()
        .filter(|value| value.role == GroupRole::Owner)
        .count();
    // Пустая группа — след ухода последнего участника; у живой владелец ровно один.
    if owners != usize::from(!state.members.is_empty()) {
        return Err(denied("У группы должен быть ровно один владелец"));
    }
    Ok(())
}

/// Проверяет, что `actor` имел право превратить `old` в `new`, и возвращает, что изменилось.
///
/// Права берутся из прежнего состояния: иначе участник мог бы в том же изменении выдать
/// себе роль администратора и тут же ей воспользоваться.
pub fn validate_transition(
    old: &GroupState,
    new: &GroupState,
    actor: &str,
    allow_equal_epoch: bool,
) -> Result<Vec<GroupChange>, CoreError> {
    validate_state(new)?;
    if new.group_id != old.group_id
        || new.created_by != old.created_by
        || new.created_at_unix_milliseconds != old.created_at_unix_milliseconds
    {
        return Err(denied("Нельзя подменить происхождение группы"));
    }
    if new.epoch < old.epoch || (new.epoch == old.epoch && !allow_equal_epoch) {
        return Err(denied("Устаревшее состояние группы"));
    }
    if new.updated_by != actor {
        return Err(denied("Изменение подписано не его автором"));
    }
    let actor_role =
        role_of(old, actor).ok_or_else(|| denied("Автор изменения не состоит в группе"))?;

    let old_members: HashMap<&str, &GroupMember> = old
        .members
        .iter()
        .map(|value| (value.user_id.as_str(), value))
        .collect();
    let new_ids: HashSet<&str> = new.members.iter().map(|value| value.user_id.as_str()).collect();
    let mut changes = Vec::new();

    let added: Vec<&GroupMember> = new
        .members
        .iter()
        .filter(|value| !old_members.contains_key(value.user_id.as_str()))
        .collect();
    if !added.is_empty() {
        if actor_role < GroupRole::Admin && !old.permissions.members_can_invite {
            return Err(denied("Недостаточно прав, чтобы добавлять участников"));
        }
        if added
            .iter()
            .any(|value| value.role != GroupRole::Member || value.added_by != actor)
        {
            return Err(denied("Новый участник добавлен с неверной ролью"));
        }
        changes.push(GroupChange::Added(
            added.iter().map(|value| value.user_id.clone()).collect(),
        ));
    }

    let mut actor_left = false;
    let mut removed = Vec::new();
    for value in &old.members {
        if new_ids.contains(value.user_id.as_str()) {
            continue;
        }
        if value.user_id == actor {
            actor_left = true;
        } else if actor_role < GroupRole::Admin || value.role >= actor_role {
            return Err(denied("Недостаточно прав, чтобы исключить участника"));
        } else {
            removed.push(value.user_id.clone());
        }
    }
    if !removed.is_empty() {
        changes.push(GroupChange::Removed(removed));
    }

    for current in &new.members {
        let Some(previous) = old_members.get(current.user_id.as_str()) else {
            continue;
        };
        if current.display_name != previous.display_name
            || current.added_by != previous.added_by
            || current.added_at_unix_milliseconds != previous.added_at_unix_milliseconds
        {
            return Err(denied("Нельзя менять данные участника"));
        }
        if current.role == previous.role {
            continue;
        }
        if actor_role != GroupRole::Owner {
            return Err(denied("Роли в группе назначает только владелец"));
        }
        // Владелец понижает себя только вместе с передачей владения — это проверит
        // требование «ровно один владелец» в validate_state.
        if current.user_id != actor && current.role != GroupRole::Owner {
            changes.push(GroupChange::RoleChanged(current.user_id.clone(), current.role));
        }
    }

    let new_owner = owner_of(new);
    if new_owner != owner_of(old) {
        match new_owner {
            Some(owner) => {
                if actor_role != GroupRole::Owner {
                    return Err(denied("Передать владение может только владелец"));
                }
                changes.push(GroupChange::OwnershipTransferred(owner.to_owned()));
            }
            None if new.members.is_empty() => {}
            None => return Err(denied("У группы должен быть ровно один владелец")),
        }
    }

    if new.name.trim() != old.name.trim() {
        changes.push(GroupChange::Renamed(new.name.trim().to_owned()));
    }
    if new.about != old.about {
        changes.push(GroupChange::AboutChanged);
    }
    if new.avatar_base64 != old.avatar_base64 {
        changes.push(GroupChange::AvatarChanged);
    }
    let info_changed = changes.iter().any(|change| {
        matches!(
            change,
            GroupChange::Renamed(_) | GroupChange::AboutChanged | GroupChange::AvatarChanged
        )
    });
    if info_changed && actor_role < GroupRole::Admin && !old.permissions.members_can_edit_info {
        return Err(denied("Недостаточно прав, чтобы менять данные группы"));
    }
    if new.permissions != old.permissions {
        if actor_role < GroupRole::Admin {
            return Err(denied("Права участников меняют администраторы"));
        }
        changes.push(GroupChange::PermissionsChanged);
    }

    if actor_left {
        // Уходя, можно только передать владение: всё остальное делается до выхода.
        if changes
            .iter()
            .any(|change| !matches!(change, GroupChange::OwnershipTransferred(_)))
        {
            return Err(denied("Выход из группы нельзя совмещать с другими изменениями"));
        }
        changes.push(GroupChange::Left);
    }
    if changes.is_empty() {
        return Err(denied("Изменений нет"));
    }
    Ok(changes)
}

/// Приглашение в незнакомую группу проверить не с чем. Остаётся убедиться, что оно
/// хотя бы внутренне согласовано и что приглашающий сам в ней состоит; пользователь
/// всё равно решает сам, принимать ли его.
fn validate_invite(state: &GroupState, actor: &str, me: &str) -> Result<(), CoreError> {
    validate_state(state)?;
    if state.updated_by != actor || member(state, actor).is_none() {
        return Err(denied("Приглашение прислал не участник группы"));
    }
    if member(state, me).is_none() {
        return Err(denied("Приглашение адресовано не вам"));
    }
    Ok(())
}

impl AppCore {
    /// Видимая группа. Надгробие удалённого чата группой для команд не считается.
    pub(super) fn group_record(&self, group_id: &str) -> Result<GroupRecord, CoreError> {
        self.store
            .group(group_id)?
            .filter(|record| !record.hidden)
            .ok_or_else(|| denied("Группа не найдена"))
    }

    /// Писать в группу можно, только состоя в ней и приняв приглашение.
    pub(super) fn ensure_group_writable(&self, group_id: &str) -> Result<GroupRecord, CoreError> {
        let record = self.group_record(group_id)?;
        if record.left || member(&record.state, &self.identity.public.user_id).is_none() {
            return Err(denied("Вы больше не участник этой группы"));
        }
        if record.pending_invite {
            return Err(denied("Сначала примите приглашение в группу"));
        }
        Ok(record)
    }

    /// Все участники, кроме себя: им уходит каждое событие группы.
    pub(super) fn group_recipients(&self, state: &GroupState) -> Vec<String> {
        state
            .members
            .iter()
            .filter(|value| value.user_id != self.identity.public.user_id)
            .map(|value| value.user_id.clone())
            .collect()
    }

    /// Одно подписанное событие — по задаче в очереди на каждого получателя.
    pub(super) fn queue_group_event<T: serde::Serialize>(
        &mut self,
        group_id: &str,
        recipients: &[String],
        event_id: &str,
        kind: &str,
        payload: &T,
    ) -> Result<(), CoreError> {
        let event = self.sign_event(group_id, event_id, kind, payload)?;
        for recipient in recipients {
            self.store.retry_now(recipient)?;
            self.store.enqueue_outbox(recipient, &event)?;
        }
        Ok(())
    }

    pub(super) fn create_group(
        &mut self,
        name: &str,
        about: &str,
        avatar_base64: Option<String>,
        member_ids: &[String],
    ) -> Result<String, CoreError> {
        let me = self.identity.public.user_id.clone();
        let now = chrono::Utc::now().timestamp_millis();
        let own_name = self.store.profile()?.display_name;
        let mut members = vec![GroupMember {
            user_id: me.clone(),
            display_name: if own_name.trim().is_empty() {
                short_id(&me)
            } else {
                truncate(&own_name, MAX_MEMBER_NAME_CHARS)
            },
            role: GroupRole::Owner,
            added_by: me.clone(),
            added_at_unix_milliseconds: now,
        }];
        members.extend(self.members_from_contacts(member_ids, &me, now)?);
        if members.len() < 2 {
            return Err(denied("Выберите хотя бы одного участника"));
        }
        let state = GroupState {
            version: GROUP_STATE_VERSION,
            group_id: format!("ttg1-{}", random_hex(32)),
            epoch: 1,
            name: name.trim().to_owned(),
            about: about.trim().to_owned(),
            avatar_base64: avatar_base64.filter(|value| !value.is_empty()),
            created_by: me.clone(),
            created_at_unix_milliseconds: now,
            members,
            permissions: GroupPermissions::default(),
            updated_by: me.clone(),
            updated_at_unix_milliseconds: now,
        };
        validate_state(&state)?;
        let group_id = state.group_id.clone();
        let recipients = self.group_recipients(&state);
        let event_id = format!("evt1-{}", random_hex(16));
        self.queue_group_event(
            &group_id,
            &recipients,
            &event_id,
            KIND_GROUP_STATE,
            &GroupStatePayload {
                version: PROTOCOL_VERSION,
                state: state.clone(),
            },
        )?;
        let name = state.name.clone();
        self.store.save_group(&GroupRecord {
            state,
            pending_invite: false,
            invited_by: None,
            left: false,
            hidden: false,
            pinned: false,
            muted: false,
            draft: String::new(),
            manual_unread: false,
            confirmed_members: vec![me.clone()],
            joined_at_unix_milliseconds: now,
        })?;
        self.save_service(&group_id, &event_id, &me, now, format!("Вы создали группу «{name}»"), true)?;
        self.selected_contact = Some(group_id.clone());
        self.store.set_selected_contact(&self.selected_contact)?;
        self.deliver_now();
        self.status = "Группа создана".to_owned();
        Ok(group_id)
    }

    /// В группу добавляются только принятые контакты: у них уже есть проверенный адрес,
    /// а незнакомца так нельзя было бы втянуть в переписку без его ведома.
    fn members_from_contacts(
        &self,
        user_ids: &[String],
        me: &str,
        now: i64,
    ) -> Result<Vec<GroupMember>, CoreError> {
        let mut seen = HashSet::new();
        let mut members = Vec::new();
        for user_id in user_ids {
            if user_id == me || !seen.insert(user_id.as_str()) {
                continue;
            }
            let contact = self
                .store
                .contact(user_id)?
                .filter(|contact| !contact.pending_approval)
                .ok_or_else(|| denied("Добавлять в группу можно только принятые контакты"))?;
            if !is_user_id(&contact.user_id) {
                return Err(denied("У контакта повреждён UserID"));
            }
            members.push(GroupMember {
                user_id: contact.user_id.clone(),
                display_name: truncate(&contact.display_name, MAX_MEMBER_NAME_CHARS),
                role: GroupRole::Member,
                added_by: me.to_owned(),
                added_at_unix_milliseconds: now,
            });
        }
        Ok(members)
    }

    pub(super) fn add_group_members(
        &mut self,
        group_id: &str,
        user_ids: &[String],
    ) -> Result<(), CoreError> {
        let me = self.identity.public.user_id.clone();
        let now = chrono::Utc::now().timestamp_millis();
        let candidates = self.members_from_contacts(user_ids, &me, now)?;
        self.commit_group_state(group_id, false, move |state, _| {
            let before = state.members.len();
            for candidate in candidates {
                if member(state, &candidate.user_id).is_none() {
                    state.members.push(candidate);
                }
            }
            if state.members.len() == before {
                return Err(denied("Эти пользователи уже в группе"));
            }
            if state.members.len() > MAX_GROUP_MEMBERS {
                return Err(denied("В группе может быть не больше 100 участников"));
            }
            Ok(())
        })?;
        self.status = "Участники добавлены".to_owned();
        Ok(())
    }

    pub(super) fn remove_group_member(&mut self, group_id: &str, user_id: &str) -> Result<(), CoreError> {
        let target = user_id.to_owned();
        self.commit_group_state(group_id, false, move |state, me| {
            if target == me {
                return Err(denied("Чтобы уйти самому, покиньте группу"));
            }
            let before = state.members.len();
            state.members.retain(|value| value.user_id != target);
            if state.members.len() == before {
                return Err(denied("Участник не найден"));
            }
            Ok(())
        })?;
        self.status = "Участник исключён".to_owned();
        Ok(())
    }

    pub(super) fn set_group_role(
        &mut self,
        group_id: &str,
        user_id: &str,
        role: GroupRole,
    ) -> Result<(), CoreError> {
        if role == GroupRole::Owner {
            return Err(denied("Для смены владельца передайте владение"));
        }
        let target = user_id.to_owned();
        self.commit_group_state(group_id, false, move |state, _| {
            let value = state
                .members
                .iter_mut()
                .find(|value| value.user_id == target)
                .ok_or_else(|| denied("Участник не найден"))?;
            if value.role == GroupRole::Owner {
                return Err(denied("Роль владельца меняется только передачей владения"));
            }
            if value.role == role {
                return Err(denied("У участника уже эта роль"));
            }
            value.role = role;
            Ok(())
        })?;
        self.status = if role == GroupRole::Admin {
            "Назначен администратор".to_owned()
        } else {
            "Права администратора сняты".to_owned()
        };
        Ok(())
    }

    pub(super) fn transfer_group_ownership(&mut self, group_id: &str, user_id: &str) -> Result<(), CoreError> {
        let target = user_id.to_owned();
        self.commit_group_state(group_id, false, move |state, me| {
            if target == me {
                return Err(denied("Вы уже владелец"));
            }
            if role_of(state, me) != Some(GroupRole::Owner) {
                return Err(denied("Передать владение может только владелец"));
            }
            let value = state
                .members
                .iter_mut()
                .find(|value| value.user_id == target)
                .ok_or_else(|| denied("Участник не найден"))?;
            value.role = GroupRole::Owner;
            if let Some(own) = state.members.iter_mut().find(|value| value.user_id == me) {
                own.role = GroupRole::Admin;
            }
            Ok(())
        })?;
        self.status = "Владение группой передано".to_owned();
        Ok(())
    }

    pub(super) fn update_group_info(
        &mut self,
        group_id: &str,
        name: &str,
        about: &str,
        avatar_base64: Option<String>,
    ) -> Result<(), CoreError> {
        let name = name.trim().to_owned();
        let about = about.trim().to_owned();
        let avatar = avatar_base64.filter(|value| !value.is_empty());
        self.commit_group_state(group_id, false, move |state, _| {
            state.name = name;
            state.about = about;
            state.avatar_base64 = avatar;
            Ok(())
        })?;
        self.status = "Данные группы обновлены".to_owned();
        Ok(())
    }

    pub(super) fn set_group_permissions(
        &mut self,
        group_id: &str,
        permissions: GroupPermissions,
    ) -> Result<(), CoreError> {
        self.commit_group_state(group_id, false, move |state, _| {
            state.permissions = permissions;
            Ok(())
        })?;
        self.status = "Права участников изменены".to_owned();
        Ok(())
    }

    /// Выход из группы. Владелец, уходя, оставляет группу старшему из оставшихся.
    /// `forget` ещё и убирает чат из списка вместе с локальной историей.
    pub(super) fn leave_group(&mut self, group_id: &str, forget: bool) -> Result<(), CoreError> {
        let me = self.identity.public.user_id.clone();
        let record = self.group_record(group_id)?;
        if !record.left && member(&record.state, &me).is_some() {
            self.commit_group_state(group_id, true, |state, me| {
                if role_of(state, me) == Some(GroupRole::Owner) {
                    let heir = state
                        .members
                        .iter()
                        .filter(|value| value.user_id != me)
                        .max_by_key(|value| (value.role, -value.added_at_unix_milliseconds))
                        .map(|value| value.user_id.clone());
                    if let Some(heir) = heir
                        && let Some(value) = state.members.iter_mut().find(|value| value.user_id == heir)
                    {
                        value.role = GroupRole::Owner;
                    }
                }
                state.members.retain(|value| value.user_id != me);
                Ok(())
            })?;
        }
        if forget {
            let mut record = self.group_record(group_id)?;
            record.hidden = true;
            record.left = true;
            record.pending_invite = false;
            record.draft.clear();
            self.store.save_group(&record)?;
            self.store.clear_conversation(group_id)?;
            self.store.clear_pending_group_states(group_id)?;
            if self.selected_contact.as_deref() == Some(group_id) {
                self.selected_contact = None;
                self.store.set_selected_contact(&None)?;
            }
            self.status = "Группа удалена".to_owned();
        } else {
            self.status = "Вы покинули группу".to_owned();
        }
        Ok(())
    }

    pub(super) fn accept_group_invite(&mut self, group_id: &str) -> Result<(), CoreError> {
        let me = self.identity.public.user_id.clone();
        let mut record = self.group_record(group_id)?;
        if record.left || member(&record.state, &me).is_none() {
            return Err(denied("Вы больше не участник этой группы"));
        }
        if !record.pending_invite {
            return Ok(());
        }
        let now = chrono::Utc::now().timestamp_millis();
        record.pending_invite = false;
        record.joined_at_unix_milliseconds = now;
        self.store.save_group(&record)?;
        let recipients = self.group_recipients(&record.state);
        let event_id = format!("evt1-{}", random_hex(16));
        self.queue_group_event(
            group_id,
            &recipients,
            &event_id,
            KIND_GROUP_JOINED,
            &GroupNoticePayload {
                version: PROTOCOL_VERSION,
                group_id: group_id.to_owned(),
            },
        )?;
        self.save_service(group_id, &event_id, &me, now, "Вы вступили в группу".to_owned(), true)?;
        self.deliver_now();
        self.status = "Вы вступили в группу".to_owned();
        Ok(())
    }

    /// Прочтение группы. Квитанция уходит только авторам и только на последнее прочитанное
    /// каждого из них: рассылать её всем участникам значило бы N² событий на сообщение.
    pub(super) fn mark_group_read(&mut self, group_id: &str) -> Result<(), CoreError> {
        let me = self.identity.public.user_id.clone();
        let record = self.group_record(group_id)?;
        let mut latest: HashMap<String, (i64, String)> = HashMap::new();
        for mut message in self.store.messages(group_id)? {
            if message.outgoing || message.read {
                continue;
            }
            message.read = true;
            self.store.save_message(&message)?;
            if message.service || message.deleted {
                continue;
            }
            let entry = latest
                .entry(message.sender_user_id.clone())
                .or_insert((i64::MIN, String::new()));
            if message.created_at_unix_milliseconds >= entry.0 {
                *entry = (message.created_at_unix_milliseconds, message.event_id.clone());
            }
        }
        // Непринятое приглашение не подтверждает, что группу кто-то читает.
        let active = !record.left && !record.pending_invite && member(&record.state, &me).is_some();
        if active && !latest.is_empty() {
            for (author, (_, target_event_id)) in latest {
                if member(&record.state, &author).is_none() {
                    continue;
                }
                self.queue_group_event(
                    group_id,
                    std::slice::from_ref(&author),
                    &format!("evt1-{}", random_hex(16)),
                    crate::protocol::KIND_RECEIPT_READ,
                    &crate::protocol::TargetPayload {
                        version: PROTOCOL_VERSION,
                        target_event_id,
                    },
                )?;
            }
            self.deliver_now();
        }
        if record.manual_unread {
            let mut record = self.group_record(group_id)?;
            record.manual_unread = false;
            self.store.save_group(&record)?;
        }
        Ok(())
    }

    /// Локальное изменение группы: та же проверка прав, что и у получателей, затем
    /// рассылка нового состояния всем, кто был в группе до изменения и после него.
    fn commit_group_state(
        &mut self,
        group_id: &str,
        allow_pending: bool,
        mutate: impl FnOnce(&mut GroupState, &str) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        let me = self.identity.public.user_id.clone();
        let mut record = self.group_record(group_id)?;
        if record.left || member(&record.state, &me).is_none() {
            return Err(denied("Вы больше не участник этой группы"));
        }
        if record.pending_invite && !allow_pending {
            return Err(denied("Сначала примите приглашение в группу"));
        }
        let old = record.state.clone();
        let mut new = old.clone();
        mutate(&mut new, &me)?;
        let now = chrono::Utc::now().timestamp_millis();
        new.epoch = old.epoch + 1;
        new.updated_by = me.clone();
        new.updated_at_unix_milliseconds = now.max(old.updated_at_unix_milliseconds);
        let changes = validate_transition(&old, &new, &me, false)?;

        // Исключённый тоже получает новое состояние: иначе он так и не узнал бы,
        // что писать в группу больше незачем.
        let mut recipients = self.group_recipients(&old);
        for value in self.group_recipients(&new) {
            if !recipients.contains(&value) {
                recipients.push(value);
            }
        }
        let event_id = format!("evt1-{}", random_hex(16));
        self.queue_group_event(
            group_id,
            &recipients,
            &event_id,
            KIND_GROUP_STATE,
            &GroupStatePayload {
                version: PROTOCOL_VERSION,
                state: new.clone(),
            },
        )?;
        record.state = new;
        if member(&record.state, &me).is_none() {
            record.left = true;
            record.pending_invite = false;
        }
        self.store.save_group(&record)?;
        self.record_changes(&record, &old, &event_id, &me, now, &changes)?;
        self.deliver_now();
        Ok(())
    }

    /// Событие группы от другого участника. Возвращает `true`, если в ленте что-то
    /// появилось или изменилось.
    pub(super) fn apply_group_event(&mut self, event: &SignedProtocolEvent) -> Result<bool, CoreError> {
        let group_id = event.conversation_id.clone();
        if event.kind == KIND_GROUP_STATE {
            let payload: GroupStatePayload = event.decode_payload()?;
            if payload.state.group_id != group_id {
                return Err(CoreError::Crypto(
                    "Состояние относится к другой группе".to_owned(),
                ));
            }
            return self.apply_remote_state(
                &event.event_id,
                &event.sender_user_id,
                event.created_at_unix_milliseconds,
                payload.state,
            );
        }

        let Some(mut record) = self.store.group(&group_id)? else {
            return Ok(false);
        };
        // Бывший участник подписью всё ещё владеет, но голоса в группе у него больше нет.
        if record.left || record.hidden || member(&record.state, &event.sender_user_id).is_none() {
            return Ok(false);
        }
        if !record.confirmed_members.contains(&event.sender_user_id) {
            record.confirmed_members.push(event.sender_user_id.clone());
            self.store.save_group(&record)?;
        }
        match event.kind.as_str() {
            KIND_GROUP_JOINED => {
                let payload: GroupNoticePayload = event.decode_payload()?;
                if payload.group_id != group_id {
                    return Ok(false);
                }
                let name = self.member_display_name(Some(&record.state), &event.sender_user_id);
                self.save_service(
                    &group_id,
                    &event.event_id,
                    &event.sender_user_id,
                    event.created_at_unix_milliseconds,
                    format!("{name} вступил(а) в группу"),
                    true,
                )?;
                // Ответ несёт наш личный обратный адрес: новичку больше не придётся
                // стучаться в публичный ящик.
                if !record.pending_invite {
                    self.queue_group_event(
                        &group_id,
                        std::slice::from_ref(&event.sender_user_id),
                        &format!("evt1-{}", random_hex(16)),
                        KIND_GROUP_ACK,
                        &GroupNoticePayload {
                            version: PROTOCOL_VERSION,
                            group_id: group_id.clone(),
                        },
                    )?;
                }
                Ok(true)
            }
            KIND_GROUP_ACK => Ok(false),
            _ => self.apply_message_event(event, &group_id, Some(&record.state)),
        }
    }

    fn apply_remote_state(
        &mut self,
        event_id: &str,
        actor: &str,
        created_at: i64,
        state: GroupState,
    ) -> Result<bool, CoreError> {
        let me = self.identity.public.user_id.clone();
        let now = chrono::Utc::now().timestamp_millis();
        let Some(mut record) = self.store.group(&state.group_id)? else {
            // Незнакомая группа интересна, только если нас в неё позвали.
            if member(&state, &me).is_none() {
                return Ok(false);
            }
            if let Err(error) = validate_invite(&state, actor, &me) {
                self.status = format!("Приглашение в группу отклонено: {error}");
                return Ok(false);
            }
            let inviter = self.member_display_name(Some(&state), actor);
            let group_id = state.group_id.clone();
            let name = state.name.clone();
            self.store.save_group(&GroupRecord {
                state,
                pending_invite: true,
                invited_by: Some(actor.to_owned()),
                left: false,
                hidden: false,
                pinned: false,
                muted: false,
                draft: String::new(),
                manual_unread: false,
                confirmed_members: vec![actor.to_owned()],
                joined_at_unix_milliseconds: now,
            })?;
            self.save_service(
                &group_id,
                event_id,
                actor,
                created_at,
                format!("{inviter} пригласил(а) вас в группу «{name}»"),
                false,
            )?;
            return Ok(true);
        };

        if state.epoch < record.state.epoch || state == record.state {
            return Ok(false);
        }
        if record.left || record.hidden {
            // Из вышедшей группы возвращает только новое приглашение.
            if state.epoch <= record.state.epoch || member(&state, &me).is_none() {
                return Ok(false);
            }
            if let Err(error) = validate_invite(&state, actor, &me) {
                self.status = format!("Приглашение в группу отклонено: {error}");
                return Ok(false);
            }
            let inviter = self.member_display_name(Some(&state), actor);
            let name = state.name.clone();
            let group_id = state.group_id.clone();
            record.state = state;
            record.pending_invite = true;
            record.left = false;
            record.hidden = false;
            record.invited_by = Some(actor.to_owned());
            record.confirmed_members = vec![actor.to_owned()];
            self.store.save_group(&record)?;
            self.store.clear_pending_group_states(&group_id)?;
            self.save_service(
                &group_id,
                event_id,
                actor,
                created_at,
                format!("{inviter} снова пригласил(а) вас в группу «{name}»"),
                false,
            )?;
            return Ok(true);
        }

        let equal = state.epoch == record.state.epoch;
        if equal && state_hash(&state) <= state_hash(&record.state) {
            return Ok(false);
        }
        let changes = match validate_transition(&record.state, &state, actor, equal) {
            Ok(changes) => changes,
            Err(_) if !equal && state.epoch > record.state.epoch + 1 => {
                // Возможно, автор получил права в изменении, которое к нам ещё не дошло.
                self.store.save_pending_group_state(&PendingGroupState {
                    event_id: event_id.to_owned(),
                    actor: actor.to_owned(),
                    created_at_unix_milliseconds: created_at,
                    state,
                })?;
                return Ok(false);
            }
            Err(error) => {
                self.status = format!("Изменение группы отклонено: {error}");
                return Ok(false);
            }
        };
        let group_id = state.group_id.clone();
        self.commit_remote_state(&mut record, state, event_id, actor, created_at, &changes)?;
        self.retry_pending_states(&group_id)?;
        Ok(true)
    }

    fn commit_remote_state(
        &mut self,
        record: &mut GroupRecord,
        state: GroupState,
        event_id: &str,
        actor: &str,
        created_at: i64,
        changes: &[GroupChange],
    ) -> Result<(), CoreError> {
        let previous = std::mem::replace(&mut record.state, state);
        if !record.confirmed_members.iter().any(|value| value == actor) {
            record.confirmed_members.push(actor.to_owned());
        }
        if member(&record.state, &self.identity.public.user_id).is_none() {
            record.left = true;
            record.pending_invite = false;
        }
        self.store.save_group(record)?;
        self.record_changes(record, &previous, event_id, actor, created_at, changes)
    }

    /// Отложенные изменения, которые теперь можно проверить.
    fn retry_pending_states(&mut self, group_id: &str) -> Result<(), CoreError> {
        let expired_before = chrono::Utc::now().timestamp_millis() - PENDING_STATE_TTL_MILLISECONDS;
        self.store.prune_pending_group_states(expired_before)?;
        loop {
            let Some(mut record) = self.store.group(group_id)? else {
                return Ok(());
            };
            if record.left || record.hidden {
                self.store.clear_pending_group_states(group_id)?;
                return Ok(());
            }
            let mut progressed = false;
            for pending in self.store.pending_group_states(group_id)? {
                if pending.state.epoch <= record.state.epoch {
                    self.store.delete_pending_group_state(&pending.event_id)?;
                    continue;
                }
                match validate_transition(&record.state, &pending.state, &pending.actor, false) {
                    Ok(changes) => {
                        self.store.delete_pending_group_state(&pending.event_id)?;
                        self.commit_remote_state(
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
                    // Следующее по порядку изменение и всё равно не проходит: ждать нечего.
                    Err(_) if pending.state.epoch == record.state.epoch + 1 => {
                        self.store.delete_pending_group_state(&pending.event_id)?;
                    }
                    Err(_) => {}
                }
            }
            if !progressed {
                return Ok(());
            }
        }
    }

    /// Служебные отметки о том, что изменилось.
    fn record_changes(
        &mut self,
        record: &GroupRecord,
        previous: &GroupState,
        event_id: &str,
        actor: &str,
        created_at: i64,
        changes: &[GroupChange],
    ) -> Result<(), CoreError> {
        let me = self.identity.public.user_id.clone();
        let by_me = actor == me;
        let actor_name = if by_me {
            "Вы".to_owned()
        } else {
            self.member_display_name_in(&[&record.state, previous], actor)
        };
        let verb = |mine: &str, theirs: &str| if by_me { mine.to_owned() } else { theirs.to_owned() };
        let object = |core: &Self, user_id: &str| {
            if user_id == me {
                "вас".to_owned()
            } else {
                core.member_display_name_in(&[&record.state, previous], user_id)
            }
        };
        for (index, change) in changes.iter().enumerate() {
            let mut notable = false;
            let text = match change {
                GroupChange::Added(ids) => format!(
                    "{actor_name} {} {}",
                    verb("добавили", "добавил(а)"),
                    ids.iter().map(|id| object(self, id)).collect::<Vec<_>>().join(", ")
                ),
                GroupChange::Removed(ids) => {
                    notable = ids.contains(&me);
                    format!(
                        "{actor_name} {} {}",
                        verb("исключили", "исключил(а)"),
                        ids.iter().map(|id| object(self, id)).collect::<Vec<_>>().join(", ")
                    )
                }
                GroupChange::Left => format!("{actor_name} {}", verb("покинули группу", "покинул(а) группу")),
                GroupChange::RoleChanged(id, GroupRole::Admin) => format!(
                    "{actor_name} {} {} администратором",
                    verb("назначили", "назначил(а)"),
                    object(self, id)
                ),
                GroupChange::RoleChanged(id, _) => format!(
                    "{actor_name} {} права администратора у {}",
                    verb("сняли", "снял(а)"),
                    if id == &me {
                        "вас".to_owned()
                    } else {
                        self.member_display_name_in(&[&record.state, previous], id)
                    }
                ),
                GroupChange::OwnershipTransferred(id) if id == &me => {
                    notable = true;
                    "Вы стали владельцем группы".to_owned()
                }
                GroupChange::OwnershipTransferred(id) => format!(
                    "Владелец группы теперь {}",
                    self.member_display_name_in(&[&record.state, previous], id)
                ),
                GroupChange::Renamed(name) => format!(
                    "{actor_name} {} группу в «{name}»",
                    verb("переименовали", "переименовал(а)")
                ),
                GroupChange::AboutChanged => format!(
                    "{actor_name} {} описание группы",
                    verb("изменили", "изменил(а)")
                ),
                GroupChange::AvatarChanged => format!(
                    "{actor_name} {} фото группы",
                    verb("изменили", "изменил(а)")
                ),
                GroupChange::PermissionsChanged => format!(
                    "{actor_name} {} права участников",
                    verb("изменили", "изменил(а)")
                ),
            };
            let id = if index == 0 {
                event_id.to_owned()
            } else {
                format!("{event_id}.{index}")
            };
            self.save_service(&record.state.group_id, &id, actor, created_at, text, !notable)?;
        }
        Ok(())
    }

    fn save_service(
        &mut self,
        group_id: &str,
        event_id: &str,
        actor: &str,
        created_at: i64,
        text: String,
        read: bool,
    ) -> Result<(), CoreError> {
        let outgoing = actor == self.identity.public.user_id;
        self.store.save_message(&Message {
            event_id: event_id.to_owned(),
            conversation_id: group_id.to_owned(),
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
        })
    }

    /// Имя участника: своё имя контакта, иначе то, под которым его добавили.
    pub(super) fn member_display_name(&self, state: Option<&GroupState>, user_id: &str) -> String {
        match state {
            Some(state) => self.member_display_name_in(&[state], user_id),
            None => self.member_display_name_in(&[], user_id),
        }
    }

    fn member_display_name_in(&self, states: &[&GroupState], user_id: &str) -> String {
        if user_id == self.identity.public.user_id {
            let own = self.store.profile().map(|value| value.display_name).unwrap_or_default();
            return if own.trim().is_empty() { "Вы".to_owned() } else { own };
        }
        if let Ok(Some(contact)) = self.store.contact(user_id) {
            return contact.display_name;
        }
        states
            .iter()
            .find_map(|state| member(state, user_id))
            .map(|value| value.display_name.clone())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| short_id(user_id))
    }

    pub(super) fn group_view(&self, record: &GroupRecord) -> Result<GroupView, CoreError> {
        let me = self.identity.public.user_id.clone();
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
                .or_else(|| member(&record.state, user_id).map(|value| value.display_name.clone()))
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| short_id(user_id))
        };
        let mut members: Vec<GroupMemberView> = record
            .state
            .members
            .iter()
            .map(|value| {
                let is_self = value.user_id == me;
                let contact = contacts.get(&value.user_id);
                GroupMemberView {
                    user_id: value.user_id.clone(),
                    display_name: if is_self {
                        if own.display_name.trim().is_empty() {
                            "Вы".to_owned()
                        } else {
                            own.display_name.clone()
                        }
                    } else {
                        name_of(&value.user_id)
                    },
                    avatar_base64: if is_self {
                        own.avatar_base64.clone()
                    } else {
                        contact.and_then(|contact| contact.avatar_base64.clone())
                    },
                    role: value.role,
                    is_self,
                    is_contact: contact.is_some_and(|contact| !contact.pending_approval),
                    confirmed: is_self || record.confirmed_members.contains(&value.user_id),
                    added_by_name: name_of(&value.added_by),
                }
            })
            .collect();
        members.sort_by(|left, right| {
            right
                .role
                .cmp(&left.role)
                .then_with(|| right.is_self.cmp(&left.is_self))
                .then_with(|| left.display_name.to_lowercase().cmp(&right.display_name.to_lowercase()))
        });
        let my_role = if record.left { None } else { role_of(&record.state, &me) };
        let active = my_role.is_some() && !record.pending_invite;
        let at_least_admin = active && my_role >= Some(GroupRole::Admin);
        Ok(GroupView {
            group_id: record.state.group_id.clone(),
            name: record.state.name.clone(),
            about: record.state.about.clone(),
            avatar_base64: record.state.avatar_base64.clone(),
            epoch: record.state.epoch,
            created_by: record.state.created_by.clone(),
            created_at_unix_milliseconds: record.state.created_at_unix_milliseconds,
            my_role,
            pending_invite: record.pending_invite,
            invited_by_name: record.invited_by.as_deref().map(name_of),
            left: record.left,
            permissions: record.state.permissions,
            members,
            can_send: active,
            can_invite: active
                && (at_least_admin || record.state.permissions.members_can_invite)
                && record.state.members.len() < MAX_GROUP_MEMBERS,
            can_edit_info: active && (at_least_admin || record.state.permissions.members_can_edit_info),
            can_remove_members: at_least_admin,
            can_manage_admins: active && my_role == Some(GroupRole::Owner),
            can_delete_messages: at_least_admin,
        })
    }
}

fn truncate(value: &str, limit: usize) -> String {
    value.trim().chars().take(limit).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(index: u8) -> String {
        format!("tt1-{}", hex::encode([index; 32]))
    }

    fn base() -> GroupState {
        let owner = user(1);
        GroupState {
            version: GROUP_STATE_VERSION,
            group_id: format!("ttg1-{}", hex::encode([9u8; 32])),
            epoch: 1,
            name: "Команда".to_owned(),
            about: String::new(),
            avatar_base64: None,
            created_by: owner.clone(),
            created_at_unix_milliseconds: 1,
            members: vec![
                GroupMember {
                    user_id: owner.clone(),
                    display_name: "Владелец".to_owned(),
                    role: GroupRole::Owner,
                    added_by: owner.clone(),
                    added_at_unix_milliseconds: 1,
                },
                GroupMember {
                    user_id: user(2),
                    display_name: "Админ".to_owned(),
                    role: GroupRole::Admin,
                    added_by: owner.clone(),
                    added_at_unix_milliseconds: 1,
                },
                GroupMember {
                    user_id: user(3),
                    display_name: "Участник".to_owned(),
                    role: GroupRole::Member,
                    added_by: owner.clone(),
                    added_at_unix_milliseconds: 1,
                },
            ],
            permissions: GroupPermissions::default(),
            updated_by: owner,
            updated_at_unix_milliseconds: 1,
        }
    }

    fn next(old: &GroupState, actor: &str, change: impl FnOnce(&mut GroupState)) -> GroupState {
        let mut new = old.clone();
        change(&mut new);
        new.epoch = old.epoch + 1;
        new.updated_by = actor.to_owned();
        new
    }

    fn newcomer(index: u8, added_by: &str) -> GroupMember {
        GroupMember {
            user_id: user(index),
            display_name: format!("Новичок {index}"),
            role: GroupRole::Member,
            added_by: added_by.to_owned(),
            added_at_unix_milliseconds: 2,
        }
    }

    #[test]
    fn admin_adds_a_member() {
        let old = base();
        let new = next(&old, &user(2), |state| state.members.push(newcomer(4, &user(2))));
        assert_eq!(
            validate_transition(&old, &new, &user(2), false).unwrap(),
            vec![GroupChange::Added(vec![user(4)])]
        );
    }

    #[test]
    fn member_invites_only_when_allowed() {
        let old = base();
        let new = next(&old, &user(3), |state| state.members.push(newcomer(4, &user(3))));
        assert!(validate_transition(&old, &new, &user(3), false).is_err());

        let mut open = base();
        open.permissions.members_can_invite = true;
        let new = next(&open, &user(3), |state| state.members.push(newcomer(4, &user(3))));
        assert!(validate_transition(&open, &new, &user(3), false).is_ok());
    }

    #[test]
    fn a_newcomer_cannot_arrive_as_admin() {
        let old = base();
        let new = next(&old, &user(1), |state| {
            let mut value = newcomer(4, &user(1));
            value.role = GroupRole::Admin;
            state.members.push(value);
        });
        assert!(validate_transition(&old, &new, &user(1), false).is_err());
    }

    #[test]
    fn a_member_cannot_promote_themselves() {
        let old = base();
        let new = next(&old, &user(3), |state| state.members[2].role = GroupRole::Admin);
        assert!(validate_transition(&old, &new, &user(3), false).is_err());
    }

    #[test]
    fn an_admin_cannot_remove_the_owner_or_another_admin() {
        let mut old = base();
        old.members[2].role = GroupRole::Admin;
        let new = next(&old, &user(2), |state| state.members.retain(|m| m.user_id != user(1)));
        assert!(validate_transition(&old, &new, &user(2), false).is_err());
        let new = next(&old, &user(2), |state| state.members.retain(|m| m.user_id != user(3)));
        assert!(validate_transition(&old, &new, &user(2), false).is_err());
    }

    #[test]
    fn an_admin_removes_a_member() {
        let old = base();
        let new = next(&old, &user(2), |state| state.members.retain(|m| m.user_id != user(3)));
        assert_eq!(
            validate_transition(&old, &new, &user(2), false).unwrap(),
            vec![GroupChange::Removed(vec![user(3)])]
        );
    }

    #[test]
    fn only_the_owner_assigns_roles() {
        let old = base();
        let by_admin = next(&old, &user(2), |state| state.members[2].role = GroupRole::Admin);
        assert!(validate_transition(&old, &by_admin, &user(2), false).is_err());
        let by_owner = next(&old, &user(1), |state| state.members[2].role = GroupRole::Admin);
        assert_eq!(
            validate_transition(&old, &by_owner, &user(1), false).unwrap(),
            vec![GroupChange::RoleChanged(user(3), GroupRole::Admin)]
        );
    }

    #[test]
    fn ownership_moves_only_from_the_owner() {
        let old = base();
        let transfer = next(&old, &user(1), |state| {
            state.members[0].role = GroupRole::Admin;
            state.members[2].role = GroupRole::Owner;
        });
        assert_eq!(
            validate_transition(&old, &transfer, &user(1), false).unwrap(),
            vec![GroupChange::OwnershipTransferred(user(3))]
        );
        let seized = next(&old, &user(2), |state| {
            state.members[0].role = GroupRole::Admin;
            state.members[1].role = GroupRole::Owner;
        });
        assert!(validate_transition(&old, &seized, &user(2), false).is_err());
    }

    #[test]
    fn the_owner_leaves_only_after_handing_over() {
        let old = base();
        let orphaned = next(&old, &user(1), |state| state.members.retain(|m| m.user_id != user(1)));
        assert!(validate_transition(&old, &orphaned, &user(1), false).is_err());
        let handed = next(&old, &user(1), |state| {
            state.members[1].role = GroupRole::Owner;
            state.members.retain(|m| m.user_id != user(1));
        });
        assert_eq!(
            validate_transition(&old, &handed, &user(1), false).unwrap(),
            vec![GroupChange::OwnershipTransferred(user(2)), GroupChange::Left]
        );
    }

    #[test]
    fn anyone_may_leave_but_not_while_changing_the_group() {
        let old = base();
        let left = next(&old, &user(3), |state| state.members.retain(|m| m.user_id != user(3)));
        assert_eq!(
            validate_transition(&old, &left, &user(3), false).unwrap(),
            vec![GroupChange::Left]
        );
        let sneaky = next(&old, &user(2), |state| {
            state.name = "Захвачено".to_owned();
            state.members.retain(|m| m.user_id != user(2));
        });
        assert!(validate_transition(&old, &sneaky, &user(2), false).is_err());
    }

    #[test]
    fn info_edits_follow_permissions() {
        let old = base();
        let by_member = next(&old, &user(3), |state| state.name = "Новое".to_owned());
        assert!(validate_transition(&old, &by_member, &user(3), false).is_err());
        let by_admin = next(&old, &user(2), |state| state.name = "Новое".to_owned());
        assert_eq!(
            validate_transition(&old, &by_admin, &user(2), false).unwrap(),
            vec![GroupChange::Renamed("Новое".to_owned())]
        );
        let loosened = next(&old, &user(3), |state| state.permissions.members_can_edit_info = true);
        assert!(validate_transition(&old, &loosened, &user(3), false).is_err());
    }

    #[test]
    fn history_and_signature_cannot_be_forged() {
        let old = base();
        let rollback = GroupState { epoch: 1, ..next(&old, &user(1), |s| s.name = "X".to_owned()) };
        assert!(validate_transition(&old, &rollback, &user(1), false).is_err());

        let mut forged = next(&old, &user(1), |state| state.name = "X".to_owned());
        forged.updated_by = user(1);
        assert!(validate_transition(&old, &forged, &user(3), false).is_err());

        let recreated = next(&old, &user(1), |state| state.created_by = user(2));
        assert!(validate_transition(&old, &recreated, &user(1), false).is_err());

        let outsider = next(&old, &user(7), |state| state.name = "X".to_owned());
        assert!(validate_transition(&old, &outsider, &user(7), false).is_err());

        let renamed_member = next(&old, &user(1), |state| state.members[2].display_name = "Шпион".to_owned());
        assert!(validate_transition(&old, &renamed_member, &user(1), false).is_err());
    }

    #[test]
    fn a_moderator_outranks_the_author() {
        let state = base();
        assert!(may_moderate(&state, &user(1), &user(2)));
        assert!(may_moderate(&state, &user(2), &user(3)));
        assert!(!may_moderate(&state, &user(2), &user(1)));
        assert!(!may_moderate(&state, &user(3), &user(2)));
        // Автор, уже покинувший группу, считается рядовым участником.
        assert!(may_moderate(&state, &user(2), &user(8)));
    }

    #[test]
    fn invites_must_come_from_inside_the_group() {
        let state = base();
        assert!(validate_invite(&state, &user(1), &user(3)).is_ok());
        assert!(validate_invite(&state, &user(3), &user(2)).is_err(), "updated_by — владелец");
        assert!(validate_invite(&state, &user(1), &user(5)).is_err());
    }
}
