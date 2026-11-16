// HUD coordinates stay far below isize::MAX, so we cast pixel positions freely
#![allow(clippy::cast_possible_wrap)]

use std::cell::RefCell;
use std::collections::HashMap;

use fontdue::{Font, FontSettings};

const TITLE: &[u8] = include_bytes!("../assets/fonts/Cinzel.ttf");
const MONO: &[u8] = include_bytes!("../assets/fonts/JetBrainsMono.ttf");

#[derive(Debug, thiserror::Error)]
#[error("a bundled font is broken: {0}")]
pub struct FontError(&'static str);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    /// Engraved capitals, for names and banners.
    Title,
    /// Even digits, for clocks, codes and key hints.
    Mono,
}

/// How a line of text sits on its anchor point.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

struct Glyph {
    left: i32,
    top: i32,
    width: usize,
    advance: f32,
    coverage: Vec<u8>,
}

/// Both bundled fonts, with every glyph they rasterized kept for reuse.
pub struct Fonts {
    title: Font,
    mono: Font,
    glyphs: RefCell<HashMap<(Face, char, u32, u32), Glyph>>,
}

impl Fonts {
    pub fn load() -> Result<Self, FontError> {
        Ok(Self {
            title: Font::from_bytes(TITLE, FontSettings::default()).map_err(FontError)?,
            mono: Font::from_bytes(MONO, FontSettings::default()).map_err(FontError)?,
            glyphs: RefCell::new(HashMap::new()),
        })
    }

    fn font(&self, face: Face) -> &Font {
        match face {
            Face::Title => &self.title,
            Face::Mono => &self.mono,
        }
    }

    /// Rasterize `letter` `size` pixels tall on pixels `aspect` as wide as tall.
    fn with_glyph<T>(
        &self,
        face: Face,
        letter: char,
        size: f32,
        aspect: f32,
        use_it: impl FnOnce(&Glyph) -> T,
    ) -> T {
        let key = (face, letter, size.to_bits(), aspect.to_bits());
        let mut glyphs = self.glyphs.borrow_mut();
        let glyph = glyphs.entry(key).or_insert_with(|| {
            let (metrics, coverage) = self.font(face).rasterize(letter, size);
            squeeze(&metrics, &coverage, aspect)
        });
        use_it(glyph)
    }

    #[must_use]
    pub fn width(&self, face: Face, text: &str, size: f32, aspect: f32) -> f32 {
        text.chars()
            .map(|letter| self.with_glyph(face, letter, size, aspect, |glyph| glyph.advance))
            .sum()
    }

    /// The distance from the top of a line to its baseline.
    fn ascent(&self, face: Face, size: f32) -> f32 {
        self.font(face)
            .horizontal_line_metrics(size)
            .map_or(size * 0.8, |metrics| metrics.ascent)
    }
}

/// Glyph coverage resampled across so it keeps its shape on pixels that are
/// `aspect` as wide as they are tall.
fn squeeze(metrics: &fontdue::Metrics, coverage: &[u8], aspect: f32) -> Glyph {
    let width =
        ((metrics.width as f32 / aspect).round() as usize).max(usize::from(metrics.width > 0));
    let mut out = vec![0_u8; width * metrics.height];
    for (row, line) in coverage.chunks(metrics.width.max(1)).enumerate() {
        for column in 0..width {
            let source = ((column as f32 + 0.5) * aspect - 0.5).max(0.0);
            let (low, high) = (source.floor() as usize, source.ceil() as usize);
            let blend = source - source.floor();
            let pick = |index: usize| f32::from(line.get(index).copied().unwrap_or(0));
            let value = pick(low) * (1.0 - blend) + pick(high) * blend;
            if let Some(slot) = out.get_mut(row * width + column) {
                *slot = value.round() as u8;
            }
        }
    }
    Glyph {
        left: (metrics.xmin as f32 / aspect).round() as i32,
        top: -(metrics.ymin + i32::try_from(metrics.height).unwrap_or(0)),
        width,
        advance: metrics.advance_width / aspect,
        coverage: out,
    }
}

/// An RGBA frame we draw the HUD onto after the game's light is tone-mapped.
pub struct Canvas<'a> {
    pub pixels: &'a mut [u8],
    pub width: usize,
    pub height: usize,
    /// One pixel's width over its height.
    pub aspect: f32,
    pub fonts: &'a Fonts,
}

