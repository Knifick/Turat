//! Доставка: очередь исходящих событий, забор входящих конвертов и применение их к истории.
//!
//! Локальная часть (`core.rs`) отвечает за то, что видит пользователь, и никогда не ждёт сеть.
//! Здесь — всё остальное: регистрация ящика, публикация адреса и предключей, шифрование
//! события под каждое устройство собеседника и разбор того, что пришло в ответ.

use std::sync::{Arc, atomic::Ordering};

use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::{ALLOWED_REACTIONS, AppCore, groups, set_reaction_mark, short_id};
use crate::{
    CoreError,
    blobs::{self, AttachmentManifest},
    identity::conversation_id,
    mailbox::{self, MailboxEnvelope, MailboxGrant, OwnedMailbox},
    media::Progress,
    models::{Attachment, Contact, GroupState, Message},
    network::NodeDescriptor,
    prekeys::{DESIRED_ONE_TIME_PREKEYS, PrekeyState},
    protocol::{
        AttachmentPayload, DeliveryPackageBody, EditPayload, KIND_ATTACHMENT, KIND_DELETE,
        KIND_EDIT, KIND_REACTION, KIND_RECEIPT_DELIVERY, KIND_RECEIPT_READ, KIND_TEXT,
        PROTOCOL_VERSION, ReactionPayload, SignedDeliveryPackage, SignedProtocolEvent,
        TargetPayload, TextPayload, WIRE_RATCHET, WIRE_SESSION_INIT, WireIdentity, WireMessage,
        KIND_CHANNEL_COMMENT, is_channel_id, is_group_id,
    },
    ratchet::{self, InitialSessionEnvelope, RatchetMessage},
    routing::{SignedDeviceList, SignedRoutingDescriptor},
    store::OutboxJob,
};

/// Публичный контактный ящик принимает только короткий текст: пока собеседник не ответил,
/// канал не должен превращаться в ретранслятор файлов и служебных событий.
const MAX_CONTACT_REQUEST_BYTES: usize = 4 * 1024;
const OUTBOX_BATCH: usize = 25;
/// Сообщение в большую группу — это десятки задач сразу. Фоновая синхронизация разбирает
/// очередь несколькими пачками, но не бесконечно: цикл не должен надолго держать ядро.
const OUTBOX_SYNC_LIMIT: usize = 250;
/// Задача группы, которую неделю не удаётся доставить (участник пропал из сети), уже
/// никому не нужна: конверт с ней всё равно истёк бы на Node.
const GROUP_JOB_TTL_MILLISECONDS: i64 = ENVELOPE_TTL_HOURS * 3_600_000;
const INBOX_BATCH: u32 = 100;
const ENVELOPE_TTL_HOURS: i64 = 7 * 24;
/// Нечитаемый конверт старше суток выбрасываем: чинить его уже нечем, а квота ящика конечна.
const UNREADABLE_ENVELOPE_GRACE_MILLISECONDS: i64 = 86_400_000;

impl AppCore {
    /// Готовит всё, без чего сообщение физически некуда положить: ящик, предключи и адрес.
    pub(super) fn ensure_transport(
        &mut self,
        node: &NodeDescriptor,
    ) -> Result<OwnedMailbox, CoreError> {
        let now = chrono::Utc::now().timestamp_millis();
        let mailbox = match self.store.mailbox()? {
            Some(value)
                if value.node_id == node.node_id
                    && value.expires_at_unix_milliseconds > now + 86_400_000 =>
            {
                value
            }
            _ => {
                let created = self
                    .network
                    .register_mailbox(node, &self.identity.public.device_id)?;
                self.store.save_mailbox(&created)?;
                created
            }
        };
        self.publish_watch(&mailbox);
        let prekey_sequence = self.ensure_prekeys(&mailbox)?;
        self.ensure_routing(node, &mailbox, prekey_sequence)?;
        Ok(mailbox)
    }

    /// Держит на Node свежую связку предключей. Номер публикации растёт, иначе Node
    /// отвергнет запись как повтор.
    fn ensure_prekeys(&mut self, mailbox: &OwnedMailbox) -> Result<i64, CoreError> {
        let mut state = match self.store.prekey_state()? {
            Some(value) => value,
            None => {
                let created = PrekeyState::create()?;
                self.store.save_prekey_state(&created)?;
                created
            }
        };
        let published = self.store.published_prekey_sequence()?;
        let depleted = state.one_time_prekeys.len() < DESIRED_ONE_TIME_PREKEYS / 2;
        let expiring = state.expires_at_unix_milliseconds
            < chrono::Utc::now().timestamp_millis() + 7 * 86_400_000;
        if published == state.sequence && !depleted && !expiring {
            return Ok(state.sequence);
        }
        state.refresh()?;
        self.store.save_prekey_state(&state)?;
        let publication = state.publication(&self.identity)?;
        self.network.publish_prekeys(mailbox, &publication)?;
        self.store.save_published_prekey_sequence(state.sequence)?;
        Ok(state.sequence)
    }

