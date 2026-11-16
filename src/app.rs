// the cursor stays on an 8 by 8 board, so we cast its coordinates freely
#![allow(clippy::cast_possible_wrap)]

mod hud;

use std::collections::BTreeSet;
use std::f32::consts::PI;
use std::rc::Rc;
use std::sync::mpsc::{Receiver, channel};
use std::time::Instant;

use katui::game::{Key, Light, P2};
use katui::proto::input::{Button, MouseAction};
use shakmaty::uci::UciMove;
use shakmaty::{File, Move, Position, Rank, Role, Square};

use crate::art::{Art, Style};
use crate::bot;
use crate::camera::Orbit;
use crate::fx::Effects;
use crate::net::{Link, Note, Role as Seat};
use crate::runner::{Frame, Play};
use crate::sound::{Cue, Material, Speaker};
use crate::stage::{Mark, Placed, Stage};
use crate::table::{Change, Reason, Settings, Side, Snapshot, Table, TableError, Verdict};
use crate::text::{Canvas, Fonts};
use crate::wire::{Message, PROTOCOL, RoomCode};

const SLIDE_SECONDS: f32 = 0.32;
const NOTICE_SECONDS: f32 = 3.5;
const RESIGN_CONFIRM_SECONDS: f32 = 3.0;
const BOT_PAUSE_SECONDS: f32 = 0.5;
/// Mouse travel in game pixels before a press turns into a drag.
const DRAG_START: f32 = 6.0;
const PROMOTIONS: [Role; 4] = [Role::Queen, Role::Rook, Role::Bishop, Role::Knight];

const MOVE_MARK: [f32; 3] = [0.3, 0.85, 0.55];
const CAPTURE_MARK: [f32; 3] = [0.95, 0.22, 0.12];
const LAST_MARK: [f32; 3] = [1.0, 0.75, 0.25];
const PICK_MARK: [f32; 3] = [1.0, 0.85, 0.35];
const CURSOR_MARK: [f32; 3] = [0.6, 0.85, 1.0];

/// Who plays from this terminal.
pub enum Mode {
    /// Both sides take turns at one keyboard.
    Hotseat,
    /// The player at the keyboard takes `human`, and the built-in bot the other side.
    Computer { human: Side },
    Online {
        link: Rc<Link>,
        seat: Seat,
        code: RoomCode,
    },
}

struct Slide {
    side: Side,
    letter: char,
    from: [f32; 2],
    to: Square,
    age: f32,
    hop: f32,
    victim: Option<(Side, char)>,
    weight: f32,
}

struct Notice {
    text: String,
    age: f32,
}

struct Drag {
    start: P2,
    last: P2,
    turning: bool,
}

pub struct Kess {
    art: Art,
    fonts: Fonts,
    style: Style,
    stage: Option<Stage>,
    orbit: Orbit,
    mode: Mode,
    table: Option<Table>,
    settings: Settings,
    opponent: Opponent,
    last_sent: Option<Snapshot>,
    brokers: BTreeSet<&'static str>,
    started: Instant,
    cursor: (usize, usize),
    picked: Option<Square>,
    promotion: Option<(Square, Square, usize)>,
    slide: Option<Slide>,
    landing_verdict: Option<Verdict>,
    fx: Effects,
    notice: Option<Notice>,
    resign_armed: f32,
    speaker: Speaker,
    bot_pause: f32,
    bot_reply: Option<Receiver<Option<String>>>,
    drag: Option<Drag>,
    screen: (f32, f32),
    clocks_shown: String,
    dirty: bool,
    copy_code: bool,
}

/// Who sits across the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Opponent {
    Awaited,
    Seated,
    Gone,
}

