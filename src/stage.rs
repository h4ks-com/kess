// screens stay far below isize::MAX pixels, so we cast pixel coordinates freely
#![allow(clippy::cast_possible_wrap)]

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use image::imageops::{FilterType, resize};
use katui::game::{Light, P2, Rgb, V3, v3};

use crate::art::{Art, Cutout, Mipmaps, PIECES, Style};
use crate::camera::{Flat, Orbit, View};
use crate::table::Side;

/// How thick the board stands above the floor, in board units.
const THICKNESS: f32 = 0.3;
/// Sprite sizes are cached in steps of this ratio, so a moving camera reuses them.
const SIZE_STEP: f32 = 1.03;
const FLOOR: [f32; 3] = [0.018, 0.014, 0.012];
/// How much of a square the largest piece of a flat board fills.
const FLAT_FILL: f32 = 0.88;

/// How the board's lacquer catches the light.
struct Finish {
    gloss: f32,
    sheen: [f32; 3],
    reflection: f32,
}

fn finish(style: Style) -> Finish {
    match style {
        Style::Amber => Finish {
            gloss: 0.5,
            sheen: [1.0, 0.82, 0.6],
            reflection: 0.24,
        },
        Style::Glass => Finish {
            gloss: 0.65,
            sheen: [0.9, 0.95, 1.0],
            reflection: 0.32,
        },
        Style::Metal => Finish {
            gloss: 0.45,
            sheen: [1.0, 0.95, 0.85],
            reflection: 0.2,
        },
    }
}

/// A piece where the screen should show it this frame.
#[derive(Debug, Clone, Copy)]
pub struct Placed {
    pub side: Side,
    pub letter: char,
    /// The board point under its center, like `(4.5, 1.5)` for e2.
    pub at: [f32; 2],
    pub lift: f32,
    pub opacity: f32,
    /// Light added on top, for a piece that glows.
    pub tint: [f32; 3],
}

/// A colored square under the pieces.
#[derive(Debug, Clone, Copy)]
pub struct Mark {
    pub file: usize,
    pub rank: usize,
    pub color: [f32; 3],
    pub strength: f32,
}

/// A sprite scaled for the screen, its colors already multiplied by alpha.
#[derive(Debug, Clone, Default)]
struct Sprite {
    width: usize,
    height: usize,
    foot: [f32; 2],
    /// The row the visible piece ends on.
    base: f32,
    pixels: Vec<[f32; 4]>,
}

impl Sprite {
    fn scaled(cutout: &Cutout, across: f32, down: f32) -> Self {
        let width = ((cutout.image.width() as f32 * across).round() as u32).max(1);
        let height = ((cutout.image.height() as f32 * down).round() as u32).max(1);
        let small = resize(&cutout.image, width, height, FilterType::Triangle);
        let pixels = small
            .pixels()
            .map(|pixel| {
                let [red, green, blue, alpha] = pixel.0.map(|byte| f32::from(byte) / 255.0);
                [red * alpha, green * alpha, blue * alpha, alpha]
            })
            .collect();
        Self {
            width: width as usize,
            height: height as usize,
            foot: [cutout.foot[0] * across, cutout.foot[1] * down],
            base: cutout.bounds[3] * down,
            pixels,
        }
    }
}

/// Where a piece was drawn, so a click on its body selects its square.
#[derive(Debug, Clone, Copy)]
struct Drawn {
    left: isize,
    top: isize,
    sprite: (usize, i32),
    square: (usize, usize),
}

/// The board, its pieces and marks for one camera, composed in display
/// values and kept as light for katui.
pub struct Stage {
    width: usize,
    height: usize,
    style: Style,
    pub orbit: Orbit,
    draft: bool,
    pub view: View,
    flat: Option<Flat>,
    base: Vec<[f32; 3]>,
    frame: Vec<[f32; 3]>,
    painted: Light,
    sprites: HashMap<(usize, i32), Sprite>,
    drawn: Vec<Drawn>,
    composed: Option<u64>,
}

