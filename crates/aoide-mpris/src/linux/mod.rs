//! The D-Bus side, Linux only.

mod inhibit;
mod instance;
mod mpris;
mod tray;

pub use inhibit::Inhibitor;
pub use instance::{InstanceGuard, single_instance};
pub use mpris::Mpris;
pub use tray::Tray;

use zbus::connection::Builder;

use crate::Bus;

fn builder(bus: &Bus) -> zbus::Result<Builder<'static>> {
    match bus {
        Bus::Session => Builder::session(),
        Bus::Address(address) => Builder::address(address.as_str()),
    }
}
