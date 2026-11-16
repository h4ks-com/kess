use shakmaty::uci::UciMove;
use shakmaty::{Chess, Move, Position, Role};

/// Plies the bot looks ahead: enough to take free pieces and dodge simple threats.
const DEPTH: i32 = 3;
const MATE: i32 = 100_000;

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

/// Material plus a small pull toward the center, for the side to move.
fn score(position: &Chess) -> i32 {
    let us = position.turn();
    position
        .board()
        .iter()
        .map(|(square, piece)| {
            let (file, rank) = (square.file() as i32, square.rank() as i32);
            let central = 6 - i32::midpoint((2 * file - 7).abs(), (2 * rank - 7).abs());
            let worth = value(piece.role) + central * 4;
            if piece.color == us { worth } else { -worth }
        })
        .sum()
}

fn after(position: &Chess, chosen: Move) -> Chess {
    let mut next = position.clone();
    next.play_unchecked(chosen);
    next
}

fn search(position: &Chess, depth: i32, mut alpha: i32, beta: i32) -> i32 {
    let mut moves = position.legal_moves();
    if moves.is_empty() {
        return if position.is_check() {
            -MATE - depth
        } else {
            0
        };
    }
    if depth <= 0 {
        return score(position);
    }
    moves.sort_by_key(|chosen| -chosen.capture().map_or(0, value));
    for chosen in moves {
        let next = after(position, chosen);
        alpha = alpha.max(-search(&next, depth - 1, -beta, -alpha));
        if alpha >= beta {
            break;
        }
    }
    alpha
}

/// The bot's move in `position` as UCI, or none when the game is over.
#[must_use]
pub fn choose(position: &Chess, seed: u32) -> Option<String> {
    let moves = position.legal_moves();
    let scored = moves.iter().enumerate().map(|(index, chosen)| {
        let next = after(position, *chosen);
        // a tiny tiebreak so the bot doesn't play the same game every time
        let mixed = seed
            .wrapping_mul(2_654_435_761)
            .wrapping_add(index as u32 * 40_503);
        let jitter = i32::from((mixed >> 28) as u8);
        (
            -search(&next, DEPTH - 1, -MATE * 2, MATE * 2) * 16 + jitter,
            *chosen,
        )
    });
    let (_, best) = scored.max_by_key(|(score, _)| *score)?;
    Some(UciMove::from_standard(best).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rules::Record;

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
}
