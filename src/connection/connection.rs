use std::net::SocketAddr;
use std::time::{Duration, Instant};

use rand::{thread_rng, RngCore};

use crate::{
    error::{ErrorKind, Result},
    net::{Connection, ConnectionMessenger},
    packet::{DeliveryGuarantee, OutgoingPackets, Packet, PacketInfo},
};

use super::events::{ConnectionEvent, DestroyReason, DisconnectReason, UserEvent};
use super::packet_header::{PacketHeader, PacketHeaderType};

#[derive(Debug)]
enum ConnectionState {
    Connecting,
    Connected,
    Disconnected(DisconnectReason),
}

#[derive(Debug)]
struct CurrentPacketToSend {
    send_packet: PacketHeaderType<'static>,
    remote_identity: u64,
    send_at: Instant,
    send_times_left: u8,
}

#[derive(Debug)]
struct ConnectionData {
    identity: u64,
    state: ConnectionState,
    packet_to_send: Option<CurrentPacketToSend>,
}

const SEND_INTERVAL_DURING_HANDSHAKE: Duration = Duration::from_millis(100); // 10 packets per second

// todo better name
#[derive(Debug)]
pub struct Conn {
    connections: Vec<ConnectionData>, // only one connection can be in connected state, and it will always be the first in vector
    handshake_last_sent: Instant,
    address: SocketAddr,
    destroy_reason: Option<DestroyReason>,
}

impl Conn {
    pub fn new(address: SocketAddr, time: Instant) -> Self {
        Self {
            connections: Vec::default(),
            handshake_last_sent: time,
            address,
            destroy_reason: None,
        }
    }
}

impl Connection for Conn {
    /// Defines a user event type.
    type UserEvent = (SocketAddr, UserEvent);
    /// Defines a connection event type.
    type ConnectionEvent = (SocketAddr, ConnectionEvent);

    /// Initial call with a payload, when connection is created by accepting remote packet.
    fn after_remote_accepted(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        payload: &[u8],
    ) {
        messenger.send_event((self.address, ConnectionEvent::Created));
        self.process_packet(time, messenger, payload);
    }

    /// Initial call with a event, when connection is created by accepting user event.
    fn after_local_accepted(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        event: Self::UserEvent,
    ) {
        messenger.send_event((self.address, ConnectionEvent::Created));
        self.process_event(time, messenger, event);
    }

    /// Processes a received packet: parse it and emit an event.
    fn process_packet(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        payload: &[u8],
    ) {
        if let Ok(header) = PacketHeader::from_bytes(payload) {
            if let Some(conn) = self
                .connections
                .iter_mut()
                .find(|conn| conn.identity == header.identity)
            {
                match conn.state {
                    ConnectionState::Connecting => {
                        let current_packet = conn
                            .packet_to_send
                            .as_ref()
                            .expect("Must be set, when in Connecting state.");
                        match (&current_packet.send_packet, header.packet) {
                            // client side
                            (
                                PacketHeaderType::ConnectionRequest,
                                PacketHeaderType::Challenge(server_salt),
                            ) => {
                                let identity = conn.identity ^ server_salt;
                                conn.identity = identity;
                                // to make sure that remote host gets our response, send it several times
                                conn.packet_to_send = Some(CurrentPacketToSend {
                                    send_packet: PacketHeaderType::ChallengeResponse,
                                    remote_identity: identity,
                                    send_at: time,
                                    send_times_left: 10,
                                });
                                conn.state = ConnectionState::Connected;
                                messenger.send_event((self.address, ConnectionEvent::Connected));
                            }
                            // server side
                            (
                                PacketHeaderType::Challenge(_),
                                PacketHeaderType::ChallengeResponse,
                            ) => {
                                conn.state = ConnectionState::Connected;
                                let identity = conn.identity;
                                // if this is a first connected connection, then emit `Connected` event,
                                if self.connections[0].identity == identity {
                                    messenger
                                        .send_event((self.address, ConnectionEvent::Connected));
                                } else {
                                    // clear all pending connections (because a new connection is established)
                                    self.connections.retain(|conn| conn.identity == identity);
                                    messenger
                                        .send_event((self.address, ConnectionEvent::Reconnected));
                                }
                            }
                            _ => {} // ignore the rest
                        }
                    }
                    ConnectionState::Connected => {
                        match &header.packet {
                            PacketHeaderType::Payload(packet) => {
                                // TODO send user events
                            }
                            PacketHeaderType::Disconnect => {
                                conn.state = ConnectionState::Disconnected(
                                    DisconnectReason::ClosedByRemoteHost,
                                );
                                messenger.send_event((
                                    self.address,
                                    ConnectionEvent::Disconnected(
                                        DisconnectReason::ClosedByRemoteHost,
                                    ),
                                ));
                            }
                            _ => {} // ignore the rest
                        }
                    }
                    ConnectionState::Disconnected(_) => {}
                }
            } else if let PacketHeaderType::ConnectionRequest = header.packet {
                loop {
                    let new_connection = accept_new_connection(header.identity, time);
                    if self.connections[0].identity != self.connections[1].identity {
                        continue;
                    }
                    if self.connections.len() < 2 {
                        self.connections.push(new_connection);
                    } else {
                        self.connections[1] = new_connection;
                    }
                    break;
                }
            }
        }
    }

