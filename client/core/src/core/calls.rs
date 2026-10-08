//! Сигналинг голосовых звонков: приглашение, ответ, завершение и история в диалоге.
//!
//! Звонить можно только принятому контакту: событие звонка от непринятого собеседника
//! отбрасывается ещё при приёме конверта, как и любое не-текстовое событие.

use base64::{Engine as _, engine::general_purpose::STANDARD};

use super::{AppCore, random_hex, short_id};
use crate::{
    CoreError,
    calls::{
        self, CallSession, OFFER_MAX_AGE_MS, Phase,
        crypto::{CallKeys, Ephemeral},
        engine::{CallMedia, now_ms},
        transport::RelayTicket,
    },
    identity::conversation_id,
    models::Message,
    protocol::{
        CallAnswerPayload, CallOfferPayload, CallSignalPayload, KIND_CALL_ANSWER, KIND_CALL_END,
        KIND_CALL_OFFER, KIND_CALL_RINGING, PROTOCOL_VERSION, SignedProtocolEvent, is_user_id,
    },
};

fn denied(message: &str) -> CoreError {
    CoreError::InvalidInput(message.to_owned())
}

fn decode_key(value: &str) -> Result<[u8; 32], CoreError> {
    STANDARD
        .decode(value)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .ok_or_else(|| CoreError::Crypto("Повреждён ключ звонка".to_owned()))
}

/// Запись о звонке, которую нужно положить в историю диалога.
struct CallRecord {
    call_id: String,
    peer_user_id: String,
    outgoing: bool,
    reason: &'static str,
    duration_ms: i64,
    created_ms: i64,
}

fn record_text(record: &CallRecord) -> String {
    let duration = if record.duration_ms > 0 {
        let seconds = record.duration_ms / 1000;
        format!(" · {}:{:02}", seconds / 60, seconds % 60)
    } else {
        String::new()
    };
    let text = match (record.outgoing, record.reason) {
        (_, "lost") if record.duration_ms > 0 => {
            if record.outgoing { "Исходящий звонок" } else { "Входящий звонок" }.to_owned()
                + &duration
                + " · связь прервалась"
        }
        (true, _) if record.duration_ms > 0 => format!("Исходящий звонок{duration}"),
        (false, _) if record.duration_ms > 0 => format!("Входящий звонок{duration}"),
        (true, "cancelled") => "Отменённый звонок".to_owned(),
        (true, "no_answer") => "Звонок без ответа".to_owned(),
        (true, "declined_remote") => "Звонок отклонён".to_owned(),
        (true, "busy") => "Собеседник занят".to_owned(),
        (false, "declined") => "Отклонённый звонок".to_owned(),
        (false, "missed" | "cancelled_remote") => "Пропущенный звонок".to_owned(),
        (_, "failed" | "lost") => "Звонок не состоялся: нет соединения".to_owned(),
        (true, _) => "Исходящий звонок".to_owned(),
        (false, _) => "Входящий звонок".to_owned(),
    };
    format!("📞 {text}")
}

