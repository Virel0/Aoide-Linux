//! Where a client's requests go right now: the server's own address, or, while
//! the app reaches Melete through its tunnel, the tunnel's local port. Every
//! clone of a client shares one route, so turning the tunnel on or off moves
//! every request at once.

use std::sync::{Arc, PoisonError, RwLock};

use reqwest::Url;

use crate::ServerAddress;

/// A client's way to Melete, shared by its clones.
#[derive(Debug, Clone, Default)]
pub struct Route(Arc<RwLock<Option<Url>>>);

impl Route {
    /// Sends requests to `base` instead of the server's own address, or back to
    /// that address with `None`.
    pub fn set(&self, base: Option<&ServerAddress>) {
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = base.map(|b| b.as_url().clone());
    }

    /// The address in use instead of the server's own, if any.
    pub fn current(&self) -> Option<Url> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}
