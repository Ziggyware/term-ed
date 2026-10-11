use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Sender, Receiver, channel};
use serde_json::Value;

use crate::agui::events::AGUIFrame;

/// In-process and network broadcaster for AG-UI streaming protocol.
#[derive(Clone, Debug)]
pub struct AGUIHub {
    subscribers: Arc<Mutex<Vec<Sender<AGUIFrame>>>>,
}

impl Default for AGUIHub {
    fn default() -> Self {
        Self::new()
    }
}

impl AGUIHub {
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Emits a new AG-UI frame to all active listeners.
    pub fn emit(&self, event_type: &str, session_id: &str, payload: Value) {
        let frame = AGUIFrame::new(event_type, session_id, payload);
        self.emit_frame(frame);
    }

    /// Emits a pre-constructed frame.
    pub fn emit_frame(&self, frame: AGUIFrame) {
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain(|tx| tx.send(frame.clone()).is_ok());
    }

    /// Registers a new receiver for the AG-UI event stream.
    pub fn subscribe(&self) -> Receiver<AGUIFrame> {
        let (tx, rx) = channel();
        let mut subs = self.subscribers.lock().unwrap();
        subs.push(tx);
        rx
    }

    /// Count of currently connected subscribers.
    pub fn subscriber_count(&self) -> usize {
        let subs = self.subscribers.lock().unwrap();
        subs.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hub_emission_and_subscription() {
        let hub = AGUIHub::new();
        let rx = hub.subscribe();
        assert_eq!(hub.subscriber_count(), 1);

        hub.emit("decision:evaluated", "s1", serde_json::json!({ "verdict": "AutoAllow" }));
        let frame = rx.recv().expect("should receive frame");
        assert_eq!(frame.event_type, "decision:evaluated");
        assert_eq!(frame.session_id, "s1");
        assert!(frame.to_sse().starts_with("event: decision:evaluated\ndata: "));
    }
}
