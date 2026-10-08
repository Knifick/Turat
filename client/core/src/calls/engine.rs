//! Медиа звонка: шифрование кадров, поток ввода-вывода и переключение транспорта.
//!
//! Платформа отдаёт сюда кадры микрофона (`push`) и забирает кадры для динамика (`pull`)
//! каждые 20 мс. Ни то, ни другое не трогает замок ядра: синхронизация переписки может
//! идти несколько секунд, а голос ждать не может.
//!
//! Поток ввода-вывода держит канал до ретранслятора. Начинает с UDP; если ретранслятор не
//! ответил за пару секунд или UDP посреди звонка замолчал (сеть сменилась, трафик режут),
//! переходит на WebSocket поверх TLS и обратно — звонок при этом не обрывается: ключи и
//! номера пакетов от транспорта не зависят.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering},
        mpsc::{self, Receiver, Sender},
    },
    time::{Duration, Instant},
};

use super::{
    audio::{Encoder, FRAME_SAMPLES, JitterBuffer, JitterStats, level},
    crypto::{CallKeys, ReplayWindow},
    transport::{
        DATA, HEADER_BYTES, JOINED, Link, ROOM_BYTES, RelayTicket, UdpLink, WebSocketLink, header,
        join_frame,
    },
};
use crate::CoreError;

const KIND_AUDIO: u8 = 1;
const KIND_HANGUP: u8 = 2;
const KIND_PING: u8 = 3;
const KIND_PONG: u8 = 4;
const KIND_STATE: u8 = 5;

/// Ретранслятор не ответил на JOIN за это время — пробуем другой транспорт.
const JOIN_TIMEOUT: Duration = Duration::from_millis(2_000);
/// Ретранслятор замолчал посреди звонка — канал считается мёртвым.
const STALL_TIMEOUT: Duration = Duration::from_millis(2_500);
const KEEPALIVE: Duration = Duration::from_millis(600);
const PING_EVERY: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(4);

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

pub struct CallMedia {
    keys: CallKeys,
    room: [u8; ROOM_BYTES],
    side: u8,
    token: [u8; 32],
    ticket: RelayTicket,
    outgoing: Mutex<Sender<Vec<u8>>>,
    receiver: Mutex<Option<Receiver<Vec<u8>>>>,
    encoder: Mutex<Encoder>,
    jitter: Mutex<JitterBuffer>,
    replay: Mutex<ReplayWindow>,
    sequence: AtomicU64,
    frame_index: AtomicU32,
    pub muted: AtomicBool,
    stop: AtomicBool,
    /// Первый настоящий кадр от собеседника: только он доказывает, что связь есть.
    pub peer_heard_ms: AtomicI64,
    pub last_peer_ms: AtomicI64,
    pub remote_hangup: AtomicBool,
    pub peer_muted: AtomicBool,
    pub rtt_ms: AtomicU32,
    transport: Mutex<&'static str>,
    local_level: AtomicU32,
    peer_level: AtomicU32,
    sent_audio: AtomicU64,
}

impl CallMedia {
    pub fn new(keys: CallKeys, ticket: RelayTicket, side: u8) -> Result<Arc<Self>, CoreError> {
        ticket.validate()?;
        let (sender, receiver) = mpsc::channel();
        Ok(Arc::new(Self {
            room: ticket.room()?,
            token: ticket.token_bytes()?,
            keys,
            side,
            ticket,
            outgoing: Mutex::new(sender),
            receiver: Mutex::new(Some(receiver)),
            encoder: Mutex::new(Encoder::new()?),
            jitter: Mutex::new(JitterBuffer::new()?),
            replay: Mutex::new(ReplayWindow::default()),
            sequence: AtomicU64::new(1),
            frame_index: AtomicU32::new(0),
            muted: AtomicBool::new(false),
            stop: AtomicBool::new(false),
            peer_heard_ms: AtomicI64::new(0),
            last_peer_ms: AtomicI64::new(0),
            remote_hangup: AtomicBool::new(false),
            peer_muted: AtomicBool::new(false),
            rtt_ms: AtomicU32::new(0),
            transport: Mutex::new("connecting"),
            local_level: AtomicU32::new(0),
            peer_level: AtomicU32::new(0),
            sent_audio: AtomicU64::new(0),
        }))
    }

