//! The TCP/IP stack inside the tunnel: one address, the device's own, and
//! connections out to Melete's address alone. Each connection's bytes come and go
//! through a pair of channels from the local port forward.

use std::collections::VecDeque;
use std::net::SocketAddrV4;
use std::sync::Arc;
use std::time::Duration;

use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{Notify, mpsc};

use crate::pump::{Ends, PumpState, pump};

/// The largest packet, as Melete's side of the tunnel takes it.
pub const MTU: u16 = 1420;
/// Packets held between the stack and WireGuard, each way.
pub const QUEUE: usize = 256;
/// What Melete may send a connection before hearing back, which is all it sends
/// at once: Melete's stack has no congestion control, so a larger window comes in
/// bursts a Wi-Fi or mobile link drops, and each loss stalls the connection for
/// a second or more. 32 KB still carries 4 Mbit/s a connection at a 60 ms round
/// trip, more than lossless music needs.
const RX_BUFFER: usize = 32 * 1024;
const TX_BUFFER: usize = 64 * 1024;
const TIMEOUT: Duration = Duration::from_secs(60);
const KEEP_ALIVE: Duration = Duration::from_secs(20);
/// The longest the stack sleeps with nothing to do, so its timers keep time.
const IDLE: Duration = Duration::from_millis(500);
/// The ephemeral ports a connection's own end is numbered from.
const FIRST_PORT: u16 = 49_152;
/// How soon packets WireGuard had no room for are offered again.
const BACKLOG: Duration = Duration::from_millis(2);
/// Times a connection Melete turns away is asked for again: Melete keeps only a
/// few sockets listening, so a burst of new connections has some refused.
const RETRIES: u32 = 5;
/// The first wait before asking again, doubled each time after.
const RETRY_AFTER: Duration = Duration::from_millis(25);
/// Refused this soon after asking, a connection was turned away; later, it
/// timed out, and Melete isn't there to ask.
const REFUSED_WITHIN: Duration = Duration::from_secs(5);

/// Packets in and out of smoltcp, held until the loop moves them on.
#[derive(Default)]
pub(crate) struct Queues {
    pub(crate) incoming: VecDeque<Vec<u8>>,
    pub(crate) outgoing: VecDeque<Vec<u8>>,
}

pub(crate) struct Incoming(Vec<u8>);
pub(crate) struct Outgoing<'a>(&'a mut VecDeque<Vec<u8>>);

impl RxToken for Incoming {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.0)
    }
}

impl TxToken for Outgoing<'_> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut packet = vec![0; len];
        let result = f(&mut packet);
        self.0.push_back(packet);
        result
    }
}

impl Device for Queues {
    type RxToken<'a> = Incoming;
    type TxToken<'a> = Outgoing<'a>;

    fn receive(&mut self, _now: Instant) -> Option<(Incoming, Outgoing<'_>)> {
        let packet = self.incoming.pop_front()?;
        Some((Incoming(packet), Outgoing(&mut self.outgoing)))
    }

    fn transmit(&mut self, _now: Instant) -> Option<Outgoing<'_>> {
        (self.outgoing.len() < QUEUE).then_some(Outgoing(&mut self.outgoing))
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ip;
        caps.max_transmission_unit = usize::from(MTU);
        caps
    }
}

/// What the stack is joined to.
pub struct Joins {
    pub from_tunnel: mpsc::Receiver<Vec<u8>>,
    pub to_tunnel: mpsc::Sender<Vec<u8>>,
    /// New connections from the local port forward.
    pub opened: mpsc::Receiver<Ends>,
    pub wake: Arc<Notify>,
}

pub struct Stack {
    server: SocketAddrV4,
    iface: Interface,
    sockets: SocketSet<'static>,
    conns: Vec<(SocketHandle, PumpState)>,
    /// Connections Melete turned away, and when to ask for each again.
    retrying: Vec<(std::time::Instant, PumpState)>,
    queues: Queues,
    joins: Joins,
    next_port: u16,
    started: std::time::Instant,
}

impl Stack {
    /// A stack at `own` in the tunnel, reaching `server`.
    pub fn new(own: std::net::Ipv4Addr, server: SocketAddrV4, joins: Joins) -> Stack {
        let mut queues = Queues::default();
        let mut config = Config::new(HardwareAddress::Ip);
        let mut seed = [0u8; 8];
        let _ = getrandom::fill(&mut seed);
        config.random_seed = u64::from_le_bytes(seed);
        // A random first port: a tunnel started again must not reuse the ports
        // of connections Melete still holds from the last one, whose packets it
        // would take as theirs.
        let span = u64::from(u16::MAX - FIRST_PORT) + 1;
        let first = FIRST_PORT + u16::try_from(u64::from_le_bytes(seed) % span).unwrap_or(0);
        let mut iface = Interface::new(config, &mut queues, Instant::from_millis(0));
        iface.update_ip_addrs(|addrs| {
            let _ = addrs.push(IpCidr::new(IpAddress::Ipv4(own), 32));
        });
        let _ = iface.routes_mut().add_default_ipv4_route(*server.ip());
        Stack {
            server,
            iface,
            sockets: SocketSet::new(Vec::new()),
            conns: Vec::new(),
            retrying: Vec::new(),
            queues,
            joins,
            next_port: first,
            started: std::time::Instant::now(),
        }
    }

