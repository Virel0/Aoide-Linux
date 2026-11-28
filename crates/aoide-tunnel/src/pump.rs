//! One connection's bytes between its smoltcp socket and the local port
//! forward's channels, each turn of the stack.

use smoltcp::socket::tcp;
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TryRecvError;

/// The most read from the socket into one message.
pub const CHUNK: usize = 16 * 1024;

/// The stack's ends of one connection's channels.
pub struct Ends {
    /// Bytes the app sent, for Melete.
    pub outgoing: mpsc::Receiver<Vec<u8>>,
    /// Bytes Melete sent, for the app.
    pub incoming: mpsc::Sender<Vec<u8>>,
}

/// Where one connection stands between turns.
pub struct PumpState {
    outgoing: Option<mpsc::Receiver<Vec<u8>>>,
    incoming: Option<mpsc::Sender<Vec<u8>>>,
    /// Bytes taken from the app that the socket hasn't room for yet.
    unsent: Vec<u8>,
    /// Whether Melete took the connection: one it turned away may be asked again.
    made: bool,
    /// Times Melete has turned it away.
    pub(crate) tries: u32,
    /// When its socket last asked Melete for it.
    pub(crate) asked: std::time::Instant,
}

impl PumpState {
    pub fn new(ends: Ends) -> Self {
        Self {
            outgoing: Some(ends.outgoing),
            incoming: Some(ends.incoming),
            unsent: Vec::new(),
            made: false,
            tries: 0,
            asked: std::time::Instant::now(),
        }
    }

    /// Closed before Melete took it, with the app still waiting on it.
    pub(crate) fn turned_away(&self) -> bool {
        !self.made && self.incoming.as_ref().is_some_and(|i| !i.is_closed())
    }
}

/// Moves what it can each way. Once the socket closes, the app's side ends
/// with it; the stack forgets the socket after its next poll, so a reset
/// queued here still goes out. A socket Melete turned away leaves the app's
/// side open, for the stack to ask again.
pub fn pump(socket: &mut tcp::Socket<'_>, state: &mut PumpState) {
    if socket.is_open() && socket.state() != tcp::State::SynSent {
        state.made = true;
    }
    to_app(socket, state);
    to_melete(socket, state);
    if !socket.is_open() && state.made {
        state.incoming = None;
    }
}

/// Melete's bytes on to the app, as fast as the app takes them: a full channel
/// leaves them in the socket, whose window then holds Melete back.
fn to_app(socket: &mut tcp::Socket<'_>, state: &mut PumpState) {
    let Some(incoming) = &state.incoming else {
        return;
    };
    if incoming.is_closed() {
        // The app hung up: Melete is told at once, so it stops sending and
        // frees the connection rather than holding it to its own time limit.
        state.incoming = None;
        state.outgoing = None;
        socket.abort();
        return;
    }
    while socket.can_recv() {
        let Ok(permit) = incoming.try_reserve() else {
            // Full: the rest waits in the socket. Closed: seen next turn.
            return;
        };
        let read = socket.recv(|buffer| {
            let taken = buffer.len().min(CHUNK);
            (taken, buffer[..taken].to_vec())
        });
        match read {
            Ok(bytes) if !bytes.is_empty() => permit.send(bytes),
            _ => break,
        }
    }
    if state.made && !socket.may_recv() {
        // Melete has finished sending: the app reads to the end and no further.
        state.incoming = None;
    }
}

/// The app's bytes on to Melete, as the socket has room.
fn to_melete(socket: &mut tcp::Socket<'_>, state: &mut PumpState) {
    loop {
        if state.unsent.is_empty() {
            let Some(outgoing) = &mut state.outgoing else {
                return;
            };
            match outgoing.try_recv() {
                Ok(bytes) => state.unsent = bytes,
                Err(TryRecvError::Empty) => return,
                Err(TryRecvError::Disconnected) => {
                    // The app has finished sending.
                    state.outgoing = None;
                    socket.close();
                    return;
                }
            }
        }
        if !socket.can_send() {
            return;
        }
        match socket.send_slice(&state.unsent) {
            Ok(sent) if sent > 0 => {
                state.unsent.drain(..sent);
            }
            _ => return,
        }
    }
}
