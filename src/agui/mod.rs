//! Agent-to-UI (AG-UI) Protocol Implementation.
//!
//! Exposes typed event frames, in-process broadcaster hub, and an embedded
//! SSE HTTP server that visual runtimes (CopilotKit, React Flow, LangGraph) can
//! connect to.

pub mod events;
pub mod html;
pub mod hub;
pub mod server;

pub use events::AGUIFrame;
pub use hub::AGUIHub;
pub use server::spawn_agui_server;