impl Stage {
    /// A stage for `light`; a `draft` one draws the board at half detail, for a camera in motion.
    #[must_use]
    pub fn new(art: &Art, style: Style, orbit: Orbit, light: &Light, draft: bool) -> Self {
        let (width, height) = (light.width() as usize, light.height() as usize);
        let view = View::new(
            &art.lens,
            orbit,
            width as f32,
            height as f32,
            light.pixel_aspect(),
        );
        let flat = orbit.flat.then(|| {
            Flat::new(
                orbit,
                width as f32,
                height as f32,
                light.pixel_aspect(),
                art.board_span,
            )
        });
        let span = art.board_span;
        let base = match (art.board(style), flat) {
            (None, _) => vec![FLOOR; width * height],
            (Some(mips), Some(flat)) => {
                backdrop((width, height), draft, top_down(flat, mips, span, style))
            }
            (Some(mips), None) => backdrop(
                (width, height),
                draft,
                perspective(&view, mips, span, style),
            ),
        };
        Self {
            width,
            height,
            style,
            orbit,
            draft,
            view,
            flat,
            frame: base.clone(),
            base,
            painted: Light::new(width as u32, height as u32, light.pixel_aspect()),
            sprites: HashMap::new(),
            drawn: Vec::new(),
            composed: None,
        }
    }

    #[must_use]
    pub fn style(&self) -> Style {
        self.style
    }

    /// The same stage seen from `orbit`, keeping the sprites it already scaled.
    #[must_use]
    pub fn turned(self, art: &Art, orbit: Orbit, light: &Light, draft: bool) -> Self {
        let mut next = Self::new(art, self.style, orbit, light, draft);
        if (next.width, next.height) == (self.width, self.height) {
            next.sprites = self.sprites;
        }
        next
    }

    /// Whether this stage was built for `light`, `style`, `orbit` and detail.
    #[must_use]
    pub fn fits(&self, light: &Light, style: Style, orbit: Orbit, draft: bool) -> bool {
        (self.width, self.height) == (light.width() as usize, light.height() as usize)
            && self.style == style
            && self.orbit == orbit
            && self.draft == draft
    }

    /// The point of the board under piece position `at`, `lift` units up.
    #[must_use]
    pub fn spot(&self, at: [f32; 2], lift: f32) -> Option<P2> {
        match &self.flat {
            Some(flat) => Some(flat.place(world(at, lift))),
            None => self.view.place(world(at, lift)),
        }
    }

    /// Screen pixels one board unit of height spans at piece position `at`.
    #[must_use]
    pub fn unit(&self, at: [f32; 2]) -> f32 {
        match &self.flat {
            Some(flat) => flat.scale,
            None => self.view.unit(world(at, 0.0)),
        }
    }

    /// How far piece position `at` lies from the eye, to draw the far pieces first.
    fn depth(&self, at: [f32; 2]) -> Option<f32> {
        match &self.flat {
            Some(flat) => Some(-flat.place(world(at, 0.0)).y),
            None => Some(self.view.project(world(at, 0.0))?.1),
        }
    }

    pub fn compose(&mut self, art: &Art, pieces: &[Placed], marks: &[Mark]) {
        let key = fingerprint(pieces, marks);
        if self.composed == Some(key) {
            return;
        }
        self.composed = Some(key);
        self.frame.clone_from(&self.base);
        for mark in marks {
            let corners = [(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0)].map(|(x, y)| {
                let at = [mark.file as f32 + x, mark.rank as f32 + y];
                self.spot(at, 0.0).unwrap_or_default()
            });
            fill_quad(
                &mut self.frame,
                self.width,
                &corners,
                mark.color,
                mark.strength,
            );
        }
        let mut order: Vec<(f32, Placed)> = pieces
            .iter()
            .filter_map(|piece| Some((self.depth(piece.at)?, *piece)))
            .collect();
        order.sort_by(|one, two| two.0.total_cmp(&one.0));
        let gloss = finish(self.style).reflection;
        self.drawn.clear();
        if self.flat.is_none() {
            for (_, piece) in &order {
                self.stamp(art, piece, Some(gloss));
            }
        }
        for (_, piece) in &order {
            self.stamp(art, piece, None);
        }
        let width = self.width;
        let frame = &self.frame;
        self.painted
            .paint(|x, y| to_light(frame.get(y * width + x).copied().unwrap_or(FLOOR)));
    }

    pub fn paint(&self, light: &mut Light) {
        light.copy_from(&self.painted);
    }