    /// Публикует адрес: список устройств, подписанный корневым ключом, и контактный ящик.
    fn ensure_routing(
        &mut self,
        node: &NodeDescriptor,
        mailbox: &OwnedMailbox,
        prekey_sequence: i64,
    ) -> Result<(), CoreError> {
        let stored = self.store.own_routing()?;
        let now = chrono::Utc::now().timestamp_millis();
        let current = stored.as_ref().is_some_and(|routing| {
            routing.descriptor.expires_at_unix_milliseconds > now + 3 * 86_400_000
                && routing.descriptor.devices.iter().any(|entry| {
                    entry.identity.device_id == self.identity.public.device_id
                        && entry.mailboxes.iter().any(|grant| {
                            grant.mailbox_id == mailbox.mailbox_id
                                && grant.write_capability == mailbox.contact_capability
                        })
                })
        });
        if current {
            return Ok(());
        }
        // Номер берём с запасом относительно того, что уже лежит на Node: чужая или
        // забытая публикация не должна блокировать нашу.
        let remote = self
            .network
            .routing(node, &self.identity.public.user_id)
            .ok()
            .flatten();
        // Другое устройство аккаунта могло обновить список устройств: публиковать адрес со
        // старым списком значило бы вычеркнуть его.
        if let Some(remote) = remote.as_ref().filter(|value| value.verify()) {
            if !self.adopt_device_list(&remote.descriptor.device_list)? {
                return Err(CoreError::InvalidInput(
                    "Этот сеанс завершён с другого устройства".to_owned(),
                ));
            }
        }
        let sequence = self
            .store
            .routing_sequence()?
            .max(stored.as_ref().map_or(0, |value| value.descriptor.sequence))
            .max(remote.as_ref().map_or(0, |value| value.descriptor.sequence))
            + 1;
        let previous = remote.as_ref().or(stored.as_ref());
        let device_list = self.ensure_device_list()?;
        let routing = SignedRoutingDescriptor::create(
            &self.identity,
            &device_list,
            mailbox,
            prekey_sequence,
            sequence,
            previous,
        )?;
        self.network.publish_routing(node, &routing)?;
        self.store.save_routing_sequence(sequence)?;
        self.store.save_own_routing(&routing)?;
        Ok(())
    }

    /// Список устройств с этим устройством. Если его в списке нет (только что вошли в аккаунт
    /// или другое устройство опубликовало список, не зная о нас) — добавляемся сами: у каждого
    /// устройства аккаунта есть ключ личности, которым список подписывается.
    fn ensure_device_list(&mut self) -> Result<SignedDeviceList, CoreError> {
        let stored = self.store.device_list()?;
        if let Some(list) = &stored
            && list.contains(&self.identity.public.device_id)
        {
            return Ok(list.clone());
        }
        if !self.identity.is_authority() {
            return Err(CoreError::InvalidInput(
                "Список устройств подписывает корневое устройство: свяжите его заново".to_owned(),
            ));
        }
        let me = self.identity.public.device_id.clone();
        let (mut devices, revocations, sequence) = match stored {
            Some(list) => (
                list.document.devices,
                list.document.revocations,
                list.document.sequence + 1,
            ),
            None => (Vec::new(), Vec::new(), 1),
        };
        devices.retain(|device| {
            device.device_id != me
                && !revocations
                    .iter()
                    .any(|revocation| revocation.device_id == device.device_id)
        });
        devices.push(WireIdentity::from(&self.identity.public));
        let list = SignedDeviceList::create(&self.identity, devices, revocations, sequence)?;
        self.store.save_device_list(&list)?;
        Ok(list)
    }

    /// Кладёт событие в очередь отправки. Ничего сетевого здесь не происходит:
    /// пользователь видит сообщение сразу, доставка идёт следом.
    pub(super) fn queue_event<T: serde::Serialize>(
        &mut self,
        user_id: &str,
        event_id: &str,
        kind: &str,
        payload: &T,
    ) -> Result<(), CoreError> {
        let conversation = conversation_id(&self.identity.public.user_id, user_id);
        let event = self.sign_event(&conversation, event_id, kind, payload)?;
        self.store.retry_now(user_id)?;
        self.store.enqueue_outbox(user_id, &event)
    }

    /// Подпись события устройства. Диалог входит в подпись: событие одного чата нельзя
    /// переложить в другой — ни в личный, ни в группу.
    pub(super) fn sign_event<T: serde::Serialize>(
        &self,
        conversation: &str,
        event_id: &str,
        kind: &str,
        payload: &T,
    ) -> Result<SignedProtocolEvent, CoreError> {
        let unsigned = SignedProtocolEvent {
            version: PROTOCOL_VERSION,
            event_id: event_id.to_owned(),
            conversation_id: conversation.to_owned(),
            sender_user_id: self.identity.public.user_id.clone(),
            sender_device_id: self.identity.public.device_id.clone(),
            device_sequence: self.store.next_device_sequence()?,
            kind: kind.to_owned(),
            created_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
            payload: STANDARD.encode(serde_json::to_vec(payload)?),
            signature: String::new(),
        };
        let signature = self.identity.sign_device(&unsigned.canonical_bytes()?)?;
        Ok(SignedProtocolEvent {
            signature,
            ..unsigned
        })
    }

