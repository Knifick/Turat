//! Синхронизация устройств одного аккаунта.
//!
//! Все устройства аккаунта — полноценные устройства одной личности: у каждого свои ключи, ящик и
//! сессии, а собеседники шифруют каждое сообщение под все устройства из подписанного списка. Так
//! входящие приходят на все устройства сами. Синхронизировать остаётся то, что устройство делает
//! само: отправленные сообщения, правки, прочтения, контакты, группы, каналы, профиль.
//!
//! Это делается двумя путями:
//!
//! * **живые изменения** — каждое сохранение записи попадает в журнал (`sync_changes`), и при
//!   синхронизации новые записи уходят другим своим устройствам событием `sync.records` — тем же
//!   сквозным шифрованием через ящики Node, что и обычные сообщения. Node видит только конверты;
//! * **снимок** — всё состояние, сжатое и зашифрованное ключом аккаунта, лежит на Node. Его берёт
//!   новое устройство при входе и устройство, которое долго было выключено и пропустило события.
//!
//! При встречных правках одной записи побеждает более поздняя (отметка времени растёт строго), а
//! «необратимые» признаки сообщения — доставлено, прочитано, удалено — только складываются.

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};

use super::{AppCore, delivery::attachment_path};
use crate::{
    CoreError,
    account::{self, DeviceName, LocalAccount},
    blobs::AttachmentManifest,
    mailbox::MailboxGrant,
    models::{ChannelRecord, ChannelSubscriber, Contact, GroupRecord, Message, Profile},
    network::NodeDescriptor,
    protocol::{
        KIND_DEVICE_HELLO, KIND_DEVICE_SIGNED_OUT, KIND_SYNC_RECORDS, KIND_SYNC_SNAPSHOT,
        PROTOCOL_VERSION, SignedProtocolEvent, WireIdentity,
    },
    routing::{DeviceRevocation, SignedDeviceList, SignedRoutingDescriptor},
    store::{
        self, SYNC_CHANNEL, SYNC_CHANNEL_POST, SYNC_CHANNEL_SUBSCRIBER, SYNC_CONTACT,
        SYNC_DEVICE_NAME, SYNC_GRANT, SYNC_GROUP, SYNC_MESSAGE, SYNC_PROFILE, Store,
        StoredChannelPost, SyncChange,
    },
};

/// Пачка записей в одном событии: конверт Node вмещает до 512 КиБ.
const RECORDS_BATCH_BYTES: usize = 192 * 1024;
/// Сколько изменений разбирать за один цикл синхронизации.
const CHANGES_PER_CYCLE: usize = 2_000;
/// Как часто сверять список устройств с Node.
const DEVICE_REFRESH_MILLISECONDS: i64 = 2 * 60_000;
/// Как часто проверять, не выложило ли другое устройство снимок новее нашего.
const SNAPSHOT_CHECK_MILLISECONDS: i64 = 30 * 60_000;
/// Плановое обновление снимка, если с прошлого что-то изменилось. Чаще — дорого для мобильной
/// сети: снимок несёт всю историю. Свежие изменения новое устройство всё равно получит сразу:
/// старые устройства, заметив его, выкладывают снимок вне очереди.
const SNAPSHOT_REFRESH_MILLISECONDS: i64 = 3_600_000;
/// Предел снимка на Node — с запасом под шифрование.
const SNAPSHOT_MAX_BYTES: usize = 22 * 1024 * 1024;
/// История в снимке: последние сообщения каждого диалога. Если снимок не влезает — меньше.
const SNAPSHOT_MESSAGES_PER_CHAT: [usize; 3] = [3_000, 800, 200];

