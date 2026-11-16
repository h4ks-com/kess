use std::time::{Duration, Instant};

use shakmaty::uci::UciMove;
use shakmaty::{Chess, Color, Move, Position, Role, Square};

/// How long the bot thinks per move; it searches one ply deeper at a time until then.
const THINK: Duration = Duration::from_millis(800);
const DEEPEST: i32 = 32;
const MATE: i32 = 100_000;
/// Mate scores sit within this many plies of `MATE`.
const MATE_HORIZON: i32 = 1_000;
/// Non-pawn material at the start, for telling the middlegame from the endgame.
const OPENING_MATERIAL: i32 = 2 * (2 * 320 + 2 * 330 + 2 * 500 + 900);
/// A lead this big lets the winning side drive the other king to the edge.
const WINNING_LEAD: i32 = 250;

fn value(role: Role) -> i32 {
    match role {
        Role::Pawn => 100,
        Role::Knight => 320,
        Role::Bishop => 330,
        Role::Rook => 500,
        Role::Queen => 900,
        Role::King => 0,
    }
}

/// How far `square` sits from the board's center, 0 in the middle four to 3 in a corner.
fn edge(square: Square) -> i32 {
    let (file, rank) = (square.file() as i32, square.rank() as i32);
    i32::midpoint((2 * file - 7).abs(), (2 * rank - 7).abs()).min(3)
}

/// How far a piece of `color` on `square` stands from its own back rank.
fn advance(square: Square, color: Color) -> i32 {
    let rank = square.rank() as i32;
    if color == Color::White {
        rank
    } else {
        7 - rank
    }
}

/// Where a piece stands, in centipawns, with `endgame` from 0 to 256.
fn placement(role: Role, color: Color, square: Square, endgame: i32) -> i32 {
    let central = 3 - edge(square);
    match role {
        Role::Pawn => {
            let steps = advance(square, color) - 1;
            steps * steps * (2 + endgame / 32) + central * 4
        }
        Role::Knight => central * 12,
        Role::Bishop => central * 8,
        Role::Rook => central * 2,
        Role::Queen => central * 4,
        // the king hides on its back rank early and walks to the center once the queens are off
        Role::King => {
            let shelter = 20 - 15 * advance(square, color);
            (shelter * (256 - endgame) + central * 16 * endgame) / 256
        }
    }
}

/// The position from the side to move's view: material, placement, and in a
/// won ending the losing king pushed to the edge with the winning king close.
fn score(position: &Chess) -> i32 {
    let board = position.board();
    let heavy: i32 = board
        .iter()
        .filter(|(_, piece)| !matches!(piece.role, Role::Pawn | Role::King))
        .map(|(_, piece)| value(piece.role))
        .sum();
    let endgame = 256 - (heavy * 256 / OPENING_MATERIAL).min(256);
    let mut white = 0;
    let mut lead = 0;
    for (square, piece) in board {
        let worth = value(piece.role) + placement(piece.role, piece.color, square, endgame);
        let sign = if piece.color == Color::White { 1 } else { -1 };
        white += sign * worth;
        lead += sign * value(piece.role);
    }
    if lead.abs() >= WINNING_LEAD
        && let (Some(white_king), Some(black_king)) =
            (board.king_of(Color::White), board.king_of(Color::Black))
    {
        let (winner, loser) = if lead > 0 {
            (white_king, black_king)
        } else {
            (black_king, white_king)
        };
        let apart = i32::try_from(winner.distance(loser)).unwrap_or(7);
        let mop = edge(loser) * 30 + (7 - apart) * 12;
        white += lead.signum() * mop * endgame / 256;
    }
    if position.turn() == Color::White {
        white
    } else {
        -white
    }
}

fn after(position: &Chess, chosen: Move) -> Chess {
    let mut next = position.clone();
    next.play_unchecked(chosen);
    next
}

/// `first` ahead of everything, then the most valuable victim by the
/// cheapest attacker, then promotions.
fn order(moves: &mut [Move], first: Option<Move>) {
    moves.sort_by_key(|chosen| {
        if Some(*chosen) == first {
            return i32::MIN;
        }
        let gain = chosen
            .capture()
            .map_or(0, |victim| value(victim) * 16 - value(chosen.role()))
            + chosen.promotion().map_or(0, value);
        -gain
    });
}

/// One search against a deadline; it gives up once the time runs out.
struct Search {
    deadline: Instant,
    nodes: u32,
    stopped: bool,
}

impl Search {
    fn out_of_time(&mut self) -> bool {
        self.nodes += 1;
        if self.nodes.is_multiple_of(1024) && Instant::now() >= self.deadline {
            self.stopped = true;
        }
        self.stopped
    }

    fn negamax(
        &mut self,
        position: &Chess,
        depth: i32,
        ply: i32,
        mut alpha: i32,
        beta: i32,
    ) -> i32 {
        if self.out_of_time() {
            return 0;
        }
        let mut moves = position.legal_moves();
        if moves.is_empty() {
            return if position.is_check() { ply - MATE } else { 0 };
        }
        if position.is_insufficient_material() || position.halfmoves() >= 100 {
            return 0;
        }
        // we never stop the search in check, or the bot walks into mates it could have seen
        let depth = if position.is_check() {
            depth.max(1)
        } else {
            depth
        };
        if depth <= 0 {
            return self.quiesce(position, alpha, beta);
        }
        order(&mut moves, None);
        for chosen in moves {
            let next = after(position, chosen);
            alpha = alpha.max(-self.negamax(&next, depth - 1, ply + 1, -beta, -alpha));
            if alpha >= beta || self.stopped {
                break;
            }
        }
        alpha
    }