impl AppCore {
    /// Исходящий звонок принятому контакту. Возвращает `callId`.
    pub(super) fn start_call(&mut self, user_id: &str) -> Result<String, CoreError> {
        if !is_user_id(user_id) {
            return Err(denied("Звонить можно только собеседнику"));
        }
        let contact = self
            .store
            .contact(user_id)?
            .filter(|contact| !contact.pending_approval)
            .ok_or_else(|| denied("Звонить можно только принятому контакту"))?;
        if calls::with_call(&self.call_slot.clone(), |slot| slot.as_ref().is_some_and(CallSession::live)) {
            return Err(denied("Уже идёт звонок"));
        }
        self.settle_calls();
        let node = self.require_node().inspect_err(|_| self.online = false)?;
        self.online = true;
        // Свой адрес и ящик нужны до приглашения: иначе ответ собеседнику некуда положить.
        self.ensure_transport(&node)?;
        let room = self.network.create_call_room(&node)?;
        let ephemeral = Ephemeral::generate();
        let call_id = format!("call1-{}", random_hex(16));
        let ticket = |token: &str| RelayTicket {
            room_id: room.room_id.clone(),
            token: token.to_owned(),
            udp_host: room.udp_host.clone(),
            udp_port: room.udp_port,
            web_socket_url: room.web_socket_url.clone(),
        };
        let own = ticket(&room.caller_token);
        own.validate()?;
        self.queue_event(
            user_id,
            &format!("evt1-{}", random_hex(16)),
            KIND_CALL_OFFER,
            &CallOfferPayload {
                version: PROTOCOL_VERSION,
                call_id: call_id.clone(),
                relay: ticket(&room.callee_token),
                ephemeral_key: STANDARD.encode(ephemeral.public),
                codec: "opus/48000/1".to_owned(),
            },
        )?;
        let now = now_ms();
        calls::with_call(&self.call_slot.clone(), |slot| {
            *slot = Some(CallSession {
                call_id: call_id.clone(),
                peer_user_id: user_id.to_owned(),
                peer_name: contact.display_name.clone(),
                peer_avatar: contact.avatar_base64.clone(),
                outgoing: true,
                phase: Phase::Calling,
                created_ms: now,
                connected_ms: None,
                ended_ms: None,
                end_reason: None,
                answered_ms: None,
                ephemeral: Some(ephemeral),
                peer_key: None,
                ticket: Some(own),
                media: None,
                pending_end: None,
                recorded: false,
            });
        });
        self.deliver_now();
        self.status = "Звоним…".to_owned();
        Ok(call_id)
    }

    /// Ответ на входящий звонок.
    pub(super) fn accept_call(&mut self) -> Result<(), CoreError> {
        let me = self.identity.public.user_id.clone();
        let (peer, call_id, ephemeral_public) = calls::with_call(&self.call_slot.clone(), |slot| {
            let call = slot
                .as_mut()
                .filter(|call| call.phase == Phase::Incoming)
                .ok_or_else(|| denied("Входящего звонка нет"))?;
            let peer_key = call.peer_key.ok_or_else(|| denied("Нет ключа звонящего"))?;
            let ticket = call.ticket.clone().ok_or_else(|| denied("Нет комнаты звонка"))?;
            let ephemeral = Ephemeral::generate();
            let keys = CallKeys::derive(&ephemeral, &peer_key, false, &call.call_id, &call.peer_user_id, &me)?;
            let media = CallMedia::new(keys, ticket, 1)?;
            media.start();
            call.media = Some(media);
            call.phase = Phase::Connecting;
            call.answered_ms = Some(now_ms());
            let public = ephemeral.public;
            call.ephemeral = None;
            Ok::<_, CoreError>((call.peer_user_id.clone(), call.call_id.clone(), public))
        })?;
        self.queue_event(
            &peer,
            &format!("evt1-{}", random_hex(16)),
            KIND_CALL_ANSWER,
            &CallAnswerPayload {
                version: PROTOCOL_VERSION,
                call_id,
                ephemeral_key: STANDARD.encode(ephemeral_public),
            },
        )?;
        self.deliver_now();
        self.status = "Соединяем…".to_owned();
        Ok(())
    }

    /// Доделать то, что звонок оставил ядру: сообщить собеседнику о завершении и записать
    /// звонок в историю диалога. Вызывается в начале каждой команды и синхронизации.
    pub(super) fn settle_calls(&mut self) {
        let (signal, record) = calls::with_call(&self.call_slot.clone(), |slot| {
            let Some(call) = slot.as_mut() else {
                return (None, None);
            };
            // Таймауты срабатывают и без интерфейса: например, звонок пропущен в фоне.
            call.tick();
            let signal = call
                .pending_end
                .take()
                .map(|reason| (call.peer_user_id.clone(), call.call_id.clone(), reason));
            let record = (call.phase == Phase::Ended && !call.recorded).then(|| {
                call.recorded = true;
                CallRecord {
                    call_id: call.call_id.clone(),
                    peer_user_id: call.peer_user_id.clone(),
                    outgoing: call.outgoing,
                    reason: call.end_reason.unwrap_or("hangup"),
                    duration_ms: call.duration_ms(),
                    created_ms: call.created_ms,
                }
            });
            (signal, record)
        });
        if let Some((peer, call_id, reason)) = signal {
            self.send_call_signal(&peer, &call_id, KIND_CALL_END, reason);
            self.deliver_now();
        }
        if let Some(record) = record {
            let _ = self.record_call(&record);
        }
    }

