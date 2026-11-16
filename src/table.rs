use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use shakmaty::{Color, Move};

use crate::rules::{Ending, Record, RuleError};

/// How much less time a player may report for a move than the opponent saw
/// pass, which covers the trip through the broker both ways.
pub const LAG_ALLOWANCE_MS: u64 = 3_000;
/// How far past zero we let our own clock run before we accept that the
/// opponent saw it fall.
pub const FLAG_GRACE_MS: i64 = 2_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    White,
    Black,
}

impl Side {
    #[must_use]
    pub fn other(self) -> Self {
        match self {
            Self::White => Self::Black,
            Self::Black => Self::White,
        }
    }

    #[must_use]
    pub fn of(color: Color) -> Self {
        match color {
            Color::White => Self::White,
            Color::Black => Self::Black,
        }
    }

    fn moved(self, ply: usize) -> bool {
        ply.is_multiple_of(2) == (self == Self::White)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeControl {
    pub base_ms: u64,
    pub increment_ms: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
#[error("write a time control as minutes+seconds, like 5+3")]
pub struct TimeControlError;

impl FromStr for TimeControl {
    type Err = TimeControlError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (minutes, seconds) = text.split_once('+').unwrap_or((text, "0"));
        let minutes: f64 = minutes.trim().parse().map_err(|_| TimeControlError)?;
        let seconds: u64 = seconds.trim().parse().map_err(|_| TimeControlError)?;
        if !(minutes > 0.0 && minutes <= 180.0) || seconds > 180 {
            return Err(TimeControlError);
        }
        Ok(Self {
            base_ms: (minutes * 60_000.0) as u64,
            increment_ms: seconds * 1_000,
        })
    }
}

impl fmt::Display for TimeControl {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let minutes = self.base_ms as f64 / 60_000.0;
        write!(out, "{minutes}+{}", self.increment_ms / 1_000)
    }
}

/// What both players agree on before the first move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// The host's side in the first round; rematches swap sides.
    pub host: Side,
    pub control: Option<TimeControl>,
}

impl Settings {
    #[must_use]
    pub fn host_side(&self, round: u32) -> Side {
        if round.is_multiple_of(2) {
            self.host
        } else {
            self.host.other()
        }
    }
}

/// The whole state of a round as both players send it to each other. Each
/// player only ever changes the facts that belong to their own side.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Snapshot {
    pub round: u32,
    pub moves: Vec<String>,
    /// Milliseconds each move took, by ply.
    pub thinks: Vec<u64>,
    pub fingerprint: u64,
    pub resigned: Option<Side>,
    pub flagged: Option<Side>,
    /// Who offered a draw, and the ply they offered it at.
    pub draw_offer: Option<(Side, usize)>,
    pub drawn: bool,
    pub rematch: Vec<Side>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    Board(Ending),
    Resignation,
    Timeout,
    Agreement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Verdict {
    pub winner: Option<Side>,
    pub reason: Reason,
}

