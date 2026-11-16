use std::f32::consts::TAU;

use katui::game::{Light, Rgb, Rng, Spark, Sparks, V3, scaled, v3};

use crate::art::Style;
use crate::stage::{Placed, Stage};
use crate::table::Side;

const EMBER: Rgb = [1.6, 0.45, 0.08];
const GOLD: Rgb = [2.2, 1.4, 0.45];
const WHITE_HOT: Rgb = [2.0, 1.8, 1.4];
const BLOOD: Rgb = [1.6, 0.12, 0.04];
const SHOCK_SECONDS: f32 = 0.6;
const FLASH_SECONDS: f32 = 0.3;
const GHOST_SECONDS: f32 = 0.8;
const BANNER_SECONDS: f32 = 1.6;
const FIREWORK_SECONDS: f32 = 4.0;

/// The color a set's pieces break into.
fn shards(style: Style) -> Rgb {
    match style {
        Style::Amber => [1.8, 0.8, 0.15],
        Style::Glass => [1.2, 1.4, 1.8],
        Style::Metal => [1.8, 1.4, 0.6],
    }
}

/// A ring that runs out across the board from where a piece landed.
struct Shock {
    at: [f32; 2],
    age: f32,
    reach: f32,
    color: Rgb,
}

struct Flash {
    at: [f32; 2],
    age: f32,
    color: Rgb,
}

/// A captured piece rising and burning away.
struct Ghost {
    side: Side,
    letter: char,
    at: [f32; 2],
    age: f32,
}

/// Big words across the screen.
#[derive(Debug, Clone)]
pub struct Banner {
    pub title: String,
    pub subtitle: String,
    pub color: [f32; 3],
    /// From 0 when it appears to 1 once it settled.
    pub grown: f32,
    /// How visible it is now, 0 to 1.
    pub strength: f32,
}

/// Every effect on screen, each one started by something that happened.
pub struct Effects {
    sparks: Sparks,
    shocks: Vec<Shock>,
    flashes: Vec<Flash>,
    ghosts: Vec<Ghost>,
    banner: Option<(Banner, f32, bool)>,
    checked: Option<([f32; 2], f32)>,
    fireworks: f32,
    rng: Rng,
    clock: f32,
}

impl Default for Effects {
    fn default() -> Self {
        Self {
            sparks: Sparks::default(),
            shocks: Vec::new(),
            flashes: Vec::new(),
            ghosts: Vec::new(),
            banner: None,
            checked: None,
            fireworks: 0.0,
            rng: Rng::new(11),
            clock: 0.0,
        }
    }
}

