//! Discord's local RPC: where its socket is, and the frames spoken on it.
//!
//! Each frame is a little-endian `u32` opcode, a little-endian `u32` length
//! and that many bytes of JSON. The client opens with a `HANDSHAKE`
//! (`{"v":1,"client_id":…}`), Discord answers with a `FRAME` carrying the
//! `READY` event, and commands such as `SET_ACTIVITY` then go as `FRAME`s,
//! each answered by a `FRAME` with the same `nonce`. `PING` is answered with
//! `PONG` carrying the same payload; `CLOSE` ends the session.

use std::ffi::OsString;
use std::path::PathBuf;

use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const HANDSHAKE: u32 = 0;
pub const FRAME: u32 = 1;
pub const CLOSE: u32 = 2;
pub const PING: u32 = 3;
pub const PONG: u32 = 4;

/// Discord's frames are small; anything this big is not Discord.
const MAX_FRAME: u32 = 64 * 1024;

/// Directories under a runtime directory where a sandboxed Discord puts its
/// socket: the plain one, the Flatpak apps (Discord, Discord Canary,
/// Vesktop) and the snap.
const SUBDIRS: &[&str] = &[
    "",
    "app/com.discordapp.Discord",
    "app/com.discordapp.DiscordCanary",
    ".flatpak/dev.vencord.Vesktop/xdg-run",
    "snap.discord",
    "snap.discord-canary",
];

/// Every place Discord's socket may be, in the order to try them:
/// `discord-ipc-0` to `-9` under `XDG_RUNTIME_DIR`, each also under the
/// Flatpak and snap subdirectories. Only without a runtime directory (macOS,
/// some setups) under `TMPDIR`, `TMP`, `TEMP` and `/tmp`: those can be
/// shared with other users, who could otherwise put a socket there first
/// and be told what plays. `env` reads an environment variable.
pub fn socket_candidates(env: impl Fn(&str) -> Option<OsString>) -> Vec<PathBuf> {
    let read = |name: &str| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let bases: Vec<PathBuf> = match read("XDG_RUNTIME_DIR") {
        Some(runtime) => vec![runtime],
        None => ["TMPDIR", "TMP", "TEMP"]
            .into_iter()
            .filter_map(read)
            .chain([PathBuf::from("/tmp")])
            .collect(),
    };
    let mut seen = Vec::new();
    let mut candidates = Vec::new();
    for base in bases {
        if seen.contains(&base) {
            continue;
        }
        for sub in SUBDIRS {
            for n in 0..10 {
                candidates.push(base.join(sub).join(format!("discord-ipc-{n}")));
            }
        }
        seen.push(base);
    }
    candidates
}

#[derive(Debug, thiserror::Error)]
pub enum FrameError {
    #[error("socket: {0}")]
    Io(#[from] std::io::Error),
    #[error("frame of {0} bytes is too large")]
    TooLarge(u32),
    #[error("frame is not JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    opcode: u32,
    payload: &Value,
) -> Result<(), FrameError> {
    let body = serde_json::to_vec(payload)?;
    let length = u32::try_from(body.len()).map_err(|_| FrameError::TooLarge(u32::MAX))?;
    let mut frame = Vec::with_capacity(8 + body.len());
    frame.extend_from_slice(&opcode.to_le_bytes());
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(&body);
    writer.write_all(&frame).await?;
    writer.flush().await?;
    Ok(())
}

pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<(u32, Value), FrameError> {
    let mut header = [0u8; 8];
    reader.read_exact(&mut header).await?;
    let opcode = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
    let length = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    if length > MAX_FRAME {
        return Err(FrameError::TooLarge(length));
    }
    let mut body = vec![0u8; length as usize];
    reader.read_exact(&mut body).await?;
    let payload = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)?
    };
    Ok((opcode, payload))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn candidates_cover_the_runtime_dir_flatpak_and_snap() {
        let env = |name: &str| match name {
            "XDG_RUNTIME_DIR" => Some(OsString::from("/run/user/1000")),
            _ => None,
        };
        let all = socket_candidates(env);
        assert_eq!(all[0], PathBuf::from("/run/user/1000/discord-ipc-0"));
        assert_eq!(all[9], PathBuf::from("/run/user/1000/discord-ipc-9"));
        for expected in [
            "/run/user/1000/app/com.discordapp.Discord/discord-ipc-0",
            "/run/user/1000/.flatpak/dev.vencord.Vesktop/xdg-run/discord-ipc-3",
            "/run/user/1000/snap.discord/discord-ipc-0",
        ] {
            assert!(all.contains(&PathBuf::from(expected)), "{expected}");
        }
        // The runtime dir alone, six places, ten numbers: never /tmp, which
        // other users can write to.
        assert_eq!(all.len(), 6 * 10);
        assert!(!all.iter().any(|p| p.starts_with("/tmp")));
    }

    #[test]
    fn candidates_skip_empty_and_repeated_directories() {
        let env = |name: &str| match name {
            "XDG_RUNTIME_DIR" => Some(OsString::new()),
            "TMPDIR" => Some(OsString::from("/tmp")),
            _ => None,
        };
        let all = socket_candidates(env);
        assert_eq!(all.len(), 6 * 10);
        assert_eq!(all[0], PathBuf::from("/tmp/discord-ipc-0"));
    }

    #[tokio::test]
    async fn frames_round_trip() {
        let payload = json!({"v": 1, "client_id": "123"});
        let mut buffer = Vec::new();
        write_frame(&mut buffer, HANDSHAKE, &payload).await.unwrap();
        assert_eq!(&buffer[..4], &[0, 0, 0, 0]);
        let length = u32::from_le_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]) as usize;
        assert_eq!(length, buffer.len() - 8);
        let (opcode, back) = read_frame(&mut buffer.as_slice()).await.unwrap();
        assert_eq!((opcode, back), (HANDSHAKE, payload));
    }

    #[tokio::test]
    async fn an_oversized_frame_is_refused() {
        let mut frame = Vec::new();
        frame.extend_from_slice(&FRAME.to_le_bytes());
        frame.extend_from_slice(&(10 * 1024 * 1024u32).to_le_bytes());
        assert!(matches!(
            read_frame(&mut frame.as_slice()).await,
            Err(FrameError::TooLarge(_))
        ));
    }
}
