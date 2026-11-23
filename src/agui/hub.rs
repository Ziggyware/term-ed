use std::sync::{Arc, Mutex};
use std::sync::mpsc::{Sender, Receiver, channel};
use serde_json::Value;

use crate::agui::events::AGUIFrame;

#[derive(Debug)]
struct HubState {
    subscribers: Vec<(u64, Sender<Arc<AGUIFrame>>)>,
    next_id: u64,
}

/// In-process and network broadcaster for AG-UI streaming protocol.
#[derive(Clone, Debug)]
pub struct AGUIHub {
    state: Arc<Mutex<HubState>>,
    /// Serializes emitters so each subscriber receives frames in
    /// emission order, without holding the subscriber list locked
    /// across sends.
    emit_gate: Arc<Mutex<()>>,
}

impl Default for AGUIHub {
    fn default() -> Self {
        Self::new()
    }
}

impl AGUIHub {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(HubState {
                subscribers: Vec::new(),
                next_id: 0,
            })),
            emit_gate: Arc::new(Mutex::new(())),
        }
    }

    /// Emits a new AG-UI frame to all active listeners.
    pub fn emit(&self, event_type: &str, session_id: &str, payload: Value) {
        let frame = AGUIFrame::new(event_type, session_id, payload);
        self.emit_frame(frame);
    }

    /// Emits a pre-constructed frame.
    pub fn emit_frame(&self, frame: AGUIFrame) {
        let _gate = self.emit_gate.lock().unwrap();
        // Snapshot the senders, then send with the list unlocked: one
        // Arc clone per subscriber instead of a deep frame clone, and
        // a slow or dead receiver never blocks subscribe/unsubscribe.
        let snapshot: Vec<(u64, Sender<Arc<AGUIFrame>>)> = {
            let state = self.state.lock().unwrap();
            state.subscribers.clone()
        };
        let frame = Arc::new(frame);
        let mut dead: Vec<u64> = Vec::new();
        for (id, tx) in &snapshot {
            if tx.send(Arc::clone(&frame)).is_err() {
                dead.push(*id);
            }
        }
        if !dead.is_empty() {
            let mut state = self.state.lock().unwrap();
            state.subscribers.retain(|(id, _)| !dead.contains(id));
        }
    }

    /// Registers a new receiver for the AG-UI event stream.
    pub fn subscribe(&self) -> Receiver<Arc<AGUIFrame>> {
        let (tx, rx) = channel();
        let mut state = self.state.lock().unwrap();
        let id = state.next_id;
        state.next_id += 1;
        state.subscribers.push((id, tx));
        rx
    }

    /// Count of currently connected subscribers.
    pub fn subscriber_count(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.subscribers.len()
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
