use serde::{Deserialize, Serialize};

/// Standardized AG-UI Event Frame.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AGUIFrame {
    pub event_id: String,
    pub timestamp_ms: u64,
    pub session_id: String,
    pub event_type: String,
    pub payload: serde_json::Value,
}

impl AGUIFrame {
    pub fn new(event_type: impl Into<String>, session_id: impl Into<String>, payload: serde_json::Value) -> Self {
        let timestamp_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let event_id = format!("agui_{:x}_{:x}", timestamp_ms, rand_u32());
        Self {
            event_id,
            timestamp_ms,
            session_id: session_id.into(),
            event_type: event_type.into(),
            payload,
        }
    }

    /// Formats as an SSE data payload: `event: <type>\ndata: <json>\n\n`
    pub fn to_sse(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        format!("event: {}\ndata: {}\n\n", self.event_type, json)
    }
}

fn rand_u32() -> u32 {
    let now = std::time::Instant::now();
    let addr = &now as *const _ as usize;
    (addr ^ (now.elapsed().as_nanos() as usize)) as u32
}
