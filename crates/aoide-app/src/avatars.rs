//! Profile pictures: the signed-in person's, asked for at sign-in
//! (`/api/v1/me`), and others' as the server lists them. Each is kept in the
//! artwork cache like a cover, under a key that holds the server's path for
//! it; that path carries the picture's hash, so a new picture is a new key.
//!
//! A server from before profile pictures sends none, and the name's initial
//! is drawn instead (`widgets::avatar`).

use aoide_melete::MeleteClient;
use iced::Task;

use crate::app::Message;
use crate::artwork::ArtKey;

/// The item every profile picture is kept under; no album has it as an id.
const ITEM: &str = "avatar";

/// The artwork key for the picture at `path` (an `avatar_url`).
pub fn key(path: &str) -> ArtKey {
    ArtKey::new(ITEM, path)
}

/// The server path a key from [`key`] stands for; `None` for a cover.
pub fn path_of(key: &ArtKey) -> Option<&str> {
    (key.item == ITEM).then_some(key.tag.as_str())
}

/// Keys for the pictures at these paths, skipping those without one.
pub fn keys<'a>(paths: impl IntoIterator<Item = Option<&'a str>>) -> Vec<ArtKey> {
    paths.into_iter().flatten().map(key).collect()
}

/// Asks for the signed-in person's picture, at sign-in. A failure leaves
/// their initial.
pub fn ask(client: MeleteClient) -> Task<Message> {
    Task::perform(async move { client.me().await }, |answer| match answer {
        Ok(me) => Message::MyAvatar(me.avatar_url),
        Err(error) => {
            log::warn!("The server didn't give this account's picture: {error}");
            Message::Ignore
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_key_gives_back_its_path_and_a_cover_key_none() {
        let path = "/api/v1/users/7/avatar?v=0123456789ab";
        assert_eq!(path_of(&key(path)), Some(path));
        assert_eq!(path_of(&ArtKey::new("al1", aoide_melete::COVER_TAG)), None);
        assert_eq!(keys([Some(path), None]), [key(path)]);
    }

    #[test]
    fn a_new_picture_is_kept_apart_from_the_old() {
        let old = key("/api/v1/users/7/avatar?v=0123456789ab").file_name();
        let new = key("/api/v1/users/7/avatar?v=ba9876543210").file_name();
        assert_ne!(old, new);
        assert!(!old.contains('/') && !old.contains('?'), "{old}");
    }
}