/// A color with its opacity, both 0 to 1.
pub type Paint = ([f32; 3], f32);

impl Canvas<'_> {
    fn blend(&mut self, x: isize, y: isize, (color, alpha): Paint) {
        if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height || alpha <= 0.0 {
            return;
        }
        let index = (y as usize * self.width + x as usize) * 4;
        if let Some(pixel) = self.pixels.get_mut(index..index + 3) {
            for (value, tint) in pixel.iter_mut().zip(color) {
                let mixed = f32::from(*value) * (1.0 - alpha) + tint * 255.0 * alpha;
                *value = mixed.clamp(0.0, 255.0) as u8;
            }
        }
    }

    /// A rounded box `width` by `height` pixels, its corners `radius` tall.
    pub fn panel(
        &mut self,
        at: (f32, f32),
        size: (f32, f32),
        radius: f32,
        fill: Paint,
        border: Option<Paint>,
    ) {
        let (left, top) = (at.0.round() as isize, at.1.round() as isize);
        let (width, height) = (size.0.round() as isize, size.1.round() as isize);
        for y in 0..height {
            for x in 0..width {
                let distance = corner_distance(
                    (x as f32, y as f32),
                    (width as f32, height as f32),
                    radius,
                    self.aspect,
                );
                let edge = (radius - distance).clamp(0.0, 1.0);
                if edge <= 0.0 {
                    continue;
                }
                let rim =
                    border.filter(|_| distance > radius - 1.5 || on_edge(x, y, width, height));
                let (color, alpha) = rim.unwrap_or(fill);
                self.blend(left + x, top + y, (color, alpha * edge));
            }
        }
    }

    /// Write `text` with its top at `at.1`, `size` pixels tall, and return its width.
    pub fn text(
        &mut self,
        face: Face,
        text: &str,
        at: (f32, f32),
        size: f32,
        align: Align,
        paint: Paint,
    ) -> f32 {
        let fonts = self.fonts;
        let width = fonts.width(face, text, size, self.aspect);
        let mut pen = match align {
            Align::Left => at.0,
            Align::Center => at.0 - width / 2.0,
            Align::Right => at.0 - width,
        };
        let baseline = at.1 + fonts.ascent(face, size);
        for letter in text.chars() {
            let advance = fonts.with_glyph(face, letter, size, self.aspect, |glyph| {
                let left = pen.round() as isize + glyph.left as isize;
                let top = baseline.round() as isize + glyph.top as isize;
                for (index, cover) in glyph.coverage.iter().enumerate() {
                    if *cover > 0 {
                        let (column, row) = (
                            (index % glyph.width) as isize,
                            (index / glyph.width) as isize,
                        );
                        self.blend(
                            left + column,
                            top + row,
                            (paint.0, paint.1 * f32::from(*cover) / 255.0),
                        );
                    }
                }
                glyph.advance
            });
            pen += advance;
        }
        width
    }

    /// `text` centered on `at.0` with a soft colored halo behind it, for banners.
    pub fn glowing(
        &mut self,
        face: Face,
        text: &str,
        at: (f32, f32),
        size: f32,
        paint: Paint,
        halo: Paint,
    ) {
        let reach = (size / 6.0).max(2.0);
        let width = self.fonts.width(face, text, size, self.aspect);
        let pad = (reach * 2.5) as usize;
        let across = width as usize + pad * 2;
        let down = (size * 1.4) as usize + pad * 2;
        let (left, top) = (at.0 - width / 2.0 - pad as f32, at.1 - pad as f32);
        let mut mask = vec![0_u8; across * down * 4];
        let mut layer = Canvas {
            pixels: &mut mask,
            width: across,
            height: down,
            aspect: self.aspect,
            fonts: self.fonts,
        };
        layer.text(
            face,
            text,
            (pad as f32, pad as f32),
            size,
            Align::Left,
            ([1.0; 3], 1.0),
        );
        let mut glow: Vec<f32> = mask
            .as_chunks::<4>()
            .0
            .iter()
            .map(|[red, ..]| f32::from(*red) / 255.0)
            .collect();
        let radius_x = ((reach / self.aspect) as usize).max(1);
        for _ in 0..3 {
            blur(&mut glow, (across, down), radius_x, reach as usize);
        }
        for (index, amount) in glow.iter().enumerate() {
            let (x, y) = (
                (index % across) as f32 + left,
                (index / across) as f32 + top,
            );
            let strength = (amount * 3.5).min(1.0) * halo.1;
            self.blend(x.round() as isize, y.round() as isize, (halo.0, strength));
        }
        self.text(face, text, at, size, Align::Center, paint);
    }

    /// Darken everything, for a banner to stand out.
    pub fn dim(&mut self, amount: f32) {
        for [red, green, blue, _] in self.pixels.as_chunks_mut::<4>().0 {
            for value in [red, green, blue] {
                *value = (f32::from(*value) * (1.0 - amount)) as u8;
            }
        }
    }
}

