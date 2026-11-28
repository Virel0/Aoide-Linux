//! A whole tunnel on loopback: this crate's side, and a stand-in for Melete's
//! (GotaTun on a UDP port, smoltcp listening on port 80 and answering HTTP).

use std::net::{Ipv4Addr, SocketAddrV4};
use std::time::Duration;

use gotatun::device::Peer;
use gotatun::x25519::PublicKey;
use ipnetwork::{IpNetwork, Ipv4Network};
use smoltcp::iface::{Config, Interface, SocketSet};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{HardwareAddress, IpAddress, IpCidr};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{Notify, mpsc};

use super::*;

const MELETE: Ipv4Addr = Ipv4Addr::new(10, 73, 0, 1);
const DEVICE: Ipv4Addr = Ipv4Addr::new(10, 73, 0, 2);

/// Melete's side: answers each request with `body`, one connection at a time.
async fn melete(
    device_key: &PrivateKey,
    key: &PrivateKey,
    psk: [u8; 32],
    body: Vec<u8>,
    listeners: usize,
) -> u16 {
    let wake = Arc::new(Notify::new());
    let (to_stack, mut from_tunnel) = mpsc::channel(stack::QUEUE);
    let (to_tunnel, from_stack) = mpsc::channel::<Vec<u8>>(stack::QUEUE);
    let peer = Peer::new(PublicKey::from(&device_key.secret()))
        .with_allowed_ip(IpNetwork::V4(Ipv4Network::new(DEVICE, 32).unwrap()))
        .with_preshared_key(psk);
    let device = gotatun::device::build()
        .with_default_udp()
        .with_ip_pair(
            link::ToStack {
                packets: to_stack,
                wake: wake.clone(),
            },
            link::FromStack {
                packets: from_stack,
                mtu: stack::MTU,
            },
        )
        .with_private_key(key.secret())
        .with_peer(peer)
        .build()
        .await
        .unwrap();
    let port = device.read(async |d| d.listen_port()).await;
    tokio::spawn(async move {
        let _device = device;
        let mut queues = stack::Queues::default();
        let mut iface = Interface::new(
            Config::new(HardwareAddress::Ip),
            &mut queues,
            Instant::from_millis(0),
        );
        iface.update_ip_addrs(|a| a.push(IpCidr::new(IpAddress::Ipv4(MELETE), 24)).unwrap());
        let mut sockets = SocketSet::new(Vec::new());
        let listening = || {
            let mut s = tcp::Socket::new(
                tcp::SocketBuffer::new(vec![0; 65_536]),
                tcp::SocketBuffer::new(vec![0; 65_536]),
            );
            s.listen(80).unwrap();
            s
        };
        // Two listeners, as a server has a few: a connection that's never ended
        // keeps one, and two such leave none for the next.
        let mut conns: Vec<_> = (0..listeners)
            .map(|_| (sockets.add(listening()), Vec::new(), None::<usize>))
            .collect();
        let answer = [
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes(),
            body,
        ]
        .concat();
        let started = std::time::Instant::now();
        loop {
            tokio::select! {
                Some(p) = from_tunnel.recv() => queues.incoming.push_back(p),
                () = tokio::time::sleep(Duration::from_millis(5)) => {}
            }
            let now = Instant::from_millis(i64::try_from(started.elapsed().as_millis()).unwrap());
            iface.poll(now, &mut queues, &mut sockets);
            for (handle, asked, sent) in &mut conns {
                let socket = sockets.get_mut::<tcp::Socket>(*handle);
                while socket.can_recv() {
                    socket
                        .recv(|b| {
                            asked.extend_from_slice(b);
                            (b.len(), ())
                        })
                        .unwrap();
                }
                if sent.is_none() && asked.windows(4).any(|w| w == b"\r\n\r\n") {
                    *sent = Some(0);
                }
                if let Some(done) = sent.as_mut() {
                    while *done < answer.len() && socket.can_send() {
                        *done += socket.send_slice(&answer[*done..]).unwrap();
                    }
                    if *done == answer.len() {
                        socket.close();
                    }
                }
                if !socket.is_open() {
                    sockets.remove(*handle);
                    *handle = sockets.add(listening());
                    (*asked, *sent) = (Vec::new(), None);
                }
            }
            iface.poll(now, &mut queues, &mut sockets);
            while let Some(p) = queues.outgoing.pop_front() {
                let _ = to_tunnel.try_send(p);
            }
        }
    });
    port
}

/// A tunnel to a stand-in Melete answering every request with `body`.
async fn tunnel_to(body: Vec<u8>) -> Tunnel {
    tunnel_to_with(body, 2).await
}

