//! The packets between WireGuard and the network stack: GotaTun hands each
//! decrypted IP packet to [`ToStack`] and takes the stack's from [`FromStack`].
//! Melete's own tunnel joins them the same way.

use std::io;

use gotatun::packet::{Ip, Packet, PacketBufPool};
use gotatun::tun::{IpRecv, IpSend, MtuWatcher};
use std::sync::Arc;
use tokio::sync::{Notify, mpsc};

/// Decrypted packets on their way into the stack.
pub struct ToStack {
    pub packets: mpsc::Sender<Vec<u8>>,
    /// Wakes the stack, which may be waiting on its own timers.
    pub wake: Arc<Notify>,
}

impl IpSend for ToStack {
    async fn send(&mut self, packet: Packet<Ip>) -> io::Result<()> {
        // A full queue drops the packet, as a busy link would; TCP sends it again.
        let _ = self.packets.try_send(packet.into_bytes().as_ref().to_vec());
        self.wake.notify_one();
        Ok(())
    }
}

/// The stack's packets on their way out through WireGuard.
pub struct FromStack {
    pub packets: mpsc::Receiver<Vec<u8>>,
    pub mtu: u16,
}

impl IpRecv for FromStack {
    async fn recv<'a>(
        &'a mut self,
        _pool: &mut PacketBufPool,
    ) -> io::Result<impl Iterator<Item = Packet<Ip>> + Send + 'a> {
        loop {
            let Some(bytes) = self.packets.recv().await else {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the tunnel's network stack stopped",
                ));
            };
            let packet = Packet::from_bytes(bytes.as_slice().into()).try_into_ip();
            if let Ok(packet) = packet {
                return Ok(std::iter::once(packet));
            }
        }
    }

    fn mtu(&self) -> MtuWatcher {
        MtuWatcher::new(self.mtu)
    }
}