    fn sprite_for(&mut self, art: &Art, piece: &Placed) -> Option<(usize, i32)> {
        let kind = PIECES.iter().position(|known| *known == piece.letter)?;
        let index = kind
            + if piece.side == Side::White {
                0
            } else {
                PIECES.len()
            };
        let cutout = art.piece(self.style, piece.side, piece.letter)?;
        let scale = if let Some(flat) = &self.flat {
            flat.scale * FLAT_FILL / art.flat_extent(self.style)
        } else {
            // a piece shrinks with its distance alone; the board's height unit also
            // grows where the view flattens, which would swell the far ranks
            let breadth = self.view.breadth(world(piece.at, 0.0));
            breadth / (cutout.unit.max(1.0) * art.lens.breadth_per_unit())
        };
        let bucket = (scale.max(1e-3).ln() / SIZE_STEP.ln()).round() as i32;
        let key = (index, bucket);
        if !self.sprites.contains_key(&key) {
            let down = SIZE_STEP.powi(bucket);
            let across = down / self.painted.pixel_aspect();
            self.sprites
                .insert(key, Sprite::scaled(cutout, across, down));
        }
        Some(key)
    }

    /// Draw `piece`, or its reflection in the board when `mirror` gives the
    /// reflection's strength.
    fn stamp(&mut self, art: &Art, piece: &Placed, mirror: Option<f32>) {
        let raised = if mirror.is_some() {
            -piece.lift
        } else {
            piece.lift
        };
        let (Some(foot), Some(key)) = (self.spot(piece.at, raised), self.sprite_for(art, piece))
        else {
            return;
        };
        let Some(sprite) = self.sprites.get(&key) else {
            return;
        };
        let foot = match &self.flat {
            // we stand the piece's lowest pixel a little above its square's lower edge
            Some(flat) => P2::new(
                foot.x,
                foot.y + flat.scale * FLAT_FILL / 2.0 - (sprite.base - sprite.foot[1]),
            ),
            None => foot,
        };
        let left = (foot.x - sprite.foot[0]).round() as isize;
        let foot_row = sprite.foot[1].round() as isize;
        let top = foot.y.round() as isize - foot_row;
        if mirror.is_none() {
            let square = (
                piece.at[0].floor().clamp(0.0, 7.0) as usize,
                piece.at[1].floor().clamp(0.0, 7.0) as usize,
            );
            self.drawn.push(Drawn {
                left,
                top,
                sprite: key,
                square,
            });
        }
        let (width, height) = (self.width as isize, self.height as isize);
        for (index, pixels) in sprite.pixels.chunks_exact(sprite.width).enumerate() {
            let row = index as isize;
            let (y, strength) = match mirror {
                None => (top + row, piece.opacity),
                Some(gloss) => {
                    let depth = (foot_row - row) as f32 / sprite.height.max(1) as f32;
                    let fade = (1.0 - depth * 1.6).max(0.0);
                    (
                        foot.y.round() as isize + (foot_row - row),
                        gloss * fade * piece.opacity,
                    )
                }
            };
            if !(0..height).contains(&y) || strength <= 0.0 {
                continue;
            }
            for (column, pixel) in pixels.iter().enumerate() {
                let x = left + column as isize;
                if !(0..width).contains(&x) || pixel[3] <= 0.0 {
                    continue;
                }
                if let Some(under) = self.frame.get_mut((y * width + x) as usize) {
                    let alpha = pixel[3] * strength;
                    for (channel, value) in under.iter_mut().enumerate() {
                        let color = pixel.get(channel).copied().unwrap_or_default();
                        let glow = piece.tint.get(channel).copied().unwrap_or_default() * alpha;
                        *value = *value * (1.0 - alpha) + color * strength + glow;
                    }
                }
            }
        }
    }

    /// The square under screen pixel `spot`: a piece's body first, nearest
    /// piece first, then the board itself.
    #[must_use]
    pub fn square_at(&self, spot: P2) -> Option<(usize, usize)> {
        if self.flat.is_some() {
            return self.board_square_at(spot);
        }
        let (x, y) = (spot.x.round() as isize, spot.y.round() as isize);
        let hit = self.drawn.iter().rev().find(|drawn| {
            let Some(sprite) = self.sprites.get(&drawn.sprite) else {
                return false;
            };
            let (column, row) = (x - drawn.left, y - drawn.top);
            (0..sprite.width as isize).contains(&column)
                && (0..sprite.height as isize).contains(&row)
                && sprite
                    .pixels
                    .get(row as usize * sprite.width + column as usize)
                    .is_some_and(|pixel| pixel[3] > 0.4)
        });
        hit.map(|drawn| drawn.square)
            .or_else(|| self.board_square_at(spot))
    }