/// A box blur across then down, `across` and `down` pixels each way.
fn blur(values: &mut [f32], (width, height): (usize, usize), across: usize, down: usize) {
    let mut scratch = values.to_vec();
    for (row, out) in values
        .chunks_exact(width)
        .zip(scratch.chunks_exact_mut(width))
    {
        running_mean(row.iter().copied(), out.iter_mut(), across, width);
    }
    for column in 0..width {
        let source: Vec<f32> = (0..height)
            .map(|y| scratch.get(y * width + column).copied().unwrap_or(0.0))
            .collect();
        let target = values.iter_mut().skip(column).step_by(width);
        running_mean(source.into_iter(), target, down, height);
    }
}

fn running_mean<'a>(
    source: impl Iterator<Item = f32>,
    target: impl Iterator<Item = &'a mut f32>,
    reach: usize,
    length: usize,
) {
    let mut sums = vec![0.0; length + 1];
    for (index, value) in source.enumerate() {
        if let (Some(sum), Some(next)) = (sums.get(index).copied(), sums.get_mut(index + 1)) {
            *next = sum + value;
        }
    }
    for (index, slot) in target.enumerate() {
        let low = index.saturating_sub(reach);
        let high = (index + reach + 1).min(length);
        let total = sums.get(high).copied().unwrap_or(0.0) - sums.get(low).copied().unwrap_or(0.0);
        *slot = total / (high - low).max(1) as f32;
    }
}

/// How far `spot` sits beyond the round of its nearest corner, in a box of `size`.
fn corner_distance(spot: (f32, f32), size: (f32, f32), radius: f32, aspect: f32) -> f32 {
    let across = radius / aspect;
    let beyond = |value: f32, reach: f32, length: f32| {
        if value < reach {
            reach - value
        } else if value > length - reach {
            value - (length - reach)
        } else {
            0.0
        }
    };
    (beyond(spot.0, across, size.0) * aspect).hypot(beyond(spot.1, radius, size.1))
}

fn on_edge(x: isize, y: isize, width: isize, height: isize) -> bool {
    x == 0 || y == 0 || x == width - 1 || y == height - 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_draws_inside_its_measured_width() {
        let fonts = Fonts::load().unwrap();
        let mut pixels = vec![0_u8; 200 * 60 * 4];
        let mut canvas = Canvas {
            pixels: &mut pixels,
            width: 200,
            height: 60,
            aspect: 1.0,
            fonts: &fonts,
        };
        let width = canvas.text(
            Face::Mono,
            "5:00",
            (10.0, 10.0),
            24.0,
            Align::Left,
            ([1.0; 3], 1.0),
        );
        assert!(width > 40.0 && width < 80.0);
        let lit: Vec<usize> = pixels
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, [red, ..])| *red > 128)
            .map(|(index, _)| index % 200)
            .collect();
        assert!(!lit.is_empty());
        assert!(
            lit.iter()
                .all(|x| (8..=10 + width as usize + 2).contains(x))
        );
    }

    #[test]
    fn wide_pixels_squeeze_text_across() {
        let fonts = Fonts::load().unwrap();
        let square = fonts.width(Face::Title, "CHECK", 20.0, 1.0);
        let wide = fonts.width(Face::Title, "CHECK", 20.0, 2.0);
        assert!((wide * 2.0 - square).abs() < 2.0);
    }
}