    /// Разбирает очередь. Неудача одного адресата не мешает остальным: задача просто
    /// откладывается с нарастающей паузой.
    pub(super) fn flush_outbox(&mut self, node: &NodeDescriptor) -> Result<usize, CoreError> {
        self.flush_outbox_limited(node, OUTBOX_SYNC_LIMIT)
    }

    pub(super) fn flush_outbox_limited(
        &mut self,
        node: &NodeDescriptor,
        limit: usize,
    ) -> Result<usize, CoreError> {
        self.drain_pending_uploads(node)?;
        let now = chrono::Utc::now().timestamp_millis();
        let mut sent = 0;
        let mut handled = 0;
        while handled < limit {
            let jobs = self.store.due_outbox(OUTBOX_BATCH.min(limit - handled))?;
            let batch = jobs.len();
            for job in jobs {
                handled += 1;
                // Канал рассылает так же, как группа: по задаче на получателя.
                let group = is_group_id(&job.event.conversation_id)
                    || is_channel_id(&job.event.conversation_id);
                if group && now - job.event.created_at_unix_milliseconds > GROUP_JOB_TTL_MILLISECONDS {
                    self.store.complete_outbox(&job.job_id)?;
                    continue;
                }
                match self.deliver(node, &job) {
                    Ok(()) => {
                        self.store.complete_outbox(&job.job_id)?;
                        // Квитанций о доставке группа не шлёт — это N² трафика. Первая
                        // галочка значит «ушло хотя бы одному участнику».
                        if group
                            && matches!(
                                job.event.kind.as_str(),
                                KIND_TEXT | KIND_ATTACHMENT | KIND_CHANNEL_COMMENT
                            )
                        {
                            self.store.mark_delivered(&job.event.event_id)?;
                        }
                        sent += 1;
                    }
                    Err(error) => {
                        self.status = format!("Не удалось отправить: {error}");
                        self.store.defer_outbox(&job.job_id, job.attempts)?;
                    }
                }
            }
            if batch < OUTBOX_BATCH {
                break;
            }
        }
        Ok(sent)
    }

    /// Вложения, отправленные без связи: выкладываем файл и только теперь собираем
    /// событие — до этого момента собеседнику нечего было бы скачивать.
    fn drain_pending_uploads(&mut self, node: &NodeDescriptor) -> Result<(), CoreError> {
        for pending in self.store.pending_uploads()? {
            let progress = Progress::default();
            let manifest = match upload_attachment(
                &self.network,
                node,
                self.store.vault_key(),
                &pending.attachment,
                &progress,
            ) {
                Ok(value) => value,
                Err(error) => {
                    self.status = format!("Файл не выложен: {error}");
                    continue;
                }
            };
            self.store
                .save_event_manifest(&pending.event_id, &manifest)?;
            // Пока файл выгружался, из группы могли и выйти: сообщение остаётся в истории,
            // но разослать его уже некому. Остальную очередь это не останавливает.
            if let Err(error) = self.queue_for_chat(
                &pending.user_id,
                &pending.event_id,
                KIND_ATTACHMENT,
                &AttachmentPayload {
                    version: PROTOCOL_VERSION,
                    caption: pending.caption.clone(),
                    manifest,
                    reply_to_event_id: pending.reply_to_event_id.clone(),
                    forwarded_from: pending.forwarded_from.clone(),
                },
            ) {
                self.status = format!("Файл не отправлен: {error}");
            }
            self.store.clear_pending_upload(&pending.event_id)?;
        }
        Ok(())
    }

