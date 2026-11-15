//! Bench audio (`WB-13`): buzzer / speaker PCM rendered by the simulator and short UI sounds
//! (relay clicks, pops, beeps) played through the default output device with `rodio`.
//!
//! The output stream is opened lazily on a dedicated thread (macOS audio streams are not
//! `Send`), so benches without anything audible never touch the audio device.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

/// What the audio thread plays.
pub enum AudioMsg {
    /// Mono PCM in −1…1.
    Pcm {
        sample_rate: u32,
        samples: Vec<f32>,
    },
    /// Relay armature click.
    Click,
    /// A part popping (magic smoke).
    Pop,
    /// A beep (continuity tester, active buzzer burst).
    Beep {
        freq_hz: f64,
        secs: f64,
    },
    /// Drop everything queued (bench paused / reset).
    Flush,
    Quit,
}

/// Handle to the audio thread.
pub struct AudioOut {
    tx: Sender<AudioMsg>,
    thread: Option<JoinHandle<()>>,
}

impl AudioOut {
    pub fn new() -> AudioOut {
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("bench-audio".into())
            .spawn(move || run(rx))
            .ok();
        AudioOut { tx, thread }
    }

    pub fn send(&self, m: AudioMsg) {
        let _ = self.tx.send(m);
    }
}

impl Default for AudioOut {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        let _ = self.tx.send(AudioMsg::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

const RATE: u32 = 44_100;

/// A short click: a decaying burst of a few hundred µs (relay armature hitting the yoke).
pub fn click_samples() -> Vec<f32> {
    let n = (RATE as f32 * 0.012) as usize;
    let mut seed = 0x1234_5678u32;
    (0..n)
        .map(|i| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let env = (-(i as f32) / (RATE as f32 * 0.0018)).exp();
            0.55 * noise * env
        })
        .collect()
}

/// A dull pop (capacitor / part blowing).
pub fn pop_samples() -> Vec<f32> {
    let n = (RATE as f32 * 0.09) as usize;
    let mut seed = 0x9e37_79b9u32;
    (0..n)
        .map(|i| {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed as f32 / u32::MAX as f32) * 2.0 - 1.0;
            let t = i as f32 / RATE as f32;
            let env = (-t / 0.018).exp();
            0.7 * env * (0.6 * noise + 0.4 * (t * 2.0 * std::f32::consts::PI * 90.0).sin())
        })
        .collect()
}

/// A sine beep with soft edges.
pub fn beep_samples(freq_hz: f64, secs: f64) -> Vec<f32> {
    let n = (RATE as f64 * secs.clamp(0.0, 2.0)) as usize;
    let edge = (RATE as f64 * 0.004) as usize;
    (0..n)
        .map(|i| {
            let t = i as f64 / RATE as f64;
            let a = if i < edge {
                i as f64 / edge as f64
            } else if i + edge > n {
                (n - i) as f64 / edge as f64
            } else {
                1.0
            };
            (0.3 * a * (2.0 * std::f64::consts::PI * freq_hz * t).sin()) as f32
        })
        .collect()
}

fn run(rx: Receiver<AudioMsg>) {
    use rodio::{OutputStream, OutputStreamBuilder, Sink, buffer::SamplesBuffer};
    let mut stream: Option<OutputStream> = None;
    let mut sink: Option<Sink> = None;
    let mut failed = false;
    while let Ok(m) = rx.recv() {
        if matches!(m, AudioMsg::Quit) {
            break;
        }
        if let AudioMsg::Flush = m {
            if let Some(s) = &sink {
                s.clear();
                s.play();
            }
            continue;
        }
        if sink.is_none() && !failed {
            match OutputStreamBuilder::open_default_stream() {
                Ok(mut s) => {
                    s.log_on_drop(false);
                    let k = Sink::connect_new(s.mixer());
                    sink = Some(k);
                    stream = Some(s);
                }
                Err(e) => {
                    log::info!("no audio output: {e}");
                    failed = true;
                }
            }
        }
        let Some(s) = &sink else { continue };
        let (rate, samples) = match m {
            AudioMsg::Pcm {
                sample_rate,
                samples,
            } => (sample_rate.max(1), samples),
            AudioMsg::Click => (RATE, click_samples()),
            AudioMsg::Pop => (RATE, pop_samples()),
            AudioMsg::Beep { freq_hz, secs } => (RATE, beep_samples(freq_hz, secs)),
            AudioMsg::Flush | AudioMsg::Quit => continue,
        };
        // keep latency bounded: a sink that fell far behind is reset
        if s.len() > 24 {
            s.clear();
            s.play();
        }
        s.append(SamplesBuffer::new(1, rate, samples));
    }
    drop(sink);
    drop(stream);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthesised_sounds_are_bounded() {
        for s in [click_samples(), pop_samples(), beep_samples(2000.0, 0.1)] {
            assert!(!s.is_empty());
            assert!(s.iter().all(|x| x.abs() <= 1.0));
        }
        assert_eq!(beep_samples(440.0, 0.5).len(), 22_050);
    }
}
