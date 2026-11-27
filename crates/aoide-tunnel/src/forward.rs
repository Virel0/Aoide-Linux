//! The local end: a port on 127.0.0.1 whose connections are carried through the
//! tunnel to Melete, so the app's HTTP clients and GStreamer need nothing more
//! than another address.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, Semaphore, mpsc};

use crate::pump::{CHUNK, Ends};

/// Connections through the tunnel at once. Melete takes 8 from one peer; one is
/// left spare for a request made while the others close.
pub const AT_ONCE: usize = 7;
/// A connection that moves nothing for this long is closed, just inside the
/// minute Melete gives one.
const IDLE_LIMIT: Duration = Duration::from_secs(55);
/// A connection idle this long gives its place to one waiting for it: HTTP
/// clients keep idle connections far longer than they need them.
const IDLE_ENOUGH: Duration = Duration::from_secs(1);
/// Messages held each way for one connection.
const DEPTH: usize = 8;

/// One connection holding a place: when it last moved a byte, and how to end it.
struct Place {
    last: Arc<AtomicU64>,
    end: Arc<Notify>,
}

#[derive(Clone)]
struct Places {
    free: Arc<Semaphore>,
    held: Arc<Mutex<Vec<Place>>>,
    started: Instant,
}

impl Places {
    fn now(&self) -> u64 {
        u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    /// Ends the connection idle longest, if one has idled long enough.
    fn make_room(&self) {
        let held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let now = self.now();
        let idlest = held
            .iter()
            .filter(|p| now.saturating_sub(p.last.load(Ordering::Relaxed)) >= millis(IDLE_ENOUGH))
            .min_by_key(|p| p.last.load(Ordering::Relaxed));
        if let Some(place) = idlest {
            place.end.notify_one();
        }
    }
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Takes local connections until the tunnel stops.
pub async fn serve(listener: TcpListener, opened: mpsc::Sender<Ends>, wake: Arc<Notify>) {
    let port = listener.local_addr().map_or(0, |a| a.port());
    let places = Places {
        free: Arc::new(Semaphore::new(AT_ONCE)),
        held: Arc::default(),
        started: Instant::now(),
    };
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let (opened, wake, places) = (opened.clone(), wake.clone(), places.clone());
        tokio::spawn(async move {
            // Past the limit, a connection waits for a place rather than being refused.
            let place = match places.free.clone().try_acquire_owned() {
                Ok(place) => place,
                Err(_) => {
                    places.make_room();
                    let Ok(place) = places.free.clone().acquire_owned().await else {
                        return;
                    };
                    place
                }
            };
            let (last, end) = (
                Arc::new(AtomicU64::new(places.now())),
                Arc::new(Notify::new()),
            );
            places
                .held
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Place {
                    last: last.clone(),
                    end: end.clone(),
                });
            let (to_melete, outgoing) = mpsc::channel(DEPTH);
            let (incoming, from_melete) = mpsc::channel(DEPTH);
            if opened.send(Ends { outgoing, incoming }).await.is_ok() {
                wake.notify_one();
                let watch = Watch {
                    places: &places,
                    last: &last,
                    end: &end,
                    port,
                };
                carry(stream, to_melete, from_melete, &wake, watch).await;
            }
            places
                .held
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .retain(|p| !Arc::ptr_eq(&p.last, &last));
            drop(place);
        });
    }
}

/// What a connection's carrying is watched by.
struct Watch<'a> {
    places: &'a Places,
    last: &'a AtomicU64,
    end: &'a Notify,
    port: u16,
}

/// Moves one connection's bytes both ways until either side ends, it idles,
/// or its place is wanted.
async fn carry(
    stream: TcpStream,
    to_melete: mpsc::Sender<Vec<u8>>,
    mut from_melete: mpsc::Receiver<Vec<u8>>,
    wake: &Notify,
    watch: Watch<'_>,
) {
    let _ = stream.set_nodelay(true);
    let (mut reader, mut writer) = stream.into_split();
    let mut to_melete = Some(to_melete);
    let mut buffer = vec![0u8; CHUNK];
    let mut first = true;
    loop {
        tokio::select! {
            read = reader.read(&mut buffer), if to_melete.is_some() => {
                match read {
                    Ok(n) if n > 0 => {
                        if std::mem::take(&mut first) && foreign_host(&buffer[..n], watch.port) {
                            log::warn!("refused a connection to the tunnel's port naming another host");
                            return;
                        }
                        let Some(sender) = &to_melete else { continue };
                        if sender.send(buffer[..n].to_vec()).await.is_err() {
                            return;
                        }
                    }
                    // The app has finished sending: Melete is told, and its answer still comes.
                    _ => to_melete = None,
                }
                watch.last.store(watch.places.now(), Ordering::Relaxed);
                wake.notify_one();
            }
            bytes = from_melete.recv() => {
                let Some(bytes) = bytes else {
                    let _ = writer.shutdown().await;
                    return;
                };
                if writer.write_all(&bytes).await.is_err() {
                    return;
                }
                watch.last.store(watch.places.now(), Ordering::Relaxed);
                // Room in the channel again: the stack may read more.
                wake.notify_one();
            }
            () = watch.end.notified() => return,
            () = tokio::time::sleep(IDLE_LIMIT) => return,
        }
    }
}

/// Whether a request names a host other than this port's own: what a web page
/// would send after pointing a name of its own at 127.0.0.1 (DNS rebinding).
/// Only the first request's headers are looked at; one without a Host passes.
fn foreign_host(request: &[u8], port: u16) -> bool {
    let head = String::from_utf8_lossy(&request[..request.len().min(CHUNK)]);
    let Some(host) = head
        .split("\r\n")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("host")
                .then(|| value.trim().to_ascii_lowercase())
        })
    else {
        return false;
    };
    ![format!("127.0.0.1:{port}"), format!("localhost:{port}")].contains(&host)
}

#[cfg(test)]
mod tests {
    use super::foreign_host;

    #[test]
    fn only_the_local_port_itself_is_let_through() {
        let ask = |host: &str| format!("GET / HTTP/1.1\r\nHost: {host}\r\n\r\n").into_bytes();
        assert!(!foreign_host(&ask("127.0.0.1:4100"), 4100));
        assert!(!foreign_host(&ask("LOCALHOST:4100"), 4100));
        assert!(foreign_host(&ask("evil.example:4100"), 4100));
        assert!(foreign_host(&ask("127.0.0.1:9999"), 4100));
        assert!(!foreign_host(b"GET / HTTP/1.1\r\n\r\n", 4100));
    }
}
