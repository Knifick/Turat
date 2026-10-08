//! Голос: Opus и джиттер-буфер.
//!
//! Кадр — 20 мс моно 48 кГц (960 отсчётов). Кодер работает в режиме VoIP со встроенной
//! избыточностью (FEC): в каждом пакете лежит грубая копия предыдущего кадра, поэтому
//! одиночная потеря восстанавливается почти без слышимого следа. Если пропало подряд
//! несколько кадров, декодер «дорисовывает» звук сам (PLC).
//!
//! Джиттер-буфер держит небольшой запас кадров, сглаживая неравномерную доставку. Запас
//! растёт, когда кадры опаздывают, и плавно сокращается на стабильной сети: голос не
//! прерывается и не копит лишнюю задержку.

use std::collections::{BTreeMap, VecDeque};

use unsafe_libopus::{
    OPUS_APPLICATION_VOIP, OPUS_SET_BITRATE_REQUEST, OPUS_SET_COMPLEXITY_REQUEST,
    OPUS_SET_DTX_REQUEST, OPUS_SET_INBAND_FEC_REQUEST, OPUS_SET_PACKET_LOSS_PERC_REQUEST,
    OPUS_SET_SIGNAL_REQUEST, OPUS_SIGNAL_VOICE, OpusDecoder, OpusEncoder, opus_decode,
    opus_decoder_create, opus_decoder_destroy, opus_encode, opus_encoder_create,
    opus_encoder_ctl, opus_encoder_destroy,
};

use crate::CoreError;

pub const SAMPLE_RATE: i32 = 48_000;
pub const FRAME_SAMPLES: usize = 960;
const MAX_PACKET_BYTES: usize = 400;

pub struct Encoder(*mut OpusEncoder);

// Состояние кодера не привязано к потоку, а доступ к нему идёт под замком.
unsafe impl Send for Encoder {}

impl Encoder {
    pub fn new() -> Result<Self, CoreError> {
        let mut error = 0;
        let state = unsafe { opus_encoder_create(SAMPLE_RATE, 1, OPUS_APPLICATION_VOIP, &mut error) };
        if state.is_null() || error != 0 {
            return Err(CoreError::InvalidInput(format!("Opus: кодер не создан ({error})")));
        }
        unsafe {
            opus_encoder_ctl!(state, OPUS_SET_BITRATE_REQUEST, 32_000i32);
            opus_encoder_ctl!(state, OPUS_SET_INBAND_FEC_REQUEST, 1i32);
            opus_encoder_ctl!(state, OPUS_SET_PACKET_LOSS_PERC_REQUEST, 10i32);
            opus_encoder_ctl!(state, OPUS_SET_SIGNAL_REQUEST, OPUS_SIGNAL_VOICE);
            opus_encoder_ctl!(state, OPUS_SET_COMPLEXITY_REQUEST, 6i32);
            opus_encoder_ctl!(state, OPUS_SET_DTX_REQUEST, 0i32);
        }
        Ok(Self(state))
    }

    /// Кадр ровно из 960 отсчётов в пакет Opus.
    pub fn encode(&mut self, pcm: &[i16]) -> Option<Vec<u8>> {
        if pcm.len() != FRAME_SAMPLES {
            return None;
        }
        let mut buffer = [0u8; MAX_PACKET_BYTES];
        let written = unsafe {
            opus_encode(self.0, pcm.as_ptr(), FRAME_SAMPLES as i32, buffer.as_mut_ptr(), MAX_PACKET_BYTES as i32)
        };
        (written > 0).then(|| buffer[..written as usize].to_vec())
    }

    /// Ожидаемая доля потерь: кодер вкладывает в пакеты столько избыточности, сколько нужно.
    pub fn set_expected_loss(&mut self, percent: i32) {
        unsafe {
            opus_encoder_ctl!(self.0, OPUS_SET_PACKET_LOSS_PERC_REQUEST, percent.clamp(5, 40));
        }
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        unsafe { opus_encoder_destroy(self.0) };
    }
}

pub struct Decoder(*mut OpusDecoder);

unsafe impl Send for Decoder {}

impl Decoder {
    pub fn new() -> Result<Self, CoreError> {
        let mut error = 0;
        let state = unsafe { opus_decoder_create(SAMPLE_RATE, 1, &mut error) };
        if state.is_null() || error != 0 {
            return Err(CoreError::InvalidInput(format!("Opus: декодер не создан ({error})")));
        }
        Ok(Self(state))
    }