    fn deliver(&mut self, node: &NodeDescriptor, job: &OutboxJob) -> Result<(), CoreError> {
        let mailbox = self
            .store
            .mailbox()?
            .ok_or_else(|| CoreError::InvalidInput("Почтовый ящик не создан".to_owned()))?;
        let own_routing = self
            .store
            .own_routing()?
            .ok_or_else(|| CoreError::InvalidInput("Свой адрес не опубликован".to_owned()))?;
        let own = job.user_id == self.identity.public.user_id;
        let routing = if own {
            match self.own_devices_routing(node)? {
                Some(routing) => routing,
                // Других устройств нет — рассылать некому, задача выполнена.
                None => return Ok(()),
            }
        } else {
            self.peer_routing(node, &job.user_id)?
        };

        // Личный ключ записи уходит только внутри уже зашифрованного пакета: получив его,
        // собеседник перестаёт зависеть от узкой квоты публичного ящика.
        let body = DeliveryPackageBody {
            version: PROTOCOL_VERSION,
            event: job.event.clone(),
            sender_routing: own_routing,
            reply_grants: vec![MailboxGrant::from_owned(&mailbox)],
        };
        let body_json = serde_json::to_string(&body)?;
        let package = SignedDeliveryPackage {
            signature: self.identity.sign_device(body_json.as_bytes())?,
            body,
            body_json,
        };
        let payload = serde_json::to_vec(&package)?;

        let mut delivered = false;
        let mut failure: Option<CoreError> = None;
        for entry in routing.descriptor.devices.clone() {
            let device_id = entry.identity.device_id.clone();
            if own && device_id == self.identity.public.device_id {
                continue;
            }
            let private = self.store.grants(&job.user_id, &device_id)?;
            let public = routing.contact_mailboxes(&device_id);
            let routes = if private.is_empty() { public } else { private.clone() };
            if routes.is_empty() {
                continue;
            }
            // Участники группы не обязаны быть знакомы. Первое событие незнакомцу уходит
            // в публичный ящик под его квотой и PoW; с ним же приходит наш личный адрес.
            if private.is_empty()
                && !own
                && !is_contact_request(&job.event)
                && !is_group_id(&job.event.conversation_id)
                && !is_channel_id(&job.event.conversation_id)
            {
                failure = Some(CoreError::InvalidInput(
                    "Собеседник ещё не ответил: пока можно отправить только короткое текстовое сообщение".to_owned(),
                ));
                continue;
            }
            match self.deliver_to_device(node, &job.user_id, &device_id, &routes, &payload, private.is_empty())
            {
                Ok(()) => delivered = true,
                Err(error) => failure = Some(error),
            }
        }
        match (delivered, failure) {
            (true, _) => Ok(()),
            (false, Some(error)) => Err(error),
            // Своё устройство без живого ящика давно не появлялось: ждать его незачем,
            // при возвращении оно возьмёт снимок.
            (false, None) if own => Ok(()),
            (false, None) => Err(CoreError::InvalidInput(
                "У собеседника нет доступного почтового ящика".to_owned(),
            )),
        }
    }

    /// Адреса других устройств аккаунта. Берётся с Node, а не из кэша: только что вошедшее
    /// устройство должно получить изменения сразу.
    fn own_devices_routing(
        &mut self,
        node: &NodeDescriptor,
    ) -> Result<Option<SignedRoutingDescriptor>, CoreError> {
        let me = self.identity.public.user_id.clone();
        let fresh = self
            .network
            .routing(node, &me)
            .ok()
            .flatten()
            .filter(|routing| routing.descriptor.user_id == me && routing.verify());
        if let Some(routing) = &fresh {
            self.store.save_peer_routing(routing)?;
        }
        let routing = match fresh {
            Some(routing) => Some(routing),
            None => self.store.peer_routing(&me)?.filter(SignedRoutingDescriptor::verify),
        };
        Ok(routing.filter(|routing| {
            routing
                .descriptor
                .devices
                .iter()
                .any(|entry| entry.identity.device_id != self.identity.public.device_id)
        }))
    }

    fn deliver_to_device(
        &mut self,
        node: &NodeDescriptor,
        user_id: &str,
        device_id: &str,
        routes: &[MailboxGrant],
        payload: &[u8],
        contact_inbox: bool,
    ) -> Result<(), CoreError> {
        let event_id = serde_json::from_slice::<serde_json::Value>(payload)
            .ok()
            .and_then(|value| {
                value
                    .pointer("/body/event/eventId")
                    .and_then(|id| id.as_str().map(str::to_owned))
            })
            .unwrap_or_default();
        let (wire_kind, body_json, session) = match self.store.session_for_device(device_id)? {
            Some(mut session) => {
                let message = session.encrypt(&self.identity, payload)?;
                (WIRE_RATCHET, serde_json::to_string(&message)?, session)
            }
            None => {
                let bundle =
                    self.network
                        .claim_prekeys(&routes[0].base_url, user_id, device_id)?;
                let state = self
                    .store
                    .prekey_state()?
                    .ok_or_else(|| CoreError::Crypto("Предключи не созданы".to_owned()))?;
                let (session, envelope) =
                    ratchet::initiate(&self.identity, &state, &bundle, payload)?;
                (
                    WIRE_SESSION_INIT,
                    serde_json::to_string(&envelope)?,
                    session,
                )
            }
        };
        let wire = WireMessage {
            kind: wire_kind.to_owned(),
            event_id,
            sender_identity: WireIdentity::from(&self.identity.public),
            body_json,
        };
        let wire_bytes = serde_json::to_vec(&wire)?;
        let bits = if contact_inbox {
            node.contact_pow_bits
        } else {
            node.envelope_pow_bits
        };
        for route in routes {
            let envelope = mailbox::encode(route, &wire_bytes, ENVELOPE_TTL_HOURS)?;
            self.network.put_envelope(route, &envelope, bits)?;
        }
        // Сессия сохраняется только после успешной отправки: иначе мы бы считали
        // рукопожатие состоявшимся, а собеседник о нём даже не узнал бы.
        self.store.save_session(&session)?;
        Ok(())
    }