    /// Processes a received event and send a packet.
    fn process_event(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        event: Self::UserEvent,
    ) {
        match event.1 {
            UserEvent::Connect => {
                // basically reset connect to "clean" state
                self.connections.clear();
                self.connections.push(init_new_connection(time));
            }
            UserEvent::Packet(packet) => {
                if let ConnectionState::Connected = self.connections[0].state {
                    // todo send packet
                }
            }
            UserEvent::Disconnect => {
                // disconnect only if in connected state
                if let ConnectionState::Connected = self.connections[0].state {
                    let mut conn = &mut self.connections[0];
                    conn.state = ConnectionState::Disconnected(DisconnectReason::ClosedByLocalHost);
                    // to cleanly disconnect simply send some disconnect packets
                    conn.packet_to_send = Some(CurrentPacketToSend {
                        send_packet: PacketHeaderType::Disconnect,
                        remote_identity: conn.identity,
                        send_at: time,
                        send_times_left: 10,
                    });
                    messenger.send_event((
                        self.address,
                        ConnectionEvent::Disconnected(DisconnectReason::ClosedByLocalHost),
                    ));
                }
            }
        }
    }

    /// Processes various connection-related tasks: resend dropped packets, send heartbeat packet, etc...
    /// This function gets called frequently.
    fn update(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
    ) {
        for conn in &mut self.connections {
            if conn.packet_to_send.is_some() {
                let mut packet = conn.packet_to_send.as_mut().unwrap(); // we can unwrap, we checked it before
                if time <= packet.send_at {
                    let header = PacketHeader {
                        identity: packet.remote_identity,
                        packet: packet.send_packet.clone(),
                    };
                    let written = header
                        .into_bytes(messenger.buffer())
                        .expect("Do not fail, when ");
                    messenger.send_packet_from_buffer(&self.address, written);
                    packet.send_at = time + SEND_INTERVAL_DURING_HANDSHAKE;
                    if packet.send_times_left > 1 {
                        packet.send_times_left = packet.send_times_left - 1;
                    } else {
                        conn.packet_to_send = None;
                    }
                }
            }
        }
    }

    /// Last call before connection is destroyed.
    fn before_discarded(
        &mut self,
        _time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
    ) {
        let reason = self
            .destroy_reason
            .take()
            .expect("Destroy reason must be set, if we got here.");
        messenger.send_event((self.address, ConnectionEvent::Destroyed(reason)));
    }

    fn should_discard(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
    ) -> bool {
        // TODO implement
        false
    }
}

fn init_new_connection(time: Instant) -> ConnectionData {
    let identity = rand::thread_rng().next_u64();
    ConnectionData {
        identity,
        state: ConnectionState::Connecting,
        packet_to_send: Some(CurrentPacketToSend {
            send_packet: PacketHeaderType::ConnectionRequest,
            remote_identity: identity,
            send_at: time,
            send_times_left: 10,
        }),
    }
}

fn accept_new_connection(client_salt: u64, time: Instant) -> ConnectionData {
    let server_salt = rand::thread_rng().next_u64();
    ConnectionData {
        identity: client_salt ^ server_salt,
        state: ConnectionState::Connecting,
        packet_to_send: Some(CurrentPacketToSend {
            send_packet: PacketHeaderType::Challenge(server_salt),
            remote_identity: client_salt,
            send_at: time,
            send_times_left: 10,
        }),
    }
}

#[derive(Debug)]
struct ConnectedState {
    identity: u64,
    client_salt: u64,
    server_salt: u64,
}

#[derive(Debug)]
struct HandshakingState {
    remote_salt: u64,
    current_state: PacketHeaderType<'static>,
}

#[derive(Debug)]
pub struct AddressConnection {
    address: SocketAddr,
    active: Option<ConnectedState>,
    pending: Vec<HandshakingState>,
    packet_to_send: Option<CurrentPacketToSend>,
    destroy_reason: Option<DestroyReason>,
}