    fn send_call_signal(&mut self, peer: &str, call_id: &str, kind: &str, reason: &str) {
        let _ = self.queue_event(
            peer,
            &format!("evt1-{}", random_hex(16)),
            kind,
            &CallSignalPayload {
                version: PROTOCOL_VERSION,
                call_id: call_id.to_owned(),
                reason: reason.to_owned(),
            },
        );
    }

    fn record_call(&mut self, record: &CallRecord) -> Result<(), CoreError> {
        // Звонок, принятый на другом устройстве, в историю этого не попадает.
        if record.reason == "answered_elsewhere" {
            return Ok(());
        }
        let me = self.identity.public.user_id.clone();
        let missed = !record.outgoing && record.duration_ms == 0 && record.reason != "declined";
        self.store.save_message(&Message {
            event_id: format!("call-{}", record.call_id),
            conversation_id: conversation_id(&me, &record.peer_user_id),
            sender_user_id: if record.outgoing { me } else { record.peer_user_id.clone() },
            text: record_text(record),
            created_at_unix_milliseconds: record.created_ms,
            outgoing: record.outgoing,
            edited: false,
            deleted: false,
            reactions: Vec::new(),
            delivered: true,
            read: !missed,
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

    /// Событие звонка от принятого контакта. `true` — что-то изменилось для интерфейса.
    pub(super) fn apply_call_event(&mut self, event: &SignedProtocolEvent) -> Result<bool, CoreError> {
        let sender = event.sender_user_id.clone();
        match event.kind.as_str() {
            KIND_CALL_OFFER => {
                let payload: CallOfferPayload = event.decode_payload()?;
                payload.relay.validate()?;
                let peer_key = decode_key(&payload.ephemeral_key)?;
                if payload.call_id.len() > 80 {
                    return Ok(false);
                }
                let age = now_ms() - event.created_at_unix_milliseconds;
                let contact = self.store.contact(&sender)?;
                let name = contact
                    .as_ref()
                    .map(|value| value.display_name.clone())
                    .unwrap_or_else(|| short_id(&sender));
                let avatar = contact.and_then(|value| value.avatar_base64);
                enum Decision {
                    Duplicate,
                    Busy,
                    Missed,
                    Ring,
                }
                let decision = calls::with_call(&self.call_slot.clone(), |slot| {
                    if let Some(call) = slot.as_ref() {
                        if call.call_id == payload.call_id {
                            return Decision::Duplicate;
                        }
                        if call.live() {
                            return Decision::Busy;
                        }
                    }
                    if age > OFFER_MAX_AGE_MS {
                        return Decision::Missed;
                    }
                    *slot = Some(CallSession {
                        call_id: payload.call_id.clone(),
                        peer_user_id: sender.clone(),
                        peer_name: name.clone(),
                        peer_avatar: avatar.clone(),
                        outgoing: false,
                        phase: Phase::Incoming,
                        created_ms: now_ms(),
                        connected_ms: None,
                        ended_ms: None,
                        end_reason: None,
                        answered_ms: None,
                        ephemeral: None,
                        peer_key: Some(peer_key),
                        ticket: Some(payload.relay.clone()),
                        media: None,
                        pending_end: None,
                        recorded: false,
                    });
                    Decision::Ring
                });
                let missed = CallRecord {
                    call_id: payload.call_id.clone(),
                    peer_user_id: sender.clone(),
                    outgoing: false,
                    reason: "missed",
                    duration_ms: 0,
                    created_ms: event.created_at_unix_milliseconds,
                };
                match decision {
                    Decision::Duplicate => Ok(false),
                    Decision::Busy => {
                        self.send_call_signal(&sender, &payload.call_id, KIND_CALL_END, "busy");
                        self.record_call(&missed)?;
                        Ok(true)
                    }
                    Decision::Missed => {
                        self.record_call(&missed)?;
                        Ok(true)
                    }
                    Decision::Ring => {
                        self.send_call_signal(&sender, &payload.call_id, KIND_CALL_RINGING, "");
                        self.status = format!("Входящий звонок: {name}");
                        Ok(true)
                    }
                }
            }
            KIND_CALL_RINGING => {
                let payload: CallSignalPayload = event.decode_payload()?;
                Ok(calls::with_call(&self.call_slot.clone(), |slot| match slot.as_mut() {
                    Some(call)
                        if call.call_id == payload.call_id
                            && call.peer_user_id == sender
                            && call.phase == Phase::Calling =>
                    {
                        call.phase = Phase::Ringing;
                        true
                    }
                    _ => false,
                }))
            }
            KIND_CALL_ANSWER => {
                let payload: CallAnswerPayload = event.decode_payload()?;
                let peer_key = decode_key(&payload.ephemeral_key)?;
                let me = self.identity.public.user_id.clone();
                let started = calls::with_call(&self.call_slot.clone(), |slot| {
                    let Some(call) = slot.as_mut().filter(|call| {
                        call.call_id == payload.call_id
                            && call.peer_user_id == sender
                            && call.outgoing
                            && matches!(call.phase, Phase::Calling | Phase::Ringing)
                    }) else {
                        return Ok(false);
                    };
                    let ephemeral = call.ephemeral.take().ok_or_else(|| denied("Нет ключа звонка"))?;
                    let ticket = call.ticket.clone().ok_or_else(|| denied("Нет комнаты звонка"))?;
                    let keys = CallKeys::derive(&ephemeral, &peer_key, true, &call.call_id, &me, &call.peer_user_id)?;
                    let media = CallMedia::new(keys, ticket, 0)?;
                    media.start();
                    call.media = Some(media);
                    call.phase = Phase::Connecting;
                    call.answered_ms = Some(now_ms());
                    Ok::<_, CoreError>(true)
                })?;
                if started {
                    // Остальные устройства собеседника перестают звонить.
                    self.send_call_signal(&sender, &payload.call_id, KIND_CALL_END, "answered");
                }
                Ok(started)
            }
            KIND_CALL_END => {
                let payload: CallSignalPayload = event.decode_payload()?;
                let changed = calls::with_call(&self.call_slot.clone(), |slot| {
                    let Some(call) = slot
                        .as_mut()
                        .filter(|call| call.call_id == payload.call_id && call.peer_user_id == sender && call.live())
                    else {
                        return false;
                    };
                    match payload.reason.as_str() {
                        "answered" if call.phase == Phase::Incoming => call.finish("answered_elsewhere", None),
                        "answered" => return false,
                        "rejected" => call.finish("declined_remote", None),
                        "busy" => call.finish("busy", None),
                        "cancelled" if call.phase == Phase::Incoming => call.finish("missed", None),
                        "cancelled" => call.finish("cancelled_remote", None),
                        "failed" | "lost" => call.finish("lost", None),
                        _ => call.finish("hangup_remote", None),
                    }
                    true
                });
                if changed {
                    self.settle_calls();
                }
                Ok(changed)
            }
            _ => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(outgoing: bool, reason: &'static str, duration_ms: i64) -> String {
        record_text(&CallRecord {
            call_id: "c".to_owned(),
            peer_user_id: "tt1-x".to_owned(),
            outgoing,
            reason,
            duration_ms,
            created_ms: 0,
        })
    }

    #[test]
    fn history_reads_like_a_phone() {
        assert_eq!(record(true, "hangup", 192_000), "📞 Исходящий звонок · 3:12");
        assert_eq!(record(false, "hangup_remote", 5_000), "📞 Входящий звонок · 0:05");
        assert_eq!(record(false, "missed", 0), "📞 Пропущенный звонок");
        assert_eq!(record(true, "cancelled", 0), "📞 Отменённый звонок");
        assert_eq!(record(true, "declined_remote", 0), "📞 Звонок отклонён");
        assert_eq!(record(false, "lost", 61_000), "📞 Входящий звонок · 1:01 · связь прервалась");
    }
}
