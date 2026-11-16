use std::collections::HashMap;

use image::imageops::{FilterType, resize};
use image::{ImageFormat, RgbaImage};
use serde::Deserialize;

use crate::camera::Lens;
use crate::table::Side;

#[derive(Debug, thiserror::Error)]
pub enum ArtError {
    #[error("a bundled image is broken: {0}")]
    Image(#[from] image::ImageError),
    #[error("the bundled layout is broken: {0}")]
    Layout(#[from] serde_json::Error),
    #[error("the bundled layout has no frame for {0}")]
    Missing(char),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Style {
    Amber,
    Glass,
    Metal,
}

impl Style {
    pub const ALL: [Self; 3] = [Self::Amber, Self::Glass, Self::Metal];

    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Amber => "amber",
            Self::Glass => "glass",
            Self::Metal => "metal",
        }
    }

    #[must_use]
    pub fn next(self) -> Self {
        match self {
            Self::Amber => Self::Glass,
            Self::Glass => Self::Metal,
            Self::Metal => Self::Amber,
        }
    }

    fn index(self) -> usize {
        self as usize
    }
}

/// Piece letters in the order the sprites are bundled.
pub const PIECES: [char; 6] = ['p', 'n', 'b', 'r', 'q', 'k'];

macro_rules! style_files {
    ($style:literal) => {
        (
            include_bytes!(concat!("../assets/boards/", $style, ".jpg")).as_slice(),
            [
                include_bytes!(concat!("../assets/pieces/", $style, "/wp.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/wn.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/wb.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/wr.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/wq.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/wk.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/bp.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/bn.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/bb.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/br.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/bq.png")).as_slice(),
                include_bytes!(concat!("../assets/pieces/", $style, "/bk.png")).as_slice(),
            ],
        )
    };
}

type StyleFiles = (&'static [u8], [&'static [u8]; 12]);

const FILES: [StyleFiles; 3] = [
    style_files!("amber"),
    style_files!("glass"),
    style_files!("metal"),
];
const LAYOUT: &str = include_str!("../assets/layout.json");

#[derive(Debug, Clone, Deserialize)]
struct CameraFrame {
    position: [f32; 3],
    target: [f32; 3],
    fov_x: f32,
    aspect: f32,
    board_span: f32,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct PieceFrame {
    foot: [f32; 2],
    unit: f32,
}

#[derive(Debug, Clone, Deserialize)]
struct Layout {
    camera: CameraFrame,
    pieces: HashMap<char, PieceFrame>,
}

/// A rendered piece, with the point it stands on.
#[derive(Debug, Clone)]
pub struct Cutout {
    pub image: RgbaImage,
    /// Where the piece touches the board, in the image's pixels.
    pub foot: [f32; 2],
    /// Pixels per board unit of height in the render.
    pub unit: f32,
}

/// A board texture at a few sizes, so far squares don't shimmer.
pub struct Mipmaps(Vec<RgbaImage>);

impl Mipmaps {
    fn new(texture: RgbaImage) -> Self {
        let mut levels = vec![texture];
        while let Some(last) = levels.last().filter(|last| last.width() > 256) {
            let half = resize(
                last,
                last.width() / 2,
                last.height() / 2,
                FilterType::Triangle,
            );
            levels.push(half);
        }
        Self(levels)
    }

    /// The full size's width in pixels.
    #[must_use]
    pub fn size(&self) -> u32 {
        self.0.first().map_or(1, RgbaImage::width)
    }