impl Kess {
    #[must_use]
    pub fn new(
        art: Art,
        fonts: Fonts,
        mode: Mode,
        settings: Settings,
        style: Style,
        speaker: Speaker,
    ) -> Self {
        let orbit = art.lens.home(true);
        let mut kess = Self {
            art,
            fonts,
            style,
            stage: None,
            orbit,
            table: None,
            settings,
            opponent: Opponent::Awaited,
            last_sent: None,
            brokers: BTreeSet::new(),
            started: Instant::now(),
            cursor: (4, 1),
            picked: None,
            promotion: None,
            slide: None,
            landing_verdict: None,
            fx: Effects::default(),
            notice: None,
            resign_armed: 0.0,
            speaker,
            bot_pause: 0.0,
            bot_reply: None,
            drag: None,
            screen: (1.0, 1.0),
            clocks_shown: String::new(),
            dirty: true,
            copy_code: true,
            mode,
        };
        match &kess.mode {
            Mode::Hotseat | Mode::Computer { .. } => {
                kess.table = Some(Table::new(settings, 0));
                kess.opponent = Opponent::Seated;
            }
            Mode::Online { link, seat, .. } => {
                if *seat == Seat::Host {
                    kess.table = Some(Table::new(settings, 0));
                    let _ = link.send(&Message::Lobby {
                        protocol: PROTOCOL,
                        settings,
                    });
                }
            }
        }
        kess.face_home();
        kess.clocks_shown = kess.clock_texts();
        kess
    }

    /// Whether we host an online game nobody joined yet, with the code on screen.
    fn hosting_alone(&self) -> bool {
        matches!(
            self.mode,
            Mode::Online {
                seat: Seat::Host,
                ..
            }
        ) && !self.joined()
    }

    fn now_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// The sides this terminal moves for in the current round.
    fn local(&self) -> Vec<Side> {
        let round = self.table.as_ref().map_or(0, Table::round);
        match &self.mode {
            Mode::Hotseat => vec![Side::White, Side::Black],
            Mode::Computer { human } => vec![*human],
            Mode::Online {
                seat: Seat::Host, ..
            } => vec![self.settings.host_side(round)],
            Mode::Online {
                seat: Seat::Guest, ..
            } => vec![self.settings.host_side(round).other()],
        }
    }

    /// The side the camera looks from.
    fn view(&self) -> Side {
        if self.orbit.home.cos() > 0.0 {
            Side::White
        } else {
            Side::Black
        }
    }

    fn face_home(&mut self) {
        let side = if self.is_hotseat() {
            self.view()
        } else {
            self.local().first().copied().unwrap_or(Side::White)
        };
        self.orbit = self.art.lens.home(side == Side::White);
        self.cursor = if side == Side::White { (4, 1) } else { (4, 6) };
        self.dirty = true;
    }

    fn is_hotseat(&self) -> bool {
        matches!(self.mode, Mode::Hotseat)
    }

    fn material(&self) -> Material {
        match self.style {
            Style::Amber => Material::Resin,
            Style::Glass => Material::Glass,
            Style::Metal => Material::Metal,
        }
    }

