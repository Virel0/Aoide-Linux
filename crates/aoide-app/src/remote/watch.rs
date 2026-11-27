//! The watch over how Melete is reached: its usual address while that
//! answers, the tunnel while it doesn't. It runs while signed in with remote
//! access set up; ending it closes the tunnel and puts every request back on the
//! usual address.

use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::time::Duration;

use aoide_melete::{MeleteClient, Route, ServerAddress};
use aoide_tunnel::{Settings, Tunnel};
use iced::Subscription;
use iced::futures::{SinkExt, Stream};

use super::Way;
use crate::app::Message as App;
use crate::app::remote::Msg;

/// How long the usual address has to answer before the tunnel is tried.
const PROBE: Duration = Duration::from_secs(4);
/// How often the usual address is asked again while it answers.
const AT_HOME: Duration = Duration::from_secs(30);
/// How often it's asked while away, to drop the tunnel soon after coming home.
const AWAY: Duration = Duration::from_secs(20);
/// How long a tunnel stays open after coming home, unused by new requests: a
/// song already playing reads on through the address it started with.
const LINGER: Duration = Duration::from_secs(15 * 60);

#[derive(Clone)]
struct Watch {
    /// The signed-in client: its route is what moves.
    client: MeleteClient,
    /// A client for the usual address alone, whatever the route says.
    probe: MeleteClient,
    settings: Arc<Settings>,
}

impl Hash for Watch {
    fn hash<H: Hasher>(&self, state: &mut H) {
        "aoide-remote-access".hash(state);
        self.settings.endpoint.hash(state);
        self.settings.address.hash(state);
    }
}

/// The tunnel while in use; dropped, it puts the route back.
struct InUse {
    route: Route,
    tunnel: Option<Tunnel>,
    /// The tunnel last used, left open for what's still reading through it.
    retired: Option<(Tunnel, std::time::Instant)>,
}

impl InUse {
    fn use_tunnel(&mut self, tunnel: Tunnel) -> Result<(), String> {
        let local = format!("http://{}", tunnel.local_address());
        let base = ServerAddress::parse(&local)
            .map_err(|_| "the tunnel's local address didn't parse".to_owned())?;
        self.route.set(Some(&base));
        self.tunnel = Some(tunnel);
        Ok(())
    }

    /// New requests back to the usual address; the tunnel lingers a while.
    fn retire(&mut self) {
        self.route.set(None);
        if let Some(tunnel) = self.tunnel.take() {
            self.retired = Some((tunnel, std::time::Instant::now()));
            log::info!("Melete answers at its usual address again: tunnel retired");
        }
        if self
            .retired
            .as_ref()
            .is_some_and(|(_, since)| since.elapsed() >= LINGER)
        {
            self.retired = None;
        }
    }
}

impl Drop for InUse {
    fn drop(&mut self) {
        self.route.set(None);
    }
}

async fn answers(client: &MeleteClient) -> bool {
    matches!(
        tokio::time::timeout(PROBE, client.server_info()).await,
        Ok(Ok(_))
    )
}

/// One look: the way Melete is reached now, with the tunnel started or
/// retired to match.
async fn look(watch: &Watch, in_use: &mut InUse) -> Way {
    if answers(&watch.probe).await {
        in_use.retire();
        return Way::Direct;
    }
    if in_use.tunnel.is_none() {
        let tunnel = match in_use.retired.take() {
            Some((tunnel, _)) => tunnel,
            None => match Tunnel::start(&watch.settings).await {
                Ok(tunnel) => tunnel,
                Err(error) => return Way::Neither(error.to_string()),
            },
        };
        if let Err(words) = in_use.use_tunnel(tunnel) {
            return Way::Neither(words);
        }
    }
    if answers(&watch.client).await {
        return Way::Tunnel;
    }
    // A tunnel that doesn't answer is started afresh next time, its endpoint
    // looked up again: the home address may have changed.
    in_use.route.set(None);
    in_use.tunnel = None;
    in_use.retired = None;
    Way::Neither("Melete didn't answer through its tunnel either".to_owned())
}

