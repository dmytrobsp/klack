//! The vendored rtc-turn's answer to a TURN "438 Stale Nonce" (VX5b, `vendor/rtc-turn/KLACK-PATCH.md`).
//!
//! Chime's TURN server can answer a refresh with 438 about ten minutes into a huddle. Upstream
//! rtc-turn 0.21.1 then deletes the channel binding, so the meeting's audio, still relayed on that
//! channel, is dropped until the media connection fails. These tests drive the client through its
//! public Sans-I/O API against a scripted server and check that each kind of request is sent again
//! at once with the new nonce, and that nothing is torn down meanwhile.

use bytes::BytesMut;
use rtc::sansio::Protocol;
use rtc::shared::{TaggedBytesMut, TransportContext, TransportProtocol};
use rtc::stun::attributes::{ATTR_NONCE, ATTR_REALM};
use rtc::stun::error_code::{CODE_STALE_NONCE, CODE_UNAUTHORIZED, ErrorCodeAttribute};
use rtc::stun::message::*;
use rtc::stun::textattrs::TextAttribute;
use rtc::turn::client::{Client, ClientConfig, Event};
use rtc::turn::proto::chandata::ChannelData;
use rtc::turn::proto::channum::ChannelNumber;
use rtc::turn::proto::lifetime::Lifetime;
use rtc::turn::proto::relayaddr::RelayedAddress;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

fn addr(s: &str) -> SocketAddr {
    s.parse().unwrap()
}

const SERVER: &str = "127.0.0.1:3478";
const LOCAL: &str = "127.0.0.1:40000";
const RELAYED: &str = "127.0.0.1:50000";
const PEER: &str = "127.0.0.1:60000";

/// What the client wrote: a STUN message, or ChannelData on a channel.
enum Out {
    Stun(Message),
    Channel(u16),
}

/// A TURN client and a scripted server that answers its requests.
struct Turn {
    client: Client,
    base: Instant,
}

impl Turn {
    fn at(&self, secs: u64) -> Instant {
        self.base + Duration::from_secs(secs)
    }

    fn new() -> Self {
        let config = ClientConfig {
            turn_serv_addr: SERVER.to_owned(),
            local_addr: addr(LOCAL),
            transport_protocol: TransportProtocol::UDP,
            username: "user".to_owned(),
            password: "pass".to_owned(),
            realm: "realm".to_owned(),
            ..Default::default()
        };
        let provider = rtc::turn::crypto::default_provider().unwrap();
        Self { client: Client::new(config, provider).unwrap(), base: Instant::now() }
    }

    /// A client with a granted allocation (lifetime 600 s) at t = 0.
    fn allocated() -> Self {
        let mut turn = Self::new();
        let now = turn.at(0);
        turn.client.allocate(now).unwrap();
        let attempt = turn.next_request();
        let unauthorized = vec![
            error(CODE_UNAUTHORIZED),
            text(ATTR_REALM, "realm"),
            text(ATTR_NONCE, "nonce-0"),
        ];
        turn.answer(now, &attempt, CLASS_ERROR_RESPONSE, unauthorized);
        let request = turn.next_request();
        let granted: Vec<Box<dyn Setter>> = vec![
            Box::new(RelayedAddress { ip: addr(RELAYED).ip(), port: addr(RELAYED).port() }),
            Box::new(Lifetime(Duration::from_secs(600))),
        ];
        turn.answer(now, &request, CLASS_SUCCESS_RESPONSE, granted);
        assert!(
            turn.events().iter().any(|e| matches!(e, Event::AllocateResponse(..))),
            "the allocation is granted"
        );
        turn
    }

    /// Everything the client has written since the last call.
    fn sent(&mut self) -> Vec<Out> {
        let mut out = Vec::new();
        while let Some(m) = self.client.poll_write() {
            if ChannelData::is_channel_data(&m.message) {
                let mut data = ChannelData { raw: m.message.to_vec(), ..Default::default() };
                data.decode().unwrap();
                out.push(Out::Channel(data.number.0));
            } else {
                let mut msg = Message::new();
                msg.raw = m.message.to_vec();
                msg.decode().unwrap();
                out.push(Out::Stun(msg));
            }
        }
        out
    }

    /// The requests the client has sent since the last call (indications and ChannelData left out).
    fn requests(&mut self) -> Vec<Message> {
        self.sent()
            .into_iter()
            .filter_map(|o| match o {
                Out::Stun(m) if m.typ.class == CLASS_REQUEST => Some(m),
                _ => None,
            })
            .collect()
    }

