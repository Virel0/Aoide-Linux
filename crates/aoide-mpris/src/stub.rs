//! The same API where there is no D-Bus. Nothing here can be constructed except
//! the single-instance guard, so the methods are never reached.

use std::convert::Infallible;

use tokio::sync::mpsc::{self, Receiver};

use crate::{Bus, Command, Error, Instance, PlayerState, Result, TrayCommand, TrayIcon, TrayState};

/// The MPRIS server. Not available on this platform: [`Mpris::start`] always
/// returns [`Error::Unavailable`].
#[derive(Debug)]
pub struct Mpris {
    never: Infallible,
}

impl Mpris {
    pub async fn start(_bus: &Bus) -> Result<(Mpris, Receiver<Command>)> {
        Err(Error::Unavailable)
    }

    pub async fn update(&self, _state: PlayerState) -> Result<()> {
        match self.never {}
    }
}

/// The tray icon. Not available on this platform: [`Tray::start`] always
/// returns [`Error::Unavailable`].
#[derive(Debug)]
pub struct Tray {
    never: Infallible,
}

impl Tray {
    pub async fn start(_bus: &Bus, _icon: TrayIcon) -> Result<(Tray, Receiver<TrayCommand>)> {
        Err(Error::Unavailable)
    }

    pub fn is_shown(&self) -> bool {
        match self.never {}
    }

    pub async fn update(&self, _state: TrayState) -> Result<()> {
        match self.never {}
    }
}

/// Keeps the "first instance" answer valid. Here it only keeps the raise
/// channel open; nothing ever asks.
#[derive(Debug)]
pub struct InstanceGuard {
    _raise: mpsc::Sender<()>,
}

pub async fn single_instance(_bus: &Bus) -> Result<Instance> {
    let (tx, raise) = mpsc::channel(1);
    Ok(Instance::First {
        guard: InstanceGuard { _raise: tx },
        raise,
    })
}

/// The sleep inhibitor. Not available on this platform:
/// [`Inhibitor::new`] always returns [`Error::Unavailable`].
#[derive(Debug)]
pub struct Inhibitor {
    never: Infallible,
}

impl Inhibitor {
    pub async fn new(_bus: &Bus) -> Result<Inhibitor> {
        Err(Error::Unavailable)
    }

    pub async fn set(&mut self, _playing: bool) -> Result<()> {
        match self.never {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mpris_is_unavailable() {
        assert!(matches!(
            Mpris::start(&Bus::Session).await,
            Err(Error::Unavailable)
        ));
    }

    #[tokio::test]
    async fn the_tray_is_unavailable() {
        assert!(matches!(
            Tray::start(&Bus::Session, TrayIcon::default()).await,
            Err(Error::Unavailable)
        ));
    }

    #[tokio::test]
    async fn the_inhibitor_is_unavailable() {
        assert!(matches!(
            Inhibitor::new(&Bus::Session).await,
            Err(Error::Unavailable)
        ));
    }

    #[tokio::test]
    async fn every_launch_is_the_first() {
        let Ok(Instance::First { guard, mut raise }) = single_instance(&Bus::Session).await else {
            panic!("expected the first instance");
        };
        // The channel stays open while the guard lives, and nothing arrives.
        assert!(raise.try_recv().is_err());
        assert!(!raise.is_closed());
        drop(guard);
        assert!(raise.recv().await.is_none());
    }
}