fn run(watch: &Watch) -> impl Stream<Item = App> + use<> {
    let watch = watch.clone();
    iced::stream::channel(4, async move |mut output| {
        let mut in_use = InUse {
            route: watch.client.route().clone(),
            tunnel: None,
            retired: None,
        };
        loop {
            let way = look(&watch, &mut in_use).await;
            let wait = if in_use.tunnel.is_some() {
                AWAY
            } else {
                AT_HOME
            };
            if output.send(App::Remote(Msg::Way(way))).await.is_err() {
                return;
            }
            tokio::time::sleep(wait).await;
        }
    })
}

/// The watch, while signed in with remote access set up.
pub fn subscription(
    client: &MeleteClient,
    probe: Option<&MeleteClient>,
    settings: Option<&Arc<Settings>>,
) -> Subscription<App> {
    let (Some(probe), Some(settings)) = (probe, settings) else {
        return Subscription::none();
    };
    Subscription::run_with(
        Watch {
            client: client.clone(),
            probe: probe.clone(),
            settings: settings.clone(),
        },
        run,
    )
}

#[cfg(test)]
mod tests {
    //! Against a real Melete with its tunnel on, ignored unless asked for:
    //! `MELETE_LIVE_SERVER` (its HTTP `host:port`), `MELETE_LIVE_ENDPOINT` (its
    //! tunnel's `host:port`) and `MELETE_LIVE_TOKEN_FILE` (a signed-in token).

    use aoide_melete::{Credentials, Device};
    use aoide_tunnel::PrivateKey;

    use super::*;
    use crate::remote;

    fn env(name: &str) -> String {
        std::env::var(name).unwrap_or_else(|_| panic!("{name} isn't set"))
    }

    fn client(address: &str) -> MeleteClient {
        let token = std::fs::read_to_string(env("MELETE_LIVE_TOKEN_FILE")).unwrap();
        MeleteClient::new(
            ServerAddress::parse(address).unwrap(),
            Device::new("Test", "remote-watch-test"),
        )
        .unwrap()
        .with_credentials(Credentials {
            user_id: "1".into(),
            access_token: token.trim().to_owned(),
            username: "tester".into(),
        })
    }

    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "needs a real Melete with its tunnel on"]
    async fn away_goes_through_the_tunnel_and_home_drops_it() {
        let home = format!("http://{}", env("MELETE_LIVE_SERVER"));
        let key = PrivateKey::generate().unwrap();
        let mut joined = client(&home)
            .join_tunnel("remote watch test", &key.public_base64())
            .await
            .unwrap();
        joined.endpoint = env("MELETE_LIVE_ENDPOINT");
        let kept = remote::remembered("test", &joined);
        let settings = remote::settings(&kept, &remote::secret(&key, &joined.preshared_key));

        // Away: the usual address is a port nothing listens on.
        let away = client("http://127.0.0.1:9");
        let mut watch = Watch {
            client: away.clone(),
            probe: MeleteClient::new(away.address().clone(), away.device().clone()).unwrap(),
            settings: Arc::new(settings.unwrap()),
        };
        let mut in_use = InUse {
            route: away.route().clone(),
            tunnel: None,
            retired: None,
        };
        let through = look(&watch, &mut in_use).await;
        let me = away.me().await.map(|me| me.username);

        // Home again: the usual address answers.
        watch.probe =
            MeleteClient::new(ServerAddress::parse(&home).unwrap(), away.device().clone()).unwrap();
        let back = look(&watch, &mut in_use).await;
        let route_after = away.route().current();
        let (in_use_after, lingering) = (in_use.tunnel.is_some(), in_use.retired.is_some());
        drop(in_use);

        // Removed while away: through a tunnel of its own, the usual address dead.
        let left =
            crate::app::remote::leave_device(away.clone(), Some(watch.settings.clone()), joined.id)
                .await;
        let listed = client(&home).tunnel_devices().await.unwrap();
        let still_there = listed.devices.iter().any(|d| d.id == joined.id);

        assert_eq!(through, Way::Tunnel);
        assert_eq!(me.as_deref().ok(), Some("tester"), "{me:?}");
        assert_eq!(back, Way::Direct);
        assert!(route_after.is_none() && !in_use_after);
        assert!(
            lingering,
            "the tunnel lingers for what still reads through it"
        );
        assert!(left.is_ok(), "{left:?}");
        assert!(!still_there, "Melete no longer lists the device");
    }
}