/// What a change did, so the screen can animate it and play a sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Moved(Move),
    DrawOffered(Side),
    RematchAsked(Side),
    Ended(Verdict),
    NewRound,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TableError {
    #[error(transparent)]
    Rule(#[from] RuleError),
    #[error("it is not that side's turn")]
    NotYourTurn,
    #[error("the game is over")]
    Over,
    #[error("the boards disagree")]
    Desync,
    #[error("the opponent sent {0}")]
    Cheat(&'static str),
}

#[derive(Debug, Clone)]
pub struct Table {
    settings: Settings,
    snap: Snapshot,
    record: Record,
    turn_started_ms: u64,
}

impl Table {
    #[must_use]
    pub fn new(settings: Settings, now_ms: u64) -> Self {
        let record = Record::new();
        Self {
            settings,
            snap: Snapshot {
                fingerprint: record.fingerprint(),
                ..Snapshot::default()
            },
            record,
            turn_started_ms: now_ms,
        }
    }

    /// Rebuild a game we joined late from the opponent's snapshot, checking
    /// every move in it against the rules.
    pub fn restore(settings: Settings, remote: &Snapshot, now_ms: u64) -> Result<Self, TableError> {
        let mut table = Self::new(settings, now_ms);
        for (ply, uci) in remote.moves.iter().enumerate() {
            let think = remote.thinks.get(ply).copied().unwrap_or_default();
            table.play_timed(table.turn(), uci, think, now_ms)?;
        }
        if table.snap.fingerprint != remote.fingerprint {
            return Err(TableError::Desync);
        }
        table.snap = Snapshot {
            fingerprint: table.snap.fingerprint,
            moves: table.snap.moves,
            ..remote.clone()
        };
        Ok(table)
    }

    #[must_use]
    pub fn settings(&self) -> Settings {
        self.settings
    }

    #[must_use]
    pub fn snapshot(&self) -> &Snapshot {
        &self.snap
    }

    #[must_use]
    pub fn record(&self) -> &Record {
        &self.record
    }

    #[must_use]
    pub fn round(&self) -> u32 {
        self.snap.round
    }

    #[must_use]
    pub fn turn(&self) -> Side {
        Side::of(self.record.turn())
    }

    #[must_use]
    pub fn verdict(&self) -> Option<Verdict> {
        let decided = |winner, reason| Some(Verdict { winner, reason });
        if let Some(side) = self.snap.resigned {
            return decided(Some(side.other()), Reason::Resignation);
        }
        if let Some(side) = self.snap.flagged {
            return decided(Some(side.other()), Reason::Timeout);
        }
        if self.snap.drawn {
            return decided(None, Reason::Agreement);
        }
        let ending = self.record.ending()?;
        let winner = match ending {
            Ending::Checkmate(color) => Some(Side::of(color)),
            _ => None,
        };
        decided(winner, Reason::Board(ending))
    }

    #[must_use]
    pub fn is_over(&self) -> bool {
        self.verdict().is_some()
    }

    #[must_use]
    pub fn draw_offered_by(&self) -> Option<Side> {
        self.snap
            .draw_offer
            .filter(|(_, ply)| *ply == self.record.len())
            .map(|(side, _)| side)
    }

    /// Milliseconds `side` has left at `now_ms`, below zero once it ran out.
    #[must_use]
    pub fn remaining(&self, side: Side, now_ms: u64) -> Option<i64> {
        let control = self.settings.control?;
        let plies = self.snap.thinks.iter().enumerate();
        let mine = plies.filter(|(ply, _)| side.moved(*ply));
        let (moves, spent) = mine.fold((0, 0), |(moves, spent), (_, think)| {
            (moves + 1, spent + think)
        });
        let running = if self.clock_runs() && self.turn() == side {
            now_ms.saturating_sub(self.turn_started_ms)
        } else {
            0
        };
        let earned = control.base_ms + control.increment_ms * moves;
        let signed = |ms: u64| i64::try_from(ms).unwrap_or(i64::MAX);
        Some(signed(earned) - signed(spent + running))
    }

    /// Clocks start once both sides made their first move, like on lichess.
    fn clock_runs(&self) -> bool {
        self.record.len() >= 2 && !self.is_over()
    }

    pub fn play(&mut self, side: Side, uci: &str, now_ms: u64) -> Result<Change, TableError> {
        let think = now_ms.saturating_sub(self.turn_started_ms);
        self.play_timed(side, uci, think, now_ms)
    }

    fn play_timed(
        &mut self,
        side: Side,
        uci: &str,
        think: u64,
        now_ms: u64,
    ) -> Result<Change, TableError> {
        if self.is_over() {
            return Err(TableError::Over);
        }
        if self.turn() != side {
            return Err(TableError::NotYourTurn);
        }
        let charged = if self.record.len() < 2 { 0 } else { think };
        let chosen = self.record.play(uci)?;
        self.snap.moves.push(uci.to_owned());
        self.snap.thinks.push(charged);
        self.snap.fingerprint = self.record.fingerprint();
        self.turn_started_ms = now_ms;
        Ok(Change::Moved(chosen))
    }

    pub fn resign(&mut self, side: Side) -> Result<Change, TableError> {
        self.end(|snap| snap.resigned = Some(side))
    }

    pub fn offer_draw(&mut self, side: Side) -> Result<Change, TableError> {
        if self.draw_offered_by() == Some(side.other()) {
            return self.end(|snap| snap.drawn = true);
        }
        if self.is_over() {
            return Err(TableError::Over);
        }
        self.snap.draw_offer = Some((side, self.record.len()));
        Ok(Change::DrawOffered(side))
    }

    pub fn ask_rematch(&mut self, side: Side, now_ms: u64) -> Result<Change, TableError> {
        if !self.is_over() {
            return Err(TableError::NotYourTurn);
        }
        if !self.snap.rematch.contains(&side) {
            self.snap.rematch.push(side);
        }
        if self.snap.rematch.contains(&side.other()) {
            self.next_round(now_ms);
            return Ok(Change::NewRound);
        }
        Ok(Change::RematchAsked(side))
    }

    /// Flag whichever of `local` sides ran out of time.
    pub fn tick(&mut self, local: &[Side], now_ms: u64) -> Option<Change> {
        let turn = self.turn();
        let out = self.remaining(turn, now_ms).is_some_and(|left| left <= 0);
        if out && local.contains(&turn) && self.clock_runs() {
            return self.end(|snap| snap.flagged = Some(turn)).ok();
        }
        None
    }

    fn end(&mut self, decide: impl FnOnce(&mut Snapshot)) -> Result<Change, TableError> {
        if self.is_over() {
            return Err(TableError::Over);
        }
        decide(&mut self.snap);
        self.verdict().map(Change::Ended).ok_or(TableError::Over)
    }

    fn next_round(&mut self, now_ms: u64) {
        let round = self.snap.round + 1;
        *self = Self::new(self.settings, now_ms);
        self.snap.round = round;
    }

    /// Take the facts `them` owns from the opponent's `remote` snapshot,
    /// checking every one of them against our own copy of the rules.
    pub fn merge(
        &mut self,
        remote: &Snapshot,
        them: Side,
        now_ms: u64,
    ) -> Result<Vec<Change>, TableError> {
        let mut changes = Vec::new();
        if remote.round < self.snap.round {
            return Ok(changes);
        }
        if remote.round == self.snap.round + 1 {
            if !self.snap.rematch.contains(&them.other()) {
                return Err(TableError::Cheat("a rematch nobody asked for"));
            }
            self.next_round(now_ms);
            changes.push(Change::NewRound);
        } else if remote.round != self.snap.round {
            return Err(TableError::Desync);
        }
        self.merge_moves(remote, them, now_ms, &mut changes)?;
        self.merge_claims(remote, them, now_ms, &mut changes);
        Ok(changes)
    }

    fn merge_moves(
        &mut self,
        remote: &Snapshot,
        them: Side,
        now_ms: u64,
        changes: &mut Vec<Change>,
    ) -> Result<(), TableError> {
        let local = self.snap.moves.len();
        let shared = local.min(remote.moves.len());
        if remote.moves.get(..shared) != self.snap.moves.get(..shared) {
            return Err(TableError::Desync);
        }
        for (mine, theirs) in self.snap.thinks.iter_mut().zip(&remote.thinks) {
            *mine = (*mine).max(*theirs);
        }
        match remote.moves.len().checked_sub(local) {
            Some(0) if remote.fingerprint != self.snap.fingerprint => Err(TableError::Desync),
            None | Some(0) => Ok(()),
            Some(1) if self.is_over() => Ok(()),
            Some(1) => {
                if self.turn() != them {
                    return Err(TableError::Cheat("a move out of turn"));
                }
                let uci = remote.moves.get(local).ok_or(TableError::Desync)?;
                let claimed = remote.thinks.get(local).copied().unwrap_or_default();
                let seen = now_ms.saturating_sub(self.turn_started_ms);
                let think = claimed.max(seen.saturating_sub(LAG_ALLOWANCE_MS));
                changes.push(self.play_timed(them, uci, think, now_ms)?);
                if remote.fingerprint != self.snap.fingerprint {
                    return Err(TableError::Desync);
                }
                Ok(())
            }
            Some(_) => Err(TableError::Cheat("several moves at once")),
        }
    }

    fn merge_claims(
        &mut self,
        remote: &Snapshot,
        them: Side,
        now_ms: u64,
        changes: &mut Vec<Change>,
    ) {
        let us = them.other();
        let ran_out = self
            .remaining(us, now_ms)
            .is_some_and(|left| left <= FLAG_GRACE_MS);
        let ended = if remote.resigned == Some(them) {
            self.end(|snap| snap.resigned = Some(them))
        } else if remote.flagged == Some(them) || (remote.flagged == Some(us) && ran_out) {
            let flagged = remote.flagged;
            self.end(|snap| snap.flagged = flagged)
        } else if remote.drawn && self.draw_offered_by() == Some(us) {
            self.end(|snap| snap.drawn = true)
        } else {
            Err(TableError::Over)
        };
        changes.extend(ended.ok());
        let offer = remote.draw_offer.filter(|(side, _)| *side == them);
        if offer.is_some_and(|(_, ply)| ply == self.record.len()) && self.snap.draw_offer != offer {
            self.snap.draw_offer = offer;
            changes.push(Change::DrawOffered(them));
        }
        if remote.rematch.contains(&them) && self.is_over() && !self.snap.rematch.contains(&them) {
            self.snap.rematch.push(them);
            changes.push(Change::RematchAsked(them));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UNTIMED: Settings = Settings {
        host: Side::White,
        control: None,
    };

    fn blitz() -> Settings {
        Settings {
            host: Side::White,
            control: Some("1+2".parse().unwrap()),
        }
    }

    /// Two tables that only talk through snapshots, like two players.
    fn pair(settings: Settings) -> (Table, Table) {
        (Table::new(settings, 0), Table::new(settings, 0))
    }

    fn send(from: &Table, to: &mut Table, them: Side, now: u64) -> Vec<Change> {
        to.merge(from.snapshot(), them, now).unwrap()
    }

    #[test]
    fn time_controls_parse_minutes_and_seconds() {
        let control: TimeControl = "5+3".parse().unwrap();
        assert_eq!((control.base_ms, control.increment_ms), (300_000, 3_000));
        assert_eq!("0.5".parse::<TimeControl>().map(|c| c.base_ms), Ok(30_000));
        assert_eq!(control.to_string(), "5+3");
        assert!("x+1".parse::<TimeControl>().is_err());
        assert!("0+1".parse::<TimeControl>().is_err());
    }

    #[test]
    fn moves_travel_between_players() {
        let (mut white, mut black) = pair(UNTIMED);
        white.play(Side::White, "e2e4", 10).unwrap();
        let changes = send(&white, &mut black, Side::White, 20);
        assert!(matches!(changes.as_slice(), [Change::Moved(_)]));
        black.play(Side::Black, "e7e5", 30).unwrap();
        send(&black, &mut white, Side::Black, 40);
        assert_eq!(white.snapshot(), black.snapshot());
        assert!(send(&black, &mut white, Side::Black, 50).is_empty());
    }

    #[test]
    fn a_player_cannot_move_for_the_other_side() {
        let (mut white, mut black) = pair(UNTIMED);
        assert_eq!(
            white.play(Side::Black, "e7e5", 0),
            Err(TableError::NotYourTurn)
        );
        black.snap.moves.push("e2e4".into());
        assert_eq!(
            white.merge(black.snapshot(), Side::Black, 0),
            Err(TableError::Cheat("a move out of turn"))
        );
    }

    #[test]
    fn an_illegal_remote_move_is_refused() {
        let (mut white, mut black) = pair(UNTIMED);
        white.snap.moves.push("e2e6".into());
        assert!(matches!(
            black.merge(white.snapshot(), Side::White, 0),
            Err(TableError::Rule(RuleError::Illegal(_)))
        ));
        assert!(black.record().is_empty());
    }

    #[test]
    fn a_rewritten_history_is_a_desync() {
        let (mut white, mut black) = pair(UNTIMED);
        white.play(Side::White, "e2e4", 0).unwrap();
        send(&white, &mut black, Side::White, 0);
        let mut forged = white.snapshot().clone();
        forged.moves = vec!["d2d4".into()];
        assert_eq!(
            black.merge(&forged, Side::White, 0),
            Err(TableError::Desync)
        );
    }

    #[test]
    fn only_the_resigning_side_can_resign() {
        let (mut white, mut black) = pair(UNTIMED);
        let mut forged = white.snapshot().clone();
        forged.resigned = Some(Side::Black);
        black.merge(&forged, Side::White, 0).unwrap();
        assert!(!black.is_over());
        white.resign(Side::White).unwrap();
        send(&white, &mut black, Side::White, 0);
        let verdict = black.verdict().unwrap();
        assert_eq!(verdict.winner, Some(Side::Black));
        assert_eq!(verdict.reason, Reason::Resignation);
    }

    #[test]
    fn a_draw_offer_lapses_after_a_move_and_holds_until_then() {
        let (mut white, mut black) = pair(UNTIMED);
        white.offer_draw(Side::White).unwrap();
        send(&white, &mut black, Side::White, 0);
        assert_eq!(black.draw_offered_by(), Some(Side::White));
        black.offer_draw(Side::Black).unwrap();
        send(&black, &mut white, Side::Black, 0);
        assert_eq!(white.verdict().map(|v| v.reason), Some(Reason::Agreement));

        let (mut white, mut black) = pair(UNTIMED);
        white.offer_draw(Side::White).unwrap();
        white.play(Side::White, "e2e4", 0).unwrap();
        send(&white, &mut black, Side::White, 0);
        assert_eq!(black.draw_offered_by(), None);
    }

    #[test]
    fn clocks_charge_thinking_and_add_the_increment() {
        let mut table = Table::new(blitz(), 0);
        table.play(Side::White, "e2e4", 5_000).unwrap();
        table.play(Side::Black, "e7e5", 9_000).unwrap();
        assert_eq!(table.remaining(Side::White, 9_000), Some(62_000));
        assert_eq!(table.remaining(Side::White, 19_000), Some(52_000));
        table.play(Side::White, "g1f3", 19_000).unwrap();
        assert_eq!(table.remaining(Side::White, 19_000), Some(54_000));
    }

    #[test]
    fn a_slow_player_flags_and_the_opponent_agrees() {
        let (mut white, mut black) = pair(blitz());
        white.play(Side::White, "e2e4", 0).unwrap();
        send(&white, &mut black, Side::White, 0);
        black.play(Side::Black, "e7e5", 0).unwrap();
        send(&black, &mut white, Side::Black, 0);
        assert_eq!(white.tick(&[Side::White], 61_000), None);
        let flagged = white.tick(&[Side::White], 62_000);
        assert!(matches!(flagged, Some(Change::Ended(_))));
        send(&white, &mut black, Side::White, 62_500);
        assert_eq!(black.verdict().map(|v| v.winner), Some(Some(Side::Black)));
    }

    #[test]
    fn a_reported_think_cannot_undercut_what_the_opponent_saw() {
        let (mut white, mut black) = pair(blitz());
        white.play(Side::White, "e2e4", 0).unwrap();
        send(&white, &mut black, Side::White, 0);
        black.play(Side::Black, "e7e5", 0).unwrap();
        send(&black, &mut white, Side::Black, 0);
        white.play(Side::White, "g1f3", 20_000).unwrap();
        let mut forged = white.snapshot().clone();
        forged.thinks[2] = 1;
        black.merge(&forged, Side::White, 20_100).unwrap();
        assert_eq!(black.snapshot().thinks[2], 20_100 - LAG_ALLOWANCE_MS);
    }

    #[test]
    fn a_late_guest_restores_the_game_and_refuses_a_forged_one() {
        let mut host = Table::new(UNTIMED, 0);
        for (ply, uci) in ["e2e4", "e7e5", "g1f3"].iter().enumerate() {
            let side = if ply % 2 == 0 {
                Side::White
            } else {
                Side::Black
            };
            host.play(side, uci, 0).unwrap();
        }
        let guest = Table::restore(UNTIMED, host.snapshot(), 0).unwrap();
        assert_eq!(guest.snapshot(), host.snapshot());
        assert_eq!(guest.turn(), Side::Black);
        let mut forged = host.snapshot().clone();
        forged.moves[2] = "g1h3".into();
        assert_eq!(
            Table::restore(UNTIMED, &forged, 0).err(),
            Some(TableError::Desync)
        );
    }

    #[test]
    fn a_rematch_starts_a_new_round_with_sides_swapped() {
        let (mut white, mut black) = pair(UNTIMED);
        white.resign(Side::White).unwrap();
        send(&white, &mut black, Side::White, 0);
        white.ask_rematch(Side::White, 0).unwrap();
        send(&white, &mut black, Side::White, 0);
        assert_eq!(black.ask_rematch(Side::Black, 0), Ok(Change::NewRound));
        let changes = send(&black, &mut white, Side::Black, 0);
        assert_eq!(changes.first(), Some(&Change::NewRound));
        assert_eq!(white.round(), 1);
        assert_eq!(white.settings().host_side(1), Side::Black);
        assert_eq!(white.snapshot(), black.snapshot());
    }
}