/// The same, the stand-in listening for `listeners` connections at once.
async fn tunnel_to_with(body: Vec<u8>, listeners: usize) -> Tunnel {
    let (device_key, melete_key) = (
        PrivateKey::generate().unwrap(),
        PrivateKey::generate().unwrap(),
    );
    let psk = decode(&PrivateKey::generate().unwrap().to_base64()).unwrap();
    let port = melete(&device_key, &melete_key, psk, body, listeners).await;
    let settings = Settings {
        private_key: device_key,
        address: DEVICE,
        server_public_key: decode(&melete_key.public_base64()).unwrap(),
        preshared_key: psk,
        server: SocketAddrV4::new(MELETE, 80),
        endpoint: format!("127.0.0.1:{port}"),
    };
    Tunnel::start(&settings).await.unwrap()
}

async fn ask(tunnel: &Tunnel) -> TcpStream {
    let mut stream = TcpStream::connect(tunnel.local_address()).await.unwrap();
    let request = format!(
        "GET /api/v1/health HTTP/1.1\r\nHost: {}\r\n\r\n",
        tunnel.local_address()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    stream
}

async fn through(body: Vec<u8>) -> Vec<Vec<u8>> {
    let tunnel = tunnel_to(body).await;
    let mut answers = Vec::new();
    for _ in 0..2 {
        let mut stream = TcpStream::connect(tunnel.local_address()).await.unwrap();
        stream
            .write_all(
                format!(
                    "GET /api/v1/health HTTP/1.1\r\nHost: {}\r\n\r\n",
                    tunnel.local_address()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let mut answer = Vec::new();
        tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut answer))
            .await
            .unwrap()
            .unwrap();
        answers.push(answer);
    }
    answers
}

fn body_of(answer: &[u8]) -> &[u8] {
    let at = answer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("a whole HTTP answer");
    &answer[at + 4..]
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_reaches_melete_and_its_answer_comes_back() {
    for answer in through(b"{\"status\":\"ok\"}".to_vec()).await {
        assert!(answer.starts_with(b"HTTP/1.1 200 OK"));
        assert_eq!(body_of(&answer), b"{\"status\":\"ok\"}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_big_answer_comes_back_whole() {
    let body: Vec<u8> = (0..1_048_576u32).map(|i| (i % 251) as u8).collect();
    for answer in through(body.clone()).await {
        assert_eq!(body_of(&answer), body.as_slice());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn hanging_up_mid_answer_frees_melete_for_the_next_request() {
    let body: Vec<u8> = (0..1_048_576u32).map(|i| (i % 251) as u8).collect();
    let tunnel = tunnel_to(body.clone()).await;
    for _ in 0..3 {
        let mut stream = ask(&tunnel).await;
        let mut some = [0u8; 4096];
        stream.read_exact(&mut some).await.unwrap();
        drop(stream); // as a skip or a seek drops a download
    }
    // The stand-in serves one connection at a time: this is answered only if
    // each hung-up one was ended at Melete's side too.
    let (mut answer, mut stream) = (Vec::new(), ask(&tunnel).await);
    let read = stream.read_to_end(&mut answer);
    tokio::time::timeout(Duration::from_secs(20), read)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body_of(&answer), body.as_slice());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_big_request_reaches_melete() {
    let tunnel = tunnel_to(b"ok".to_vec()).await;
    let mut stream = TcpStream::connect(tunnel.local_address()).await.unwrap();
    let padding = "a".repeat(8_000);
    let request = format!(
        "GET /api/v1/me HTTP/1.1\r\nHost: {}\r\nX-Pad: {padding}\r\n\r\n",
        tunnel.local_address()
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let read = stream.read_to_end(&mut answer);
    tokio::time::timeout(Duration::from_secs(20), read)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body_of(&answer), b"ok");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_request_past_the_limit_takes_an_idle_connections_place_soon() {
    // Melete takes 8 connections from one peer.
    let tunnel = tunnel_to_with(b"ok".to_vec(), 8).await;
    // As an HTTP client leaves them: every place held by a connection that has
    // just answered and now sits idle in its pool.
    let mut idle = Vec::new();
    for _ in 0..crate::forward::AT_ONCE {
        idle.push(TcpStream::connect(tunnel.local_address()).await.unwrap());
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    let started = std::time::Instant::now();
    let (mut answer, mut stream) = (Vec::new(), ask(&tunnel).await);
    let read = stream.read_to_end(&mut answer);
    tokio::time::timeout(Duration::from_secs(20), read)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(body_of(&answer), b"ok");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "waited {:?} for a place",
        started.elapsed()
    );
    drop(idle);
}

#[tokio::test(flavor = "multi_thread")]
async fn connections_melete_turns_away_in_a_burst_are_made_again() {
    // Melete keeps a few sockets listening, refilled as it goes: a burst of
    // connections larger than that has the rest turned away at first.
    let tunnel = tunnel_to_with(b"ok".to_vec(), 1).await;
    let mut streams = Vec::new();
    for _ in 0..4 {
        streams.push(ask(&tunnel).await);
    }
    for mut stream in streams {
        let mut answer = Vec::new();
        tokio::time::timeout(Duration::from_secs(20), stream.read_to_end(&mut answer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(body_of(&answer), b"ok");
    }
}
