//! A stream link the player can decode.
//!
//! `StreamFormat::Original` sends the file as it is, and symphonia has no
//! decoder for some of what a library can hold: Opus, WavPack, Monkey's Audio,
//! Dolby. Such a file is asked for again as FLAC, which holds exactly what it
//! decodes to. Ogg is Vorbis, which plays, or Opus, which doesn't, so for Ogg
//! the track's codec is looked up; the other formats are made for the player.

use aoide_melete::{MeleteClient, Stream, StreamFormat};

/// The link for `track_id` in `format`, or in FLAC when the file wouldn't play.
///
/// # Errors
///
/// What [`MeleteClient::play`] fails with. A failed codec lookup isn't an
/// error: the track is asked for as FLAC.
pub async fn playable_stream(
    client: &MeleteClient,
    track_id: &str,
    format: StreamFormat,
) -> aoide_melete::Result<Stream> {
    let stream = client.play(track_id, format).await?;
    if format != StreamFormat::Original {
        return Ok(stream);
    }
    let codec = if base_type(&stream.content_type) == "audio/ogg" {
        client
            .item_details(track_id)
            .await
            .ok()
            .and_then(|d| d.codec)
    } else {
        None
    };
    if plays_as_is(&stream.content_type, codec.as_deref()) {
        Ok(stream)
    } else {
        client.play(track_id, StreamFormat::Flac).await
    }
}

/// Whether the player decodes a file of this content type (and, for Ogg, codec).
fn plays_as_is(content_type: &str, codec: Option<&str>) -> bool {
    match base_type(content_type).as_str() {
        "audio/flac" | "audio/mpeg" | "audio/mp4" | "audio/aac" | "audio/wav" | "audio/x-wav"
        | "audio/aiff" | "audio/x-aiff" => true,
        "audio/ogg" => codec.is_some_and(|codec| codec.eq_ignore_ascii_case("vorbis")),
        _ => false,
    }
}

fn base_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use aoide_melete::{Credentials, Device, ServerAddress};
    use serde_json::json;
    use wiremock::matchers::{body_partial_json, method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn client(server: &MockServer) -> MeleteClient {
        MeleteClient::new(
            ServerAddress::parse(&server.uri()).unwrap(),
            Device::new("Test", "device-1"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "u1".into(),
            access_token: "tok".into(),
            username: "alice".into(),
        })
    }

    fn link(name: &str, content_type: &str) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({
            "stream_url": format!("/api/v1/music/stream/{name}"), "expires_at": 1,
            "format": name, "content_type": content_type,
        }))
    }

    async fn serve(server: &MockServer, original: &str, codec: &str) {
        Mock::given(method("POST"))
            .and(path("/api/v1/music/tracks/5/play"))
            .and(body_partial_json(json!({"format": "original"})))
            .respond_with(link("original", original))
            .mount(server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/music/tracks/5/play"))
            .and(body_partial_json(json!({"format": "flac"})))
            .respond_with(link("flac", "audio/flac"))
            .mount(server)
            .await;
        let mut track = crate::test_json::track(5, "Song");
        track["codec"] = json!(codec);
        Mock::given(method("GET"))
            .and(path("/api/v1/music/tracks"))
            .and(query_param("id", "5"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(crate::test_json::page(vec![track])),
            )
            .mount(server)
            .await;
    }

    async fn served(original: &str, codec: &str, format: StreamFormat) -> String {
        let server = MockServer::start().await;
        serve(&server, original, codec).await;
        let stream = playable_stream(&client(&server), "5", format)
            .await
            .unwrap();
        stream.url.path().rsplit('/').next().unwrap().to_owned()
    }

    #[tokio::test]
    async fn an_opus_file_is_asked_for_as_flac() {
        assert_eq!(
            served("audio/ogg", "opus", StreamFormat::Original).await,
            "flac"
        );
    }

    #[tokio::test]
    async fn vorbis_and_the_formats_the_player_decodes_stay_as_they_are() {
        assert_eq!(
            served("audio/ogg", "vorbis", StreamFormat::Original).await,
            "original"
        );
        assert_eq!(
            served("audio/flac", "flac", StreamFormat::Original).await,
            "original"
        );
        assert_eq!(
            served("audio/mp4", "alac", StreamFormat::Original).await,
            "original"
        );
    }

    #[tokio::test]
    async fn a_file_with_no_decoder_here_is_asked_for_as_flac() {
        assert_eq!(
            served("audio/x-wavpack", "wavpack", StreamFormat::Original).await,
            "flac"
        );
    }

    #[test]
    fn content_types_are_read_without_their_parameters() {
        assert!(plays_as_is("Audio/MPEG; charset=binary", None));
        assert!(!plays_as_is("audio/ogg", None), "Ogg of an unknown codec");
        assert!(!plays_as_is("audio/x-ape", None));
    }
}
