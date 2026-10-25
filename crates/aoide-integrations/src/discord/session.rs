//! The task behind [`super::Presence`]: find Discord, keep one session open
//! while enabled, send what changed.

use std::path::Path;
use std::time::Duration;

use serde_json::{Value, json};
use tokio::net::UnixStream;
use tokio::net::unix::OwnedWriteHalf;
use tokio::sync::{mpsc, watch};
use tokio::time::{Instant, sleep, timeout};

use super::activity::{activity, same_activity};
use super::ipc::{CLOSE, FRAME, HANDSHAKE, PING, PONG, read_frame, write_frame};
use super::{APPLICATION_ID, Desired, Endpoint, PresenceStatus, Timing};

/// How long Discord has to answer the handshake.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

enum End {
    /// The handle was dropped.
    Closed,
    /// Turned off by the options.
    Disabled,
    /// Discord went away or misbehaved.
    Lost,
}

pub(super) async fn run(
    endpoint: Endpoint,
    mut desired: watch::Receiver<Desired>,
    status: watch::Sender<PresenceStatus>,
    timing: Timing,
) {
    loop {
        while !desired.borrow_and_update().options.enabled {
            status.send_replace(PresenceStatus::Off);
            if desired.changed().await.is_err() {
                return;
            }
        }
        status.send_replace(PresenceStatus::Searching);
        let Some(stream) = connect(&endpoint).await else {
            tokio::select! {
                () = sleep(timing.retry) => {}
                changed = desired.changed() => if changed.is_err() { return },
            }
            continue;
        };
        status.send_replace(PresenceStatus::Connected);
        match session(stream, &mut desired, timing).await {
            End::Closed => return,
            End::Disabled | End::Lost => {}
        }
    }
}

/// The first candidate socket that accepts and completes the handshake.
async fn connect(endpoint: &Endpoint) -> Option<UnixStream> {
    for path in endpoint.candidates() {
        if let Some(stream) = handshake(&path).await {
            return Some(stream);
        }
    }
    None
}

async fn handshake(path: &Path) -> Option<UnixStream> {
    let mut stream = UnixStream::connect(path).await.ok()?;
    let hello = json!({ "v": 1, "client_id": APPLICATION_ID });
    write_frame(&mut stream, HANDSHAKE, &hello).await.ok()?;
    let (opcode, reply) = timeout(HANDSHAKE_TIMEOUT, read_frame(&mut stream))
        .await
        .ok()?
        .ok()?;
    (opcode == FRAME && reply.get("evt").and_then(Value::as_str) == Some("READY")).then_some(stream)
}

async fn set_activity(
    writer: &mut OwnedWriteHalf,
    nonce: &mut u64,
    activity: Option<&Value>,
) -> bool {
    *nonce += 1;
    let mut args = json!({ "pid": std::process::id() });
    if let Some(activity) = activity {
        args["activity"] = activity.clone();
    }
    let command = json!({ "cmd": "SET_ACTIVITY", "args": args, "nonce": nonce.to_string() });
    write_frame(writer, FRAME, &command).await.is_ok()
}

async fn session(
    stream: UnixStream,
    desired: &mut watch::Receiver<Desired>,
    timing: Timing,
) -> End {
    let (mut reader, mut writer) = stream.into_split();
    // Frames are read on a task of their own: a read cut short by `select!`
    // would lose its place in the stream.
    let (frames_tx, mut frames) = mpsc::channel(8);
    let reading = tokio::spawn(async move {
        while let Ok(frame) = read_frame(&mut reader).await {
            if frames_tx.send(frame).await.is_err() {
                break;
            }
        }
    });
    let end = serve(&mut writer, &mut frames, desired, timing).await;
    reading.abort();
    end
}

async fn serve(
    writer: &mut OwnedWriteHalf,
    frames: &mut mpsc::Receiver<(u32, Value)>,
    desired: &mut watch::Receiver<Desired>,
    timing: Timing,
) -> End {
    // A new session starts with no activity of ours on Discord's side.
    let mut shown: Option<Value> = None;
    let mut last_send: Option<Instant> = None;
    let mut nonce = 0u64;
    loop {
        let (enabled, wanted) = {
            let d = desired.borrow_and_update();
            (d.options.enabled, activity(d.now.as_ref(), &d.options))
        };
        if !enabled {
            if shown.is_some() {
                set_activity(writer, &mut nonce, None).await;
            }
            return End::Disabled;
        }

        let mut wait = None;
        if !same_activity(shown.as_ref(), wanted.as_ref()) {
            let due = last_send.map_or_else(Instant::now, |t| t + timing.min_interval);
            if due <= Instant::now() {
                if !set_activity(writer, &mut nonce, wanted.as_ref()).await {
                    return End::Lost;
                }
                shown = wanted;
                last_send = Some(Instant::now());
            } else {
                wait = Some(due);
            }
        }

        tokio::select! {
            changed = desired.changed() => if changed.is_err() { return End::Closed },
            frame = frames.recv() => match frame {
                None | Some((CLOSE, _)) => return End::Lost,
                Some((PING, payload)) => {
                    if write_frame(writer, PONG, &payload).await.is_err() {
                        return End::Lost;
                    }
                }
                // Replies to SET_ACTIVITY. An error reply (bad field) is not
                // worth a reconnect; the next change sends afresh.
                Some(_) => {}
            },
            () = tokio::time::sleep_until(wait.unwrap_or_else(Instant::now)), if wait.is_some() => {}
        }
    }
}
