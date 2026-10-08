//! Голосовые звонки один-на-один.
//!
//! Сигналинг (приглашение, ответ, завершение) идёт обычными подписанными событиями по
//! double ratchet — так же, как сообщения. Голос идёт через ретранслятор на Node, но
//! зашифрован ключами звонка из `crypto`: Node видит только непрозрачные пакеты.
//!
//! Состояние звонка живёт вне ядра, в глобальном слоте: интерфейс опрашивает его по
//! несколько раз в секунду, а аудиопотоки отдают и забирают кадры каждые 20 мс — ни то,
//! ни другое не должно ждать замок ядра, пока идёт синхронизация переписки.

pub mod audio;
pub mod crypto;
pub mod engine;
pub mod transport;

use std::sync::{Arc, Mutex, atomic::Ordering};

use serde_json::json;

use self::{audio::FRAME_SAMPLES, engine::CallMedia, engine::now_ms};

/// Сколько звонит телефон, пока звонок не считается пропущенным.
pub const RING_TIMEOUT_MS: i64 = 45_000;
/// Приглашение старше этого уже не звонит, а сразу становится пропущенным.
pub const OFFER_MAX_AGE_MS: i64 = 60_000;
/// Сколько ждать первого звука после ответа.
const CONNECT_TIMEOUT_MS: i64 = 25_000;
/// Собеседник замолчал (оба транспорта мертвы) — звонок прерван.
const LOST_TIMEOUT_MS: i64 = 25_000;
/// Сколько показывать «Звонок завершён», прежде чем экран закроется сам.
const ENDED_LINGER_MS: i64 = 3_500;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Исходящий: приглашение отправлено.
    Calling,
    /// Исходящий: устройство собеседника звонит.
    Ringing,
    /// Входящий: звонит у нас.
    Incoming,
    /// Ключи согласованы, ждём первый звук.
    Connecting,
    Active,
    Ended,
}

impl Phase {
    fn name(self) -> &'static str {
        match self {
            Self::Calling => "calling",
            Self::Ringing => "ringing",
            Self::Incoming => "incoming",
            Self::Connecting => "connecting",
            Self::Active => "active",
            Self::Ended => "ended",
        }
    }
}

pub struct CallSession {
    pub call_id: String,
    pub peer_user_id: String,
    pub peer_name: String,
    pub peer_avatar: Option<String>,
    pub outgoing: bool,
    pub phase: Phase,
    pub created_ms: i64,
    pub connected_ms: Option<i64>,
    pub ended_ms: Option<i64>,
    pub end_reason: Option<&'static str>,
    /// Когда звонок принят: от этого момента считается ожидание первого звука.
    pub answered_ms: Option<i64>,
    pub ephemeral: Option<crypto::Ephemeral>,
    /// Одноразовый ключ собеседника из приглашения (у принимающего — до ответа).
    pub peer_key: Option<[u8; 32]>,
    /// Своё место в комнате ретранслятора.
    pub ticket: Option<transport::RelayTicket>,
    pub media: Option<Arc<CallMedia>>,
    /// Завершение, о котором ядро ещё должно сообщить собеседнику событием.
    pub pending_end: Option<&'static str>,
    /// Звонок уже записан в историю диалога.
    pub recorded: bool,
}

impl CallSession {
    pub fn live(&self) -> bool {
        self.phase != Phase::Ended
    }

    pub fn duration_ms(&self) -> i64 {
        match self.connected_ms {
            Some(start) => self.ended_ms.unwrap_or_else(now_ms) - start,
            None => 0,
        }
    }

    /// Завершить звонок. `signal` — что сообщить собеседнику, если он ещё не знает.
    pub fn finish(&mut self, reason: &'static str, signal: Option<&'static str>) {
        if self.phase == Phase::Ended {
            return;
        }
        if let Some(media) = self.media.take() {
            if signal.is_some() {
                // Быстрый путь: собеседник узнает о конце по медиаканалу за доли секунды.
                media.hang_up();
            } else {
                media.stop();
            }
        }
        self.phase = Phase::Ended;
        self.ended_ms = Some(now_ms());
        self.end_reason = Some(reason);
        self.pending_end = signal;
        self.ephemeral = None;
    }

