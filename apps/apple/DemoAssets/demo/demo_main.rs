//! Tomi demo: a small blind "store-and-forward" relay.
use std::collections::HashMap;
use std::time::Duration;

/// An encrypted item: the relay only sees opaque bytes.
#[derive(Debug, Clone)]
pub struct Envelope {
    pub id: String,
    pub payload: Vec<u8>,
    pub ttl: Option<Duration>,
}

pub struct Relay {
    store: HashMap<String, Envelope>,
    max_size: usize,
}

impl Relay {
    pub fn new(max_size: usize) -> Self {
        Self { store: HashMap::new(), max_size }
    }

    /// Stores an envelope; refuses anything above the size limit.
    pub fn put(&mut self, envelope: Envelope) -> Result<(), String> {
        if envelope.payload.len() > self.max_size {
            return Err(format!("too large: {} bytes", envelope.payload.len()));
        }
        self.store.insert(envelope.id.clone(), envelope);
        Ok(())
    }

    pub fn take(&mut self, id: &str) -> Option<Envelope> {
        self.store.remove(id)
    }
}

fn main() {
    let mut relay = Relay::new(25 * 1024 * 1024);
    let envelope = Envelope {
        id: "demo-1".to_string(),
        payload: vec![0x54, 0x6f, 0x6d, 0x69],
        ttl: Some(Duration::from_secs(86_400)),
    };
    match relay.put(envelope) {
        Ok(()) => println!("stored"),
        Err(e) => eprintln!("error: {e}"),
    }
    assert!(relay.take("demo-1").is_some());
}
