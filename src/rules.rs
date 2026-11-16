use shakmaty::uci::UciMove;
use shakmaty::zobrist::Zobrist64;
use shakmaty::{Chess, Color, EnPassantMode, KnownOutcome, Move, Outcome, Position};

/// Half moves without a capture or pawn move that end the game on their own.
const SEVENTY_FIVE_MOVES: u32 = 150;
const FIVEFOLD: usize = 5;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RuleError {
    #[error("{0} is not a move")]
    Unreadable(String),
    #[error("{0} is not legal here")]
    Illegal(String),
    #[error("the game is over")]
    Over,
}

/// Why a game ended on the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ending {
    Checkmate(Color),
    Stalemate,
    InsufficientMaterial,
    Repetition,
    SeventyFiveMoves,
}

/// A game from the start position, as the list of moves that both players
/// replay and check on their own machines.
#[derive(Debug, Clone)]
pub struct Record {
    position: Chess,
    before: Vec<Chess>,
    moves: Vec<UciMove>,
    hashes: Vec<u64>,
}

impl Default for Record {
    fn default() -> Self {
        Self::new()
    }
}

impl Record {
    #[must_use]
    pub fn new() -> Self {
        let position = Chess::default();
        Self {
            hashes: vec![hash(&position)],
            position,
            before: Vec::new(),
            moves: Vec::new(),
        }
    }

    pub fn replay<'a>(moves: impl IntoIterator<Item = &'a str>) -> Result<Self, RuleError> {
        let mut record = Self::new();
        for uci in moves {
            record.play(uci)?;
        }
        Ok(record)
    }

    #[must_use]
    pub fn position(&self) -> &Chess {
        &self.position
    }

    #[must_use]
    pub fn moves(&self) -> Vec<String> {
        self.moves.iter().map(ToString::to_string).collect()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.moves.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    #[must_use]
    pub fn last_move(&self) -> Option<Move> {
        self.moves.last()?.to_move(self.before.last()?).ok()
    }

    #[must_use]
    pub fn turn(&self) -> Color {
        self.position().turn()
    }

    pub fn play(&mut self, uci: &str) -> Result<Move, RuleError> {
        if self.ending().is_some() {
            return Err(RuleError::Over);
        }
        let parsed: UciMove = uci
            .parse()
            .map_err(|_| RuleError::Unreadable(uci.to_owned()))?;
        let illegal = |_| RuleError::Illegal(uci.to_owned());
        let chosen = parsed.to_move(&self.position).map_err(illegal)?;
        let next = self
            .position
            .clone()
            .play(chosen)
            .map_err(|_| RuleError::Illegal(uci.to_owned()))?;
        self.hashes.push(hash(&next));
        self.before
            .push(std::mem::replace(&mut self.position, next));
        self.moves.push(parsed);
        Ok(chosen)
    }

    #[must_use]
    pub fn ending(&self) -> Option<Ending> {
        let position = self.position();
        match position.outcome() {
            Outcome::Known(KnownOutcome::Decisive { winner }) => {
                return Some(Ending::Checkmate(winner));
            }
            Outcome::Known(KnownOutcome::Draw) if position.is_stalemate() => {
                return Some(Ending::Stalemate);
            }
            Outcome::Known(KnownOutcome::Draw) => return Some(Ending::InsufficientMaterial),
            Outcome::Unknown => {}
        }
        let current = self.hashes.last().copied().unwrap_or_default();
        if self.hashes.iter().filter(|seen| **seen == current).count() >= FIVEFOLD {
            return Some(Ending::Repetition);
        }
        (position.halfmoves() >= SEVENTY_FIVE_MOVES).then_some(Ending::SeventyFiveMoves)
    }

    /// A short fingerprint of the current position, so two players notice
    /// when their boards disagree.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        self.hashes.last().copied().unwrap_or_default()
    }
}

fn hash(position: &Chess) -> u64 {
    position.zobrist_hash::<Zobrist64>(EnPassantMode::Legal).0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fools_mate_ends_in_checkmate_for_black() {
        let record = Record::replay(["f2f3", "e7e5", "g2g4", "d8h4"]).unwrap();
        assert_eq!(record.ending(), Some(Ending::Checkmate(Color::Black)));
        let mut over = record.clone();
        assert_eq!(over.play("a2a3"), Err(RuleError::Over));
    }

    #[test]
    fn an_illegal_move_leaves_the_record_alone() {
        let mut record = Record::new();
        assert_eq!(record.play("e2e5"), Err(RuleError::Illegal("e2e5".into())));
        assert_eq!(record.play("zz"), Err(RuleError::Unreadable("zz".into())));
        assert!(record.is_empty());
    }

    #[test]
    fn five_repetitions_draw() {
        let shuffle = ["g1f3", "g8f6", "f3g1", "f6g8"];
        let moves: Vec<&str> = shuffle.iter().cycle().take(16).copied().collect();
        let record = Record::replay(moves).unwrap();
        assert_eq!(record.ending(), Some(Ending::Repetition));
    }

    #[test]
    fn castling_and_promotion_replay() {
        let record = Record::replay([
            "e2e4", "d7d5", "e4d5", "c7c6", "d5c6", "g8f6", "c6b7", "e7e6", "b7a8q", "f8e7",
            "g1f3", "e8g8", "f1e2", "a7a6", "e1g1",
        ])
        .unwrap();
        assert_eq!(record.len(), 15);
        assert_eq!(record.moves().get(8).map(String::as_str), Some("b7a8q"));
        assert_eq!(record.ending(), None);
    }

    #[test]
    fn the_same_moves_give_the_same_fingerprint() {
        let one = Record::replay(["e2e4", "e7e5"]).unwrap();
        let two = Record::replay(["e2e4", "e7e5"]).unwrap();
        let other = Record::replay(["d2d4", "e7e5"]).unwrap();
        assert_eq!(one.fingerprint(), two.fingerprint());
        assert_ne!(one.fingerprint(), other.fingerprint());
    }
}