    /// `packet: None` — кадр потерян, декодер дорисовывает его сам. `fec` — восстановить
    /// предыдущий кадр по избыточности из этого пакета.
    fn decode(&mut self, packet: Option<&[u8]>, fec: bool, output: &mut [i16; FRAME_SAMPLES]) -> bool {
        let (pointer, length) = match packet {
            Some(value) => (value.as_ptr(), value.len() as i32),
            None => (std::ptr::null(), 0),
        };
        let decoded = unsafe {
            opus_decode(self.0, pointer, length, output.as_mut_ptr(), FRAME_SAMPLES as i32, i32::from(fec))
        };
        if decoded != FRAME_SAMPLES as i32 {
            output.fill(0);
            return false;
        }
        true
    }
}

impl Drop for Decoder {
    fn drop(&mut self) {
        unsafe { opus_decoder_destroy(self.0) };
    }
}

//// Счётчики качества приёма — для индикатора связи в интерфейсе.
#[derive(Debug, Default, Clone, Copy)]
pub struct JitterStats {
    pub received: u64,
    /// Потерянный кадр восстановлен по избыточности следующего пакета.
    pub recovered: u64,
    /// Кадр «дорисован» декодером: пакет потерялся или ещё не пришёл.
    pub concealed: u64,
    /// Пакет пришёл, когда его место уже было проиграно.
    pub late: u64,
    /// Кадры, срезанные ради меньшей задержки.
    pub trimmed: u64,
    pub depth_frames: usize,
    pub target_frames: usize,
}

/// Запас, который буфер держит сверх проигрываемого кадра, — в кадрах по 20 мс.
const MIN_SPARE: usize = 1;
const START_SPARE: usize = 2;
const MAX_SPARE: usize = 10;
/// Пакетов накопилось на полсекунды и больше — звук отстал, догоняем рывком.
const MAX_BACKLOG: usize = 25;
/// Сколько подряд «дорисовывать» звук в ожидании опоздавшего пакета; дальше — тишина.
const CONCEAL_LIMIT: u32 = 10;
/// Собеседник молчит секунду: буферизация начнётся заново, когда звук вернётся.
const RESTART_AFTER: u32 = 50;
/// Окно, по которому решается, можно ли срезать задержку: 2 секунды.
const TRIM_WINDOW: u32 = 100;
/// Столько ровной работы без провалов — и запас можно уменьшить: 10 секунд.
const CALM_FRAMES: u32 = 500;
/// Перекрёстный переход при срезе задержки: 10 мс, чтобы срез не щёлкал.
const CROSSFADE_SAMPLES: usize = 480;
/// Качество связи считается по последним секундам, а не за весь звонок.
const QUALITY_SECONDS: usize = 5;

/// Джиттер-буфер с подстройкой под сеть.
///
/// Главное правило: опоздавший пакет ждём, а не выбрасываем. Если буфер опустел, декодер
/// «дорисовывает» 20 мс, но номер ожидаемого кадра не сдвигается: пришедший следом пакет
/// проигрывается целиком, а запас буфера сам вырастает на величину опоздания. Прежний
/// буфер в такой ситуации дорисовывал кадр, сдвигался дальше и выбрасывал настоящий пакет
/// как опоздавший — на неровной сети (мобильная связь, Wi-Fi, кадры пачками от аудио-HAL)
/// это давало непрерывное «дорисовывание» с металлическим треском и ложный «плохой сигнал».
///
/// Лишняя задержка срезается по 40 мс с перекрёстным переходом и только тогда, когда за
/// последние две секунды запас ни разу не опускался до нужного.
pub struct JitterBuffer {
    frames: BTreeMap<u32, Vec<u8>>,
    next: Option<u32>,
    decoder: Decoder,
    /// Желаемый запас кадров сверх проигрываемого.
    spare: usize,
    /// Подряд выданные кадры при пустом буфере.
    waiting: u32,
    trim_frames: u32,
    trim_min_depth: usize,
    calm_frames: u32,
    /// Последние секунды: (выдано кадров, из них дорисовано).
    seconds: VecDeque<(u32, u32)>,
    second_frames: u32,
    second_damaged: u32,
    pub stats: JitterStats,
}