    /// The square of the board itself under `spot`, looking past any piece in front.
    #[must_use]
    pub fn board_square_at(&self, spot: P2) -> Option<(usize, usize)> {
        let point = match &self.flat {
            Some(flat) => flat.on_board(spot),
            None => self.view.on_plane(spot, 0.0)?,
        };
        let (file, rank) = ((point.x + 4.0).floor(), (point.y + 4.0).floor());
        ((0.0..8.0).contains(&file) && (0.0..8.0).contains(&rank))
            .then_some((file as usize, rank as usize))
    }

    /// Where to write the file letters and rank numbers, on the frame nearest the player.
    #[must_use]
    pub fn labels(&self) -> Vec<(String, P2)> {
        let white = self.orbit.home.cos() > 0.0;
        let (edge, side) = if white { (-0.24, -0.24) } else { (8.24, 8.24) };
        let files = "abcdefgh"
            .chars()
            .enumerate()
            .filter_map(|(index, letter)| {
                Some((
                    letter.to_string(),
                    self.spot([index as f32 + 0.5, edge], 0.0)?,
                ))
            });
        let ranks = (0..8).filter_map(|index| {
            Some((
                (index + 1).to_string(),
                self.spot([side, index as f32 + 0.5], 0.0)?,
            ))
        });
        files.chain(ranks).collect()
    }
}

fn world(at: [f32; 2], lift: f32) -> V3 {
    v3(at[0] - 4.0, at[1] - 4.0, lift)
}

fn fingerprint(pieces: &[Placed], marks: &[Mark]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for piece in pieces {
        (piece.side as u8, piece.letter).hash(&mut hasher);
        for value in [piece.at[0], piece.at[1], piece.lift, piece.opacity] {
            value.to_bits().hash(&mut hasher);
        }
        piece.tint.map(f32::to_bits).hash(&mut hasher);
    }
    for mark in marks {
        (mark.file, mark.rank).hash(&mut hasher);
        mark.color.map(f32::to_bits).hash(&mut hasher);
        mark.strength.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

/// The light katui needs to show `display` once it tone-maps the frame.
fn to_light(display: [f32; 3]) -> Rgb {
    display.map(|value| {
        // we keep the board and pieces under the bloom threshold, so only effects glow
        let shown = value.clamp(0.0, 0.91);
        shown * shown / (1.0 - shown * shown)
    })
}

/// Every pixel `shade` gives, in display values, with the rows split over
/// every core; a `draft` shades every other row and column.
fn backdrop(
    size: (usize, usize),
    draft: bool,
    shade: impl Fn(P2) -> [f32; 3] + Sync,
) -> Vec<[f32; 3]> {
    let (width, height) = size;
    let mut out = vec![FLOOR; width * height];
    let cores = std::thread::available_parallelism().map_or(4, std::num::NonZero::get);
    // bands hold whole row pairs, so a draft can copy each row into the one below
    let band = height.div_ceil(cores).div_ceil(2).max(1) * 2 * width;
    let shade = &shade;
    std::thread::scope(|scope| {
        for (index, rows) in out.chunks_mut(band).enumerate() {
            let first = index * band / width.max(1);
            scope.spawn(move || {
                let mut lines = rows.chunks_exact_mut(width).enumerate();
                while let Some((offset, row)) = lines.next() {
                    shade_row(shade, first + offset, row, draft);
                    if draft && let Some((_, below)) = lines.next() {
                        below.copy_from_slice(row);
                    }
                }
            });
        }
    });
    out
}

fn shade_row(shade: &impl Fn(P2) -> [f32; 3], y: usize, row: &mut [[f32; 3]], draft: bool) {
    let mut previous = FLOOR;
    for (x, pixel) in row.iter_mut().enumerate() {
        if draft && x % 2 == 1 {
            *pixel = previous;
            continue;
        }
        *pixel = shade(P2::new(x as f32 + 0.5, y as f32 + 0.5));
        previous = *pixel;
    }
}

/// The key light hangs behind the board, so its sheen shows on the far squares.
fn key_light() -> V3 {
    v3(-0.25, 0.85, 0.6).normalized()
}

/// The board texture's color at board point `hit`, `texels` texture pixels per screen pixel.
fn albedo(mips: &Mipmaps, span: f32, hit: V3, texels: f32) -> [f32; 3] {
    let half = span / 2.0;
    mips.sample((hit.x + half) / span, (half - hit.y) / span, texels)
}

/// The board in perspective with its frame, sides, floor and a sheen that follows the camera.
fn perspective<'a>(
    view: &'a View,
    mips: &'a Mipmaps,
    span: f32,
    style: Style,
) -> impl Fn(P2) -> [f32; 3] + Sync + 'a {
    let half = span / 2.0;
    let finish = finish(style);
    let light = key_light();
    let edge = mips.sample(0.01, 0.5, 64.0);
    let texel = span / mips.size() as f32;
    move |spot| {
        let ray = view.ray(spot);
        match view.on_plane(spot, 0.0) {
            Some(hit) if hit.x.abs() <= half && hit.y.abs() <= half => {
                // one pixel down spans the most board, so it sets the texture detail
                let next = view
                    .on_plane(P2::new(spot.x, spot.y + 1.0), 0.0)
                    .unwrap_or(hit);
                let reach = (next - hit).dot(next - hit).sqrt();
                lacquer(albedo(mips, span, hit, reach / texel), ray, light, &finish)
            }
            _ => sides(view, ray, half, edge, light).unwrap_or_else(|| {
                view.on_plane(spot, -THICKNESS)
                    .map_or(FLOOR, |hit| floor(hit, half))
            }),
        }
    }
}