    /// The display color at `u`, `v` (0 to 1, `v` down), `texels` texture
    /// pixels per screen pixel.
    #[must_use]
    pub fn sample(&self, u: f32, v: f32, texels: f32) -> [f32; 3] {
        let level = texels.max(1.0).log2().floor() as usize;
        let Some(image) = self.0.get(level.min(self.0.len().saturating_sub(1))) else {
            return [0.0; 3];
        };
        let (width, height) = (image.width() as f32, image.height() as f32);
        let (x, y) = ((u * width - 0.5).max(0.0), (v * height - 0.5).max(0.0));
        let (x0, y0) = (x.floor(), y.floor());
        let (tx, ty) = (x - x0, y - y0);
        let fetch = |dx: f32, dy: f32| {
            let px = ((x0 + dx) as u32).min(image.width() - 1);
            let py = ((y0 + dy) as u32).min(image.height() - 1);
            image.get_pixel(px, py).0
        };
        let corners = [
            fetch(0.0, 0.0),
            fetch(1.0, 0.0),
            fetch(0.0, 1.0),
            fetch(1.0, 1.0),
        ];
        let weights = [
            (1.0 - tx) * (1.0 - ty),
            tx * (1.0 - ty),
            (1.0 - tx) * ty,
            tx * ty,
        ];
        [0, 1, 2].map(|channel| {
            corners
                .iter()
                .zip(weights)
                .map(|(pixel, weight)| {
                    f32::from(pixel.get(channel).copied().unwrap_or(0)) / 255.0 * weight
                })
                .sum()
        })
    }
}

/// Every board texture and piece, decoded once.
pub struct Art {
    pub lens: Lens,
    /// The board and frame, edge to edge, in board units.
    pub board_span: f32,
    boards: Vec<Mipmaps>,
    pieces: Vec<Vec<Cutout>>,
}

impl Art {
    pub fn load() -> Result<Self, ArtError> {
        let layout: Layout = serde_json::from_str(LAYOUT)?;
        let camera = &layout.camera;
        let mut boards = Vec::new();
        let mut pieces = Vec::new();
        for (board, piece_files) in FILES {
            boards.push(Mipmaps::new(decode(board, ImageFormat::Jpeg)?));
            let colored = piece_files.iter().zip(PIECES.iter().cycle());
            let cut = colored
                .map(|(bytes, letter)| {
                    let frame = layout
                        .pieces
                        .get(letter)
                        .ok_or(ArtError::Missing(*letter))?;
                    Ok(Cutout {
                        image: decode(bytes, ImageFormat::Png)?,
                        foot: frame.foot,
                        unit: frame.unit,
                    })
                })
                .collect::<Result<Vec<_>, ArtError>>()?;
            pieces.push(cut);
        }
        Ok(Self {
            lens: Lens::from_render(camera.position, camera.target, camera.fov_x, camera.aspect),
            board_span: camera.board_span,
            boards,
            pieces,
        })
    }

    /// The board seen from straight above, rank 8 at the top.
    #[must_use]
    pub fn board(&self, style: Style) -> Option<&Mipmaps> {
        self.boards.get(style.index())
    }

    /// The sprite for a piece letter (`p`, `n`, ...) of `side`.
    #[must_use]
    pub fn piece(&self, style: Style, side: Side, letter: char) -> Option<&Cutout> {
        let kind = PIECES.iter().position(|known| *known == letter)?;
        let offset = if side == Side::White { 0 } else { PIECES.len() };
        self.pieces.get(style.index())?.get(offset + kind)
    }
}

fn decode(bytes: &[u8], format: ImageFormat) -> Result<RgbaImage, ArtError> {
    Ok(image::load_from_memory_with_format(bytes, format)?.to_rgba8())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_piece_stands_on_its_foot() {
        let art = Art::load().unwrap();
        for style in Style::ALL {
            assert_eq!(art.board(style).unwrap().size(), 2048);
            for side in [Side::White, Side::Black] {
                for letter in PIECES {
                    let piece = art.piece(style, side, letter).unwrap();
                    let (width, height) = (piece.image.width() as f32, piece.image.height() as f32);
                    assert!(
                        width > 40.0 && height > 40.0,
                        "{} {letter} is empty",
                        style.name()
                    );
                    assert!((0.0..width).contains(&piece.foot[0]));
                    assert!(piece.foot[1] > height * 0.5 && piece.foot[1] < height);
                    assert!(piece.unit > 50.0);
                }
            }
        }
    }
}