    /// Адрес собеседника: из кэша, а если он устарел — с Node.
    fn peer_routing(
        &mut self,
        node: &NodeDescriptor,
        user_id: &str,
    ) -> Result<SignedRoutingDescriptor, CoreError> {
        if let Some(cached) = self.store.peer_routing(user_id)?
            && cached.verify()
        {
            return Ok(cached);
        }
        let fetched = self.network.routing(node, user_id)?.ok_or_else(|| {
            CoreError::InvalidInput(
                "Собеседник ещё не публиковал адрес: он не заходил в сеть".to_owned(),
            )
        })?;
        if fetched.descriptor.user_id != user_id {
            return Err(CoreError::Crypto("Node вернул чужой адрес".to_owned()));
        }
        self.store.save_peer_routing(&fetched)?;
        Ok(fetched)
    }

    /// Забирает конверты из своего ящика и применяет их к истории.
    pub(super) fn fetch_inbox(&mut self, node: &NodeDescriptor) -> Result<usize, CoreError> {
        let Some(mailbox) = self.store.mailbox()? else {
            return Ok(0);
        };
        let envelopes = self.network.fetch_envelopes(&mailbox, INBOX_BATCH)?;
        let now = chrono::Utc::now().timestamp_millis();
        let mut acknowledge = Vec::new();
        let mut accepted = 0;
        for envelope in envelopes {
            match self.accept_envelope(node, &mailbox, &envelope) {
                Ok(applied) => {
                    acknowledge.push(envelope.envelope_id.clone());
                    if applied {
                        accepted += 1;
                    }
                }
                Err(error) => {
                    self.status = format!("Конверт не принят: {error}");
                    let created = mailbox::parse_rfc3339(&envelope.created_at).unwrap_or(now);
                    if now - created > UNREADABLE_ENVELOPE_GRACE_MILLISECONDS {
                        acknowledge.push(envelope.envelope_id.clone());
                    }
                }
            }
        }
        self.network.acknowledge(&mailbox, &acknowledge)?;
        Ok(accepted)
    }

    fn accept_envelope(
        &mut self,
        node: &NodeDescriptor,
        mailbox: &OwnedMailbox,
        envelope: &MailboxEnvelope,
    ) -> Result<bool, CoreError> {
        let inner = mailbox::decode(mailbox, envelope)?;
        let wire: WireMessage = serde_json::from_slice(&inner)?;
        if !wire.sender_identity.verify_certificate() {
            return Err(CoreError::Crypto(
                "Личность отправителя не подтверждена".to_owned(),
            ));
        }
        let payload = match wire.kind.as_str() {
            WIRE_SESSION_INIT => {
                let initial: InitialSessionEnvelope = serde_json::from_str(&wire.body_json)?;
                if self.store.session(&initial.header.session_id)?.is_some() {
                    // Повтор доставки уже принятого рукопожатия: одноразовый ключ
                    // потрачен, второй раз это же согласование не воспроизвести.
                    return Ok(false);
                }
                let mut state = self
                    .store
                    .prekey_state()?
                    .ok_or_else(|| CoreError::Crypto("Предключи не созданы".to_owned()))?;
                let (session, payload) = ratchet::accept(&self.identity, &mut state, &initial)?;
                self.store.save_prekey_state(&state)?;
                self.store.save_session(&session)?;
                payload
            }
            WIRE_RATCHET => {
                let message: RatchetMessage = serde_json::from_str(&wire.body_json)?;
                let mut session = self.store.session(&message.session_id)?.ok_or_else(|| {
                    CoreError::Crypto("Сессия для этого сообщения неизвестна".to_owned())
                })?;
                let payload = session.decrypt(&self.identity, &message)?;
                self.store.save_session(&session)?;
                payload
            }
            _ => {
                return Err(CoreError::Crypto(
                    "Неизвестный формат сообщения".to_owned(),
                ));
            }
        };

        let package: SignedDeliveryPackage = serde_json::from_slice(&payload)?;
        if serde_json::to_string(&package.body)? != package.body_json
            || !wire
                .sender_identity
                .verify_device_data(package.body_json.as_bytes(), &package.signature)
        {
            return Err(CoreError::Crypto(
                "Пакет доставки не прошёл проверку подписи".to_owned(),
            ));
        }
        let event = &package.body.event;
        if event.event_id != wire.event_id
            || !event.verify(&wire.sender_identity)
            || (event.conversation_id
                != conversation_id(&self.identity.public.user_id, &event.sender_user_id)
                && !is_group_id(&event.conversation_id)
                && !is_channel_id(&event.conversation_id))
        {
            return Err(CoreError::Crypto(
                "Событие не прошло проверку подписи".to_owned(),
            ));
        }
        if !self.store.mark_seen(&event.event_id)? {
            return Ok(false);
        }
        self.apply_incoming(node, &package, &wire.sender_identity)
    }