    pub fn safety_code(&self) -> Vec<&'static str> {
        self.keys.safety_code.clone()
    }

    /// Запуск потока ввода-вывода.
    pub fn start(self: &Arc<Self>) {
        let Some(receiver) = self.receiver.lock().ok().and_then(|mut slot| slot.take()) else {
            return;
        };
        let media = Arc::clone(self);
        std::thread::Builder::new()
            .name("turat-call-io".to_owned())
            .spawn(move || media.run(receiver))
            .ok();
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    /// Кадр микрофона: 960 отсчётов, 20 мс.
    pub fn push(&self, pcm: &[i16]) {
        if self.stopped() || pcm.len() != FRAME_SAMPLES {
            return;
        }
        let muted = self.muted.load(Ordering::Relaxed);
        self.local_level
            .store(if muted { 0f32 } else { level(pcm) }.to_bits(), Ordering::Relaxed);
        // Без звука шлём кодированную тишину: у собеседника не «рвётся» буфер, и после
        // включения микрофона голос пойдёт без паузы на разгон.
        let silence = [0i16; FRAME_SAMPLES];
        let source = if muted { &silence[..] } else { pcm };
        let Some(packet) = self.encoder.lock().ok().and_then(|mut encoder| encoder.encode(source)) else {
            return;
        };
        let index = self.frame_index.fetch_add(1, Ordering::Relaxed);
        let mut plain = Vec::with_capacity(5 + packet.len());
        plain.push(KIND_AUDIO);
        plain.extend_from_slice(&index.to_be_bytes());
        plain.extend_from_slice(&packet);
        self.send_plain(&plain);
        self.sent_audio.fetch_add(1, Ordering::Relaxed);
        if index.is_multiple_of(50) {
            self.send_plain(&[KIND_STATE, u8::from(muted)]);
        }
    }

    /// 20 мс для динамика. Без звонка или до первых кадров — тишина.
    pub fn pull(&self, output: &mut [i16; FRAME_SAMPLES]) {
        match self.jitter.lock() {
            Ok(mut jitter) => jitter.pull(output),
            Err(_) => output.fill(0),
        }
        self.peer_level.store(level(output).to_bits(), Ordering::Relaxed);
    }

    /// Мгновенный сигнал завершения прямо по медиаканалу: событие через почтовый ящик
    /// дойдёт следом, но собеседник не должен ждать его, чтобы увидеть конец звонка.
    pub fn hang_up(&self) {
        for _ in 0..3 {
            self.send_plain(&[KIND_HANGUP]);
        }
        // Даём потоку ввода-вывода отправить их до остановки.
        let media_stop = &self.stop;
        std::thread::sleep(Duration::from_millis(60));
        media_stop.store(true, Ordering::SeqCst);
    }

    pub fn transport(&self) -> &'static str {
        self.transport.lock().map(|value| *value).unwrap_or("connecting")
    }

    pub fn local_level(&self) -> f32 {
        f32::from_bits(self.local_level.load(Ordering::Relaxed))
    }

    pub fn peer_level(&self) -> f32 {
        f32::from_bits(self.peer_level.load(Ordering::Relaxed))
    }

    pub fn stats(&self) -> JitterStats {
        self.jitter.lock().map(|jitter| jitter.stats).unwrap_or_default()
    }

    /// Оценка связи для значка в интерфейсе.
    pub fn quality(&self) -> &'static str {
        let stats = self.stats();
        let rtt = self.rtt_ms.load(Ordering::Relaxed);
        let total = stats.received.max(1) as f64;
        let damaged = (stats.concealed + stats.late) as f64 / total;
        let last_peer = self.last_peer_ms.load(Ordering::Relaxed);
        let silent = last_peer > 0 && now_ms() - last_peer > 2_000;
        if silent || damaged > 0.08 || rtt > 600 {
            "poor"
        } else if damaged > 0.02 || rtt > 300 {
            "fair"
        } else {
            "good"
        }
    }

    fn send_plain(&self, plain: &[u8]) {
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let header = header(DATA, &self.room, self.side);
        let sealed = self.keys.seal(&header, sequence, plain);
        let mut frame = Vec::with_capacity(HEADER_BYTES + sealed.len());
        frame.extend_from_slice(&header);
        frame.extend_from_slice(&sealed);
        if let Ok(sender) = self.outgoing.lock() {
            let _ = sender.send(frame);
        }
    }

    fn set_transport(&self, value: &'static str) {
        if let Ok(mut slot) = self.transport.lock() {
            *slot = value;
        }
    }

    fn run(self: Arc<Self>, receiver: Receiver<Vec<u8>>) {
        let udp_available = self.ticket.udp_host.is_some() && self.ticket.udp_port > 0;
        let mut prefer_udp = udp_available;
        let mut backoff = Duration::from_millis(200);
        while !self.stopped() {
            let link: Option<Box<dyn Link>> = if prefer_udp {
                let host = self.ticket.udp_host.clone().unwrap_or_default();
                UdpLink::connect(&host, self.ticket.udp_port)
                    .ok()
                    .map(|link| Box::new(link) as Box<dyn Link>)
            } else {
                WebSocketLink::connect(&self.ticket.web_socket_url)
                    .ok()
                    .map(|link| Box::new(link) as Box<dyn Link>)
            };
            let outcome = match link {
                Some(link) => self.session(link, &receiver),
                None => Outcome::Failed,
            };
            if self.stopped() {
                break;
            }
            match outcome {
                Outcome::Stopped => break,
                Outcome::Worked => backoff = Duration::from_millis(200),
                Outcome::Failed => {
                    std::thread::sleep(backoff);
                    backoff = (backoff * 2).min(Duration::from_secs(2));
                }
            }
            // Сменить транспорт: UDP не отвечает — TLS, TLS оборвался — снова UDP.
            if udp_available {
                prefer_udp = !prefer_udp;
            }
            self.set_transport("reconnecting");
        }
        // Остаток очереди (например, сигнал завершения) — на последнем канале уже не уйдёт.
        drop(receiver);
    }

    fn session(&self, mut link: Box<dyn Link>, receiver: &Receiver<Vec<u8>>) -> Outcome {
        let join = join_frame(&self.room, self.side, &self.token);
        let started = Instant::now();
        let mut joined = false;
        let mut last_join = Instant::now() - KEEPALIVE;
        let mut last_ping = Instant::now();
        let mut last_heard = Instant::now();
        let mut buffer = Vec::with_capacity(2048);
        let peer_side = 1 - self.side;
        loop {
            if self.stopped() {
                // Последние кадры (сигнал завершения) уходят перед выходом.
                while let Ok(frame) = receiver.try_recv() {
                    let _ = link.send(&frame);
                }
                return Outcome::Stopped;
            }
            let interval = if joined { KEEPALIVE } else { Duration::from_millis(300) };
            if last_join.elapsed() >= interval {
                if link.send(&join).is_err() {
                    return if joined { Outcome::Worked } else { Outcome::Failed };
                }
                last_join = Instant::now();
            }
            if !joined && started.elapsed() > JOIN_TIMEOUT {
                return Outcome::Failed;
            }
            if joined && last_heard.elapsed() > STALL_TIMEOUT {
                return Outcome::Worked;
            }
            if joined {
                while let Ok(frame) = receiver.try_recv() {
                    if link.send(&frame).is_err() {
                        return Outcome::Worked;
                    }
                }
                if last_ping.elapsed() >= PING_EVERY {
                    let mut ping = vec![KIND_PING];
                    ping.extend_from_slice(&now_ms().to_be_bytes());
                    self.send_plain(&ping);
                    last_ping = Instant::now();
                }
            }
            let received = match link.receive(&mut buffer, POLL) {
                Ok(value) => value,
                Err(_) => return if joined { Outcome::Worked } else { Outcome::Failed },
            };
            let Some(length) = received else {
                continue;
            };
            let frame = &buffer[..length];
            if frame.len() < HEADER_BYTES || frame[1..1 + ROOM_BYTES] != self.room {
                continue;
            }
            match frame[0] {
                JOINED if frame[HEADER_BYTES - 1] == self.side => {
                    if !joined {
                        joined = true;
                        self.set_transport(link.kind());
                    }
                    last_heard = Instant::now();
                }
                DATA if frame[HEADER_BYTES - 1] == peer_side => {
                    last_heard = Instant::now();
                    self.receive(&frame[..HEADER_BYTES], &frame[HEADER_BYTES..]);
                }
                _ => {}
            }
        }
    }

    fn receive(&self, header: &[u8], packet: &[u8]) {
        let Some((sequence, plain)) = self.keys.open(header, packet) else {
            return;
        };
        let fresh = self.replay.lock().map(|mut window| window.accept(sequence)).unwrap_or(false);
        if !fresh || plain.is_empty() {
            return;
        }
        let now = now_ms();
        self.last_peer_ms.store(now, Ordering::Relaxed);
        let _ = self
            .peer_heard_ms
            .compare_exchange(0, now, Ordering::Relaxed, Ordering::Relaxed);
        match plain[0] {
            KIND_AUDIO if plain.len() > 5 => {
                let index = u32::from_be_bytes(plain[1..5].try_into().expect("4 bytes"));
                if let Ok(mut jitter) = self.jitter.lock() {
                    jitter.insert(index, plain[5..].to_vec());
                }
            }
            KIND_HANGUP => self.remote_hangup.store(true, Ordering::SeqCst),
            KIND_PING if plain.len() == 9 => {
                let mut pong = vec![KIND_PONG];
                pong.extend_from_slice(&plain[1..9]);
                self.send_plain(&pong);
            }
            KIND_PONG if plain.len() == 9 => {
                let sent = i64::from_be_bytes(plain[1..9].try_into().expect("8 bytes"));
                let rtt = (now - sent).clamp(0, 60_000) as u32;
                self.rtt_ms.store(rtt, Ordering::Relaxed);
                // Чем хуже сеть, тем больше избыточности кладёт кодер.
                let stats = self.stats();
                let loss = ((stats.concealed + stats.recovered) * 100 / stats.received.max(1) + 5) as i32;
                if let Ok(mut encoder) = self.encoder.lock() {
                    encoder.set_expected_loss(loss);
                }
            }
            KIND_STATE if plain.len() == 2 => self.peer_muted.store(plain[1] != 0, Ordering::Relaxed),
            _ => {}
        }
    }
}

enum Outcome {
    Stopped,
    /// Канал работал, но умер: переподключаемся сразу.
    Worked,
    /// Канал так и не заработал: пробуем другой с паузой.
    Failed,
}
