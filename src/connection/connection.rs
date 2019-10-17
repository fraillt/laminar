use std::net::SocketAddr;
use std::time::{Duration, Instant};

use rand::{thread_rng, RngCore};

use crate::{
    error::{ErrorKind, Result},
    net::{Connection, ConnectionMessenger},
    packet::{DeliveryGuarantee, OutgoingPackets, Packet, PacketInfo},
};

use super::events::{ConnectionEvent, UserEvent};
use super::packet_header::{PacketHeader, PacketHeaderType};

#[derive(Debug)]
enum ConnectionState {
    Connecting {
        send_packet: PacketHeaderType<'static>,
        remote_identity: u64,
        last_sent: Instant,
    },
    Connected,
}

#[derive(Debug)]
struct ConnectionData {
    identity: u64,
    state: ConnectionState,
}

const SEND_INTERVAL_DURING_HANDSHAKE: Duration = Duration::from_millis(100); // 10 packets per second

// todo better name
#[derive(Debug)]
struct Conn {
    connections: Vec<ConnectionData>, // can store maximum of 2 connections: 1 connected and 1 pending (in case client wants to reconnect)
    handshake_last_sent: Instant,
    address: SocketAddr,
}

impl Connection for Conn {
    /// Defines a user event type.
    type UserEvent = UserEvent;
    /// Defines a connection event type.
    type ConnectionEvent = ConnectionEvent;

    /// Initial call with a payload, when connection is created by accepting remote packet.
    fn after_remote_accepted(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        payload: &[u8],
    ) {
    }

    /// Initial call with a event, when connection is created by accepting user event.
    fn after_local_accepted(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        event: Self::UserEvent,
    ) {
        // clear all current connections, when it is local connection request
        self.connections.clear();

        // generate client salt
        let identity = rand::thread_rng().next_u64();
        self.connections.push(ConnectionData {
            identity,
            state: ConnectionState::Connecting {
                send_packet: PacketHeaderType::ConnectionRequest,
                remote_identity: identity,
                last_sent: time
                    .checked_sub(SEND_INTERVAL_DURING_HANDSHAKE + Duration::from_millis(1)) // come back just a little bit more, so that this gets called in update function this iteration
                    .expect("Congratulations! You just got back in time!"),
            },
        });
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
                .iter()
                .find(|conn| conn.identity == header.identity)
            {
                match &conn.state {
                    ConnectionState::Connecting{ send_packet, remote_identity, last_sent: _ }  => {
                        match send_packet {

                        }
                    },
                    ConnectionState::Connected => {

                    }
                }
            } else if let PacketHeaderType::ConnectionRequest = header.packet {
                loop {
                    let server_salt = rand::thread_rng().next_u64();
                    let new_connection = ConnectionData {
                        identity: header.identity ^ server_salt,
                        state: ConnectionState::Connecting {
                            send_packet: PacketHeaderType::Challenge(server_salt),
                            remote_identity: header.identity,
                            last_sent: time
                                .checked_sub(SEND_INTERVAL_DURING_HANDSHAKE + Duration::from_millis(1)) // come back just a little bit more, so that this gets called in update function this iteration
                                .expect("Congratulations! You just got back in time!"),
                        },
                    };
                    if self.connections.len() < 2 {
                        self.connections.push(new_connection);
                    } else {
                        self.connections[1] = new_connection;
                    }
                    // check situation where client_salt ^ server_salt match existing value
                    if self.connections[0].identity != self.connections[1].identity {
                        break;
                    }
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
    }

    /// Processes various connection-related tasks: resend dropped packets, send heartbeat packet, etc...
    /// This function gets called frequently.
    fn update(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
    ) {
        for conn in &self.connections {
            if let ConnectionState::Connecting {
                send_packet,
                remote_identity,
                last_sent,
            } = &conn.state
            {
                if time.duration_since(*last_sent) > SEND_INTERVAL_DURING_HANDSHAKE {
                    let header = PacketHeader {
                        identity: *remote_identity,
                        packet: send_packet.clone(),
                    };
                    let written = header
                        .into_bytes(messenger.buffer())
                        .expect("Do not fail, when ");
                    messenger.send_packet_from_buffer(&self.address, written);
                }
            }
        }
    }

    /// Last call before connection is destroyed.
    fn before_discarded(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
    ) {
    }
}
