use crate::net::{BROKERS, Role as Seat};
use crate::table::{Side, Table};
use crate::text::{Align, Canvas, Face, Paint};

use super::{Kess, Mode, PROMOTIONS, clock_text, name};

const GOLD: [f32; 3] = [1.0, 0.8, 0.45];
const DIM: [f32; 3] = [0.62, 0.58, 0.54];
const INK: [f32; 3] = [0.03, 0.025, 0.02];
const RED: [f32; 3] = [1.0, 0.35, 0.28];
/// Below this many pixels tall the key hints would cover the board.
const HINTS_MIN_HEIGHT: f32 = 300.0;
const HINTS: &str = "arrows or hjkl aim · enter move · drag turn · wheel zoom · r resign · d draw · n rematch · s set · m mute · q quit";

/// Text sizes that follow the screen.
#[derive(Clone, Copy)]
struct Sizes {
    unit: f32,
    width: f32,
    height: f32,
    margin: f32,
    aspect: f32,
}

impl Sizes {
    fn of(canvas: &Canvas<'_>) -> Self {
        let height = canvas.height as f32;
        let unit = (height / 30.0).max(7.0);
        Self {
            unit,
            width: canvas.width as f32,
            height,
            margin: unit * 0.6,
            aspect: canvas.aspect,
        }
    }

    /// `pixels` of height as the same length across.
    fn across(&self, pixels: f32) -> f32 {
        pixels / self.aspect
    }
}

pub(super) fn draw(kess: &Kess, canvas: &mut Canvas<'_>) {
    let sizes = Sizes::of(canvas);
    labels(kess, canvas, &sizes);
    match &kess.table {
        Some(table) => {
            players(kess, table, canvas, &sizes);
            status(kess, table, canvas, &sizes);
        }
        None => lobby(kess, canvas, &sizes),
    }
    hints(kess, canvas, &sizes);
    notice(kess, canvas, &sizes);
    promotion(kess, canvas, &sizes);
    banner(kess, canvas, &sizes);
    waiting(kess, canvas, &sizes);
}

fn labels(kess: &Kess, canvas: &mut Canvas<'_>, sizes: &Sizes) {
    let Some(stage) = &kess.stage else {
        return;
    };
    let size = sizes.unit * 0.75;
    for (text, spot) in stage.labels() {
        canvas.text(
            Face::Title,
            &text,
            (spot.x, spot.y - size * 0.55),
            size,
            Align::Center,
            (GOLD, 0.85),
        );
    }
}

fn players(kess: &Kess, table: &Table, canvas: &mut Canvas<'_>, sizes: &Sizes) {
    let now = kess.now_ms();
    let far = kess.view().other();
    let timed = table.settings().control.is_some();
    let height = sizes.unit * if timed { 3.3 } else { 1.9 };
    let width = sizes.across(sizes.unit * 9.5);
    for (side, top) in [
        (far, sizes.margin),
        (far.other(), sizes.height - sizes.margin - height),
    ] {
        let active = table.turn() == side && !table.is_over();
        let border = if active { (GOLD, 0.9) } else { (DIM, 0.35) };
        canvas.panel(
            (sizes.margin, top),
            (width, height),
            sizes.unit * 0.4,
            (INK, 0.62),
            Some(border),
        );
        let label = player_name(kess, side);
        let pad = sizes.unit * 0.45;
        let ink = if active { GOLD } else { DIM };
        canvas.text(
            Face::Title,
            &label,
            (sizes.margin + sizes.across(pad), top + pad),
            sizes.unit * 0.85,
            Align::Left,
            (ink, 1.0),
        );
        if let Some(left) = table.remaining(side, now) {
            let urgent = active && left < 20_000;
            let color = if urgent {
                RED
            } else if active {
                [1.0, 0.95, 0.88]
            } else {
                DIM
            };
            let spot = (
                sizes.margin + sizes.across(pad),
                top + pad + sizes.unit * 1.1,
            );
            canvas.text(
                Face::Mono,
                &clock_text(left),
                spot,
                sizes.unit * 1.5,
                Align::Left,
                (color, 1.0),
            );
        }
    }
}

fn player_name(kess: &Kess, side: Side) -> String {
    let mine = kess.local().contains(&side);
    match (&kess.mode, mine) {
        (Mode::Online { .. } | Mode::Computer { .. }, true) => format!("You · {}", name(side)),
        (Mode::Computer { .. }, false) => format!("kess bot · {}", name(side)),
        (Mode::Online { .. }, false) => format!("Opponent · {}", name(side)),
        (Mode::Hotseat, _) => capitalized(name(side)),
    }
}

