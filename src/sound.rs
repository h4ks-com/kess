use std::f32::consts::TAU;

const RATE: u32 = 44_100;
#[cfg(feature = "sound")]
const BUFFER_FRAMES: u32 = 512;

/// What the pieces are made of, which decides how they ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Material {
    /// Warm and short, with a woody knock under it.
    Resin,
    /// Bright, with the uneven overtones of a struck glass rod.
    Glass,
    /// Long and shimmering, with the overtones of a small bell.
    Metal,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Cue {
    /// A piece leaves the board; `weight` runs from 0 (pawn) to 1 (king).
    Lift {
        material: Material,
        weight: f32,
    },
    /// A piece lands; `pan` runs from -1 (left) to 1 (right).
    Place {
        material: Material,
        weight: f32,
        pan: f32,
    },
    /// A piece lands on another and breaks it.
    Capture {
        material: Material,
        pan: f32,
    },
    Check,
    Mate,
    Notice,
}

/// Overtone ratios of a struck free bar, for glass.
const BAR: [f32; 4] = [1.0, 2.756, 5.404, 8.933];
/// Overtone ratios of a small bell, for metal.
const BELL: [f32; 6] = [0.5, 1.0, 1.183, 1.506, 2.0, 2.514];
/// A wooden block's few strong modes, for resin.
const BLOCK: [f32; 3] = [1.0, 1.58, 2.31];

/// Small noise source so every strike comes out a little different.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as f32 / u32::MAX as f32 * 2.0 - 1.0
    }
}

/// A stereo buffer that strikes and bursts are mixed into.
struct Mix {
    left: Vec<f32>,
    right: Vec<f32>,
    noise: Noise,
}

impl Mix {
    fn new(seconds: f32, seed: u32) -> Self {
        let frames = (seconds * RATE as f32) as usize;
        Self {
            left: vec![0.0; frames],
            right: vec![0.0; frames],
            noise: Noise(seed | 1),
        }
    }

    fn add(&mut self, at: usize, sample: f32, pan: f32) {
        let (left, right) = (f32::midpoint(1.0, -pan), f32::midpoint(1.0, pan));
        if let (Some(l), Some(r)) = (self.left.get_mut(at), self.right.get_mut(at)) {
            *l += sample * left.sqrt();
            *r += sample * right.sqrt();
        }
    }

    /// Ring `ratios` over `hertz`, each overtone dying faster than the one below.
    fn strike(&mut self, start: f32, hertz: f32, ratios: &[f32], decay: f32, level: f32, pan: f32) {
        let detune = 1.0 + self.noise.next() * 0.012;
        let offset = (start * RATE as f32) as usize;
        let frames = ((decay * 6.0) * RATE as f32) as usize;
        for index in 0..frames {
            let time = index as f32 / RATE as f32;
            let sample: f32 = ratios
                .iter()
                .enumerate()
                .map(|(order, ratio)| {
                    let fade = (-time * (1.0 + order as f32 * 0.9) / decay).exp();
                    (TAU * hertz * detune * ratio * time).sin() * fade / (1.0 + order as f32)
                })
                .sum();
            self.add(offset + index, sample * level, pan);
        }
    }

    /// A burst of noise through a one-pole filter: `bright` near 1 hisses, near 0 thumps.
    fn burst(&mut self, start: f32, length: f32, bright: f32, level: f32, pan: f32) {
        let offset = (start * RATE as f32) as usize;
        let frames = (length * RATE as f32) as usize;
        let mut smooth = 0.0;
        for index in 0..frames {
            let fade = 1.0 - index as f32 / frames as f32;
            smooth += (self.noise.next() - smooth) * bright;
            self.add(offset + index, smooth * fade * fade * level, pan);
        }
    }

    fn finish(self) -> Vec<f32> {
        let peak = self
            .left
            .iter()
            .chain(&self.right)
            .fold(0.0_f32, |most, sample| most.max(sample.abs()))
            .max(1.0);
        let frames = self.left.len();
        let ramp = 256.min(frames);
        self.left
            .iter()
            .zip(&self.right)
            .enumerate()
            .flat_map(|(index, (left, right))| {
                let tail = (frames - index).min(ramp) as f32 / ramp as f32;
                [left / peak * tail, right / peak * tail]
            })
            .collect()
    }
}