impl AddressConnection {
    pub fn new(address: SocketAddr) -> Self {
        Self {
            address,
            active: None,
            pending: Vec::default(),
            packet_to_send: None,
        }
    }

    pub fn process_payload(
        &self,
        identity: u64,
        packet: PacketInfo,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
        if self.is_connected(identity) {
            // todo packet event to user
        }
    }

    pub fn process_disconnect(
        &mut self,
        identity: u64,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
        self.disconnect(messenger, DisconnectReason::ClosedByRemoteHost);
    }

    pub fn process_connection_request(
        &mut self,
        identity: u64,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
        // let server_salt = rand::thread_rng().next_u64();
        // conn.pending.push(HandshakingState {
        //     remote_salt: header.identity,
        //     current_state: PacketHeaderType::Challenge(server_salt),
        // });
    }

    pub fn process_challenge_request(
        &mut self,
        identity: u64,
        server_salt: u64,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
    }

    pub fn process_challenge_response(
        &mut self,
        identity: u64,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
        if let Some(active) = self.active {
            if active.identity == identity {
                self.send_packet(identity, PacketHeaderType::ConnectionAccepted, messenger);
            }
        } else {
            // 
        }
    }
    pub fn process_connection_denied(
        &mut self,
        identity: u64,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
        if self.active.is_none() && self.pending.len() == 1 && self.pending[0].remote_salt == identity {
            self.pending.clear();
            self.packet_to_send = None;
            self.destroy_reason = Some(DestroyReason::Declined);
        }
    }

    pub fn user_packet(
        &mut self,
        packet: Packet,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
        if let Some(active) = &self.active {
            let packet = PacketInfo::user_packet(
                packet.payload().as_ref(),
                packet.delivery_guarantee(),
                packet.order_guarantee(),
            );
            self.send_packet(
                active.identity,
                PacketHeaderType::Payload(packet),
                messenger,
            );
        }
    }

    /// Initiates a new connection if not already connected, by removing all previous handshaking state.
    pub fn user_connect(
        &mut self,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
        if self.active.is_none() {
            let salt = rand::thread_rng().next_u64();
            self.pending.clear();
            self.pending.push(HandshakingState {
                remote_salt: salt,
                current_state: PacketHeaderType::ConnectionRequest,
            });

            self.packet_to_send = Some(CurrentPacketToSend {
                send_packet: PacketHeaderType::ConnectionRequest,
                remote_identity: salt,
                send_at: messenger.time(),
                send_times_left: 10,
            });
        }
    }

    pub fn user_disconnect(
        &mut self,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
        self.disconnect(messenger, DisconnectReason::ClosedByLocalHost);
    }

    pub fn should_drop(
        &self,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) -> bool {
        if let Some(reason) = self.destroy_reason {
            if self.packet_to_send.is_none() {
                messenger.send_event((self.address, ConnectionEvent::Destroyed(reason)));
                return true;
            }
        }
        false
    }

    pub fn update(
        &mut self,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
        if self.packet_to_send.is_some() {
            let mut packet = self.packet_to_send.as_mut().unwrap(); // we can unwrap, we checked it before
            if messenger.time() <= packet.send_at {
                self.send_packet(
                    packet.remote_identity,
                    packet.send_packet.clone(),
                    messenger,
                );
                packet.send_at = messenger.time() + SEND_INTERVAL_DURING_HANDSHAKE;
                if packet.send_times_left > 1 {
                    packet.send_times_left = packet.send_times_left - 1;
                } else {
                    self.packet_to_send = None;
                }
            }
        }
    }

    fn send_packet(
        &self,
        identity: u64,
        packet: PacketHeaderType,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
    ) {
        let header = PacketHeader { identity, packet };
        let written = header
            .into_bytes(messenger.buffer())
            .expect("Do not fail, when ");
        messenger.send_packet_from_buffer(&self.address, written);
    }

    fn disconnect(
        &mut self,
        messenger: &mut impl ConnectionMessenger<(SocketAddr, ConnectionEvent)>,
        reason: DisconnectReason,
    ) {
        if let Some(state) = self.active.take() {
            self.pending.clear();
            self.packet_to_send = Some(CurrentPacketToSend {
                send_packet: PacketHeaderType::Disconnect,
                remote_identity: state.identity,
                send_at: messenger.time(),
                send_times_left: 10,
            });
            messenger.send_event((self.address, ConnectionEvent::Disconnected(reason)));
        }
    }
}