    fn next_request(&mut self) -> Message {
        let mut requests = self.requests();
        assert_eq!(requests.len(), 1, "exactly one request was sent");
        requests.remove(0)
    }

    fn events(&mut self) -> Vec<Event> {
        std::iter::from_fn(|| self.client.poll_event()).collect()
    }

    fn deliver(&mut self, now: Instant, raw: Vec<u8>) -> rtc::shared::error::Result<()> {
        self.client.handle_read(TaggedBytesMut {
            now,
            transport: TransportContext {
                local_addr: addr(LOCAL),
                peer_addr: addr(SERVER),
                transport_protocol: TransportProtocol::UDP,
                ecn: None,
            },
            message: BytesMut::from(&raw[..]),
        })
    }

    fn answer(&mut self, now: Instant, req: &Message, class: MessageClass, attrs: Vec<Box<dyn Setter>>) {
        let mut setters: Vec<Box<dyn Setter>> =
            vec![Box::new(req.transaction_id), Box::new(MessageType::new(req.typ.method, class))];
        setters.extend(attrs);
        let mut res = Message::new();
        res.build(&setters).unwrap();
        self.deliver(now, res.raw).unwrap();
    }

    fn ok(&mut self, now: Instant, req: &Message) {
        let attrs: Vec<Box<dyn Setter>> = if req.typ.method == METHOD_REFRESH {
            vec![Box::new(Lifetime(Duration::from_secs(600)))]
        } else {
            vec![]
        };
        self.answer(now, req, CLASS_SUCCESS_RESPONSE, attrs);
    }

    fn stale(&mut self, now: Instant, req: &Message, nonce: &str) {
        self.answer(now, req, CLASS_ERROR_RESPONSE, vec![error(CODE_STALE_NONCE), text(ATTR_NONCE, nonce)]);
    }

    /// Relayed data from the peer, as the server sends it on `channel`.
    fn from_peer(&mut self, now: Instant, channel: u16) -> rtc::shared::error::Result<()> {
        let mut data =
            ChannelData { data: b"audio".to_vec(), number: ChannelNumber(channel), ..Default::default() };
        data.encode();
        self.deliver(now, data.raw)
    }

    /// Permission for the peer, granted at `now`.
    fn permit(&mut self, now: Instant) {
        self.client.relay(addr(RELAYED)).unwrap().create_permission(now, addr(PEER)).unwrap();
        let req = self.next_request();
        self.ok(now, &req);
        self.events();
    }

    fn send(&mut self, now: Instant) {
        self.client.relay(addr(RELAYED)).unwrap().send_to(now, b"audio", addr(PEER)).unwrap();
    }
}

fn error(code: rtc::stun::error_code::ErrorCode) -> Box<dyn Setter> {
    Box::new(ErrorCodeAttribute { code, reason: b"test".to_vec() })
}

fn text(attr: rtc::stun::attributes::AttrType, s: &str) -> Box<dyn Setter> {
    Box::new(TextAttribute::new(attr, s.to_owned()))
}

fn nonce_of(req: &Message) -> String {
    TextAttribute::get_from_as(req, ATTR_NONCE).unwrap().text
}

fn channel_of(req: &Message) -> u16 {
    let mut n = ChannelNumber::default();
    n.get_from(req).unwrap();
    n.0
}

fn lifetime_of(req: &Message) -> Duration {
    let mut l = Lifetime::default();
    l.get_from(req).unwrap();
    l.0
}

/// A bound channel for the peer, at t = 0; returns its number.
fn bound(turn: &mut Turn) -> u16 {
    let now = turn.at(0);
    turn.permit(now);
    turn.send(now);
    let bind = turn.next_request();
    assert_eq!(bind.typ.method, METHOD_CHANNEL_BIND);
    turn.ok(now, &bind);
    channel_of(&bind)
}