    /// Captures and promotions until the position is quiet, so a trade
    /// half done never looks like a won piece.
    fn quiesce(&mut self, position: &Chess, mut alpha: i32, beta: i32) -> i32 {
        if self.out_of_time() {
            return 0;
        }
        let standing = score(position);
        if standing >= beta {
            return standing;
        }
        alpha = alpha.max(standing);
        let mut moves = position.capture_moves();
        order(&mut moves, None);
        for chosen in moves {
            let next = after(position, chosen);
            alpha = alpha.max(-self.quiesce(&next, -beta, -alpha));
            if alpha >= beta || self.stopped {
                break;
            }
        }
        alpha
    }
}

/// The bot's move in `position` as UCI, or none when the game is over.
#[must_use]
pub fn choose(position: &Chess, seed: u32) -> Option<String> {
    choose_within(position, seed, THINK)
}

fn choose_within(position: &Chess, seed: u32, think: Duration) -> Option<String> {
    let mut moves = position.legal_moves();
    let mut best = *moves.first()?;
    let mut search = Search {
        deadline: Instant::now() + think,
        nodes: 0,
        stopped: false,
    };
    for depth in 1..=DEEPEST {
        order(&mut moves, Some(best));
        let mut alpha = -MATE * 2;
        let mut found = None;
        for (index, chosen) in moves.iter().enumerate() {
            let next = after(position, *chosen);
            // a tiny tiebreak so the bot doesn't play the same game every time
            let mixed = seed
                .wrapping_mul(2_654_435_761)
                .wrapping_add(index as u32 * 40_503);
            let jitter = i32::from((mixed >> 29) as u8);
            let score = -search.negamax(&next, depth - 1, 1, -MATE * 2, -alpha + 8) + jitter;
            if search.stopped {
                break;
            }
            if score > alpha {
                alpha = score;
                found = Some(*chosen);
            }
        }
        if search.stopped {
            break;
        }
        if let Some(chosen) = found {
            best = chosen;
        }
        if alpha.abs() >= MATE - MATE_HORIZON {
            break;
        }
    }
    Some(UciMove::from_standard(best).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Record;
    use shakmaty::fen::Fen;
    use shakmaty::{CastlingMode, EnPassantMode};

    fn from_fen(fen: &str) -> Chess {
        fen.parse::<Fen>()
            .unwrap()
            .into_position(CastlingMode::Standard)
            .unwrap()
    }

    #[test]
    fn the_bot_takes_a_hanging_queen_and_finds_mate_in_one() {
        let hanging = Record::replay(["e2e4", "d7d5", "d1g4"]).unwrap();
        assert_eq!(choose(hanging.position(), 1).as_deref(), Some("c8g4"));
        let mate = Record::replay(["f2f3", "e7e5", "g2g4"]).unwrap();
        assert_eq!(choose(mate.position(), 1).as_deref(), Some("d8h4"));
    }

    #[test]
    fn the_bot_has_no_move_after_mate() {
        let over = Record::replay(["f2f3", "e7e5", "g2g4", "d8h4"]).unwrap();
        assert_eq!(choose(over.position(), 1), None);
    }

    #[test]
    fn the_bot_sees_a_mate_in_two() {
        // Re8+ Rxe8 Rxe8# down the open e file
        let position = from_fen("4r1k1/5ppp/8/8/8/8/5PPP/3RR1K1 w - - 0 1");
        assert!(matches!(
            choose(&position, 1).as_deref(),
            Some("e1e8" | "d1d8")
        ));
    }

    #[test]
    fn the_bot_keeps_a_knight_the_pawn_on_d5_would_cost() {
        // Nxd5 wins a pawn and loses the knight to the c6 pawn
        let position = from_fen("rnbqkbnr/pp2pppp/2p5/3p4/8/2N5/PPPPPPPP/R1BQKBNR w KQkq - 0 3");
        assert_ne!(choose(&position, 1).as_deref(), Some("c3d5"));
    }

    #[test]
    fn the_bot_mates_a_lone_king_with_its_queen() {
        let mut position = from_fen("8/8/8/4k3/8/8/8/3QK3 w - - 0 1");
        for turn in 0..50 {
            let uci = choose_within(&position, turn, Duration::from_millis(250)).unwrap();
            let chosen = uci.parse::<UciMove>().unwrap().to_move(&position).unwrap();
            position.play_unchecked(chosen);
            let Some(reply) = position
                .legal_moves()
                .into_iter()
                .min_by_key(|answer| edge(answer.to()))
            else {
                break;
            };
            // the lone king heads for the center, where a mate is hardest
            position.play_unchecked(reply);
        }
        assert!(
            position.is_checkmate(),
            "no mate in 50 moves, ended at {}",
            Fen::from_position(&position, EnPassantMode::Legal)
        );
    }
}
