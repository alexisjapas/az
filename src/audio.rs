use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use crossbeam_channel::{Receiver, bounded};
use std::thread;

pub const TARGET_RATE: u32 = 16_000;
pub const FRAME_MS: usize = 30;
pub const FRAME_SIZE: usize = TARGET_RATE as usize * FRAME_MS / 1000; // 480

pub struct AudioCapture {
    _stream: cpal::Stream,
    utterance_rx: Receiver<Vec<f32>>,
    sample_rate: u32,
    channels: u16,
}

impl AudioCapture {
    pub fn start() -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or_else(|| anyhow::anyhow!("aucun périphérique d'entrée disponible"))?;
        let default_config = device.default_input_config()?;
        let sample_rate = default_config.sample_rate().0;
        let channels = default_config.channels();
        let sample_format = default_config.sample_format();
        let config: StreamConfig = default_config.config();

        let (raw_tx, raw_rx) = bounded::<Vec<f32>>(64);
        let err_fn = |err| eprintln!("[audio] stream error: {err}");

        let stream = match sample_format {
            SampleFormat::F32 => {
                let tx = raw_tx.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[f32], _: &_| {
                        let mono = to_mono_f32(data, channels);
                        let _ = tx.try_send(mono);
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::I16 => {
                let tx = raw_tx.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[i16], _: &_| {
                        let f: Vec<f32> =
                            data.iter().map(|&s| s as f32 / i16::MAX as f32).collect();
                        let mono = to_mono_f32(&f, channels);
                        let _ = tx.try_send(mono);
                    },
                    err_fn,
                    None,
                )?
            }
            SampleFormat::U16 => {
                let tx = raw_tx.clone();
                device.build_input_stream(
                    &config,
                    move |data: &[u16], _: &_| {
                        let f: Vec<f32> = data
                            .iter()
                            .map(|&s| (s as f32 - 32768.0) / 32768.0)
                            .collect();
                        let mono = to_mono_f32(&f, channels);
                        let _ = tx.try_send(mono);
                    },
                    err_fn,
                    None,
                )?
            }
            other => anyhow::bail!("format d'échantillon non supporté: {other:?}"),
        };

        stream.play()?;

        let (utterance_tx, utterance_rx) = bounded::<Vec<f32>>(8);
        let in_rate = sample_rate;

        thread::spawn(move || {
            let mut vad = vad::Vad::new();
            let mut buf16k: Vec<f32> = Vec::new();
            for chunk in raw_rx.iter() {
                let resampled = resample_linear(&chunk, in_rate, TARGET_RATE);
                buf16k.extend_from_slice(&resampled);
                while buf16k.len() >= FRAME_SIZE {
                    let frame: Vec<f32> = buf16k.drain(..FRAME_SIZE).collect();
                    if let Some(utt) = vad.push_frame(&frame)
                        && utterance_tx.send(utt).is_err()
                    {
                        return;
                    }
                }
            }
        });

        Ok(Self {
            _stream: stream,
            utterance_rx,
            sample_rate,
            channels,
        })
    }

    pub fn utterances(&self) -> Receiver<Vec<f32>> {
        self.utterance_rx.clone()
    }

    pub fn input_sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn input_channels(&self) -> u16 {
        self.channels
    }
}

fn to_mono_f32(data: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return data.to_vec();
    }
    let ch = channels as usize;
    data.chunks(ch)
        .map(|c| c.iter().sum::<f32>() / ch as f32)
        .collect()
}

// Resampling linéaire : qualité acceptable pour STT (Whisper est robuste).
// À remplacer par un resampler de qualité (rubato sinc, polyphase) si on
// commence à voir des artefacts de transcription.
fn resample_linear(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if input.is_empty() {
        return Vec::new();
    }
    if in_rate == out_rate {
        return input.to_vec();
    }
    let ratio = in_rate as f64 / out_rate as f64;
    let out_len = ((input.len() as f64) / ratio).floor() as usize;
    let mut out = Vec::with_capacity(out_len);
    let last = input.len() - 1;
    for i in 0..out_len {
        let src = i as f64 * ratio;
        let idx = src.floor() as usize;
        let frac = (src - idx as f64) as f32;
        let a = input[idx.min(last)];
        let b = if idx < last { input[idx + 1] } else { a };
        out.push(a + (b - a) * frac);
    }
    out
}

pub mod vad {
    use super::{FRAME_MS, FRAME_SIZE};