    fn say(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            age: 0.0,
        });
        self.dirty = true;
    }

    fn pump(&mut self) {
        let Mode::Online { link, .. } = &self.mode else {
            return;
        };
        let notes: Vec<Note> = link.notes().collect();
        for note in notes {
            self.dirty = true;
            match note {
                Note::Online(broker) => {
                    self.brokers.insert(broker);
                }
                Note::Offline(broker) => {
                    self.brokers.remove(broker);
                }
                Note::Received(message) => self.receive(message),
                Note::Rejected(_) => {}
            }
        }
        self.publish();
    }

    fn receive(&mut self, message: Message) {
        let now = self.now_ms();
        let Mode::Online { seat, .. } = &self.mode else {
            return;
        };
        let seat = *seat;
        match message {
            Message::Lobby { protocol, .. } | Message::Join { protocol }
                if protocol != PROTOCOL =>
            {
                self.say("the other player runs another kess version");
            }
            Message::Lobby { settings, .. } if seat == Seat::Guest && self.table.is_none() => {
                self.settings = settings;
                self.table = Some(Table::new(settings, now));
                self.opponent = Opponent::Seated;
                self.face_home();
                self.send(&Message::Join { protocol: PROTOCOL });
                self.say("you joined the game");
                self.speaker.play(Cue::Notice);
            }
            Message::Join { .. } if seat == Seat::Host => {
                if !self.joined() {
                    self.say("your opponent is here");
                    self.speaker.play(Cue::Notice);
                }
                self.opponent = Opponent::Seated;
                self.last_sent = None;
            }
            Message::Table { settings, snapshot } => self.receive_table(settings, &snapshot, now),
            Message::Leave if self.opponent == Opponent::Seated => {
                self.opponent = Opponent::Gone;
                self.picked = None;
                self.promotion = None;
                self.resign_armed = 0.0;
                self.speaker.play(Cue::Notice);
            }
            _ => {}
        }
    }

    fn receive_table(&mut self, settings: Settings, snapshot: &Snapshot, now: u64) {
        let them = self
            .local()
            .first()
            .map_or(Side::Black, |side| side.other());
        let fresh = self
            .table
            .as_ref()
            .is_none_or(|table| table.record().is_empty());
        if fresh && snapshot.moves.len() > 1 {
            match Table::restore(settings, snapshot, now) {
                Ok(table) => {
                    self.settings = settings;
                    self.table = Some(table);
                    self.opponent = Opponent::Seated;
                    self.face_home();
                    self.say("you rejoined the game");
                }
                Err(error) => self.refuse(&error),
            }
            return;
        }
        let Some(table) = self.table.as_mut() else {
            return;
        };
        self.opponent = Opponent::Seated;
        match table.merge(snapshot, them, now) {
            Ok(changes) => {
                for change in changes {
                    self.react(change);
                }
            }
            Err(error) => self.refuse(&error),
        }
    }

    fn refuse(&mut self, error: &TableError) {
        let text = match error {
            TableError::Desync => "the boards disagree, rejoin to resync",
            TableError::Cheat(_) | TableError::Rule(_) => "refused an illegal move",
            TableError::NotYourTurn | TableError::Over => return,
        };
        self.say(text);
    }

    fn send(&self, message: &Message) {
        if let Mode::Online { link, .. } = &self.mode {
            let _ = link.send(message);
        }
    }

    fn publish(&mut self) {
        let (Some(table), true) = (&self.table, self.joined()) else {
            return;
        };
        if !matches!(self.mode, Mode::Online { .. })
            || self.last_sent.as_ref() == Some(table.snapshot())
        {
            return;
        }
        let snapshot = table.snapshot().clone();
        self.send(&Message::Table {
            settings: self.settings,
            snapshot: snapshot.clone(),
        });
        self.last_sent = Some(snapshot);
    }

    /// Words, sounds and effects for something that happened on the table.
    fn react(&mut self, change: Change) {
        self.dirty = true;
        match change {
            Change::Moved(chosen) => {
                self.start_slide(chosen);
                self.landing_verdict = self.table.as_ref().and_then(Table::verdict);
            }
            Change::DrawOffered(side) if !self.local().contains(&side) || self.is_hotseat() => {
                self.say(format!("{} offers a draw, press d to accept", name(side)));
                self.speaker.play(Cue::Notice);
            }
            Change::RematchAsked(side) if !self.local().contains(&side) => {
                self.say("your opponent wants a rematch, press n to accept");
                self.speaker.play(Cue::Notice);
            }
            Change::Ended(verdict) if self.slide.is_none() => self.ended(verdict),
            Change::Ended(verdict) => self.landing_verdict = Some(verdict),
            Change::NewRound => {
                self.picked = None;
                self.slide = None;
                self.fx.new_round();
                self.face_home();
                self.say("new game, sides swapped");
            }
            Change::DrawOffered(_) | Change::RematchAsked(_) => {}
        }
    }

    fn ended(&mut self, verdict: Verdict) {
        let (title, subtitle) = verdict_text(verdict);
        let decisive = verdict.winner.is_some();
        self.fx.end(title, &subtitle, decisive);
        self.speaker
            .play(if decisive { Cue::Mate } else { Cue::Notice });
    }

    fn start_slide(&mut self, chosen: Move) {
        let Some(from) = chosen.from() else {
            return;
        };
        let side = self
            .table
            .as_ref()
            .map_or(Side::White, |table| table.turn().other());
        let weight = weight(chosen.role());
        self.slide = Some(Slide {
            side,
            letter: chosen.promotion().unwrap_or(chosen.role()).char(),
            from: center(from),
            to: landing(chosen),
            age: 0.0,
            hop: if chosen.role() == Role::Knight {
                0.8
            } else {
                0.25
            },
            victim: chosen.capture().map(|role| (side.other(), role.char())),
            weight,
        });
        self.speaker.play(Cue::Lift {
            material: self.material(),
            weight,
        });
    }

    /// The moving piece touched down: the hit, then check or the end.
    fn land(&mut self, slide: &Slide) {
        let at = center(slide.to);
        let pan = at[0] / 4.0 - 1.0;
        let material = self.material();
        if let Some(victim) = slide.victim {
            self.fx.captured(at, victim, self.style);
            self.speaker.play(Cue::Capture { material, pan });
        } else {
            self.fx.landed(at);
            self.speaker.play(Cue::Place {
                material,
                weight: slide.weight,
                pan,
            });
        }
        if let Some(verdict) = self.landing_verdict.take() {
            self.ended(verdict);
        } else if let Some(king) = self.checked_king() {
            self.fx.check(center(king));
            self.speaker.play(Cue::Check);
        }
    }

    fn checked_king(&self) -> Option<Square> {
        let position = self.table.as_ref()?.record().position();
        position
            .is_check()
            .then(|| position.board().king_of(position.turn()))
            .flatten()
    }

    /// Start the bot thinking on its own thread once its pause is over.
    fn bot_turn(&mut self, dt: f32) {
        let Mode::Computer { human } = self.mode else {
            return;
        };
        if let Some(reply) = &self.bot_reply {
            if let Ok(uci) = reply.try_recv() {
                self.bot_reply = None;
                let now = self.now_ms();
                let change =
                    uci.and_then(|uci| self.table.as_mut()?.play(human.other(), &uci, now).ok());
                if let Some(change) = change {
                    self.react(change);
                }
            }
            return;
        }
        let Some(table) = &self.table else {
            return;
        };
        if table.is_over() || table.turn() == human || self.slide.is_some() {
            self.bot_pause = 0.0;
            return;
        }
        self.bot_pause += dt;
        if self.bot_pause < BOT_PAUSE_SECONDS {
            return;
        }
        self.bot_pause = 0.0;
        let position = table.record().position().clone();
        let seed = self.now_ms() as u32;
        let (answer, reply) = channel();
        std::thread::spawn(move || {
            let _ = answer.send(bot::choose(&position, seed));
        });
        self.bot_reply = Some(reply);
    }

    fn act(&mut self, key: Key) {
        self.dirty = true;
        if let Some(direction) = direction(key) {
            self.aim(direction);
            return;
        }
        match key {
            Key::Enter | Key::Char(' ') => self.choose(),
            Key::Escape | Key::Backspace | Key::Delete => {
                self.picked = None;
                self.promotion = None;
            }
            Key::Char('s') => {
                self.style = self.style.next();
                self.say(format!("{} set", self.style.name()));
            }
            Key::Char('f') if self.is_hotseat() => {
                let white = self.view() == Side::Black;
                self.orbit = self.art.lens.home(white);
            }
            Key::Char('c') => self.orbit = self.art.lens.home(self.view() == Side::White),
            Key::Char('m') => {
                self.speaker.muted = !self.speaker.muted;
                self.say(if self.speaker.muted {
                    "sound off"
                } else {
                    "sound on"
                });
            }
            _ => self.decide(key),
        }
    }

    /// The keys that change the course of the game.
    fn decide(&mut self, key: Key) {
        match key {
            Key::Char('y') if self.hosting_alone() => self.copy_code = true,
            Key::Char('r') => self.arm_resign(),
            Key::Char('y') if self.resign_armed > 0.0 => self.resign(),
            Key::Char('d') => self.offer_draw(),
            Key::Char('n') => self.rematch(),
            _ => {}
        }
    }

    fn aim(&mut self, key: Key) {
        if let Some((from, to, choice)) = self.promotion {
            let step = if key == Key::Left {
                PROMOTIONS.len() - 1
            } else {
                1
            };
            self.promotion = Some((from, to, (choice + step) % PROMOTIONS.len()));
            return;
        }
        let (mut file, mut rank) = (self.cursor.0 as isize, self.cursor.1 as isize);
        let flip = if self.view() == Side::White { 1 } else { -1 };
        match key {
            Key::Up => rank += flip,
            Key::Down => rank -= flip,
            Key::Left => file -= flip,
            _ => file += flip,
        }
        self.cursor = (file.clamp(0, 7) as usize, rank.clamp(0, 7) as usize);
    }

    fn mouse(&mut self, action: MouseAction, at: P2) {
        match action {
            MouseAction::Press(Button::Left | Button::Right) => {
                self.drag = Some(Drag {
                    start: at,
                    last: at,
                    turning: false,
                });
            }
            MouseAction::Drag(_) => self.turn_board(at),
            MouseAction::Release(button) => {
                self.dirty = true;
                let clicked = self.drag.take().is_some_and(|drag| !drag.turning);
                if clicked && button == Button::Left {
                    self.click(at);
                }
            }
            MouseAction::Move => self.hover(at),
            MouseAction::WheelUp => self.zoom(0.95),
            MouseAction::WheelDown => self.zoom(1.05),
            MouseAction::Press(_) | MouseAction::WheelLeft | MouseAction::WheelRight => {}
        }
    }

    fn turn_board(&mut self, at: P2) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        let moved = (at.x - drag.start.x).hypot(at.y - drag.start.y);
        drag.turning |= moved > DRAG_START;
        if drag.turning {
            let (width, height) = self.screen;
            let yaw = (drag.last.x - at.x) / width * 2.2;
            let pitch = (at.y - drag.last.y) / height * 1.4;
            self.orbit = self.orbit.swung(yaw, pitch);
            self.dirty = true;
        }
        drag.last = at;
    }

    fn busy_turning(&self) -> bool {
        self.drag.as_ref().is_some_and(|drag| drag.turning)
    }

    fn zoom(&mut self, factor: f32) {
        self.orbit = self.orbit.zoomed(factor);
        self.dirty = true;
    }

    fn hover(&mut self, at: P2) {
        let square = self.stage.as_ref().and_then(|stage| stage.square_at(at));
        if let Some(square) = square.filter(|square| *square != self.cursor) {
            self.cursor = square;
            self.dirty = true;
        }
    }

    fn click(&mut self, at: P2) {
        if self.promotion.is_some() {
            self.choose();
            return;
        }
        let Some(stage) = &self.stage else {
            return;
        };
        // a piece in front can cover a lit target square, so a target under the click wins
        let floor = stage.board_square_at(at).filter(|(file, rank)| {
            self.picked.is_some_and(|from| {
                self.targets(from)
                    .iter()
                    .any(|(to, _)| *to == square(*file, *rank))
            })
        });
        if let Some(square) = floor.or_else(|| stage.square_at(at)) {
            self.cursor = square;
            self.choose();
        } else {
            self.picked = None;
        }
        self.dirty = true;
    }

    fn cursor_square(&self) -> Square {
        square(self.cursor.0, self.cursor.1)
    }

    fn my_turn(&self) -> bool {
        let Some(table) = &self.table else {
            return false;
        };
        self.in_play() && !table.is_over() && self.local().contains(&table.turn())
    }

    /// Whether both players are at the table.
    fn in_play(&self) -> bool {
        self.opponent == Opponent::Seated
    }

    /// Whether the game ever started: the opponent sat down, even if they left since.
    fn joined(&self) -> bool {
        self.opponent != Opponent::Awaited
    }

    fn choose(&mut self) {
        if let Some((from, to, choice)) = self.promotion.take() {
            let promote = PROMOTIONS.get(choice).copied().unwrap_or(Role::Queen);
            self.play(from, to, Some(promote));
            return;
        }
        let Some(table) = &self.table else {
            return;
        };
        let target = self.cursor_square();
        let piece = table.record().position().board().piece_at(target);
        let own = piece.is_some_and(|piece| Side::of(piece.color) == table.turn());
        if !self.my_turn() {
            return;
        }
        match self.picked {
            Some(from) if from == target => self.picked = None,
            _ if own => {
                self.picked = Some(target);
                if let Some(piece) = piece {
                    self.speaker.play(Cue::Lift {
                        material: self.material(),
                        weight: weight(piece.role) * 0.5,
                    });
                }
            }
            Some(from) => {
                let targets = self.targets(from);
                let reachable = targets.iter().any(|(to, _)| *to == target);
                let promotes = targets
                    .iter()
                    .any(|(to, promotion)| *to == target && promotion.is_some());
                if promotes {
                    self.promotion = Some((from, target, 0));
                } else if reachable {
                    self.play(from, target, None);
                } else {
                    self.picked = None;
                }
            }
            None => {}
        }
    }

    /// Where the piece on `from` can go, with the promotion each move needs.
    fn targets(&self, from: Square) -> Vec<(Square, Option<Role>)> {
        let Some(table) = &self.table else {
            return Vec::new();
        };
        table
            .record()
            .position()
            .legal_moves()
            .iter()
            .filter(|chosen| chosen.from() == Some(from))
            .map(|chosen| (landing(*chosen), chosen.promotion()))
            .collect()
    }

    fn play(&mut self, from: Square, to: Square, promotion: Option<Role>) {
        let now = self.now_ms();
        let uci = UciMove::Normal {
            from,
            to,
            promotion,
        }
        .to_string();
        self.picked = None;
        let Some(table) = self.table.as_mut() else {
            return;
        };
        let side = table.turn();
        if let Ok(change) = table.play(side, &uci, now) {
            self.react(change);
        }
    }

    fn arm_resign(&mut self) {
        if self.table.as_ref().is_none_or(Table::is_over) || !self.in_play() {
            return;
        }
        self.resign_armed = RESIGN_CONFIRM_SECONDS;
        self.say("press y to resign");
    }

    fn resign(&mut self) {
        self.resign_armed = 0.0;
        let side = self.acting_side();
        if let Some(change) = self
            .table
            .as_mut()
            .and_then(|table| table.resign(side).ok())
        {
            self.react(change);
        }
    }

    /// The side a resignation or draw offer speaks for.
    fn acting_side(&self) -> Side {
        match (&self.mode, &self.table) {
            (Mode::Hotseat, Some(table)) => table.turn(),
            _ => self.local().first().copied().unwrap_or(Side::White),
        }
    }

    fn offer_draw(&mut self) {
        if !self.in_play() {
            return;
        }
        let side = self.acting_side();
        let Some(table) = self.table.as_mut() else {
            return;
        };
        match table.offer_draw(side) {
            Ok(Change::DrawOffered(_)) => self.say("you offered a draw"),
            Ok(change) => self.react(change),
            Err(_) => {}
        }
    }

    fn rematch(&mut self) {
        if !self.in_play() {
            return;
        }
        let now = self.now_ms();
        let sides = match self.mode {
            Mode::Computer { .. } => vec![Side::White, Side::Black],
            _ => self.local(),
        };
        let Some(table) = self.table.as_mut() else {
            return;
        };
        let mut last = None;
        for side in sides {
            last = table.ask_rematch(side, now).ok();
        }
        match last {
            Some(Change::RematchAsked(_)) => self.say("you asked for a rematch"),
            Some(change) => self.react(change),
            None => {}
        }
    }

    fn pieces(&self) -> Vec<Placed> {
        let Some(table) = &self.table else {
            return Vec::new();
        };
        let sliding = self.slide.as_ref().map(|slide| slide.to);
        let mated = table
            .verdict()
            .filter(|verdict| {
                matches!(
                    verdict.reason,
                    Reason::Board(crate::rules::Ending::Checkmate(_))
                )
            })
            .and(self.checked_king());
        let mut placed: Vec<Placed> = table
            .record()
            .position()
            .board()
            .iter()
            .filter(|(at, _)| Some(*at) != sliding)
            .map(|(at, piece)| Placed {
                side: Side::of(piece.color),
                letter: piece.role.char(),
                at: center(at),
                lift: if self.picked == Some(at) { 0.18 } else { 0.0 },
                opacity: 1.0,
                tint: if Some(at) == mated && self.slide.is_none() {
                    [0.5, 0.02, 0.0]
                } else {
                    [0.0; 3]
                },
            })
            .collect();
        if let Some(slide) = &self.slide {
            let t = (slide.age / SLIDE_SECONDS).min(1.0);
            let eased = t * t * (3.0 - 2.0 * t);
            let to = center(slide.to);
            placed.push(Placed {
                side: slide.side,
                letter: slide.letter,
                at: [
                    slide.from[0] + (to[0] - slide.from[0]) * eased,
                    slide.from[1] + (to[1] - slide.from[1]) * eased,
                ],
                lift: (PI * t).sin() * slide.hop,
                opacity: 1.0,
                tint: [0.0; 3],
            });
        }
        placed.extend(self.fx.ghosts());
        placed
    }

    fn marks(&self) -> Vec<Mark> {
        let mut marks = Vec::new();
        let mark = |at: Square, color: [f32; 3], strength: f32| Mark {
            file: at.file() as usize,
            rank: at.rank() as usize,
            color,
            strength,
        };
        if let Some(last) = self
            .table
            .as_ref()
            .and_then(|table| table.record().last_move())
        {
            if let Some(from) = last.from() {
                marks.push(mark(from, LAST_MARK, 0.25));
            }
            marks.push(mark(landing(last), LAST_MARK, 0.35));
        }
        if let Some(king) = self.checked_king() {
            marks.push(mark(king, CAPTURE_MARK, 0.55));
        }
        if let Some(picked) = self.picked {
            marks.push(mark(picked, PICK_MARK, 0.6));
            let board = self
                .table
                .as_ref()
                .map(|table| table.record().position().board().clone());
            for (to, _) in self.targets(picked) {
                let taken = board
                    .as_ref()
                    .is_some_and(|board| board.piece_at(to).is_some());
                marks.push(if taken {
                    mark(to, CAPTURE_MARK, 0.6)
                } else {
                    mark(to, MOVE_MARK, 0.5)
                });
            }
        }
        marks.push(mark(self.cursor_square(), CURSOR_MARK, 0.3));
        marks
    }

    fn clock_texts(&self) -> String {
        let Some(table) = &self.table else {
            return String::new();
        };
        let now = self.now_ms();
        [Side::White, Side::Black]
            .iter()
            .filter_map(|side| table.remaining(*side, now).map(clock_text))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

impl Play for Kess {
    fn key(&mut self, key: Key) {
        self.act(key);
    }

    fn pointer(&mut self, action: MouseAction, at: P2) {
        self.mouse(action, at);
    }

    fn step(&mut self, dt: f32) {
        self.resign_armed = (self.resign_armed - dt).max(0.0);
        self.pump();
        let now = self.now_ms();
        let local = self.local();
        if let Some(change) = self
            .table
            .as_mut()
            .and_then(|table| table.tick(&local, now))
        {
            self.react(change);
        }
        self.bot_turn(dt);
        if let Some(slide) = self.slide.as_mut() {
            slide.age += dt;
            let t = (slide.age / SLIDE_SECONDS).min(1.0);
            let (from, to) = (slide.from, center(slide.to));
            let at = [
                from[0] + (to[0] - from[0]) * t,
                from[1] + (to[1] - from[1]) * t,
            ];
            let lift = (PI * t).sin() * slide.hop;
            self.fx.trail(at, lift, self.style);
        }
        if let Some(slide) = self.slide.take_if(|slide| slide.age >= SLIDE_SECONDS) {
            self.land(&slide);
        }
        if let Some(notice) = self.notice.as_mut() {
            notice.age += dt;
        }
        if self
            .notice
            .take_if(|notice| notice.age > NOTICE_SECONDS)
            .is_some()
        {
            self.dirty = true;
        }
        self.fx.step(dt);
        let clocks = self.clock_texts();
        if clocks != self.clocks_shown {
            self.clocks_shown = clocks;
            self.dirty = true;
        }
        self.publish();
    }

    fn busy(&self) -> bool {
        self.slide.is_some() || self.fx.busy() || self.busy_turning()
    }

    fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    fn take_clipboard(&mut self) -> Option<String> {
        let Mode::Online { code, .. } = &self.mode else {
            return None;
        };
        (self.hosting_alone() && std::mem::take(&mut self.copy_code)).then(|| code.to_string())
    }

    fn render(&mut self, light: &mut Light) {
        self.screen = (light.width() as f32, light.height() as f32);
        let draft = self.busy_turning();
        let mut stage = match self.stage.take() {
            Some(stage) if stage.fits(light, self.style, self.orbit, draft) => stage,
            Some(stage) if stage.style() == self.style => {
                stage.turned(&self.art, self.orbit, light, draft)
            }
            _ => Stage::new(&self.art, self.style, self.orbit, light, draft),
        };
        stage.compose(&self.art, &self.pieces(), &self.marks());
        stage.paint(light);
        self.fx.draw(light, &stage);
        light.bloom(5.0, 0.4);
        self.stage = Some(stage);
    }

    fn overlay(&mut self, frame: Frame<'_>) {
        let mut canvas = Canvas {
            pixels: frame.pixels,
            width: frame.width,
            height: frame.height,
            aspect: frame.aspect,
            fonts: &self.fonts,
        };
        hud::draw(self, &mut canvas);
    }
}

/// The arrow `key` stands for, with hjkl as in vim.
fn direction(key: Key) -> Option<Key> {
    match key {
        Key::Up | Key::Down | Key::Left | Key::Right => Some(key),
        Key::Char('k') => Some(Key::Up),
        Key::Char('j') => Some(Key::Down),
        Key::Char('h') => Some(Key::Left),
        Key::Char('l') => Some(Key::Right),
        _ => None,
    }
}

/// How heavy a piece sounds, 0 for a pawn to 1 for the king.
fn weight(role: Role) -> f32 {
    match role {
        Role::Pawn => 0.0,
        Role::Knight | Role::Bishop => 0.35,
        Role::Rook => 0.55,
        Role::Queen => 0.8,
        Role::King => 1.0,
    }
}

fn name(side: Side) -> &'static str {
    match side {
        Side::White => "white",
        Side::Black => "black",
    }
}