impl JitterBuffer {
    pub fn new() -> Result<Self, CoreError> {
        Ok(Self {
            frames: BTreeMap::new(),
            next: None,
            decoder: Decoder::new()?,
            spare: START_SPARE,
            waiting: 0,
            trim_frames: 0,
            trim_min_depth: usize::MAX,
            calm_frames: 0,
            seconds: VecDeque::with_capacity(QUALITY_SECONDS + 1),
            second_frames: 0,
            second_damaged: 0,
            stats: JitterStats {
                target_frames: START_SPARE + 1,
                ..JitterStats::default()
            },
        })
    }

    pub fn insert(&mut self, index: u32, packet: Vec<u8>) {
        if let Some(next) = self.next
            && index < next
        {
            self.stats.late += 1;
            return;
        }
        // Защита от мусора: дальний «будущий» кадр означает сброс у собеседника.
        if self.frames.len() > 200 {
            self.frames.clear();
            self.next = None;
        }
        self.stats.received += 1;
        self.frames.insert(index, packet);
    }

    /// Кадры, которые ещё предстоит проиграть, включая ожидаемый.
    fn depth(&self) -> usize {
        match self.next {
            Some(next) => self.frames.range(next..).count(),
            None => self.frames.len(),
        }
    }

    /// Доля дорисованных кадров за последние секунды, 0..1.
    pub fn recent_damage(&self) -> f64 {
        let (frames, damaged) = self
            .seconds
            .iter()
            .fold((self.second_frames, self.second_damaged), |(f, d), &(sf, sd)| {
                (f + sf, d + sd)
            });
        if frames < 25 {
            return 0.0;
        }
        f64::from(damaged) / f64::from(frames)
    }

    /// Следующие 20 мс для динамика.
    pub fn pull(&mut self, output: &mut [i16; FRAME_SAMPLES]) {
        let next = match self.next {
            Some(value) => value,
            None => {
                // Запускаемся, только накопив запас: иначе первые же доли секунды
                // сетевой неровности превратились бы в щелчки.
                if self.frames.len() < self.spare + 1 {
                    output.fill(0);
                    return;
                }
                *self.frames.keys().next().expect("non-empty")
            }
        };
        self.next = Some(next);
        while let Some((&first, _)) = self.frames.iter().next() {
            if first >= next {
                break;
            }
            self.frames.remove(&first);
        }

        let depth = self.depth();
        self.trim_min_depth = self.trim_min_depth.min(depth);
        if depth > MAX_BACKLOG {
            // Звук отстал на полсекунды и больше (например, после переподключения):
            // оставляем нужный запас и догоняем сразу, а не по кадру в секунду.
            let keep = self.spare + 1;
            let skip_to = *self.frames.keys().nth(depth - keep).expect("depth counted");
            self.frames.retain(|&index, _| index >= skip_to);
            self.stats.trimmed += (depth - keep) as u64;
            self.next = Some(skip_to);
            self.trim_min_depth = usize::MAX;
            return self.pull(output);
        }

        let mut damaged = false;
        if let Some(packet) = self.frames.remove(&next) {
            self.decoder.decode(Some(&packet), false, output);
            self.waiting = 0;
            self.next = Some(next.wrapping_add(1));
        } else if self.frames.range(next..).next().is_some() {
            // Следующие пакеты уже здесь, а этого нет — он потерян, ждать бессмысленно:
            // восстанавливаем по избыточности соседа или дорисовываем.
            if let Some(following) = self.frames.get(&next.wrapping_add(1)).cloned() {
                self.decoder.decode(Some(&following), true, output);
                self.stats.recovered += 1;
            } else {
                self.decoder.decode(None, false, output);
                self.stats.concealed += 1;
                damaged = true;
            }
            self.waiting = 0;
            self.next = Some(next.wrapping_add(1));
        } else {
            // Буфер пуст: пакет опаздывает. Дорисовываем 20 мс, но ждём именно его —
            // номер не сдвигается, и запас вырастает ровно на опоздание.
            if self.waiting == 0 {
                self.spare = (self.spare + 1).min(MAX_SPARE);
                self.calm_frames = 0;
            }
            self.waiting += 1;
            if self.waiting <= CONCEAL_LIMIT {
                self.decoder.decode(None, false, output);
                self.stats.concealed += 1;
                damaged = true;
            } else {
                output.fill(0);
            }
            if self.waiting >= RESTART_AFTER {
                // Собеседник пропал надолго — начинаем заново, когда звук вернётся.
                self.next = None;
                self.waiting = 0;
            }
        }
        if !damaged && self.waiting == 0 && self.should_trim() {
            self.trim(output);
        }
        self.account(damaged);
    }

