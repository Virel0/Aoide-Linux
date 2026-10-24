//! Something a listener was *in* — an album, a playlist, a station — so Home can open on
//! the last few and resume one in a tap. Local to the device on purpose; a plain value
//! the app persists wherever it keeps preferences.

use serde::{Deserialize, Serialize};

use crate::Millis;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecentContextKind {
    Album,
    JellyfinPlaylist,
    LocalPlaylist,
    Station,
}

impl RecentContextKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Album => "album",
            Self::JellyfinPlaylist => "jellyfinPlaylist",
            Self::LocalPlaylist => "localPlaylist",
            Self::Station => "station",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentContext {
    pub kind: RecentContextKind,
    /// The album, playlist or seed item id, in whichever store the kind names.
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subtitle: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artwork_item_id: Option<String>,
    pub last_opened: Millis,
}

impl RecentContext {
    pub fn new(
        kind: RecentContextKind,
        id: impl Into<String>,
        name: impl Into<String>,
        last_opened: Millis,
    ) -> Self {
        Self {
            kind,
            id: id.into(),
            name: name.into(),
            subtitle: None,
            artwork_item_id: None,
            last_opened,
        }
    }

    /// One entry per thing, whatever its name is today.
    pub fn key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.id)
    }
}

/// The recent contexts, newest first, one per thing, bounded.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentContextList {
    contexts: Vec<RecentContext>,
}

impl RecentContextList {
    /// Enough that a busy week does not push Monday's album out before the weekend.
    pub const CAPACITY: usize = 24;

    pub fn contexts(&self) -> &[RecentContext] {
        &self.contexts
    }

    /// Records a visit: the same thing again moves to the front with its new name.
    pub fn remember(&mut self, context: RecentContext) {
        let key = context.key();
        self.contexts.retain(|existing| existing.key() != key);
        self.contexts.insert(0, context);
        self.contexts.truncate(Self::CAPACITY);
    }

    pub fn forget(&mut self, key: &str) {
        self.contexts.retain(|existing| existing.key() != key);
    }

    /// The most recent `limit`, newest first.
    pub fn latest(&self, limit: usize) -> &[RecentContext] {
        &self.contexts[..limit.min(self.contexts.len())]
    }
}