fn voice(material: Material) -> (&'static [f32], f32, f32) {
    match material {
        Material::Resin => (&BLOCK, 330.0, 0.05),
        Material::Glass => (&BAR, 1250.0, 0.09),
        Material::Metal => (&BELL, 880.0, 0.22),
    }
}

/// The interleaved stereo samples of `cue`, built fresh each time from `seed`.
#[must_use]
pub fn samples(cue: Cue, seed: u32) -> Vec<f32> {
    match cue {
        Cue::Lift { material, weight } => {
            let (ratios, hertz, decay) = voice(material);
            let mut mix = Mix::new(0.2, seed);
            mix.strike(
                0.0,
                hertz * 2.0 / (1.0 + weight),
                ratios,
                decay * 0.3,
                0.12,
                0.0,
            );
            mix.burst(0.0, 0.02, 0.6, 0.08, 0.0);
            mix.finish()
        }
        Cue::Place {
            material,
            weight,
            pan,
        } => {
            let (ratios, hertz, decay) = voice(material);
            let mut mix = Mix::new(0.9, seed);
            mix.burst(0.0, 0.012, 0.9, 0.5, pan);
            mix.burst(0.0, 0.06, 0.04, 0.9, pan);
            mix.strike(0.002, hertz / (1.0 + weight), ratios, decay, 0.5, pan);
            mix.strike(0.002, 120.0 / (1.0 + weight * 0.5), &BLOCK, 0.04, 0.4, pan);
            mix.finish()
        }
        Cue::Capture { material, pan } => capture(material, pan, seed),
        Cue::Check => {
            let mut mix = Mix::new(2.2, seed);
            mix.strike(0.0, 196.0, &BELL, 0.5, 0.6, -0.2);
            mix.strike(0.18, 207.7, &BELL, 0.45, 0.35, 0.2);
            mix.burst(0.0, 0.5, 0.01, 0.6, 0.0);
            mix.finish()
        }
        Cue::Mate => mate(seed),
        Cue::Notice => {
            let mut mix = Mix::new(1.2, seed);
            mix.strike(0.0, 1568.0, &BAR, 0.15, 0.3, -0.3);
            mix.strike(0.09, 2093.0, &BAR, 0.2, 0.3, 0.3);
            mix.finish()
        }
    }
}

/// The struck piece shatters: a heavy hit, then shards scattering and a crackle of embers.
fn capture(material: Material, pan: f32, seed: u32) -> Vec<f32> {
    let (ratios, hertz, decay) = voice(material);
    let mut mix = Mix::new(1.4, seed);
    mix.burst(0.0, 0.18, 0.02, 1.0, pan);
    mix.burst(0.0, 0.03, 0.95, 0.7, pan);
    mix.strike(0.0, hertz * 0.7, ratios, decay * 1.5, 0.6, pan);
    for shard in 0..24 {
        let when = 0.01 + (mix.noise.next() * 0.5 + 0.5).powi(2) * 0.45;
        let pitch = hertz * (2.0 + (mix.noise.next() * 0.5 + 0.5) * 4.0);
        let spread = (pan + mix.noise.next() * 0.6).clamp(-1.0, 1.0);
        let level = 0.25 / (1.0 + shard as f32 * 0.1);
        mix.strike(when, pitch, ratios, decay * 0.25, level, spread);
    }
    for _ in 0..40 {
        let when = 0.05 + (mix.noise.next() * 0.5 + 0.5) * 1.1;
        let spread = (pan + mix.noise.next() * 0.8).clamp(-1.0, 1.0);
        mix.burst(when, 0.004, 0.8, 0.12, spread);
    }
    mix.finish()
}

/// A low gong under a falling cascade of glassy bells.
fn mate(seed: u32) -> Vec<f32> {
    let mut mix = Mix::new(4.5, seed);
    mix.burst(0.0, 0.6, 0.01, 1.0, 0.0);
    mix.strike(0.0, 65.4, &BELL, 1.2, 1.0, -0.15);
    mix.strike(0.0, 98.0, &BELL, 1.0, 0.6, 0.15);
    let scale = [1046.5, 1174.7, 1318.5, 1568.0, 1760.0, 2093.0];
    for step in 0..18 {
        let note = scale
            .get((step * 5) % scale.len())
            .copied()
            .unwrap_or(1046.5);
        let pan = ((step as f32) * 0.7).sin();
        mix.strike(
            0.35 + step as f32 * 0.09,
            note * 2.0 / (1.0 + step as f32 * 0.06),
            &BAR,
            0.3,
            0.22,
            pan,
        );
    }
    mix.finish()
}

