use std::fmt::{self, Write};
use std::str::FromStr;

use chacha20poly1305::aead::{Aead, Payload};
use chacha20poly1305::{ChaCha20Poly1305, KeyInit, Nonce};
use hkdf::Hkdf;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::table::{Settings, Snapshot};

/// Bumped whenever two builds could no longer understand each other.
pub const PROTOCOL: u32 = 1;
const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
/// Twelve base32 letters, 60 bits: too many to guess from a broker's topic list.
const CODE_LETTERS: usize = 12;
const NONCE_BYTES: usize = 12;
const SALT: &[u8] = b"kess room v1";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WireError {
    #[error("a room code has {CODE_LETTERS} letters and digits, like 7FQ2-K9XM-3TBW")]
    BadCode,
    #[error("a message failed to decrypt")]
    Sealed,
    #[error("a message did not parse: {0}")]
    Garbled(String),
    #[error("the system random source failed")]
    Random,
}

/// The secret two players share: it names the room and keys its messages.
#[derive(Clone, PartialEq, Eq)]
pub struct RoomCode(String);

impl RoomCode {
    pub fn random() -> Result<Self, WireError> {
        let mut bytes = [0_u8; CODE_LETTERS];
        getrandom::fill(&mut bytes).map_err(|_| WireError::Random)?;
        let letters = bytes
            .iter()
            .filter_map(|byte| {
                ALPHABET
                    .get(usize::from(byte % 32))
                    .copied()
                    .map(char::from)
            })
            .collect();
        Ok(Self(letters))
    }

    fn derive(&self, label: &[u8]) -> [u8; 32] {
        let mut out = [0_u8; 32];
        Hkdf::<Sha256>::new(Some(SALT), self.0.as_bytes())
            .expand(label, &mut out)
            .map_or_else(
                |_| unreachable!("32 bytes is a valid HKDF length"),
                |()| out,
            )
    }

    /// The topic prefix on the broker, which reveals nothing of the code.
    #[must_use]
    pub fn room(&self) -> String {
        let id = self.derive(b"topic");
        format!("kess/{}", hex(id.get(..12).unwrap_or(&id)))
    }

    #[must_use]
    pub fn cipher(&self) -> Cipher {
        Cipher(ChaCha20Poly1305::new(&self.derive(b"key").into()))
    }
}

#[must_use]
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::new(), |mut out, byte| {
        let _ = write!(out, "{byte:02x}");
        out
    })
}

impl FromStr for RoomCode {
    type Err = WireError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let letters: String = text
            .chars()
            .filter(|letter| !matches!(letter, '-' | ' '))
            .map(|letter| match letter.to_ascii_uppercase() {
                'O' => '0',
                'I' | 'L' => '1',
                other => other,
            })
            .collect();
        let known = letters.bytes().all(|byte| ALPHABET.contains(&byte));
        if letters.len() == CODE_LETTERS && known {
            Ok(Self(letters))
        } else {
            Err(WireError::BadCode)
        }
    }
}

impl fmt::Display for RoomCode {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        let groups: Vec<&str> = [0, 4, 8]
            .iter()
            .filter_map(|start| self.0.get(*start..start + 4))
            .collect();
        write!(out, "{}", groups.join("-"))
    }
}

impl fmt::Debug for RoomCode {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.write_str("RoomCode(..)")
    }
}

/// Everything players send each other.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Message {
    /// The host's offer, kept on the broker so a guest can arrive any time.
    Lobby { protocol: u32, settings: Settings },
    /// The guest takes the seat.
    Join { protocol: u32 },
    /// The game as the sender sees it.
    Table {
        settings: Settings,
        snapshot: Snapshot,
    },
}

#[derive(Clone)]
pub struct Cipher(ChaCha20Poly1305);

impl Cipher {
    /// Encrypt `message` for `topic`, which also authenticates where it was
    /// sent, so nobody can replay the host's messages as the guest's.
    pub fn seal(&self, topic: &str, message: &Message) -> Result<Vec<u8>, WireError> {
        let plain =
            serde_json::to_vec(message).map_err(|error| WireError::Garbled(error.to_string()))?;
        let mut nonce = [0_u8; NONCE_BYTES];
        getrandom::fill(&mut nonce).map_err(|_| WireError::Random)?;
        let payload = Payload {
            msg: &plain,
            aad: topic.as_bytes(),
        };
        let sealed = self
            .0
            .encrypt(&Nonce::from(nonce), payload)
            .map_err(|_| WireError::Sealed)?;
        Ok([nonce.as_slice(), &sealed].concat())
    }

    pub fn open(&self, topic: &str, bytes: &[u8]) -> Result<Message, WireError> {
        let (nonce, sealed) = bytes
            .split_at_checked(NONCE_BYTES)
            .ok_or(WireError::Sealed)?;
        let nonce: [u8; NONCE_BYTES] = nonce.try_into().map_err(|_| WireError::Sealed)?;
        let payload = Payload {
            msg: sealed,
            aad: topic.as_bytes(),
        };
        let plain = self
            .0
            .decrypt(&Nonce::from(nonce), payload)
            .map_err(|_| WireError::Sealed)?;
        serde_json::from_slice(&plain).map_err(|error| WireError::Garbled(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::table::Side;

    fn lobby() -> Message {
        Message::Lobby {
            protocol: PROTOCOL,
            settings: Settings {
                host: Side::Black,
                control: Some("3+2".parse().unwrap()),
            },
        }
    }

    #[test]
    fn codes_print_in_groups_and_read_back_forgivingly() {
        let code = RoomCode::random().unwrap();
        let shown = code.to_string();
        assert_eq!(shown.len(), 14);
        assert_eq!(shown.to_lowercase().parse::<RoomCode>(), Ok(code));
        assert_eq!(
            "0OIl-2345-6789".parse::<RoomCode>().map(|c| c.to_string()),
            Ok("0011-2345-6789".into())
        );
        assert_eq!(
            "UUUU-2345-6789".parse::<RoomCode>(),
            Err(WireError::BadCode)
        );
        assert_eq!("12345".parse::<RoomCode>(), Err(WireError::BadCode));
    }

    #[test]
    fn the_room_name_hides_the_code() {
        let code: RoomCode = "7FQ2-K9XM-3TBW".parse().unwrap();
        let room = code.room();
        assert!(room.starts_with("kess/"));
        assert!(!room.to_uppercase().contains("7FQ2"));
        assert_eq!(room, "7FQ2K9XM3TBW".parse::<RoomCode>().unwrap().room());
    }

    #[test]
    fn a_sealed_message_opens_only_with_the_same_code_and_topic() {
        let code: RoomCode = "7FQ2-K9XM-3TBW".parse().unwrap();
        let other: RoomCode = "7FQ2-K9XM-3TBX".parse().unwrap();
        let sealed = code.cipher().seal("kess/a/host", &lobby()).unwrap();
        assert_eq!(code.cipher().open("kess/a/host", &sealed), Ok(lobby()));
        assert_eq!(
            code.cipher().open("kess/a/guest", &sealed),
            Err(WireError::Sealed)
        );
        assert_eq!(
            other.cipher().open("kess/a/host", &sealed),
            Err(WireError::Sealed)
        );
        let mut flipped = sealed.clone();
        if let Some(byte) = flipped.last_mut() {
            *byte ^= 1;
        }
        assert_eq!(
            code.cipher().open("kess/a/host", &flipped),
            Err(WireError::Sealed)
        );
        assert_eq!(
            code.cipher().open("kess/a/host", &[1, 2]),
            Err(WireError::Sealed)
        );
    }
}