/// The end banner's title and subtitle.
fn verdict_text(verdict: Verdict) -> (&'static str, String) {
    use crate::rules::Ending;
    let title = match verdict.reason {
        Reason::Board(Ending::Checkmate(_)) => "CHECKMATE",
        Reason::Board(Ending::Stalemate) => "STALEMATE",
        Reason::Board(Ending::InsufficientMaterial) => "DEAD POSITION",
        Reason::Board(Ending::Repetition) => "REPETITION",
        Reason::Board(Ending::SeventyFiveMoves) => "75 MOVE RULE",
        Reason::Resignation => "RESIGNED",
        Reason::Timeout => "TIME OUT",
        Reason::Agreement => "DRAW AGREED",
    };
    let subtitle = match verdict.winner {
        Some(side) => format!("{} wins", name(side)),
        None => "the game is drawn".to_owned(),
    };
    (title, subtitle)
}

fn clock_text(left: i64) -> String {
    let left = left.max(0);
    if left < 10_000 {
        format!("{}.{}", left / 1_000, left % 1_000 / 100)
    } else {
        format!("{}:{:02}", left / 60_000, left % 60_000 / 1_000)
    }
}

fn square(file: usize, rank: usize) -> Square {
    Square::from_coords(File::new(file as u32), Rank::new(rank as u32))
}

fn center(at: Square) -> [f32; 2] {
    [
        at.file() as usize as f32 + 0.5,
        at.rank() as usize as f32 + 0.5,
    ]
}

/// Where the moving piece ends up, the king's square for castling.
fn landing(chosen: Move) -> Square {
    UciMove::from_standard(chosen).to().unwrap_or(chosen.to())
}

#[cfg(test)]
mod tests;