/// The failure VX5 saw at ~10 min: the binding's refresh is answered 438.
#[test]
fn a_stale_nonce_on_a_channel_bind_refresh_retries_and_keeps_the_channel() {
    let mut turn = Turn::allocated();
    let channel = bound(&mut turn);

    // Past the 5-minute binding lifetime, sending refreshes the binding.
    let now = turn.at(302);
    turn.send(now);
    let refresh = turn.next_request();
    assert_eq!(refresh.typ.method, METHOD_CHANNEL_BIND);
    turn.stale(now, &refresh, "nonce-1");

    let retry = turn.next_request();
    assert_eq!(retry.typ.method, METHOD_CHANNEL_BIND, "the ChannelBind is sent again at once");
    assert_ne!(retry.transaction_id, refresh.transaction_id, "as a new transaction");
    assert_eq!(nonce_of(&retry), "nonce-1", "with the server's new nonce");
    assert_eq!(channel_of(&retry), channel, "for the same channel");

    // Meanwhile the channel stays: the peer's audio on it is delivered, and ours still goes on it.
    turn.from_peer(now, channel).expect("data on the channel is still accepted");
    assert!(turn.events().iter().any(|e| matches!(e, Event::DataIndicationOrChannelData(Some(n), _, _) if n.0 == channel)));
    turn.send(turn.at(303));
    let sent = turn.sent();
    assert!(
        sent.iter().all(|o| matches!(o, Out::Channel(n) if *n == channel)),
        "only ChannelData on the same channel is sent, no new binding"
    );

    turn.ok(turn.at(303), &retry);
    turn.from_peer(turn.at(304), channel).unwrap();
    turn.send(turn.at(304));
    assert!(turn.requests().is_empty(), "the refreshed binding needs no further request");
}

#[test]
fn a_stale_nonce_is_retried_only_once() {
    let mut turn = Turn::allocated();
    bound(&mut turn);

    let now = turn.at(302);
    turn.send(now);
    let refresh = turn.next_request();
    turn.stale(now, &refresh, "nonce-1");
    let retry = turn.next_request();
    turn.stale(now, &retry, "nonce-2");
    assert!(turn.requests().is_empty(), "a second 438 is not answered with a third request");
}

#[test]
fn a_stale_nonce_on_a_permission_retries_under_the_callers_id() {
    let mut turn = Turn::allocated();
    let now = turn.at(0);
    let tid = turn.client.relay(addr(RELAYED)).unwrap().create_permission(now, addr(PEER)).unwrap().unwrap();
    let first = turn.next_request();
    turn.stale(now, &first, "nonce-1");

    let retry = turn.next_request();
    assert_eq!(retry.typ.method, METHOD_CREATE_PERMISSION, "the CreatePermission is sent again at once");
    assert_eq!(nonce_of(&retry), "nonce-1");
    assert!(
        !turn.events().iter().any(|e| matches!(e, Event::CreatePermissionError(..))),
        "the caller is not told of a failure"
    );

    turn.ok(now, &retry);
    let events = turn.events();
    assert!(
        events.iter().any(|e| matches!(e, Event::CreatePermissionResponse(id, p) if *id == tid && *p == addr(PEER))),
        "the grant arrives under the id the caller was given"
    );
    turn.send(now);
    let bind = turn.next_request();
    turn.ok(now, &bind);

    // The periodic refresh (every 120 s) answered 438 is sent again too, and the permission stays.
    let now = turn.at(120);
    turn.client.handle_timeout(now).unwrap();
    let refresh = turn.next_request();
    assert_eq!(refresh.typ.method, METHOD_CREATE_PERMISSION);
    turn.stale(now, &refresh, "nonce-2");
    let retry = turn.next_request();
    assert_eq!(retry.typ.method, METHOD_CREATE_PERMISSION);
    assert_eq!(nonce_of(&retry), "nonce-2");
    turn.send(now);
}

#[test]
fn a_stale_nonce_on_an_allocation_refresh_resends_it_at_once() {
    let mut turn = Turn::allocated();

    // Half the lifetime in, the allocation is refreshed.
    let now = turn.at(300);
    turn.client.handle_timeout(now).unwrap();
    let refresh = turn.next_request();
    assert_eq!(refresh.typ.method, METHOD_REFRESH);
    turn.stale(now, &refresh, "nonce-1");

    let retry = turn.next_request();
    assert_eq!(retry.typ.method, METHOD_REFRESH, "the Refresh is sent again at once, not at the next timer");
    assert_eq!(nonce_of(&retry), "nonce-1");
    assert_eq!(lifetime_of(&retry), Duration::from_secs(600));
    turn.ok(now, &retry);

    // Releasing the allocation keeps its zero lifetime when it is sent again.
    let now = turn.at(310);
    turn.client.relay(addr(RELAYED)).unwrap().close(now).unwrap();
    let release = turn.next_request();
    turn.stale(now, &release, "nonce-2");
    let retry = turn.next_request();
    assert_eq!(lifetime_of(&retry), Duration::ZERO, "a release stays a release");
}