fn capitalized(word: &str) -> String {
    let mut letters = word.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

fn status(kess: &Kess, table: &Table, canvas: &mut Canvas<'_>, sizes: &Sizes) {
    let text = if let Some(verdict) = table.verdict() {
        super::verdict_text(verdict).1
    } else if !kess.joined {
        "waiting for your opponent".to_owned()
    } else {
        let whose = match (&kess.mode, kess.my_turn()) {
            (Mode::Hotseat, _) => format!("{} to move", name(table.turn())),
            (Mode::Computer { .. }, false) => "kess bot is thinking".to_owned(),
            (_, true) => "your move".to_owned(),
            (_, false) => "their move".to_owned(),
        };
        if kess.checked_king().is_some() {
            format!("check · {whose}")
        } else {
            whose
        }
    };
    let size = sizes.unit * 0.9;
    let width =
        kess.fonts.width(Face::Title, &text, size, sizes.aspect) + sizes.across(sizes.unit * 1.2);
    let left = sizes.width - sizes.margin - width;
    canvas.panel(
        (left, sizes.margin),
        (width, sizes.unit * 1.7),
        sizes.unit * 0.4,
        (INK, 0.62),
        Some((GOLD, 0.35)),
    );
    let spot = (
        sizes.width - sizes.margin - sizes.across(sizes.unit * 0.6),
        sizes.margin + sizes.unit * 0.35,
    );
    canvas.text(Face::Title, &text, spot, size, Align::Right, (GOLD, 1.0));
    if let Mode::Online { code, .. } = &kess.mode {
        let line = format!(
            "room {code} · {}/{} brokers",
            kess.brokers.len(),
            BROKERS.len()
        );
        let spot = (sizes.width - sizes.margin, sizes.margin + sizes.unit * 2.0);
        canvas.text(
            Face::Mono,
            &line,
            spot,
            sizes.unit * 0.55,
            Align::Right,
            (DIM, 0.9),
        );
    }
}

fn hints(kess: &Kess, canvas: &mut Canvas<'_>, sizes: &Sizes) {
    if sizes.height < HINTS_MIN_HEIGHT {
        return;
    }
    let size = sizes.unit * 0.5;
    let room = sizes.width - sizes.across(sizes.unit * 20.0);
    let lines: Vec<&str> = if kess.fonts.width(Face::Mono, HINTS, size, sizes.aspect) <= room {
        vec![HINTS]
    } else {
        HINTS.splitn(2, " · r ").collect()
    };
    let count = lines.len();
    let widest = lines
        .iter()
        .map(|line| kess.fonts.width(Face::Mono, line, size, sizes.aspect))
        .fold(0.0, f32::max)
        + sizes.across(sizes.unit * 1.2);
    let strip = size * 1.4 * count as f32 + size * 0.6;
    let at = (
        (sizes.width - widest) / 2.0,
        sizes.height - sizes.margin - strip + size * 0.1,
    );
    canvas.panel(at, (widest, strip), size * 0.6, (INK, 0.55), None);
    for (index, line) in lines.iter().enumerate() {
        let text = if index == 1 {
            format!("r {line}")
        } else {
            (*line).to_owned()
        };
        let top = sizes.height - sizes.margin - size * 1.4 * (count - index) as f32;
        canvas.text(
            Face::Mono,
            &text,
            (sizes.width / 2.0, top),
            size,
            Align::Center,
            (DIM, 0.75),
        );
    }
}

fn notice(kess: &Kess, canvas: &mut Canvas<'_>, sizes: &Sizes) {
    let Some(notice) = &kess.notice else {
        return;
    };
    let size = sizes.unit * 0.75;
    let width = kess
        .fonts
        .width(Face::Mono, &notice.text, size, sizes.aspect)
        + sizes.across(sizes.unit * 1.4);
    let top = sizes.unit * 3.0;
    let left = (sizes.width - width) / 2.0;
    canvas.panel(
        (left, top),
        (width, sizes.unit * 1.5),
        sizes.unit * 0.5,
        (INK, 0.75),
        Some((GOLD, 0.5)),
    );
    canvas.text(
        Face::Mono,
        &notice.text,
        (sizes.width / 2.0, top + sizes.unit * 0.35),
        size,
        Align::Center,
        ([1.0, 0.93, 0.82], 1.0),
    );
}

fn promotion(kess: &Kess, canvas: &mut Canvas<'_>, sizes: &Sizes) {
    let Some((_, _, choice)) = kess.promotion else {
        return;
    };
    let names = ["Queen", "Rook", "Bishop", "Knight"];
    let size = sizes.unit * 1.1;
    let slot = sizes.across(sizes.unit * 6.0);
    let width = slot * PROMOTIONS.len() as f32;
    let (left, top) = (
        (sizes.width - width) / 2.0,
        sizes.height / 2.0 - sizes.unit * 1.5,
    );
    canvas.panel(
        (left - slot * 0.1, top - sizes.unit * 1.6),
        (width + slot * 0.2, sizes.unit * 4.2),
        sizes.unit * 0.6,
        (INK, 0.85),
        Some((GOLD, 0.7)),
    );
    canvas.text(
        Face::Mono,
        "promote to · ← → pick · enter confirm",
        (sizes.width / 2.0, top - sizes.unit * 1.2),
        sizes.unit * 0.55,
        Align::Center,
        (DIM, 1.0),
    );
    for (index, label) in names.iter().enumerate() {
        let center = left + slot * (index as f32 + 0.5);
        let chosen = index == choice;
        if chosen {
            canvas.panel(
                (center - slot * 0.45, top - sizes.unit * 0.2),
                (slot * 0.9, sizes.unit * 1.8),
                sizes.unit * 0.4,
                (GOLD, 0.18),
                Some((GOLD, 0.9)),
            );
        }
        let paint: Paint = if chosen { (GOLD, 1.0) } else { (DIM, 0.9) };
        canvas.text(
            Face::Title,
            label,
            (center, top),
            size,
            Align::Center,
            paint,
        );
    }
}

fn banner(kess: &Kess, canvas: &mut Canvas<'_>, sizes: &Sizes) {
    let Some(banner) = kess.fx.banner() else {
        return;
    };
    let strength = banner.strength;
    if banner.subtitle.is_empty() {
        let size = sizes.unit * 2.6 * (0.8 + 0.2 * banner.grown);
        let at = (sizes.width / 2.0, sizes.height * 0.42 - size / 2.0);
        canvas.glowing(
            Face::Title,
            &banner.title,
            at,
            size,
            (banner.color, strength),
            (banner.color, 0.5 * strength),
        );
        return;
    }
    canvas.dim(0.45 * strength);
    let size = sizes.unit * 4.2 * (0.75 + 0.25 * banner.grown);
    let at = (sizes.width / 2.0, sizes.height * 0.36 - size / 2.0);
    canvas.glowing(
        Face::Title,
        &banner.title,
        at,
        size,
        ([1.0, 0.95, 0.85], strength),
        (banner.color, 0.95 * strength),
    );
    let below = (sizes.width / 2.0, at.1 + size * 1.15);
    canvas.text(
        Face::Title,
        &capitalized(&banner.subtitle),
        below,
        sizes.unit * 1.3,
        Align::Center,
        (banner.color, strength),
    );
    let again = "press n for a rematch";
    let further = (sizes.width / 2.0, below.1 + sizes.unit * 2.0);
    canvas.text(
        Face::Mono,
        again,
        further,
        sizes.unit * 0.6,
        Align::Center,
        (DIM, strength),
    );
}

fn waiting(kess: &Kess, canvas: &mut Canvas<'_>, sizes: &Sizes) {
    let (
        Mode::Online {
            code,
            seat: Seat::Host,
            ..
        },
        false,
    ) = (&kess.mode, kess.joined)
    else {
        return;
    };
    canvas.dim(0.6);
    let center = sizes.width / 2.0;
    let code = code.to_string();
    canvas.text(
        Face::Title,
        "Share this code",
        (center, sizes.height * 0.3),
        sizes.unit * 1.1,
        Align::Center,
        (DIM, 1.0),
    );
    canvas.glowing(
        Face::Mono,
        &code,
        (center, sizes.height * 0.38),
        sizes.unit * 2.6,
        ([1.0, 0.95, 0.85], 1.0),
        (GOLD, 0.5),
    );
    let command = format!("kess join {code}");
    canvas.text(
        Face::Mono,
        &command,
        (center, sizes.height * 0.52),
        sizes.unit * 0.8,
        Align::Center,
        (GOLD, 1.0),
    );
    let net = format!(
        "waiting for your opponent · {}/{} brokers online",
        kess.brokers.len(),
        BROKERS.len()
    );
    canvas.text(
        Face::Mono,
        &net,
        (center, sizes.height * 0.6),
        sizes.unit * 0.6,
        Align::Center,
        (DIM, 1.0),
    );
}

fn lobby(kess: &Kess, canvas: &mut Canvas<'_>, sizes: &Sizes) {
    canvas.dim(0.6);
    let text = if kess.brokers.is_empty() {
        "connecting to the brokers"
    } else {
        "looking for the host"
    };
    canvas.text(
        Face::Title,
        text,
        (sizes.width / 2.0, sizes.height * 0.45),
        sizes.unit * 1.2,
        Align::Center,
        (GOLD, 1.0),
    );
}
