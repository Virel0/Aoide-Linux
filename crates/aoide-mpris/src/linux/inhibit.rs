//! Keep the machine awake while music plays.
//!
//! First choice is the desktop portal's `Inhibit` with the suspend flag only:
//! the screen may still blank and lock, the machine just does not go to sleep.
//! It works inside and outside Flatpak on GNOME, KDE and others. Where no
//! portal answers, `org.freedesktop.ScreenSaver` is the fallback; it also keeps
//! the screen on, which is more than needed but better than a sleeping player.
//! Both release the inhibition by themselves if the app's connection goes away.

use std::collections::HashMap;

use zbus::Connection;
use zbus::zvariant::{OwnedObjectPath, Value};

use super::builder;
use crate::{Bus, Error, IDENTITY, Result};

const PORTAL_NAME: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const PORTAL_INHIBIT: &str = "org.freedesktop.portal.Inhibit";
const PORTAL_REQUEST: &str = "org.freedesktop.portal.Request";
/// `Inhibit` flag bits from the portal spec: 1 logout, 2 user switch,
/// 4 suspend, 8 idle.
const PORTAL_SUSPEND: u32 = 4;

const SCREENSAVER_NAME: &str = "org.freedesktop.ScreenSaver";
const SCREENSAVER_PATH: &str = "/org/freedesktop/ScreenSaver";
const SCREENSAVER_INTERFACE: &str = "org.freedesktop.ScreenSaver";

const REASON: &str = "Playing music";

#[derive(Debug)]
enum Held {
    /// The portal's request handle; closing it ends the inhibition.
    Portal(OwnedObjectPath),
    /// The screensaver's cookie.
    ScreenSaver(u32),
}

/// Holds a sleep inhibition while [`Inhibitor::set`] says something plays.
#[derive(Debug)]
pub struct Inhibitor {
    connection: Connection,
    held: Option<Held>,
}

impl Inhibitor {
    /// Connects to `bus`. Nothing is inhibited until [`Inhibitor::set`].
    ///
    /// # Errors
    ///
    /// [`Error::DBus`] when the bus can't be reached.
    pub async fn new(bus: &Bus) -> Result<Inhibitor> {
        Ok(Inhibitor {
            connection: builder(bus)?.build().await?,
            held: None,
        })
    }

    /// Inhibits sleep when `playing`, releases it otherwise. Calling it again
    /// with the same value does nothing. Fails with [`Error::NoInhibitor`] if
    /// neither the portal nor the screensaver answers.
    ///
    /// # Errors
    ///
    /// [`Error::NoInhibitor`] as above; [`Error::DBus`] when releasing fails, in
    /// which case the inhibition is kept for the next call to try again.
    pub async fn set(&mut self, playing: bool) -> Result<()> {
        match (playing, &self.held) {
            (true, None) => self.held = Some(self.inhibit().await?),
            (false, Some(held)) => {
                // Kept on failure, so the next call can try again.
                self.release(held).await?;
                self.held = None;
            }
            _ => {}
        }
        Ok(())
    }

    async fn inhibit(&self) -> Result<Held> {
        let options: HashMap<&str, Value<'_>> = HashMap::from([("reason", Value::from(REASON))]);
        let portal = self
            .connection
            .call_method(
                Some(PORTAL_NAME),
                PORTAL_PATH,
                Some(PORTAL_INHIBIT),
                "Inhibit",
                &("", PORTAL_SUSPEND, options),
            )
            .await;
        if let Ok(reply) = portal
            && let Ok(handle) = reply.body().deserialize::<OwnedObjectPath>()
        {
            return Ok(Held::Portal(handle));
        }
        let screensaver = self
            .connection
            .call_method(
                Some(SCREENSAVER_NAME),
                SCREENSAVER_PATH,
                Some(SCREENSAVER_INTERFACE),
                "Inhibit",
                &(IDENTITY, REASON),
            )
            .await;
        if let Ok(reply) = screensaver
            && let Ok(cookie) = reply.body().deserialize::<u32>()
        {
            return Ok(Held::ScreenSaver(cookie));
        }
        Err(Error::NoInhibitor)
    }

    async fn release(&self, held: &Held) -> Result<()> {
        match held {
            Held::Portal(handle) => {
                self.connection
                    .call_method(
                        Some(PORTAL_NAME),
                        handle.as_ref(),
                        Some(PORTAL_REQUEST),
                        "Close",
                        &(),
                    )
                    .await?;
            }
            Held::ScreenSaver(cookie) => {
                self.connection
                    .call_method(
                        Some(SCREENSAVER_NAME),
                        SCREENSAVER_PATH,
                        Some(SCREENSAVER_INTERFACE),
                        "UnInhibit",
                        &(*cookie,),
                    )
                    .await?;
            }
        }
        Ok(())
    }
}
