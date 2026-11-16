use std::io;
use std::time::{Duration, Instant};

use katui::game::{Key, Light, P2};
use katui::present::select::{Backend, select};
use katui::present::session::copy_to_clipboard;
use katui::present::surface::{PatchedKitty, PixelScreen};
use katui::proto::input::{InputEvent, Mods, MouseAction, Pointer};
use katui::proto::probe::{Capabilities, CellPixels};
use katui::rt::RtError;
use katui::rt::app::{App, AppEvent, Control, run_app};
use katui::rt::events::Waker;
use katui::scene::CellSize;
use katui::scene::surface::Surface;

const FRAMES_PER_SECOND: u32 = 60;
const STEP: Duration = Duration::from_micros(16_667);
const MOST_STEPS_PER_FRAME: u32 = 6;
/// How often a resting game still looks at its clocks and the network.
const TICK: Duration = Duration::from_millis(100);
const BASE_IMAGE: u32 = 900;
const TILE_BUDGET: usize = 320;
const TILE_HEIGHTS: u32 = 3;
/// The most game pixels we compose each frame, which is what the CPU keeps up with.
const MOST_PIXELS_ACROSS: u32 = 1800;
const MOST_PIXELS_DOWN: u32 = 1100;
/// A multiplexer carries graphics far slower, so we send this many times fewer pixels each way.
const MULTIPLEXED_SHRINK: u32 = 2;

/// A game the runner drives: keys, the mouse, a fixed step, light to draw
/// into, and a sharp overlay on top.
pub trait Play {
    fn key(&mut self, key: Key);
    /// A mouse event at a game pixel.
    fn pointer(&mut self, action: MouseAction, at: P2);
    /// Advance the world by `dt` seconds, a fixed step.
    fn step(&mut self, dt: f32);
    /// Whether the picture moves on its own right now.
    fn busy(&self) -> bool;
    /// Whether something changed since the last frame; reading it clears it.
    fn take_dirty(&mut self) -> bool;
    /// Text to put on the system clipboard; reading it clears it.
    fn take_clipboard(&mut self) -> Option<String>;
    fn render(&mut self, light: &mut Light);
    /// Draw text and panels onto the finished frame.
    fn overlay(&mut self, frame: Frame<'_>);
}

/// The finished frame as RGBA bytes, for sharp text on top of the light.
pub struct Frame<'a> {
    pub pixels: &'a mut [u8],
    pub width: usize,
    pub height: usize,
    /// One pixel's width over its height.
    pub aspect: f32,
}

/// Run `game` on the terminal until the player quits.
pub fn run<G: Play>(game: G) -> Result<(), RtError> {
    let caps = katui::rt::tty::probe_default();
    let mut runner = Runner::new(game, caps.clone());
    run_app(&mut runner, io::stdout(), &caps, FRAMES_PER_SECOND)
}

struct Runner<G> {
    game: G,
    caps: Capabilities,
    cell: CellPixels,
    density: (u32, u32),
    light: Light,
    screen: PixelScreen,
    pointer: Pointer,
    fitted: Option<CellSize>,
    last_step: Instant,
    banked: Duration,
    fresh: bool,
}

/// Game pixels per cell across and down: as sharp as the budget allows, each
/// height twice its width so tiles span whole cells.
fn density(size: CellSize, caps: &Capabilities, cell: CellPixels) -> (u32, u32) {
    if select(caps) == Backend::Blocks {
        return (2, 2);
    }
    let shrink = if caps.multiplexed() {
        MULTIPLEXED_SHRINK
    } else {
        1
    };
    let fits = |across: u32| {
        u32::from(size.columns) * across <= MOST_PIXELS_ACROSS / shrink
            && u32::from(size.rows) * across * 2 <= MOST_PIXELS_DOWN / shrink
    };
    let widest = u32::from(cell.width).max(2);
    let across = (1..=widest).rev().find(|across| fits(*across)).unwrap_or(1);
    (across, across * 2)
}

impl<G: Play> Runner<G> {
    fn new(game: G, caps: Capabilities) -> Self {
        let cell = katui::rt::tty::cell_size(&caps);
        Self {
            game,
            pointer: Pointer::new(caps.cell, cell),
            caps,
            cell,
            density: (1, 2),
            light: Light::new(1, 1, 1.0),
            screen: PixelScreen::default(),
            fitted: None,
            last_step: Instant::now(),
            banked: Duration::ZERO,
            fresh: true,
        }
    }