/// The board seen straight from above, on the floor.
fn top_down(
    flat: Flat,
    mips: &Mipmaps,
    span: f32,
    style: Style,
) -> impl Fn(P2) -> [f32; 3] + Sync + '_ {
    let half = span / 2.0;
    let finish = finish(style);
    let light = key_light();
    let texels = mips.size() as f32 / span / flat.scale;
    let down = v3(0.0, 0.0, -1.0);
    move |spot| {
        let hit = flat.on_board(spot);
        if hit.x.abs() <= half && hit.y.abs() <= half {
            lacquer(albedo(mips, span, hit, texels), down, light, &finish)
        } else {
            floor(hit, half)
        }
    }
}

fn lacquer(albedo: [f32; 3], ray: V3, light: V3, finish: &Finish) -> [f32; 3] {
    let toward_eye = ray * -1.0;
    let halfway = (light + toward_eye).normalized();
    let highlight = halfway.z.max(0.0).powi(24) * finish.gloss * 0.3;
    let grazing = (1.0 - toward_eye.z.max(0.0)).powi(4) * finish.gloss * 0.25;
    let mut out = albedo;
    for (value, tint) in out.iter_mut().zip(finish.sheen) {
        *value = value.powf(1.15) * 0.92 + (highlight + grazing) * tint;
    }
    out
}

/// The board's outer sides, shaded by which way each one faces.
fn sides(view: &View, ray: V3, half: f32, edge: [f32; 3], light: V3) -> Option<[f32; 3]> {
    let faces = [
        (v3(1.0, 0.0, 0.0), half),
        (v3(-1.0, 0.0, 0.0), half),
        (v3(0.0, 1.0, 0.0), half),
        (v3(0.0, -1.0, 0.0), half),
    ];
    let mut nearest: Option<(f32, V3)> = None;
    for (normal, offset) in faces {
        let facing = ray.dot(normal);
        if facing >= 0.0 {
            continue;
        }
        let along = (offset - view.eye.dot(normal)) / facing;
        let hit = view.eye + ray * along;
        let inside = hit.x.abs() <= half + 1e-3 && hit.y.abs() <= half + 1e-3;
        if inside
            && (-THICKNESS..=0.0).contains(&hit.z)
            && nearest.is_none_or(|(best, _)| along < best)
        {
            nearest = Some((along, normal));
        }
    }
    let (_, normal) = nearest?;
    let shade = 0.25 + 0.55 * normal.dot(light).max(0.0);
    Some(edge.map(|channel| channel * shade))
}

