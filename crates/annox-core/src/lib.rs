//! Reference implementation of the annox specification (`spec/`).
//!
//! Module map, following the spec's sections:
//! - [`text`]: normalized text, offsets, and versions (§3.2–§3.4)
//! - [`anchor`]: creating and resolving anchors (§3.5–§3.8)
//! - [`suggestion`]: applicability and applying suggestions (§4)
//! - [`event`] and [`replay`]: the event model and deriving state (§2)
//! - [`storage`]: reading and writing `.annox/` (§5)
//! - [`ops`]: higher-level operations that write events

pub mod anchor;
pub mod event;
pub mod ops;
pub mod replay;
pub mod storage;
pub mod suggestion;
pub mod text;

/// A new event or document id: a UUIDv7 in canonical lowercase form (§2.1).
pub fn new_id() -> String {
    uuid::Uuid::now_v7().to_string()
}
