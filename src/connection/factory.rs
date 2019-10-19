use std::collections::HashMap;
use std::hash::Hasher;
use std::net::SocketAddr;
use std::time::Instant;

use rand::{thread_rng, RngCore};

use crate::net::{Connection, ConnectionFactory, ConnectionMessenger};
use crate::packet::{Packet, PacketInfo};

use super::{
    connection::AddressConnection,
    events::{ConnectionEvent, UserEvent},
    packet_header::{PacketHeader, PacketHeaderType},
};

#[derive(Debug)]
struct Factory {
    connections: HashMap<SocketAddr, AddressConnection>,
    max_connections: usize,
}

impl ConnectionFactory for Factory {
    /// Defines a user event type.
    type UserEvent = (SocketAddr, UserEvent);
    /// Defines a connection event type.
    type ConnectionEvent = (SocketAddr, ConnectionEvent);

    fn process_packet(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        address: &SocketAddr,
        payload: &[u8],
    ) {
        if let Ok(header) = PacketHeader::from_bytes(payload) {
            if let Some(conn) = self.connections.get_mut(address) {
                match header.packet {
                    PacketHeaderType::Payload(packet) => {
                        conn.process_payload(header.identity, packet, messenger)
                    }
                    PacketHeaderType::Disconnect => {
                        conn.process_disconnect(header.identity, messenger)
                    }
                    PacketHeaderType::ConnectionRequest => {
                        conn.process_connection_request(header.identity, messenger)
                    }
                    PacketHeaderType::Challenge(server_salt) => {
                        conn.process_challenge_request(header.identity, server_salt, messenger)
                    }
                    PacketHeaderType::ChallengeResponse => {
                        conn.process_challenge_response(header.identity, messenger)
                    }
                    PacketHeaderType::ConnectionDenied => {
                        conn.process_connection_denied(header.identity, messenger)
                    }
                }
            } else if let PacketHeaderType::ConnectionRequest = header.packet {
                if self.connections.len() < self.max_connections {
                    let conn = self
                        .connections
                        .entry(*address)
                        .or_insert(AddressConnection::new());
                    conn.process_connection_request(header.identity, messenger);
                } else {
                    // todo send connection denied
                }
            }
        }
    }

    fn process_event(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        event: Self::UserEvent,
    ) {
        if let Some(conn) = self.connections.get_mut(&event.0) {
            match event.1 {
                UserEvent::Connect => {
                    conn.user_connect(messenger);
                }
                UserEvent::Packet(packet) => {
                    conn.user_packet(packet, messenger);
                }
                UserEvent::Disconnect => {
                    conn.user_disconnect(messenger);
                }
            }
        } else if let UserEvent::Connect = event.1 {
            // do not check for maximum connections, user probably knows what he/she is doing
            let conn = self
                .connections
                .entry(event.0)
                .or_insert(AddressConnection::new());
            conn.user_connect();
        }
    }

    fn update_connections(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
    ) {
        // for conn in self.connections.values_mut() {
        //     conn.update(time, messenger);
        // }

        // self.connections
        //     .retain(|_, conn| !conn.should_discard(time, messenger));
    }
}