/// Одна запись синхронизации. `None` в содержимом — запись удалена.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum SyncRecord {
    Profile {
        stamp: i64,
        profile: Profile,
    },
    Contact {
        stamp: i64,
        user_id: String,
        contact: Option<Contact>,
    },
    Message {
        stamp: i64,
        event_id: String,
        message: Option<Message>,
        #[serde(default)]
        manifest: Option<AttachmentManifest>,
    },
    Group {
        stamp: i64,
        group_id: String,
        record: Option<GroupRecord>,
    },
    Channel {
        stamp: i64,
        channel_id: String,
        record: Option<ChannelRecord>,
    },
    ChannelPost {
        stamp: i64,
        event_id: String,
        channel_id: String,
        created_at_unix_milliseconds: i64,
        post: Option<StoredChannelPost>,
    },
    ChannelSubscriber {
        stamp: i64,
        channel_id: String,
        user_id: String,
        subscriber: Option<ChannelSubscriber>,
    },
    Grant {
        user_id: String,
        device_id: String,
        grant: MailboxGrant,
    },
    DeviceName {
        stamp: i64,
        device_id: String,
        name: DeviceName,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SyncRecordsPayload {
    version: i32,
    records: Vec<SyncRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotNoticePayload {
    version: i32,
    snapshot_version: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct DevicePayload {
    pub version: i32,
    pub device_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotDocument {
    version: i32,
    created_at_unix_milliseconds: i64,
    records: Vec<SyncRecord>,
}

impl AppCore {
    /// Другие устройства аккаунта по последнему известному списку.
    pub(super) fn other_devices(&self) -> Result<Vec<String>, CoreError> {
        let mine = &self.identity.public.device_id;
        Ok(self
            .store
            .device_list()?
            .map(|list| {
                list.document
                    .devices
                    .iter()
                    .map(|device| device.device_id.clone())
                    .filter(|device| device != mine)
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Обслуживание аккаунта в каждом цикле синхронизации. Ошибки здесь не мешают переписке:
    /// они видны в строке состояния, а следующий цикл попробует снова.
    pub(super) fn account_maintenance(&mut self, node: &NodeDescriptor) {
        let Ok(Some(account)) = self.store.account() else {
            return;
        };
        let now = chrono::Utc::now().timestamp_millis();
        if account.base_url.trim_end_matches('/') != node.base_url.trim_end_matches('/') {
            // Перенос стоит запросов к Node: пробуем не чаще, чем сверяем устройства.
            if now - self.last_device_refresh_unix_milliseconds >= DEVICE_REFRESH_MILLISECONDS {
                self.last_device_refresh_unix_milliseconds = now;
                if let Err(error) = self.migrate_account(node) {
                    self.status = format!("Аккаунт не перенесён на новый Node: {error}");
                }
            }
            return;
        }
        if now - self.last_device_refresh_unix_milliseconds >= DEVICE_REFRESH_MILLISECONDS {
            self.last_device_refresh_unix_milliseconds = now;
            if let Err(error) = self.refresh_account_devices(node) {
                self.status = format!("Список устройств не обновлён: {error}");
            }
            if self.store.account().ok().flatten().is_none() {
                // Сеанс завершён с другого устройства — дальше делать нечего.
                return;
            }
        }
        if let Err(error) = self.push_sync_records() {
            self.status = format!("Изменения не разосланы другим устройствам: {error}");
        }
        if let Err(error) = self.snapshot_maintenance(now) {
            self.status = format!("Снимок данных аккаунта не обновлён: {error}");
        }
    }

    /// Сверка своего списка устройств с тем, что опубликовано на Node.
    pub(super) fn refresh_account_devices(&mut self, node: &NodeDescriptor) -> Result<(), CoreError> {
        let me = self.me();
        if let Some(remote) = self.network.routing(node, &me)?
            && remote.descriptor.user_id == me
            && remote.verify()
        {
            if !self.adopt_device_list(&remote.descriptor.device_list)? {
                return Ok(());
            }
            let published = remote
                .descriptor
                .devices
                .iter()
                .any(|entry| entry.identity.device_id == self.identity.public.device_id);
            // Другое устройство опубликовало адрес без нас (гонка при входе) — публикуемся заново.
            if !published {
                self.store.forget_own_routing()?;
            }
            self.store.save_peer_routing(&remote)?;
        }
        self.note_device_changes()
    }

    /// Принять список устройств, если он новее своего. `false` — это устройство из аккаунта
    /// удалили, и оно уже вышло.
    pub(super) fn adopt_device_list(&mut self, list: &SignedDeviceList) -> Result<bool, CoreError> {
        let me = self.me();
        if list.document.user_id != me || !list.verify() {
            return Ok(true);
        }
        let local_sequence = self
            .store
            .device_list()?
            .map_or(0, |local| local.document.sequence);
        if list.document.sequence <= local_sequence {
            return Ok(true);
        }
        if self.store.account()?.is_some()
            && list
                .document
                .revocations
                .iter()
                .any(|revocation| revocation.device_id == self.identity.public.device_id)
        {
            self.sign_out_local(Some(
                "Этот сеанс завершён с другого устройства. Войдите снова, чтобы продолжить.",
            ))?;
            return Ok(false);
        }
        self.store.save_device_list(list)?;
        if !list.contains(&self.identity.public.device_id) {
            // Нас нет в списке, но и отзыва нет: другое устройство опубликовало список, не зная о
            // нас. Добавимся заново при ближайшей публикации адреса.
            self.store.forget_own_routing()?;
        }
        Ok(true)
    }

    /// Новые устройства в аккаунте получают свежий снимок, ушедшие забываются.
    fn note_device_changes(&mut self) -> Result<(), CoreError> {
        let Some(mut account) = self.store.account()? else {
            return Ok(());
        };
        let current = self.other_devices()?;
        let fresh = current
            .iter()
            .any(|device| !account.known_devices.contains(device));
        if fresh || current.len() != account.known_devices.len() {
            if fresh {
                account.snapshot_requested = true;
            }
            account.known_devices = current;
            self.store.save_account(&account)?;
        }
        Ok(())
    }

    /// Разослать другим своим устройствам всё, что изменилось с прошлого раза.
    pub(super) fn push_sync_records(&mut self) -> Result<(), CoreError> {
        let Some(mut account) = self.store.account()? else {
            return Ok(());
        };
        if self.other_devices()?.is_empty() {
            // Одно устройство: рассылать некому. Всё, что было, войдёт в снимок.
            let latest = self.store.sync_max_seq()?;
            if account.sync_seq != latest {
                account.sync_seq = latest;
                self.store.save_account(&account)?;
            }
            return Ok(());
        }
        let changes = self.store.sync_changes_since(account.sync_seq, CHANGES_PER_CYCLE)?;
        if changes.is_empty() {
            return Ok(());
        }
        let mut batch = Vec::new();
        let mut size = 0;
        for change in &changes {
            if let Some(record) = self.sync_record(change)? {
                size += serde_json::to_vec(&record)?.len();
                batch.push(record);
            }
            if size >= RECORDS_BATCH_BYTES {
                self.send_sync_records(std::mem::take(&mut batch))?;
                size = 0;
            }
        }
        if !batch.is_empty() {
            self.send_sync_records(batch)?;
        }
        account.sync_seq = changes.last().map_or(account.sync_seq, |change| change.seq);
        self.store.save_account(&account)?;
        Ok(())
    }

    fn send_sync_records(&mut self, records: Vec<SyncRecord>) -> Result<(), CoreError> {
        let me = self.me();
        self.queue_event(
            &me,
            &format!("evt1-{}", super::random_hex(16)),
            KIND_SYNC_RECORDS,
            &SyncRecordsPayload {
                version: PROTOCOL_VERSION,
                records,
            },
        )
    }

    /// Запись в её текущем виде. `None` — посылать нечего (например, ключ записи уже истёк).
    fn sync_record(&self, change: &SyncChange) -> Result<Option<SyncRecord>, CoreError> {
        let stamp = change.stamp;
        let key = change.key.clone();
        Ok(match change.kind.as_str() {
            SYNC_PROFILE => Some(SyncRecord::Profile {
                stamp,
                profile: self.store.profile()?,
            }),
            SYNC_CONTACT => Some(SyncRecord::Contact {
                stamp,
                contact: self.store.contact(&key)?,
                user_id: key,
            }),
            SYNC_MESSAGE => Some(SyncRecord::Message {
                stamp,
                message: self.store.message(&key)?,
                manifest: self.manifest_of(&key)?,
                event_id: key,
            }),
            SYNC_GROUP => Some(SyncRecord::Group {
                stamp,
                record: self.store.group(&key)?,
                group_id: key,
            }),
            SYNC_CHANNEL => Some(SyncRecord::Channel {
                stamp,
                record: self.store.channel(&key)?,
                channel_id: key,
            }),
            SYNC_CHANNEL_POST => {
                let post = self.store.channel_post(&key)?;
                let (channel_id, created) = match self.store.channel_post_place(&key)? {
                    Some(place) => place,
                    None => match &post {
                        Some(post) => (
                            post.post.event.conversation_id.clone(),
                            post.post.event.created_at_unix_milliseconds,
                        ),
                        // Удалённый пост: канал не важен, получатель удаляет по номеру события.
                        None => (String::new(), 0),
                    },
                };
                Some(SyncRecord::ChannelPost {
                    stamp,
                    event_id: key,
                    channel_id,
                    created_at_unix_milliseconds: created,
                    post,
                })
            }
            SYNC_CHANNEL_SUBSCRIBER => {
                let Some((channel_id, user_id)) = key.split_once('|') else {
                    return Ok(None);
                };
                Some(SyncRecord::ChannelSubscriber {
                    stamp,
                    subscriber: self.store.channel_subscriber(channel_id, user_id)?,
                    channel_id: channel_id.to_owned(),
                    user_id: user_id.to_owned(),
                })
            }
            SYNC_GRANT => {
                let mut parts = key.splitn(3, '|');
                let (Some(user_id), Some(device_id), Some(mailbox_id)) =
                    (parts.next(), parts.next(), parts.next())
                else {
                    return Ok(None);
                };
                self.store
                    .grant(user_id, device_id, mailbox_id)?
                    .filter(MailboxGrant::alive)
                    .map(|grant| SyncRecord::Grant {
                        user_id: user_id.to_owned(),
                        device_id: device_id.to_owned(),
                        grant,
                    })
            }
            SYNC_DEVICE_NAME => self.store.device_names()?.remove(&key).map(|name| {
                SyncRecord::DeviceName {
                    stamp,
                    device_id: key,
                    name,
                }
            }),
            _ => None,
        })
    }

    /// Манифест вложения: по нему другое устройство скачает файл сам.
    fn manifest_of(&self, event_id: &str) -> Result<Option<AttachmentManifest>, CoreError> {
        if let Some(manifest) = self.store.event_manifest(event_id)? {
            return Ok(Some(manifest));
        }
        Ok(self
            .store
            .pending_blobs()?
            .into_iter()
            .find(|(id, _)| id == event_id)
            .map(|(_, manifest)| manifest))
    }

    /// Событие от другого своего устройства.
    pub(super) fn apply_self_event(
        &mut self,
        event: &SignedProtocolEvent,
        sender: &WireIdentity,
        routing: &SignedRoutingDescriptor,
    ) -> Result<bool, CoreError> {
        if sender.device_id == self.identity.public.device_id {
            return Ok(false);
        }
        // Ничего, кроме служебных событий синхронизации, своё устройство не присылает.
        if self.store.account()?.is_none() {
            return Ok(false);
        }
        if !self.adopt_device_list(&routing.descriptor.device_list)? {
            return Ok(false);
        }
        // Изменения принимаются только от устройств, которые есть в нашем списке.
        let listed = self
            .store
            .device_list()?
            .is_some_and(|list| list.contains(&sender.device_id));
        if !listed {
            return Ok(false);
        }
        match event.kind.as_str() {
            KIND_SYNC_RECORDS => {
                let payload: SyncRecordsPayload = event.decode_payload()?;
                let me = self.me();
                let changed = self
                    .store
                    .quietly(|| apply_records(&self.store, &me, payload.records))?;
                self.start_pending_downloads();
                Ok(changed)
            }
            KIND_SYNC_SNAPSHOT => {
                let payload: SnapshotNoticePayload = event.decode_payload()?;
                let Some(mut account) = self.store.account()? else {
                    return Ok(false);
                };
                if payload.snapshot_version > account.snapshot_version {
                    self.merge_remote_snapshot(&mut account)?;
                    self.store.save_account(&account)?;
                    return Ok(true);
                }
                Ok(false)
            }
            KIND_DEVICE_HELLO => {
                self.note_device_changes()?;
                Ok(false)
            }
            KIND_DEVICE_SIGNED_OUT => {
                let payload: DevicePayload = event.decode_payload()?;
                if payload.device_id == sender.device_id {
                    // Устройство вышло само — убираем его из списка.
                    self.remove_device_from_account(&payload.device_id, "signed_out")?;
                } else if payload.device_id == self.identity.public.device_id {
                    // Сеанс этого устройства завершили с другого: подпись и место отправителя
                    // в списке устройств уже проверены при приёме конверта.
                    self.sign_out_local(Some(
                        "Этот сеанс завершён с другого устройства. Войдите снова, чтобы продолжить.",
                    ))?;
                    return Ok(true);
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }

    /// Убрать устройство из подписанного списка и опубликовать адрес без него.
    pub(super) fn remove_device_from_account(
        &mut self,
        device_id: &str,
        reason: &str,
    ) -> Result<(), CoreError> {
        if device_id == self.identity.public.device_id {
            return Err(CoreError::InvalidInput(
                "Чтобы выйти на этом устройстве, используйте «Выйти из аккаунта»".to_owned(),
            ));
        }
        if !self.identity.is_authority() {
            return Err(CoreError::InvalidInput(
                "Завершать сеансы может только устройство, вошедшее в аккаунт".to_owned(),
            ));
        }
        let Some(list) = self.store.device_list()? else {
            return Ok(());
        };
        let known = list.contains(device_id);
        let revoked = list
            .document
            .revocations
            .iter()
            .any(|revocation| revocation.device_id == device_id);
        if !known && revoked {
            return Ok(());
        }
        let devices: Vec<WireIdentity> = list
            .document
            .devices
            .iter()
            .filter(|device| device.device_id != device_id)
            .cloned()
            .collect();
        let mut revocations = list.document.revocations.clone();
        if !revoked {
            revocations.push(DeviceRevocation {
                device_id: device_id.to_owned(),
                revoked_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
                reason: reason.to_owned(),
            });
        }
        let updated = SignedDeviceList::create(
            &self.identity,
            devices,
            revocations,
            list.document.sequence + 1,
        )?;
        self.store.save_device_list(&updated)?;
        self.store.revoke_device(device_id)?;
        self.store.forget_own_routing()?;
        if let Some(mut account) = self.store.account()? {
            account.known_devices.retain(|known| known != device_id);
            self.store.save_account(&account)?;
        }
        Ok(())
    }

    /// Снимок: проверить, нет ли на Node более нового, и выложить свой, когда пора.
    fn snapshot_maintenance(&mut self, now: i64) -> Result<(), CoreError> {
        let Some(mut account) = self.store.account()? else {
            return Ok(());
        };
        if now - account.last_snapshot_check_unix_milliseconds >= SNAPSHOT_CHECK_MILLISECONDS {
            account.last_snapshot_check_unix_milliseconds = now;
            let meta = self
                .network
                .account_meta(&account.base_url, &account.account_id, &account.access_token)?;
            if meta.snapshot_version > account.snapshot_version {
                self.merge_remote_snapshot(&mut account)?;
            }
            self.store.save_account(&account)?;
        }
        let changed = self.store.sync_max_seq()? > account.snapshot_seq;
        let due = now - account.snapshot_uploaded_at_unix_milliseconds >= SNAPSHOT_REFRESH_MILLISECONDS;
        if account.snapshot_requested || (changed && due) {
            let fresh_devices = account.snapshot_requested && !self.other_devices()?.is_empty();
            self.upload_snapshot()?;
            if fresh_devices {
                // Новое устройство уже взяло снимок при входе; этот — свежее, с тем, что
                // изменилось между его снимком и появлением в списке устройств.
                if let Some(account) = self.store.account()? {
                    let me = self.me();
                    self.queue_event(
                        &me,
                        &format!("evt1-{}", super::random_hex(16)),
                        KIND_SYNC_SNAPSHOT,
                        &SnapshotNoticePayload {
                            version: PROTOCOL_VERSION,
                            snapshot_version: account.snapshot_version,
                        },
                    )?;
                }
            }
        }
        Ok(())
    }

    /// Собрать снимок и выложить его на Node поверх известной версии. Если другое устройство
    /// успело выложить свой — сначала слить его, потом повторить.
    pub(super) fn upload_snapshot(&mut self) -> Result<(), CoreError> {
        for _ in 0..3 {
            let Some(mut account) = self.store.account()? else {
                return Ok(());
            };
            let key = account.key()?;
            let seq = self.store.sync_max_seq()?;
            let sealed = self.sealed_snapshot(&key)?;
            match self.network.put_account_snapshot(
                &account.base_url,
                &account.account_id,
                &account.access_token,
                sealed,
                account.snapshot_version,
            )? {
                Some(version) => {
                    account.snapshot_version = version;
                    account.snapshot_seq = seq;
                    account.snapshot_uploaded_at_unix_milliseconds =
                        chrono::Utc::now().timestamp_millis();
                    account.snapshot_requested = false;
                    self.store.save_account(&account)?;
                    return Ok(());
                }
                None => {
                    self.merge_remote_snapshot(&mut account)?;
                    self.store.save_account(&account)?;
                }
            }
        }
        Err(CoreError::Node(
            "снимок аккаунта всё время меняется другим устройством".to_owned(),
        ))
    }

    fn sealed_snapshot(&self, key: &[u8; 32]) -> Result<Vec<u8>, CoreError> {
        for limit in SNAPSHOT_MESSAGES_PER_CHAT {
            let document = SnapshotDocument {
                version: 1,
                created_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
                records: self.snapshot_records(limit)?,
            };
            let sealed = account::seal_snapshot(key, &serde_json::to_vec(&document)?)?;
            if sealed.len() <= SNAPSHOT_MAX_BYTES {
                return Ok(sealed);
            }
        }
        Err(CoreError::InvalidInput(
            "История слишком большая для снимка аккаунта".to_owned(),
        ))
    }

    /// Всё состояние аккаунта в виде записей синхронизации.
    fn snapshot_records(&self, messages_per_chat: usize) -> Result<Vec<SyncRecord>, CoreError> {
        let stamp = |kind: &str, key: &str| -> Result<i64, CoreError> {
            Ok(self.store.sync_stamp(kind, key)?.unwrap_or(1))
        };
        let mut records = vec![SyncRecord::Profile {
            stamp: stamp(SYNC_PROFILE, "")?,
            profile: self.store.profile()?,
        }];
        for (device_id, name) in self.store.device_names()? {
            records.push(SyncRecord::DeviceName {
                stamp: stamp(SYNC_DEVICE_NAME, &device_id)?,
                device_id,
                name,
            });
        }
        for contact in self.store.contacts()? {
            records.push(SyncRecord::Contact {
                stamp: stamp(SYNC_CONTACT, &contact.user_id)?,
                user_id: contact.user_id.clone(),
                contact: Some(contact),
            });
        }
        for group in self.store.groups()? {
            records.push(SyncRecord::Group {
                stamp: stamp(SYNC_GROUP, &group.state.group_id)?,
                group_id: group.state.group_id.clone(),
                record: Some(group),
            });
        }
        for channel in self.store.channels()? {
            let channel_id = channel.state.channel_id.clone();
            for post in self.store.recent_channel_posts(&channel_id, 500)? {
                let event_id = post.post.event.event_id.clone();
                records.push(SyncRecord::ChannelPost {
                    stamp: stamp(SYNC_CHANNEL_POST, &event_id)?,
                    channel_id: channel_id.clone(),
                    created_at_unix_milliseconds: post.post.event.created_at_unix_milliseconds,
                    event_id,
                    post: Some(post),
                });
            }
            for subscriber in self.store.channel_subscribers(&channel_id)? {
                records.push(SyncRecord::ChannelSubscriber {
                    stamp: stamp(
                        SYNC_CHANNEL_SUBSCRIBER,
                        &store::subscriber_key(&channel_id, &subscriber.user_id),
                    )?,
                    channel_id: channel_id.clone(),
                    user_id: subscriber.user_id.clone(),
                    subscriber: Some(subscriber),
                });
            }
            records.push(SyncRecord::Channel {
                stamp: stamp(SYNC_CHANNEL, &channel_id)?,
                channel_id,
                record: Some(channel),
            });
        }
        let mut per_chat: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for message in self.store.all_messages()?.into_iter().rev() {
            let count = per_chat.entry(message.conversation_id.clone()).or_default();
            if *count >= messages_per_chat {
                continue;
            }
            *count += 1;
            records.push(SyncRecord::Message {
                stamp: stamp(SYNC_MESSAGE, &message.event_id)?,
                event_id: message.event_id.clone(),
                manifest: self.manifest_of(&message.event_id)?,
                message: Some(message),
            });
        }
        for (user_id, device_id, grant) in self.store.all_grants()? {
            records.push(SyncRecord::Grant {
                user_id,
                device_id,
                grant,
            });
        }
        Ok(records)
    }

    /// Скачать снимок с Node и слить его с локальными данными.
    pub(super) fn merge_remote_snapshot(&mut self, account: &mut LocalAccount) -> Result<(), CoreError> {
        let Some((sealed, version)) =
            self.network
                .account_snapshot(&account.base_url, &account.account_id, &account.access_token)?
        else {
            return Ok(());
        };
        let key = account.key()?;
        let document: SnapshotDocument =
            serde_json::from_slice(&account::open_snapshot(&key, &sealed)?)?;
        let me = self.me();
        self.store
            .quietly(|| apply_records(&self.store, &me, document.records))?;
        account.snapshot_version = account.snapshot_version.max(version);
        self.start_pending_downloads();
        Ok(())
    }

    /// Сообщить своим устройствам и собеседникам, что появилось новое устройство: собеседники
    /// получат свежий адрес и начнут шифровать и под него.
    pub(super) fn announce_device(&mut self) -> Result<(), CoreError> {
        let me = self.me();
        let payload = DevicePayload {
            version: PROTOCOL_VERSION,
            device_id: self.identity.public.device_id.clone(),
        };
        self.queue_event(
            &me,
            &format!("evt1-{}", super::random_hex(16)),
            KIND_DEVICE_HELLO,
            &payload,
        )?;
        for contact in self.store.contacts()? {
            if contact.pending_approval
                || crate::protocol::is_group_id(&contact.user_id)
                || crate::protocol::is_channel_id(&contact.user_id)
            {
                continue;
            }
            self.queue_event(
                &contact.user_id,
                &format!("evt1-{}", super::random_hex(16)),
                KIND_DEVICE_HELLO,
                &payload,
            )?;
        }
        Ok(())
    }
}

/// Применить записи с другого устройства. Вызывается в «тихом» режиме хранилища — принятые
/// записи не уходят обратно эхом.
fn apply_records(store: &Store, me: &str, records: Vec<SyncRecord>) -> Result<bool, CoreError> {
    let mut changed = false;
    for record in records {
        changed |= apply_record(store, me, record)?;
    }
    Ok(changed)
}

/// Запись новее локальной? Тогда её отметка становится локальной.
fn newer(store: &Store, kind: &str, key: &str, stamp: i64) -> Result<bool, CoreError> {
    Ok(store.sync_stamp(kind, key)?.is_none_or(|local| stamp > local))
}

fn apply_record(store: &Store, me: &str, record: SyncRecord) -> Result<bool, CoreError> {
    match record {
        SyncRecord::Profile { stamp, profile } => {
            if !newer(store, SYNC_PROFILE, "", stamp)? {
                return Ok(false);
            }
            store.save_profile(&profile)?;
            store.set_sync_stamp(SYNC_PROFILE, "", stamp)?;
        }
        SyncRecord::Contact {
            stamp,
            user_id,
            contact,
        } => {
            if user_id == me || !newer(store, SYNC_CONTACT, &user_id, stamp)? {
                return Ok(false);
            }
            match contact {
                Some(mut contact) => {
                    if let Some(local) = store.contact(&user_id)? {
                        contact.last_seen_unix_milliseconds = contact
                            .last_seen_unix_milliseconds
                            .max(local.last_seen_unix_milliseconds);
                    }
                    store.save_contact(&contact)?;
                }
                None => store.delete_contact(&user_id)?,
            }
            store.set_sync_stamp(SYNC_CONTACT, &user_id, stamp)?;
        }
        SyncRecord::Message {
            stamp,
            event_id,
            message,
            manifest,
        } => {
            let local = store.message(&event_id)?;
            let take = newer(store, SYNC_MESSAGE, &event_id, stamp)?;
            match message {
                None => {
                    if !take {
                        return Ok(false);
                    }
                    store.delete_message(&event_id)?;
                }
                Some(incoming) => {
                    let mut message = match (&local, take) {
                        (Some(local), false) => local.clone(),
                        _ => incoming.clone(),
                    };
                    // Доставлено, прочитано и удалено — необратимо: такие признаки только
                    // складываются, кто бы ни победил по времени.
                    if let Some(local) = &local {
                        message.delivered |= local.delivered || incoming.delivered;
                        message.read |= local.read || incoming.read;
                        message.deleted |= local.deleted || incoming.deleted;
                    }
                    if message.deleted {
                        message.text.clear();
                        message.attachment = None;
                        message.reactions.clear();
                        message.reaction_marks.clear();
                    }
                    if let Some(attachment) = message.attachment.as_mut() {
                        let path = attachment_path(&store.app_dir, &attachment.attachment_id);
                        attachment.local_path = path.to_string_lossy().into_owned();
                        if let Some(manifest) = &manifest {
                            store.save_event_manifest(&event_id, manifest)?;
                            let now = chrono::Utc::now().timestamp_millis();
                            let alive = manifest
                                .blobs
                                .iter()
                                .all(|blob| blob.expires_at_unix_milliseconds > now);
                            if !path.exists() && alive {
                                store.save_pending_blob(&event_id, manifest)?;
                            }
                        }
                    }
                    if local.as_ref().is_some_and(|local| same_message(local, &message)) {
                        return Ok(false);
                    }
                    store.save_message(&message)?;
                    // Это же событие может прийти и от собеседника напрямую — второй раз
                    // применять его незачем.
                    store.mark_seen(&event_id)?;
                }
            }
            if take {
                store.set_sync_stamp(SYNC_MESSAGE, &event_id, stamp)?;
            }
        }
        SyncRecord::Group {
            stamp,
            group_id,
            record,
        } => {
            if !newer(store, SYNC_GROUP, &group_id, stamp)? {
                return Ok(false);
            }
            let Some(mut record) = record else {
                return Ok(false);
            };
            if let Some(local) = store.group(&group_id)?
                && local.state.epoch > record.state.epoch
            {
                // Своё состояние группы новее: у другого устройства не хватает изменений,
                // которые оно ещё получит само. Берём только то, что решает пользователь.
                let flags = record;
                record = local;
                record.pinned = flags.pinned;
                record.muted = flags.muted;
                record.draft = flags.draft;
                record.manual_unread = flags.manual_unread;
                record.hidden = flags.hidden;
            }
            store.save_group(&record)?;
            store.set_sync_stamp(SYNC_GROUP, &group_id, stamp)?;
        }
        SyncRecord::Channel {
            stamp,
            channel_id,
            record,
        } => {
            if !newer(store, SYNC_CHANNEL, &channel_id, stamp)? {
                return Ok(false);
            }
            let Some(mut record) = record else {
                return Ok(false);
            };
            if let Some(local) = store.channel(&channel_id)?
                && local.state.epoch > record.state.epoch
            {
                let flags = record;
                record = local;
                record.pinned = flags.pinned;
                record.muted = flags.muted;
                record.draft = flags.draft;
                record.manual_unread = flags.manual_unread;
                record.hidden = flags.hidden;
            }
            store.save_channel(&record)?;
            store.set_sync_stamp(SYNC_CHANNEL, &channel_id, stamp)?;
        }
        SyncRecord::ChannelPost {
            stamp,
            event_id,
            channel_id,
            created_at_unix_milliseconds,
            post,
        } => {
            if !newer(store, SYNC_CHANNEL_POST, &event_id, stamp)? {
                return Ok(false);
            }
            match post {
                Some(post) => store.save_channel_post(&channel_id, created_at_unix_milliseconds, &post)?,
                None => store.delete_channel_post(&event_id)?,
            }
            store.set_sync_stamp(SYNC_CHANNEL_POST, &event_id, stamp)?;
        }
        SyncRecord::ChannelSubscriber {
            stamp,
            channel_id,
            user_id,
            subscriber,
        } => {
            let key = store::subscriber_key(&channel_id, &user_id);
            if !newer(store, SYNC_CHANNEL_SUBSCRIBER, &key, stamp)? {
                return Ok(false);
            }
            match subscriber {
                Some(subscriber) => store.save_channel_subscriber(&channel_id, &subscriber)?,
                None => store.delete_channel_subscriber(&channel_id, &user_id)?,
            }
            store.set_sync_stamp(SYNC_CHANNEL_SUBSCRIBER, &key, stamp)?;
        }
        SyncRecord::Grant {
            user_id,
            device_id,
            grant,
        } => {
            if !grant.alive() || user_id == me {
                return Ok(false);
            }
            store.save_grant(&user_id, &device_id, &grant)?;
        }
        SyncRecord::DeviceName {
            stamp,
            device_id,
            name,
        } => {
            if !newer(store, SYNC_DEVICE_NAME, &device_id, stamp)? {
                return Ok(false);
            }
            store.save_device_name(&device_id, &name)?;
            store.set_sync_stamp(SYNC_DEVICE_NAME, &device_id, stamp)?;
        }
    }
    Ok(true)
}

fn same_message(left: &Message, right: &Message) -> bool {
    serde_json::to_vec(left).ok() == serde_json::to_vec(right).ok()
}

/// Ключ аккаунта в виде, в котором он лежит в локальной записи.
pub(super) fn encode_key(key: &[u8; 32]) -> String {
    STANDARD.encode(key)
}