    const MIN_UTT_FRAMES: usize = 1_000 / FRAME_MS; // ~1s
    const MAX_UTT_FRAMES: usize = 30_000 / FRAME_MS; // ~30s
    const HANGOVER_FRAMES: usize = 700 / FRAME_MS; // ~23 frames (~700ms)
    const TRIGGER_VOICED_FRAMES: usize = 3;
    const FIXED_THRESHOLD: f32 = 0.005;
    const NOISE_THRESHOLD_FACTOR: f32 = 3.0;

    pub struct Vad {
        buffer: Vec<f32>,
        voiced: bool,
        voiced_run: usize,
        silence_run: usize,
        frames_in_utt: usize,
        noise_floor: f32,
    }

    impl Vad {
        pub fn new() -> Self {
            Self {
                buffer: Vec::with_capacity(FRAME_SIZE * MAX_UTT_FRAMES),
                voiced: false,
                voiced_run: 0,
                silence_run: 0,
                frames_in_utt: 0,
                noise_floor: 0.01,
            }
        }

        pub fn push_frame(&mut self, frame: &[f32]) -> Option<Vec<f32>> {
            debug_assert_eq!(frame.len(), FRAME_SIZE);
            let rms = (frame.iter().map(|&x| x * x).sum::<f32>() / frame.len() as f32).sqrt();
            let threshold = (self.noise_floor * NOISE_THRESHOLD_FACTOR).max(FIXED_THRESHOLD);
            let is_voiced = rms > threshold;

            if is_voiced {
                self.voiced_run += 1;
                self.silence_run = 0;
                if !self.voiced && self.voiced_run >= TRIGGER_VOICED_FRAMES {
                    self.voiced = true;
                }
                if self.voiced {
                    self.buffer.extend_from_slice(frame);
                    self.frames_in_utt += 1;
                }
            } else {
                self.noise_floor = self.noise_floor * 0.95 + rms * 0.05;
                self.voiced_run = 0;
                if self.voiced {
                    self.silence_run += 1;
                    self.buffer.extend_from_slice(frame);
                    self.frames_in_utt += 1;
                    if self.silence_run > HANGOVER_FRAMES {
                        return self.finalize();
                    }
                }
            }

            if self.frames_in_utt >= MAX_UTT_FRAMES {
                return self.finalize();
            }
            None
        }

        fn finalize(&mut self) -> Option<Vec<f32>> {
            let result = if self.frames_in_utt >= MIN_UTT_FRAMES {
                Some(std::mem::take(&mut self.buffer))
            } else {
                self.buffer.clear();
                None
            };
            self.buffer = Vec::with_capacity(FRAME_SIZE * MAX_UTT_FRAMES);
            self.voiced = false;
            self.voiced_run = 0;
            self.silence_run = 0;
            self.frames_in_utt = 0;
            result
        }
    }

    impl Default for Vad {
        fn default() -> Self {
            Self::new()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn silence_frame() -> Vec<f32> {
            vec![0.0; FRAME_SIZE]
        }

        fn voiced_frame() -> Vec<f32> {
            (0..FRAME_SIZE)
                .map(|i| 0.3 * ((i as f32) * 0.2).sin())
                .collect()
        }

        #[test]
        fn detects_utterance_boundaries() {
            let mut vad = Vad::new();

            for _ in 0..30 {
                assert!(vad.push_frame(&silence_frame()).is_none());
            }

            let v = voiced_frame();
            let mut got: Option<Vec<f32>> = None;
            for _ in 0..50 {
                if let Some(r) = vad.push_frame(&v) {
                    got = Some(r);
                }
            }
            for _ in 0..40 {
                if let Some(r) = vad.push_frame(&silence_frame()) {
                    got = Some(r);
                    break;
                }
            }

            let utt = got.expect("un énoncé doit être détecté");
            assert!(utt.len() >= FRAME_SIZE * MIN_UTT_FRAMES);
        }

        #[test]
        fn pure_silence_emits_no_utterance() {
            let mut vad = Vad::new();
            for _ in 0..200 {
                assert!(vad.push_frame(&silence_frame()).is_none());
            }
        }

        #[test]
        fn max_duration_flushes_utterance() {
            let mut vad = Vad::new();
            let v = voiced_frame();
            let mut got: Option<Vec<f32>> = None;
            for _ in 0..(MAX_UTT_FRAMES + 10) {
                if let Some(r) = vad.push_frame(&v) {
                    got = Some(r);
                    break;
                }
            }
            assert!(got.is_some(), "flush attendu au plafond de durée");
        }
    }
}