impl Effects {
    /// Whether anything still moves, so the screen keeps redrawing.
    #[must_use]
    pub fn busy(&self) -> bool {
        !self.sparks.is_empty()
            || !self.shocks.is_empty()
            || !self.flashes.is_empty()
            || !self.ghosts.is_empty()
            || self.checked.is_some()
            || self.fireworks > 0.0
            || self.banner.as_ref().is_some_and(|(banner, age, stays)| {
                !*stays || banner.grown < 1.0 || *age < BANNER_SECONDS
            })
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// A light trail behind a sliding piece.
    pub fn trail(&mut self, at: [f32; 2], lift: f32, style: Style) {
        let drift = self.rng.direction();
        self.sparks.add(Spark {
            at: v3(at[0], at[1], lift + 0.4),
            velocity: v3(drift.x * 0.2, drift.y * 0.2, 0.3),
            color: scaled(shards(style), 0.5),
            age: 0.0,
            life: self.rng.range(0.3, 0.6),
            width: 1.2,
        });
    }

    /// A quiet move: a little dust ring where it landed.
    pub fn landed(&mut self, at: [f32; 2]) {
        self.shocks.push(Shock {
            at,
            age: 0.0,
            reach: 0.6,
            color: [0.5, 0.4, 0.3],
        });
    }

    /// A piece took another: a flash, a shockwave, shards and embers, and the
    /// taken piece rising as a burning ghost.
    pub fn captured(&mut self, at: [f32; 2], victim: (Side, char), style: Style) {
        self.flashes.push(Flash {
            at,
            age: 0.0,
            color: WHITE_HOT,
        });
        self.shocks.push(Shock {
            at,
            age: 0.0,
            reach: 2.2,
            color: shards(style),
        });
        self.ghosts.push(Ghost {
            side: victim.0,
            letter: victim.1,
            at,
            age: 0.0,
        });
        for index in 0..140 {
            let direction = self.rng.direction();
            let shard = index % 2 == 0;
            let speed = if shard {
                self.rng.range(2.0, 5.0)
            } else {
                self.rng.range(0.8, 2.0)
            };
            self.sparks.add(Spark {
                at: v3(at[0], at[1], self.rng.range(0.2, 0.9)),
                velocity: v3(
                    direction.x * speed,
                    direction.y * speed,
                    direction.z.abs() * speed + 1.0,
                ),
                color: if shard {
                    shards(style)
                } else {
                    scaled(EMBER, self.rng.range(1.0, 2.0))
                },
                age: 0.0,
                life: self.rng.range(0.5, 1.4),
                width: if shard { 1.6 } else { 1.1 },
            });
        }
    }

    pub fn check(&mut self, king: [f32; 2]) {
        self.checked = Some((king, 0.0));
        self.flashes.push(Flash {
            at: king,
            age: 0.0,
            color: BLOOD,
        });
        self.show(
            Banner {
                title: "CHECK".to_owned(),
                subtitle: String::new(),
                color: [1.0, 0.3, 0.2],
                grown: 0.0,
                strength: 0.0,
            },
            false,
        );
    }

    pub fn end(&mut self, title: &str, subtitle: &str, decisive: bool) {
        self.checked = None;
        self.fireworks = if decisive { FIREWORK_SECONDS } else { 0.0 };
        self.show(
            Banner {
                title: title.to_owned(),
                subtitle: subtitle.to_owned(),
                color: if decisive {
                    [1.0, 0.78, 0.35]
                } else {
                    [0.8, 0.85, 0.95]
                },
                grown: 0.0,
                strength: 0.0,
            },
            true,
        );
    }

    fn show(&mut self, banner: Banner, stays: bool) {
        self.banner = Some((banner, 0.0, stays));
    }

    #[must_use]
    pub fn banner(&self) -> Option<&Banner> {
        self.banner.as_ref().map(|(banner, _, _)| banner)
    }

    /// The burning ghosts of captured pieces, to draw with the others.
    #[must_use]
    pub fn ghosts(&self) -> Vec<Placed> {
        self.ghosts
            .iter()
            .map(|ghost| {
                let t = ghost.age / GHOST_SECONDS;
                Placed {
                    side: ghost.side,
                    letter: ghost.letter,
                    at: ghost.at,
                    lift: t * t * 1.2,
                    opacity: (1.0 - t).powi(2),
                    tint: [0.9 * (1.0 - t), 0.35 * (1.0 - t), 0.05],
                }
            })
            .collect()
    }

    pub fn step(&mut self, dt: f32) {
        self.clock += dt;
        self.sparks.step(dt, v3(0.0, 0.0, -2.5));
        for shock in &mut self.shocks {
            shock.age += dt;
        }
        self.shocks.retain(|shock| shock.age < SHOCK_SECONDS);
        for flash in &mut self.flashes {
            flash.age += dt;
        }
        self.flashes.retain(|flash| flash.age < FLASH_SECONDS);
        for ghost in &mut self.ghosts {
            ghost.age += dt;
        }
        self.ghosts.retain(|ghost| ghost.age < GHOST_SECONDS);
        if let Some((_, age)) = self.checked.as_mut() {
            *age += dt;
        }
        self.checked.take_if(|(_, age)| *age > 1.5);
        self.step_banner(dt);
        if self.fireworks > 0.0 {
            self.fireworks -= dt;
            if self.rng.range(0.0, 1.0) < dt * 4.0 {
                self.firework();
            }
        }
    }

    fn step_banner(&mut self, dt: f32) {
        if let Some((banner, age, stays)) = self.banner.as_mut() {
            *age += dt;
            banner.grown = (*age / 0.35).min(1.0);
            let fade = if *stays {
                1.0
            } else {
                (1.0 - (*age - BANNER_SECONDS * 0.6) / (BANNER_SECONDS * 0.4)).clamp(0.0, 1.0)
            };
            banner.strength = banner.grown.min(fade);
        }
        self.banner
            .take_if(|(_, age, stays)| !*stays && *age > BANNER_SECONDS);
    }

    fn firework(&mut self) {
        let at = v3(
            self.rng.range(0.5, 7.5),
            self.rng.range(0.5, 7.5),
            self.rng.range(1.5, 3.0),
        );
        let colors = [GOLD, WHITE_HOT, EMBER, [1.4, 0.6, 1.6]];
        let color = colors
            .get((self.rng.next_u32() % 4) as usize)
            .copied()
            .unwrap_or(GOLD);
        self.flashes.push(Flash {
            at: [at.x, at.y],
            age: 0.0,
            color: scaled(color, 0.6),
        });
        for _ in 0..120 {
            let direction = self.rng.direction();
            let speed = self.rng.range(2.0, 3.5);
            self.sparks.add(Spark {
                at,
                velocity: direction * speed,
                color: scaled(color, 1.6),
                age: 0.0,
                life: self.rng.range(0.9, 1.8),
                width: 2.2,
            });
        }
    }

    /// Clear the end banner, for a new game.
    pub fn new_round(&mut self) {
        self.banner = None;
        self.fireworks = 0.0;
        self.checked = None;
    }

    pub fn draw(&self, light: &mut Light, stage: &Stage) {
        for shock in &self.shocks {
            let t = shock.age / SHOCK_SECONDS;
            let radius = shock.reach * (1.0 - (1.0 - t).powi(3));
            let fade = (1.0 - t).powi(2);
            ring(
                light,
                stage,
                shock.at,
                radius,
                scaled(shock.color, 1.5 * fade),
                2.5 * fade + 0.5,
            );
            ring(
                light,
                stage,
                shock.at,
                radius * 0.75,
                scaled(shock.color, 0.6 * fade),
                1.5,
            );
        }
        for flash in &self.flashes {
            let t = flash.age / FLASH_SECONDS;
            if let Some(spot) = stage.spot(flash.at, 0.4) {
                light.glow(
                    spot,
                    stage.unit(flash.at) * (0.8 + t),
                    scaled(flash.color, 3.0 * (1.0 - t).powi(2)),
                );
            }
        }
        if let Some((king, age)) = self.checked {
            let pulse = 0.5 + 0.5 * (age * 9.0).sin();
            ring(
                light,
                stage,
                king,
                0.45 + 0.1 * pulse,
                scaled(BLOOD, 1.2 + pulse),
                3.0,
            );
        }
        // a spark that fell through the board would glow on the floor in front of it
        self.sparks.draw(light, |at: V3| {
            (at.z > -0.05)
                .then(|| stage.spot([at.x, at.y], at.z))
                .flatten()
        });
    }
}

/// A circle lying on the board, drawn in perspective.
fn ring(light: &mut Light, stage: &Stage, at: [f32; 2], radius: f32, color: Rgb, width: f32) {
    let points: Vec<_> = (0..=48)
        .filter_map(|step| {
            let angle = TAU * step as f32 / 48.0;
            stage.spot(
                [at[0] + angle.cos() * radius, at[1] + angle.sin() * radius],
                0.02,
            )
        })
        .collect();
    for pair in points.windows(2) {
        if let [from, to] = pair {
            light.line(*from, *to, width, color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_capture_burns_out_and_the_end_banner_stays() {
        let mut effects = Effects::default();
        effects.captured([3.5, 3.5], (Side::Black, 'q'), Style::Glass);
        assert!(effects.busy());
        assert_eq!(effects.ghosts().len(), 1);
        for _ in 0..180 {
            effects.step(1.0 / 60.0);
        }
        assert!(!effects.busy());
        effects.end("CHECKMATE", "WHITE WINS", true);
        for _ in 0..600 {
            effects.step(1.0 / 60.0);
        }
        assert_eq!(effects.banner().map(|banner| banner.strength), Some(1.0));
        assert!(!effects.busy());
    }

    #[test]
    fn the_check_banner_fades_away() {
        let mut effects = Effects::default();
        effects.check([4.5, 0.5]);
        for _ in 0..200 {
            effects.step(1.0 / 60.0);
        }
        assert!(effects.banner().is_none());
    }
}
