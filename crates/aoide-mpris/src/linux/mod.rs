//! The D-Bus side, Linux only.

mod inhibit;
mod instance;
mod mpris;

pub use inhibit::Inhibitor;
pub use instance::{InstanceGuard, single_instance};
pub use mpris::Mpris;

use zbus::connection::Builder;

use crate::Bus;

fn builder(bus: &Bus) -> zbus::Result<Builder<'static>> {
    match bus {
        Bus::Session => Builder::session(),
        Bus::Address(address) => Builder::address(address.as_str()),
    }
}