    fn apply_incoming(
        &mut self,
        node: &NodeDescriptor,
        package: &SignedDeliveryPackage,
        sender: &WireIdentity,
    ) -> Result<bool, CoreError> {
        let event = &package.body.event;
        let routing = &package.body.sender_routing;
        if !routing.verify()
            || routing.descriptor.user_id != event.sender_user_id
            || !routing
                .descriptor
                .device_list
                .contains(&sender.device_id)
        {
            return Err(CoreError::Crypto(
                "Обратный адрес отправителя недействителен".to_owned(),
            ));
        }
        // Отозванное устройство может предъявить старый список устройств, где оно ещё есть.
        // Если нам уже известен более новый список без него — это откат, и событие не принимается.
        if self.device_was_revoked(&event.sender_user_id, &sender.device_id, routing)? {
            return Err(CoreError::Crypto(
                "Устройство отправителя исключено из его аккаунта".to_owned(),
            ));
        }
        self.store.save_peer_routing(routing)?;
        for grant in &package.body.reply_grants {
            // Принимаем обратный адрес только на том же Node, с которым работаем сами:
            // иначе отправитель мог бы увести наши исходящие запросы на чужой хост.
            if grant.node_id == node.node_id
                && grant.base_url.trim_end_matches('/') == node.base_url.trim_end_matches('/')
                && grant.alive()
            {
                self.store
                    .save_grant(&event.sender_user_id, &sender.device_id, grant)?;
            }
        }

        // Своё другое устройство: синхронизация, а не переписка.
        if event.sender_user_id == self.identity.public.user_id {
            return self.apply_self_event(event, sender, routing);
        }

        // Событие группы не создаёт запрос на общение: у группы свои правила допуска.
        if is_group_id(&event.conversation_id) {
            return self.apply_group_event(event);
        }
        if is_channel_id(&event.conversation_id) {
            return self.apply_channel_event(event, sender);
        }

        let existing = self.store.contact(&event.sender_user_id)?;
        let accepted = existing.as_ref().is_some_and(|value| !value.pending_approval);
        if !accepted && event.kind != KIND_TEXT {
            // Пока диалог не принят, канал несёт только запрос на общение.
            return Ok(false);
        }
        // Звонок: только от принятого контакта (проверено выше) и мимо истории сообщений.
        if crate::protocol::is_call_kind(&event.kind) {
            return self.apply_call_event(event);
        }
        if existing.is_none() {
            self.create_pending_contact(node, &event.sender_user_id)?;
        }

        let conversation = conversation_id(&self.identity.public.user_id, &event.sender_user_id);
        let applied = self.apply_message_event(event, &conversation, None)?;

        // Квитанция о доставке уходит только по принятому диалогу: она подтверждает
        // и получение, и то, что мы вообще держим этот канал открытым.
        if accepted && matches!(event.kind.as_str(), KIND_TEXT | KIND_ATTACHMENT) {
            self.queue_event(
                &event.sender_user_id,
                &format!("evt1-{}", super::random_hex(16)),
                KIND_RECEIPT_DELIVERY,
                &TargetPayload {
                    version: PROTOCOL_VERSION,
                    target_event_id: event.event_id.clone(),
                },
            )?;
        }
        Ok(applied)
    }

    /// Есть ли у нас более новый список устройств этого пользователя, где отправителя уже нет.
    fn device_was_revoked(
        &self,
        user_id: &str,
        device_id: &str,
        presented: &SignedRoutingDescriptor,
    ) -> Result<bool, CoreError> {
        let presented_sequence = presented.descriptor.device_list.document.sequence;
        let mut known = Vec::new();
        if let Some(routing) = self.store.peer_routing(user_id)? {
            known.push(routing.descriptor.device_list);
        }
        if user_id == self.identity.public.user_id
            && let Some(list) = self.store.device_list()?
        {
            known.push(list);
        }
        Ok(known.iter().any(|list| {
            list.document.sequence > presented_sequence
                && (!list.contains(device_id)
                    || list
                        .document
                        .revocations
                        .iter()
                        .any(|revocation| revocation.device_id == device_id))
        }))
    }

