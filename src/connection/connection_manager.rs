use std::collections::HashMap;
use std::net::SocketAddr;

use crate::net::{ConnectionManager, ContextWithSender};

use super::{
    address_connection::AddressConnection,
    events::{ConnectionEvent, UserEvent},
    packet_header::{PacketHeader, PacketHeaderType},
};

#[derive(Debug, Eq, PartialEq)]
struct AddressWithConnId(SocketAddr, i64);

#[derive(Debug)]
pub struct Manager {
    connections: HashMap<SocketAddr, AddressConnection>,
    connected_connections: usize,
    max_connected_connections: usize,
    max_total_connections: usize,
    max_pending_per_connection: usize,
}

impl Manager {
    pub fn new() -> Self {
        Self {
            connections: Default::default(),
            connected_connections: 0,
            max_connected_connections: 10,
            max_total_connections: 20,
            max_pending_per_connection: 5,
        }
    }
}

impl ConnectionManager for Manager {
    /// Defines a user event type.
    type UserEvent = (SocketAddr, UserEvent);
    /// Defines a connection event type.
    type ConnectionEvent = (SocketAddr, ConnectionEvent);

    fn process_packet(
        &mut self,
        messenger: &mut impl ContextWithSender<Self::ConnectionEvent>,
        address: &SocketAddr,
        payload: &[u8],
    ) {
        if let Ok(header) = PacketHeader::from_bytes(payload) {
            if let Some(conn) = self.connections.get_mut(address) {
                match header.packet {
                    PacketHeaderType::Payload(payload) => {
                        conn.process_payload(header.identity, payload, messenger)
                    }
                    PacketHeaderType::Disconnect => {
                        conn.process_disconnect(header.identity, messenger)
                    }
                    PacketHeaderType::ConnectionRequest => conn.process_connection_request(
                        header.identity,
                        messenger,
                        self.max_pending_per_connection,
                    ),
                    PacketHeaderType::Challenge(server_salt) => {
                        conn.process_challenge(header.identity, server_salt, messenger)
                    }
                    PacketHeaderType::ChallengeResponse => {
                        if conn.process_challenge_response(
                            header.identity,
                            messenger,
                            self.connected_connections < self.max_connected_connections,
                        ) {
                            self.connected_connections = self.connected_connections + 1;
                        }
                    }
                    PacketHeaderType::ConnectionAccepted => {
                        conn.process_connection_accepted(header.identity, messenger);
                    }
                    PacketHeaderType::ConnectionDenied => {
                        conn.process_connection_denied(header.identity, messenger)
                    }
                }
            } else if let PacketHeaderType::ConnectionRequest = header.packet {
                if self.connections.len() < self.max_total_connections {
                    messenger.send_event((*address, ConnectionEvent::Created));
                    let conn = self
                        .connections
                        .entry(*address)
                        .or_insert(AddressConnection::new(
                            *address,
                            messenger.config(),
                            messenger.time(),
                        ));
                    conn.process_connection_request(
                        header.identity,
                        messenger,
                        self.max_pending_per_connection,
                    );
                } else {
                    // todo send connection denied
                }
            }
        }
    }

    fn process_event(
        &mut self,
        messenger: &mut impl ContextWithSender<Self::ConnectionEvent>,
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
            messenger.send_event((event.0, ConnectionEvent::Created));
            let conn = self
                .connections
                .entry(event.0)
                .or_insert(AddressConnection::new(
                    event.0,
                    messenger.config(),
                    messenger.time(),
                ));
            conn.user_connect(messenger);
        }
    }

    fn update_connections(
        &mut self,
        messenger: &mut impl ContextWithSender<Self::ConnectionEvent>,
    ) {
        for conn in self.connections.values_mut() {
            conn.update(messenger);
        }

        self.connected_connections = self
            .connections
            .iter()
            .filter(|(_, conn)| conn.is_connected())
            .count();

        self.connections.retain(|address, conn| {
            if let Some(reason) = conn.should_drop() {
                messenger.send_event((*address, ConnectionEvent::Destroyed(reason)));
                false
            } else {
                true
            }
        });
    }
}