    /// Можно ли срезать задержку: за окно запас ни разу не опускался до нужного.
    fn should_trim(&mut self) -> bool {
        self.trim_frames += 1;
        if self.trim_frames < TRIM_WINDOW {
            return false;
        }
        let excess = self.trim_min_depth != usize::MAX && self.trim_min_depth > self.spare + 3;
        self.trim_frames = 0;
        self.trim_min_depth = usize::MAX;
        let Some(next) = self.next else {
            return false;
        };
        excess
            && self.frames.contains_key(&next)
            && self.frames.contains_key(&next.wrapping_add(1))
    }

    /// Срез 40 мс без щелчка. Во второй половине только что выданного кадра звук плавно
    /// переходит в вторую половину кадра через один; оба пропускаемых кадра всё равно
    /// декодируются, поэтому состояние декодера остаётся непрерывным, и следующий кадр
    /// продолжает уже то, чем закончился переход.
    fn trim(&mut self, output: &mut [i16; FRAME_SAMPLES]) {
        let Some(next) = self.next else {
            return;
        };
        let (Some(skipped), Some(following)) = (
            self.frames.remove(&next),
            self.frames.remove(&next.wrapping_add(1)),
        ) else {
            return;
        };
        let mut incoming = [0i16; FRAME_SAMPLES];
        self.decoder.decode(Some(&skipped), false, &mut incoming);
        self.decoder.decode(Some(&following), false, &mut incoming);
        let start = FRAME_SAMPLES - CROSSFADE_SAMPLES;
        for i in 0..CROSSFADE_SAMPLES {
            let weight = (i + 1) as f32 / CROSSFADE_SAMPLES as f32;
            let blended =
                f32::from(output[start + i]) * (1.0 - weight) + f32::from(incoming[start + i]) * weight;
            output[start + i] = blended.round().clamp(f32::from(i16::MIN), f32::from(i16::MAX)) as i16;
        }
        self.next = Some(next.wrapping_add(2));
        self.stats.trimmed += 2;
    }

    fn account(&mut self, damaged: bool) {
        self.second_frames += 1;
        if damaged {
            self.second_damaged += 1;
        } else {
            self.calm_frames += 1;
            if self.calm_frames >= CALM_FRAMES && self.spare > MIN_SPARE {
                self.spare -= 1;
                self.calm_frames = 0;
            }
        }
        if self.second_frames >= 50 {
            self.seconds.push_back((self.second_frames, self.second_damaged));
            while self.seconds.len() > QUALITY_SECONDS {
                self.seconds.pop_front();
            }
            self.second_frames = 0;
            self.second_damaged = 0;
        }
        self.stats.target_frames = self.spare + 1;
        self.stats.depth_frames = self.depth();
    }
}