    /// Сообщение, правка, реакция или квитанция — общие для личного диалога и группы.
    ///
    /// Цель правки или реакции ищется только в том же диалоге: знание чужого `eventId`
    /// не должно позволять трогать сообщения другого чата. В группе реакции учитываются
    /// по авторам, а чужое сообщение может удалить старший по роли.
    pub(super) fn apply_message_event(
        &mut self,
        event: &SignedProtocolEvent,
        conversation: &str,
        group: Option<&GroupState>,
    ) -> Result<bool, CoreError> {
        let find = |core: &Self, event_id: &str| -> Result<Option<Message>, CoreError> {
            Ok(core
                .store
                .message(event_id)?
                .filter(|message| message.conversation_id == conversation && !message.service))
        };
        Ok(match event.kind.as_str() {
            KIND_TEXT => {
                let payload: TextPayload = event.decode_payload()?;
                if payload.text.trim().is_empty() || self.store.message(&event.event_id)?.is_some() {
                    return Ok(false);
                }
                self.store.save_message(&Message {
                    event_id: event.event_id.clone(),
                    conversation_id: conversation.to_owned(),
                    sender_user_id: event.sender_user_id.clone(),
                    text: payload.text,
                    created_at_unix_milliseconds: event.created_at_unix_milliseconds,
                    outgoing: false,
                    edited: false,
                    deleted: false,
                    reactions: Vec::new(),
                    delivered: true,
                    read: false,
                    pinned: false,
                    attachment: None,
                    reply_to_event_id: payload.reply_to_event_id,
                    forwarded_from: payload.forwarded_from,
                    service: false,
                    reaction_marks: Vec::new(),
                    sender_name: None,
                    channel_post: None,
                })?;
                true
            }
            KIND_ATTACHMENT => {
                let payload: AttachmentPayload = event.decode_payload()?;
                if self.store.message(&event.event_id)?.is_some() {
                    return Ok(false);
                }
                // Манифест хранится и после загрузки: по нему файл скачает другое устройство
                // аккаунта.
                self.store.save_event_manifest(&event.event_id, &payload.manifest)?;
                self.store.save_message(&Message {
                    event_id: event.event_id.clone(),
                    conversation_id: conversation.to_owned(),
                    sender_user_id: event.sender_user_id.clone(),
                    text: payload.caption,
                    created_at_unix_milliseconds: event.created_at_unix_milliseconds,
                    outgoing: false,
                    edited: false,
                    deleted: false,
                    reactions: Vec::new(),
                    delivered: true,
                    read: false,
                    pinned: false,
                    attachment: Some(incoming_attachment(
                        &self.store.app_dir,
                        &payload.manifest,
                    )),
                    reply_to_event_id: payload.reply_to_event_id,
                    forwarded_from: payload.forwarded_from,
                    service: false,
                    reaction_marks: Vec::new(),
                    sender_name: None,
                    channel_post: None,
                })?;
                // Файл догружается фоном: история не должна ждать стомегабайтного видео.
                self.store
                    .save_pending_blob(&event.event_id, &payload.manifest)?;
                self.start_pending_downloads();
                true
            }
            KIND_EDIT => {
                let payload: EditPayload = event.decode_payload()?;
                if let Some(mut message) = find(self, &payload.target_event_id)?
                    && message.sender_user_id == event.sender_user_id
                    && !message.deleted
                {
                    message.text = payload.text;
                    message.edited = true;
                    self.store.save_message(&message)?;
                }
                true
            }
            KIND_DELETE => {
                let payload: TargetPayload = event.decode_payload()?;
                if let Some(mut message) = find(self, &payload.target_event_id)?
                    && (message.sender_user_id == event.sender_user_id
                        || group.is_some_and(|state| {
                            groups::may_moderate(state, &event.sender_user_id, &message.sender_user_id)
                        }))
                {
                    message.deleted = true;
                    message.text.clear();
                    message.attachment = None;
                    message.reaction_marks.clear();
                    message.reactions.clear();
                    self.store.save_message(&message)?;
                }
                true
            }
            KIND_REACTION => {
                let payload: ReactionPayload = event.decode_payload()?;
                if !ALLOWED_REACTIONS.contains(&payload.reaction.as_str()) {
                    return Ok(false);
                }
                if let Some(mut message) = find(self, &payload.target_event_id)?
                    && !message.deleted
                {
                    if group.is_some() {
                        set_reaction_mark(
                            &mut message,
                            &event.sender_user_id,
                            &payload.reaction,
                            Some(payload.active),
                        );
                    } else {
                        message.reactions.retain(|value| value != &payload.reaction);
                        if payload.active {
                            message.reactions.push(payload.reaction);
                        }
                    }
                    self.store.save_message(&message)?;
                }
                true
            }
            KIND_RECEIPT_DELIVERY => {
                let payload: TargetPayload = event.decode_payload()?;
                if group.is_none()
                    && let Some(mut message) = find(self, &payload.target_event_id)?
                    && message.outgoing
                {
                    message.delivered = true;
                    self.store.save_message(&message)?;
                }
                false
            }
            KIND_RECEIPT_READ => {
                let payload: TargetPayload = event.decode_payload()?;
                if let Some(mut message) = find(self, &payload.target_event_id)?
                    && message.outgoing
                {
                    if group.is_some() {
                        // Участник шлёт квитанцию только на последнее прочитанное: всё,
                        // что было раньше, он тоже видел.
                        for mut earlier in self.store.messages(conversation)? {
                            if earlier.outgoing
                                && !earlier.service
                                && !earlier.read
                                && earlier.created_at_unix_milliseconds
                                    <= message.created_at_unix_milliseconds
                            {
                                earlier.delivered = true;
                                earlier.read = true;
                                self.store.save_message(&earlier)?;
                            }
                        }
                    } else {
                        message.delivered = true;
                        message.read = true;
                        self.store.save_message(&message)?;
                    }
                }
                false
            }
            _ => false,
        })
    }