    /// Таймауты и сигналы медиаканала. Вызывается при каждом чтении состояния.
    pub fn tick(&mut self) {
        let now = now_ms();
        match self.phase {
            Phase::Calling | Phase::Ringing if now - self.created_ms > RING_TIMEOUT_MS => {
                self.finish("no_answer", Some("cancelled"));
            }
            Phase::Incoming if now - self.created_ms > RING_TIMEOUT_MS => {
                self.finish("missed", None);
            }
            Phase::Connecting | Phase::Active => {
                let Some(media) = self.media.clone() else {
                    return;
                };
                if media.remote_hangup.load(Ordering::SeqCst) {
                    self.finish("hangup_remote", None);
                    return;
                }
                let heard = media.peer_heard_ms.load(Ordering::Relaxed);
                if self.phase == Phase::Connecting {
                    if heard > 0 {
                        self.phase = Phase::Active;
                        self.connected_ms = Some(heard);
                    } else if now - self.answered_ms.unwrap_or(self.created_ms) > CONNECT_TIMEOUT_MS {
                        self.finish("failed", Some("failed"));
                    }
                } else if now - media.last_peer_ms.load(Ordering::Relaxed) > LOST_TIMEOUT_MS {
                    self.finish("lost", Some("lost"));
                }
            }
            _ => {}
        }
    }
}

/// Единственный звонок устройства. Слот принадлежит ядру, но живёт под своим замком.
pub type CallSlot = Arc<Mutex<Option<CallSession>>>;

pub fn with_call<T>(slot: &CallSlot, action: impl FnOnce(&mut Option<CallSession>) -> T) -> T {
    let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    action(&mut guard)
}

fn media(slot: &CallSlot) -> Option<Arc<CallMedia>> {
    with_call(slot, |call| call.as_ref().and_then(|call| call.media.clone()))
}

/// Кадр микрофона от платформы (960 отсчётов, 48 кГц, моно).
pub fn push(slot: &CallSlot, pcm: &[i16]) {
    if let Some(media) = media(slot) {
        media.push(pcm);
    }
}

/// Кадр для динамика. Возвращает `false`, если звонка нет.
pub fn pull(slot: &CallSlot, output: &mut [i16]) -> bool {
    if output.len() != FRAME_SAMPLES {
        output.fill(0);
        return false;
    }
    let mut frame = [0i16; FRAME_SAMPLES];
    let present = match media(slot) {
        Some(media) => {
            media.pull(&mut frame);
            true
        }
        None => false,
    };
    output.copy_from_slice(&frame);
    present
}

/// Действие пользователя, не требующее ядра: микрофон, сброс, закрытие экрана.
/// Сигнал собеседнику ядро отправит при ближайшей синхронизации.
pub fn action(slot: &CallSlot, name: &str) -> bool {
    with_call(slot, |slot| {
        let Some(call) = slot.as_mut() else {
            return false;
        };
        match name {
            "mute" | "unmute" => {
                if let Some(media) = &call.media {
                    media.muted.store(name == "mute", Ordering::Relaxed);
                }
                true
            }
            "hangup" => {
                match call.phase {
                    Phase::Incoming => call.finish("declined", Some("rejected")),
                    Phase::Calling | Phase::Ringing => call.finish("cancelled", Some("cancelled")),
                    Phase::Connecting | Phase::Active => call.finish("hangup", Some("hangup")),
                    Phase::Ended => {}
                }
                true
            }
            "dismiss" => {
                if call.phase == Phase::Ended && call.pending_end.is_none() && call.recorded {
                    *slot = None;
                } else if call.phase == Phase::Ended {
                    call.ended_ms = Some(0);
                }
                true
            }
            _ => false,
        }
    })
}

/// Состояние звонка для интерфейса. `{"active":false}` — звонка нет.
pub fn status_json(slot: &CallSlot) -> String {
    with_call(slot, |slot| {
        let Some(call) = slot.as_mut() else {
            return json!({ "active": false }).to_string();
        };
        call.tick();
        // Завершённый звонок висит на экране пару секунд, потом закрывается сам.
        let hidden = call.phase == Phase::Ended
            && call.ended_ms.is_some_and(|ended| now_ms() - ended > ENDED_LINGER_MS);
        let media = call.media.clone();
        let value = json!({
            "active": !hidden,
            "callId": call.call_id,
            "peerUserId": call.peer_user_id,
            "peerName": call.peer_name,
            "peerAvatarBase64": call.peer_avatar,
            "outgoing": call.outgoing,
            "phase": call.phase.name(),
            "endReason": call.end_reason,
            "durationMs": call.duration_ms(),
            "muted": media.as_ref().is_some_and(|value| value.muted.load(Ordering::Relaxed)),
            "peerMuted": media.as_ref().is_some_and(|value| value.peer_muted.load(Ordering::Relaxed)),
            "transport": media.as_ref().map(|value| value.transport()).unwrap_or(""),
            "quality": media.as_ref().map(|value| value.quality()).unwrap_or("good"),
            "rttMs": media.as_ref().map(|value| value.rtt_ms.load(Ordering::Relaxed)).unwrap_or(0),
            "localLevel": media.as_ref().map(|value| value.local_level()).unwrap_or(0.0),
            "peerLevel": media.as_ref().map(|value| value.peer_level()).unwrap_or(0.0),
            "safetyCode": media.as_ref().map(|value| value.safety_code()).unwrap_or_default(),
        });
        value.to_string()
    })
}