// Громкость кадра 0..1 по логарифмической шкале — для анимации волны в интерфейсе.
pub fn level(pcm: &[i16]) -> f32 {
    if pcm.is_empty() {
        return 0.0;
    }
    let energy: f64 = pcm.iter().map(|&value| (value as f64) * (value as f64)).sum::<f64>() / pcm.len() as f64;
    let rms = energy.sqrt() / 32768.0;
    if rms <= 0.0 {
        return 0.0;
    }
    // −60 дБ → 0, 0 дБ → 1.
    ((20.0 * rms.log10() + 60.0) / 60.0).clamp(0.0, 1.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(frame: u32) -> Vec<i16> {
        (0..FRAME_SAMPLES)
            .map(|i| {
                let t = (frame as usize * FRAME_SAMPLES + i) as f32 / SAMPLE_RATE as f32;
                ((t * 300.0 * std::f32::consts::TAU).sin() * 9000.0) as i16
            })
            .collect()
    }

    #[test]
    fn audio_survives_loss_and_reordering() {
        let mut encoder = Encoder::new().unwrap();
        let mut buffer = JitterBuffer::new().unwrap();
        let packets: Vec<Vec<u8>> = (0..200).map(|frame| encoder.encode(&tone(frame)).unwrap()).collect();
        // Каждый седьмой потерян, соседние пары переставлены.
        let mut order: Vec<u32> = (0..200).filter(|frame| frame % 7 != 3).collect();
        for pair in order.chunks_mut(2) {
            pair.reverse();
        }
        let mut output = [0i16; FRAME_SAMPLES];
        let mut loud = 0;
        let mut fed = order.into_iter().peekable();
        for _ in 0..220 {
            for _ in 0..1 {
                if let Some(frame) = fed.next() {
                    buffer.insert(frame, packets[frame as usize].clone());
                }
            }
            buffer.pull(&mut output);
            if level(&output) > 0.5 {
                loud += 1;
            }
        }
        assert!(buffer.stats.recovered > 20, "{:?}", buffer.stats);
        assert!(loud > 180, "звук должен идти почти без провалов: {loud} {:?}", buffer.stats);
    }

    /// Пакеты приходят пачками по три раз в 60 мс, как бывает на мобильной сети и
    /// с аудио-HAL, отдающим микрофон крупными порциями. Потерь нет вовсе — значит, после
    /// короткой подстройки звук должен идти без дорисовывания и без выброшенных пакетов.
    #[test]
    fn bursty_delivery_without_loss_sounds_clean() {
        let mut encoder = Encoder::new().unwrap();
        let mut buffer = JitterBuffer::new().unwrap();
        let mut output = [0i16; FRAME_SAMPLES];
        let mut sent = 0u32;
        for tick in 0..1_500u32 {
            if tick % 3 == 0 {
                for _ in 0..3 {
                    buffer.insert(sent, encoder.encode(&tone(sent)).unwrap());
                    sent += 1;
                }
            }
            buffer.pull(&mut output);
        }
        assert_eq!(buffer.stats.late, 0, "{:?}", buffer.stats);
        assert!(buffer.recent_damage() < 0.01, "{} {:?}", buffer.recent_damage(), buffer.stats);
        assert!(buffer.stats.concealed < 10, "{:?}", buffer.stats);
    }

    /// Пакет задержался на 100 мс: прежний буфер дорисовывал его место и выбрасывал
    /// сам пакет. Теперь буфер ждёт: пакет проигрывается, а не теряется.
    #[test]
    fn a_late_packet_is_played_not_dropped() {
        let mut encoder = Encoder::new().unwrap();
        let mut buffer = JitterBuffer::new().unwrap();
        let mut output = [0i16; FRAME_SAMPLES];
        let packets: Vec<Vec<u8>> = (0..400).map(|frame| encoder.encode(&tone(frame)).unwrap()).collect();
        for tick in 0..400u32 {
            match tick {
                // Кадры 200–204 застряли в сети и пришли разом вместе с 205-м.
                200..=204 => {}
                205 => {
                    for frame in 200..=205 {
                        buffer.insert(frame, packets[frame as usize].clone());
                    }
                }
                _ => buffer.insert(tick, packets[tick as usize].clone()),
            }
            buffer.pull(&mut output);
        }
        assert_eq!(buffer.stats.late, 0, "{:?}", buffer.stats);
        assert_eq!(buffer.stats.recovered, 0, "ни один кадр не потерян: {:?}", buffer.stats);
    }

    /// После обрыва связи пришла секунда накопившегося звука: буфер догоняет сразу,
    /// а не проигрывает её с опозданием.
    #[test]
    fn a_backlog_is_skipped_at_once() {
        let mut encoder = Encoder::new().unwrap();
        let mut buffer = JitterBuffer::new().unwrap();
        let mut output = [0i16; FRAME_SAMPLES];
        for frame in 0..60u32 {
            buffer.insert(frame, encoder.encode(&tone(frame)).unwrap());
        }
        buffer.pull(&mut output);
        buffer.pull(&mut output);
        assert!(buffer.stats.depth_frames <= MAX_SPARE + 1, "{:?}", buffer.stats);
        assert!(buffer.stats.trimmed > 40, "{:?}", buffer.stats);
    }

    #[test]
    fn levels_follow_loudness() {
        assert_eq!(level(&[0; 960]), 0.0);
        assert!(level(&tone(0)) > 0.7);
        assert!(level(&[30; 960]) < 0.3);
    }
}
