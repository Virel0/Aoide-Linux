//! The rows, and how each maps to its table and its payload.
//!
//! Field names on the wire are the phone's: `camelCase`, an empty optional left out
//! rather than written as `null`, flags as JSON booleans. The phone's JSON decoder and
//! this one both ignore keys they do not know, so a column either side adds later
//! rides along instead of failing the row.

/// The four metadata accessors every syncable row implements identically.
macro_rules! sync_meta {
    () => {
        fn id(&self) -> &str {
            &self.id
        }

        fn updated_at(&self) -> Millis {
            self.updated_at
        }

        fn deleted(&self) -> bool {
            self.deleted
        }

        fn origin_device(&self) -> &str {
            &self.origin_device
        }

        fn set_meta(&mut self, updated_at: Millis, deleted: bool, origin_device: &str) {
            self.updated_at = updated_at;
            self.deleted = deleted;
            self.origin_device = origin_device.to_owned();
        }
    };
}

mod curation;
mod history;
mod op;
mod queue;
mod taste;
mod track;

pub use curation::{Folder, Playlist, PlaylistItem};
pub use history::{PlayEvent, PlaySource};
pub use op::{Op, OpKind};
pub use queue::QueueState;
pub use taste::{Like, TrackFlags};
pub use track::{ContentKey, TrackRef};