    fn now(&self) -> Instant {
        let micros = self.started.elapsed().as_micros();
        Instant::from_micros(i64::try_from(micros).unwrap_or(i64::MAX))
    }

    /// Runs until the tunnel stops.
    pub async fn run(mut self) {
        let wake = self.joins.wake.clone();
        loop {
            let delay = self
                .iface
                .poll_delay(self.now(), &self.sockets)
                .map_or(IDLE, |d| Duration::from_micros(d.total_micros()))
                .min(if self.queues.outgoing.is_empty() {
                    IDLE
                } else {
                    BACKLOG
                })
                .min(self.next_retry());
            tokio::select! {
                got = self.joins.from_tunnel.recv() => match got {
                    Some(packet) => self.queues.incoming.push_back(packet),
                    None => return,
                },
                opened = self.joins.opened.recv() => match opened {
                    Some(ends) => self.open(ends),
                    None => return,
                },
                () = wake.notified() => {}
                () = tokio::time::sleep(delay) => {}
            }
            while let Ok(packet) = self.joins.from_tunnel.try_recv() {
                self.queues.incoming.push_back(packet);
            }
            if !self.step() {
                return;
            }
        }
    }

    /// One turn: packets in, bytes moved, packets out. False once WireGuard is gone.
    fn step(&mut self) -> bool {
        self.ask_again();
        let now = self.now();
        self.iface.poll(now, &mut self.queues, &mut self.sockets);
        for (handle, state) in &mut self.conns {
            pump(self.sockets.get_mut::<tcp::Socket>(*handle), state);
        }
        self.iface.poll(now, &mut self.queues, &mut self.sockets);
        // Closed sockets go only now, after the poll that sent their last packet.
        for (handle, mut state) in std::mem::take(&mut self.conns) {
            if self.sockets.get::<tcp::Socket>(handle).is_open() {
                self.conns.push((handle, state));
                continue;
            }
            self.sockets.remove(handle);
            if state.turned_away()
                && state.tries < RETRIES
                && state.asked.elapsed() < REFUSED_WITHIN
            {
                let wait = RETRY_AFTER * 2u32.pow(state.tries);
                state.tries += 1;
                self.retrying
                    .push((std::time::Instant::now() + wait, state));
            }
            // Otherwise its state goes, and the app's connection closes with it.
        }
        while let Some(packet) = self.queues.outgoing.pop_front() {
            match self.joins.to_tunnel.try_send(packet) {
                Ok(()) => {}
                Err(TrySendError::Full(packet)) => {
                    self.queues.outgoing.push_front(packet);
                    break;
                }
                Err(TrySendError::Closed(_)) => return false,
            }
        }
        true
    }

    fn open(&mut self, ends: Ends) {
        self.connect(PumpState::new(ends));
    }

    /// How long until a turned-away connection is next asked for.
    fn next_retry(&self) -> Duration {
        self.retrying
            .iter()
            .map(|(at, _)| at.saturating_duration_since(std::time::Instant::now()))
            .min()
            .unwrap_or(IDLE)
    }

    /// Asks again for the turned-away connections whose wait is over.
    fn ask_again(&mut self) {
        let now = std::time::Instant::now();
        let (due, later) = std::mem::take(&mut self.retrying)
            .into_iter()
            .partition::<Vec<_>, _>(|(at, _)| *at <= now);
        self.retrying = later;
        for (_, state) in due {
            self.connect(state);
        }
    }

    fn connect(&mut self, mut state: PumpState) {
        let rx = tcp::SocketBuffer::new(vec![0; RX_BUFFER]);
        let tx = tcp::SocketBuffer::new(vec![0; TX_BUFFER]);
        let mut socket = tcp::Socket::new(rx, tx);
        socket.set_timeout(Some(TIMEOUT.into()));
        socket.set_keep_alive(Some(KEEP_ALIVE.into()));
        socket.set_nagle_enabled(false);
        let port = self.next_port;
        self.next_port = self.next_port.checked_add(1).unwrap_or(FIRST_PORT);
        let remote = (IpAddress::Ipv4(*self.server.ip()), self.server.port());
        if socket.connect(self.iface.context(), remote, port).is_err() {
            return; // Dropping the ends closes the local connection at once.
        }
        state.asked = std::time::Instant::now();
        let handle = self.sockets.add(socket);
        self.conns.push((handle, state));
    }
}
