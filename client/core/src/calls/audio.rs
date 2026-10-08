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

use std::collections::BTreeMap;

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
const MIN_DEPTH: usize = 2;
const MAX_DEPTH: usize = 12;
const START_DEPTH: usize = 3;

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

/// Счётчики качества приёма — для индикатора связи в интерфейсе.
#[derive(Debug, Default, Clone, Copy)]
pub struct JitterStats {
    pub received: u64,
    pub recovered: u64,
    pub concealed: u64,
    pub late: u64,
    pub depth_frames: usize,
    pub target_frames: usize,
}

pub struct JitterBuffer {
    frames: BTreeMap<u32, Vec<u8>>,
    next: Option<u32>,
    target: usize,
    decoder: Decoder,
    /// Подряд не пришедшие кадры: долгий провал лучше пережить тишиной, чем «роботом».
    missing_streak: u32,
    /// Окно наблюдения для подстройки запаса.
    window_frames: u32,
    window_underruns: u32,
    window_min_depth: usize,
    calm_windows: u32,
    pub stats: JitterStats,
}

impl JitterBuffer {
    pub fn new() -> Result<Self, CoreError> {
        Ok(Self {
            frames: BTreeMap::new(),
            next: None,
            target: START_DEPTH,
            decoder: Decoder::new()?,
            missing_streak: 0,
            window_frames: 0,
            window_underruns: 0,
            window_min_depth: usize::MAX,
            calm_windows: 0,
            stats: JitterStats {
                target_frames: START_DEPTH,
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

    fn depth(&self) -> usize {
        match self.next {
            Some(next) => self.frames.range(next..).count(),
            None => self.frames.len(),
        }
    }

    /// Следующие 20 мс для динамика.
    pub fn pull(&mut self, output: &mut [i16; FRAME_SAMPLES]) {
        let next = match self.next {
            Some(value) => value,
            None => {
                // Запускаемся, только накопив запас: иначе первые же доли секунды
                // сетевой неровности превратились бы в щелчки.
                if self.frames.len() < self.target {
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
        self.window_min_depth = self.window_min_depth.min(depth);
        if let Some(packet) = self.frames.remove(&next) {
            self.decoder.decode(Some(&packet), false, output);
            self.missing_streak = 0;
        } else if let Some(following) = self.frames.get(&(next + 1)).cloned() {
            self.decoder.decode(Some(&following), true, output);
            self.stats.recovered += 1;
            self.missing_streak = 0;
        } else {
            self.window_underruns += 1;
            self.missing_streak += 1;
            if self.missing_streak <= 10 {
                self.decoder.decode(None, false, output);
                self.stats.concealed += 1;
            } else {
                output.fill(0);
            }
            if self.frames.is_empty() && self.missing_streak > 10 {
                // Собеседник пропал надолго — начинаем заново, когда звук вернётся.
                self.next = None;
                self.adapt();
                return;
            }
            if !self.frames.is_empty() && self.missing_streak > 3 {
                // Впереди уже есть звук: перепрыгиваем дыру, а не ждём её.
                self.next = self.frames.keys().next().copied();
                self.adapt();
                return;
            }
        }
        self.next = Some(next.wrapping_add(1));
        self.adapt();
    }

    /// Раз в секунду: опаздывали кадры — запас растёт, сеть ровная — сокращается.
    fn adapt(&mut self) {
        self.window_frames += 1;
        if self.window_frames < 50 {
            self.stats.depth_frames = self.depth();
            return;
        }
        if self.window_underruns >= 2 {
            self.target = (self.target + 1).min(MAX_DEPTH);
            self.calm_windows = 0;
        } else {
            self.calm_windows += 1;
            if self.calm_windows >= 5 && self.target > MIN_DEPTH {
                self.target -= 1;
                self.calm_windows = 0;
            }
        }
        // Лишний запас — лишняя задержка: выбрасываем один самый старый кадр.
        if self.window_min_depth != usize::MAX
            && self.window_min_depth > self.target + 2
            && let Some(next) = self.next
        {
            self.frames.remove(&next);
            self.next = Some(next.wrapping_add(1));
        }
        self.window_frames = 0;
        self.window_underruns = 0;
        self.window_min_depth = usize::MAX;
        self.stats.target_frames = self.target;
        self.stats.depth_frames = self.depth();
    }
}

/// Громкость кадра 0..1 по логарифмической шкале — для анимации волны в интерфейсе.
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

    #[test]
    fn levels_follow_loudness() {
        assert_eq!(level(&[0; 960]), 0.0);
        assert!(level(&tone(0)) > 0.7);
        assert!(level(&[30; 960]) < 0.3);
    }
}