    fn create_pending_contact(
        &mut self,
        node: &NodeDescriptor,
        user_id: &str,
    ) -> Result<(), CoreError> {
        let profile = self.network.profile(node, user_id).ok().flatten();
        self.store.save_contact(&Contact {
            user_id: user_id.to_owned(),
            display_name: profile
                .as_ref()
                .map(|value| value.display_name.clone())
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| short_id(user_id)),
            username: None,
            about: profile.as_ref().map(|value| value.about.clone()),
            avatar_base64: profile.and_then(|value| value.avatar_base64),
            added_at_unix_milliseconds: chrono::Utc::now().timestamp_millis(),
            fingerprint_verified: false,
            pending_approval: true,
            last_seen_unix_milliseconds: None,
            pinned: false,
            muted: false,
            draft: String::new(),
            manual_unread: false,
        })
    }

    /// Догружает вложения, которых ещё нет на диске. Каждое качается в своём потоке,
    /// поэтому ядро остаётся отзывчивым.
    pub(super) fn start_pending_downloads(&mut self) {
        let Ok(pending) = self.store.pending_blobs() else {
            return;
        };
        for (event_id, manifest) in pending {
            let destination = attachment_path(&self.store.app_dir, &manifest.attachment_id);
            if destination.exists() {
                let _ = self.store.clear_pending_blob(&event_id);
                continue;
            }
            if self
                .media_jobs
                .values()
                .any(|job| job.attachment_id.as_deref() == Some(manifest.attachment_id.as_str()))
            {
                continue;
            }
            let key = *self.store.vault_key();
            let progress = Arc::new(Progress::default());
            progress
                .total
                .store(manifest.plaintext_size, Ordering::Relaxed);
            let attachment_id = manifest.attachment_id.clone();
            self.spawn_download_job(progress, attachment_id, move |progress| {
                let network = crate::network::Network::new()?;
                blobs::download(&network, &manifest, &key, &destination, progress)
            });
        }
    }

    /// Проверяет, какие фоновые загрузки завершились, и убирает их из очереди.
    pub(super) fn settle_downloads(&mut self) -> Option<String> {
        let mut failure = None;
        let finished: Vec<(String, String, String)> = self
            .media_jobs
            .iter()
            .filter(|(_, job)| job.attachment_id.is_some() && job.state() != super::JOB_RUNNING)
            .map(|(id, job)| {
                (
                    id.clone(),
                    job.attachment_id.clone().unwrap_or_default(),
                    job.error(),
                )
            })
            .collect();
        for (job_id, attachment_id, error) in finished {
            // Об ошибке загрузки пользователю лучше узнать из строки состояния,
            // чем гадать, почему файл в переписке не открывается.
            if !error.is_empty() {
                failure = Some(error);
            }
            self.media_jobs.remove(&job_id);
            if attachment_path(&self.store.app_dir, &attachment_id).exists()
                && let Ok(pending) = self.store.pending_blobs()
            {
                for (event_id, manifest) in pending {
                    if manifest.attachment_id == attachment_id {
                        let _ = self.store.clear_pending_blob(&event_id);
                    }
                }
            }
        }
        failure
    }
}

/// Запрос на общение: только текст и только короткий.
fn is_contact_request(event: &SignedProtocolEvent) -> bool {
    if event.kind != KIND_TEXT {
        return false;
    }
    event
        .decode_payload::<TextPayload>()
        .is_ok_and(|payload| payload.text.len() <= MAX_CONTACT_REQUEST_BYTES)
}

pub(super) fn attachment_path(app_dir: &std::path::Path, attachment_id: &str) -> std::path::PathBuf {
    app_dir
        .join("local-first")
        .join("attachments")
        .join(format!("{attachment_id}.bin"))
}

fn incoming_attachment(app_dir: &std::path::Path, manifest: &AttachmentManifest) -> Attachment {
    Attachment {
        attachment_id: manifest.attachment_id.clone(),
        file_name: manifest.file_name.clone(),
        mime_type: manifest.mime_type.clone(),
        size: manifest.plaintext_size,
        local_path: attachment_path(app_dir, &manifest.attachment_id)
            .to_string_lossy()
            .into_owned(),
        kind: manifest.kind,
        width: manifest.width,
        height: manifest.height,
        duration_milliseconds: manifest.duration_milliseconds,
        thumbnail_base64: manifest.thumbnail_base64.clone(),
    }
}

/// Отправка вложения: файл уезжает в blob-хранилище, а получателю уходит ключ и манифест.
pub(super) fn upload_attachment(
    network: &crate::network::Network,
    node: &NodeDescriptor,
    vault_key: &[u8; 32],
    attachment: &Attachment,
    progress: &Progress,
) -> Result<AttachmentManifest, CoreError> {
    blobs::upload(
        network,
        &node.node_id,
        &node.base_url,
        vault_key,
        &blobs::UploadRequest {
            local_path: std::path::Path::new(&attachment.local_path),
            file_name: &attachment.file_name,
            mime_type: &attachment.mime_type,
            kind: attachment.kind,
            width: attachment.width,
            height: attachment.height,
            duration_milliseconds: attachment.duration_milliseconds,
            thumbnail_base64: attachment.thumbnail_base64.clone(),
        },
        progress,
    )
}
