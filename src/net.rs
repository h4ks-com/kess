use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use rumqttc::{Client, Connection, Event, MqttOptions, Packet, QoS, Transport};

use crate::wire::{Cipher, Message, RoomCode, WireError, hex};

/// Public brokers that take anonymous TLS clients. We join every one at once
/// and send everything to all of them, so two players meet even when one
/// broker is down for one of them.
pub const BROKERS: [&str; 2] = ["broker.hivemq.com", "broker.emqx.io"];
const PORT: u16 = 8883;
const KEEP_ALIVE: Duration = Duration::from_secs(20);
const RETRY: Duration = Duration::from_secs(2);
const QUEUE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Host,
    Guest,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Guest => "guest",
        }
    }

    fn peer(self) -> Self {
        match self {
            Self::Host => Self::Guest,
            Self::Guest => Self::Host,
        }
    }
}

#[derive(Debug)]
pub enum Note {
    Online(&'static str),
    Offline(&'static str),
    Received(Message),
    Rejected(WireError),
}

/// Our end of a room: sends to our topic, hears the peer's.
pub struct Link {
    clients: Vec<Client>,
    outbox: String,
    cipher: Cipher,
    last: Arc<Mutex<Vec<u8>>>,
    notes: Receiver<Note>,
}

impl Link {
    #[must_use]
    pub fn open(code: &RoomCode, role: Role) -> Self {
        let room = code.room();
        let outbox = format!("{room}/{}", role.name());
        let inbox = format!("{room}/{}", role.peer().name());
        let last = Arc::new(Mutex::new(Vec::new()));
        let (notes_in, notes) = channel();
        let clients = BROKERS
            .iter()
            .map(|broker| {
                let (client, connection) = Client::new(options(broker), QUEUE);
                let feed = Feed {
                    broker,
                    client: client.clone(),
                    inbox: inbox.clone(),
                    outbox: outbox.clone(),
                    cipher: code.cipher(),
                    last: Arc::clone(&last),
                    notes: notes_in.clone(),
                };
                thread::spawn(move || feed.run(connection));
                client
            })
            .collect();
        Self {
            clients,
            outbox,
            cipher: code.cipher(),
            last,
            notes,
        }
    }

    /// Keep `message` on every broker as our latest word, replacing the last.
    pub fn send(&self, message: &Message) -> Result<(), WireError> {
        let sealed = self.cipher.seal(&self.outbox, message)?;
        sealed.clone_into(&mut self.last.lock().unwrap_or_else(PoisonError::into_inner));
        for client in &self.clients {
            // a full or offline client gets the latest message again when it reconnects
            let _ = client.try_publish(&self.outbox, QoS::AtLeastOnce, true, sealed.clone());
        }
        Ok(())
    }

    pub fn notes(&self) -> impl Iterator<Item = Note> + '_ {
        self.notes.try_iter()
    }

    /// Leave word that we quit in place of our last message, and hang up.
    pub fn close(&self) {
        let parting = self
            .cipher
            .seal(&self.outbox, &Message::Leave)
            .unwrap_or_default();
        for client in &self.clients {
            let _ = client.try_publish(&self.outbox, QoS::AtLeastOnce, true, parting.clone());
            let _ = client.try_disconnect();
        }
        thread::sleep(Duration::from_millis(400));
    }
}

fn options(broker: &str) -> MqttOptions {
    let mut id = [0_u8; 8];
    // without a random source we keep the zero id, which only risks a clash with another kess client
    let _ = getrandom::fill(&mut id);
    let mut options = MqttOptions::new(format!("kess-{}", hex(&id)), broker, PORT);
    options.set_keep_alive(KEEP_ALIVE);
    options.set_max_packet_size(256 * 1024, 256 * 1024);
    options.set_transport(Transport::tls_with_default_config());
    options
}

struct Feed {
    broker: &'static str,
    client: Client,
    inbox: String,
    outbox: String,
    cipher: Cipher,
    last: Arc<Mutex<Vec<u8>>>,
    notes: Sender<Note>,
}

impl Feed {
    fn run(self, mut connection: Connection) {
        for event in connection.iter() {
            let note = match event {
                Ok(Event::Incoming(Packet::ConnAck(_))) => self.welcome(),
                Ok(Event::Incoming(Packet::Publish(publish))) if publish.topic == self.inbox => {
                    if publish.payload.is_empty() {
                        continue;
                    }
                    match self.cipher.open(&self.inbox, &publish.payload) {
                        Ok(message) => Note::Received(message),
                        Err(error) => Note::Rejected(error),
                    }
                }
                Ok(_) => continue,
                Err(_) => {
                    thread::sleep(RETRY);
                    Note::Offline(self.broker)
                }
            };
            if self.notes.send(note).is_err() {
                return;
            }
        }
    }

    fn welcome(&self) -> Note {
        let _ = self.client.try_subscribe(&self.inbox, QoS::AtLeastOnce);
        let last = self
            .last
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if !last.is_empty() {
            let _ = self
                .client
                .try_publish(&self.outbox, QoS::AtLeastOnce, true, last);
        }
        Note::Online(self.broker)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::table::{Settings, Side};
    use crate::wire::PROTOCOL;

    /// Talks to the real public brokers, so it only runs on request:
    /// `cargo test -- --ignored`.
    #[test]
    #[ignore = "needs the internet"]
    fn a_host_and_a_guest_meet_through_the_public_brokers() {
        let code = RoomCode::random().unwrap();
        let host = Link::open(&code, Role::Host);
        let guest = Link::open(&code, Role::Guest);
        let lobby = Message::Lobby {
            protocol: PROTOCOL,
            settings: Settings {
                host: Side::White,
                control: None,
            },
        };
        host.send(&lobby).unwrap();
        guest.send(&Message::Join { protocol: PROTOCOL }).unwrap();
        let started = Instant::now();
        let (mut heard_lobby, mut heard_join) = (false, false);
        while !(heard_lobby && heard_join) && started.elapsed() < Duration::from_secs(20) {
            heard_lobby |= guest
                .notes()
                .any(|note| matches!(note, Note::Received(Message::Lobby { .. })));
            heard_join |= host
                .notes()
                .any(|note| matches!(note, Note::Received(Message::Join { .. })));
            thread::sleep(Duration::from_millis(100));
        }
        host.close();
        guest.close();
        assert!(heard_lobby && heard_join);
    }
}