    fn fit(&mut self, out: &mut Vec<u8>, size: CellSize) -> io::Result<()> {
        if self.fitted == Some(size) {
            return Ok(());
        }
        let (across, down) = density(size, &self.caps, self.cell);
        let pixel_width = f32::from(self.cell.width) / across as f32;
        let pixel_height = f32::from(self.cell.height) / down as f32;
        self.density = (across, down);
        self.light = Light::new(
            u32::from(size.columns) * across,
            u32::from(size.rows) * down,
            pixel_width / pixel_height,
        );
        self.screen.free(out)?;
        let mut patched =
            PatchedKitty::with_tiles(BASE_IMAGE, self.cell, down * TILE_HEIGHTS, TILE_BUDGET);
        if let Some(stash) = katui::rt::stash::local_stash(&self.caps) {
            patched = patched.with_stash(stash);
        }
        self.screen = PixelScreen::new(&self.caps, self.cell, patched);
        self.pointer.resize(size.columns, size.rows);
        self.fitted = Some(size);
        self.fresh = true;
        Ok(())
    }

    fn advance(&mut self) {
        let now = Instant::now();
        self.banked += now.saturating_duration_since(self.last_step);
        self.last_step = now;
        let mut steps = 0;
        while self.banked >= STEP && steps < MOST_STEPS_PER_FRAME {
            self.game.step(STEP.as_secs_f32());
            self.banked -= STEP;
            steps += 1;
        }
        if steps == MOST_STEPS_PER_FRAME {
            self.banked = Duration::ZERO;
        }
    }

    fn on_input(&mut self, event: &InputEvent) -> Control {
        match event {
            InputEvent::Key(Key::Char('q'), Mods::NONE) => return Control::Quit,
            InputEvent::Key(Key::Char('c'), mods) if mods.ctrl => return Control::Quit,
            InputEvent::Key(key, _) => self.game.key(*key),
            InputEvent::Mouse(mouse) => {
                let (x, y) = self.pointer.locate(mouse, self.cell);
                let spot = P2::new(
                    x * self.density.0 as f32 / f32::from(self.cell.width),
                    y * self.density.1 as f32 / f32::from(self.cell.height),
                );
                self.game.pointer(mouse.action, spot);
            }
            other => self.pointer.observe(other),
        }
        Control::Redraw
    }

    fn frame(&mut self) -> Surface {
        let surface = self.light.to_surface();
        let (width, height) = (surface.width(), surface.height());
        let mut pixels = surface.pixels().to_vec();
        self.game.overlay(Frame {
            pixels: &mut pixels,
            width: width as usize,
            height: height as usize,
            aspect: self.light.pixel_aspect(),
        });
        Surface::from_rgba(width, height, pixels).unwrap_or(surface)
    }
}

impl<G: Play> App for Runner<G> {
    type Message = ();

    fn start(&mut self, waker: Waker<()>) {
        waker.every(TICK, || ());
    }

    fn update(&mut self, event: AppEvent<()>) -> Control {
        match event {
            AppEvent::Input(input) => self.on_input(&input),
            AppEvent::Repaint => {
                self.last_step = Instant::now();
                self.screen.repaint();
                self.fresh = true;
                Control::Redraw
            }
            AppEvent::Message(()) | AppEvent::Resized(_) => Control::Redraw,
        }
    }

    fn draw(&mut self, out: &mut Vec<u8>, size: CellSize) -> io::Result<Control> {
        self.fit(out, size)?;
        if let Some(text) = self.game.take_clipboard() {
            copy_to_clipboard(out, &text, self.caps.multiplexed())?;
        }
        self.advance();
        let busy = self.game.busy();
        if !(self.fresh || busy || self.game.take_dirty()) {
            return Ok(Control::Wait);
        }
        self.fresh = false;
        self.game.render(&mut self.light);
        let surface = self.frame();
        self.screen
            .render_frame(out, &surface, size.columns, size.rows)?;
        Ok(if busy {
            Control::Animate
        } else {
            Control::Wait
        })
    }
}

#[cfg(test)]
mod tests {
    use katui::proto::probe::Identity;

    use super::*;

    #[test]
    fn density_follows_the_cell_and_shrinks_to_stay_affordable() {
        let cell = CellPixels {
            width: 16,
            height: 34,
        };
        let kitty = Capabilities {
            kitty: true,
            ..Capabilities::default()
        };
        let small = CellSize {
            columns: 80,
            rows: 24,
        };
        let huge = CellSize {
            columns: 300,
            rows: 80,
        };
        assert_eq!(density(small, &kitty, cell), (16, 32));
        assert_eq!(density(huge, &kitty, cell), (6, 12));
        let tmux = Capabilities {
            identity: Some(Identity {
                name: "tmux".to_owned(),
                version: None,
            }),
            ..kitty.clone()
        };
        assert_eq!(density(huge, &tmux, cell), (3, 6));
        assert_eq!(density(huge, &Capabilities::default(), cell), (2, 2));
    }
}