#[cfg(feature = "sound")]
pub struct Speaker {
    sink: Option<rodio::MixerDeviceSink>,
    seed: std::cell::Cell<u32>,
    pub muted: bool,
}

#[cfg(feature = "sound")]
impl Speaker {
    /// Open the default output, or stay silent when there is none. Call it
    /// before the game takes over the terminal.
    #[must_use]
    pub fn open(muted: bool) -> Self {
        // the default buffer of 2048 frames is past what some macOS outputs take, and
        // the default error callback prints over the game screen
        let sink = rodio::DeviceSinkBuilder::from_default_device()
            .and_then(|builder| {
                builder
                    .with_buffer_size(rodio::cpal::BufferSize::Fixed(BUFFER_FRAMES))
                    .with_error_callback(|_| {})
                    .open_stream()
            })
            .ok()
            .map(|mut sink| {
                sink.log_on_drop(false);
                sink
            });
        Self {
            sink,
            seed: std::cell::Cell::new(0x9e37_79b9),
            muted,
        }
    }

    /// Synthesize `cue` on a worker thread so a long cue never holds up a frame.
    pub fn play(&self, cue: Cue) {
        let (Some(sink), false) = (&self.sink, self.muted) else {
            return;
        };
        let seed = self
            .seed
            .get()
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.seed.set(seed);
        let mixer = sink.mixer().clone();
        std::thread::spawn(move || {
            if let (Some(channels), Some(rate)) =
                (std::num::NonZero::new(2), std::num::NonZero::new(RATE))
            {
                mixer.add(rodio::buffer::SamplesBuffer::new(
                    channels,
                    rate,
                    samples(cue, seed),
                ));
            }
        });
    }
}

#[cfg(not(feature = "sound"))]
pub struct Speaker {
    pub muted: bool,
}

#[cfg(not(feature = "sound"))]
impl Speaker {
    #[must_use]
    pub fn open(muted: bool) -> Self {
        Self { muted }
    }

    pub fn play(&self, _cue: Cue) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Cue; 6] = [
        Cue::Lift {
            material: Material::Glass,
            weight: 0.5,
        },
        Cue::Place {
            material: Material::Resin,
            weight: 0.0,
            pan: -0.5,
        },
        Cue::Capture {
            material: Material::Metal,
            pan: 0.3,
        },
        Cue::Check,
        Cue::Mate,
        Cue::Notice,
    ];

    /// Plays every cue in every material on the speakers, for a person to listen to.
    #[cfg(feature = "sound")]
    #[test]
    #[ignore = "plays sound"]
    fn every_cue_plays_on_the_speakers() {
        let speaker = Speaker::open(false);
        assert!(speaker.sink.is_some(), "no audio output opened");
        for material in [Material::Resin, Material::Glass, Material::Metal] {
            for weight in [0.0, 1.0] {
                speaker.play(Cue::Place {
                    material,
                    weight,
                    pan: weight * 2.0 - 1.0,
                });
                std::thread::sleep(std::time::Duration::from_millis(700));
            }
            speaker.play(Cue::Capture { material, pan: 0.0 });
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
        for cue in [Cue::Check, Cue::Notice, Cue::Mate] {
            speaker.play(cue);
            std::thread::sleep(std::time::Duration::from_millis(2500));
        }
    }

    #[test]
    fn every_cue_is_stereo_bounded_and_ends_silent() {
        for cue in ALL {
            let wave = samples(cue, 7);
            assert!(!wave.is_empty() && wave.len().is_multiple_of(2));
            assert!(wave.iter().all(|sample| sample.abs() <= 1.0));
            let tail = wave
                .iter()
                .rev()
                .take(16)
                .fold(0.0_f32, |most, s| most.max(s.abs()));
            assert!(tail < 0.05, "{cue:?} ends with a click");
        }
    }

    #[test]
    fn two_strikes_never_sound_the_same() {
        let place = Cue::Place {
            material: Material::Glass,
            weight: 0.2,
            pan: 0.0,
        };
        assert_ne!(samples(place, 1), samples(place, 2));
    }
}