/// The dark floor at `hit`, with the board's soft shadow and a faint pool of light.
fn floor(hit: V3, half: f32) -> [f32; 3] {
    let outside = (hit.x.abs() - half).max(hit.y.abs() - half).max(0.0);
    let shadow = 1.0 - 0.85 * (-outside * 1.4).exp();
    let pool = 1.0 + 1.6 * (-(hit.x * hit.x + hit.y * hit.y) / 90.0).exp();
    FLOOR.map(|channel| channel * shadow * pool)
}

fn fill_quad(
    frame: &mut [[f32; 3]],
    width: usize,
    corners: &[P2; 4],
    color: [f32; 3],
    strength: f32,
) {
    let low_x = corners
        .iter()
        .map(|p| p.x)
        .fold(f32::MAX, f32::min)
        .max(0.0) as usize;
    let high_x = corners
        .iter()
        .map(|p| p.x)
        .fold(f32::MIN, f32::max)
        .max(0.0) as usize;
    let low_y = corners
        .iter()
        .map(|p| p.y)
        .fold(f32::MAX, f32::min)
        .max(0.0) as usize;
    let high_y = corners
        .iter()
        .map(|p| p.y)
        .fold(f32::MIN, f32::max)
        .max(0.0) as usize;
    let height = frame.len() / width.max(1);
    for y in low_y..=high_y.min(height.saturating_sub(1)) {
        for x in low_x..=high_x.min(width.saturating_sub(1)) {
            if inside(corners, P2::new(x as f32 + 0.5, y as f32 + 0.5))
                && let Some(pixel) = frame.get_mut(y * width + x)
            {
                for (value, tint) in pixel.iter_mut().zip(color) {
                    *value += (tint - *value) * strength;
                }
            }
        }
    }
}

fn inside(corners: &[P2; 4], point: P2) -> bool {
    let sides = corners
        .iter()
        .zip(corners.iter().cycle().skip(1))
        .map(|(from, to)| {
            (to.x - from.x) * (point.y - from.y) - (to.y - from.y) * (point.x - from.x)
        });
    let signs: Vec<f32> = sides.collect();
    signs.iter().all(|side| *side >= 0.0) || signs.iter().all(|side| *side <= 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(side: Side) -> (Art, Stage) {
        let art = Art::load().unwrap();
        let orbit = art.lens.home(side == Side::White);
        let stage = Stage::new(&art, Style::Glass, orbit, &Light::new(640, 400, 1.0), false);
        (art, stage)
    }

    fn king() -> Placed {
        Placed {
            side: Side::Black,
            letter: 'k',
            at: [4.5, 3.5],
            lift: 0.0,
            opacity: 1.0,
            tint: [0.0; 3],
        }
    }

    #[test]
    fn a_click_on_a_square_or_a_piece_finds_its_square() {
        let (art, mut stage) = stage(Side::White);
        stage.compose(&art, &[king()], &[]);
        let c3 = stage.spot([2.5, 2.5], 0.0).unwrap();
        assert_eq!(stage.square_at(c3), Some((2, 2)));
        let crown = stage.spot([4.5, 3.5], 1.2).unwrap();
        assert_eq!(stage.square_at(crown), Some((4, 3)));
        assert_eq!(stage.square_at(P2::new(1.0, 1.0)), None);
    }

    #[test]
    fn a_piece_covers_its_square_and_a_mark_tints_it() {
        let (art, mut stage) = stage(Side::Black);
        stage.compose(&art, &[], &[]);
        let empty = stage.frame.clone();
        stage.compose(&art, &[king()], &[]);
        let changed = stage
            .frame
            .iter()
            .zip(&empty)
            .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| (x - y).abs() > 1e-4))
            .count();
        assert!(changed > 200, "the king drew {changed} pixels");
        let mark = Mark {
            file: 0,
            rank: 0,
            color: [1.0, 0.0, 0.0],
            strength: 0.5,
        };
        stage.compose(&art, &[], &[mark]);
        let center = stage.spot([0.5, 0.5], 0.0).unwrap();
        let index = center.y as usize * 640 + center.x as usize;
        assert!(stage.frame[index][0] > empty[index][0]);
    }

    #[test]
    fn the_labels_sit_on_the_players_near_edge() {
        let (_, stage) = stage(Side::Black);
        let labels = stage.labels();
        let spot = |name: &str| labels.iter().find(|(text, _)| text == name).unwrap().1;
        assert!(spot("h").x < spot("a").x, "black reads the files from h");
        assert!(spot("8").y > spot("1").y, "black sees rank 8 nearest");
    }
}
