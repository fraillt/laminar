use std::collections::HashMap;
use std::net::SocketAddr;
use std::time::Instant;

use crate::net::{Connection, ConnectionFactory, ConnectionMessenger};

use super::{
    connection::Conn,
    events::UserEvent,
    packet_header::{PacketHeader, PacketHeaderType},
};

#[derive(Debug)]
struct Factory {
    connections: HashMap<SocketAddr, Conn>,
}

impl Factory {
    fn should_accept_remote(
        &mut self,
        time: Instant,
        address: SocketAddr,
        data: &[u8],
    ) -> Option<Conn> {
        if let Ok(header) = PacketHeader::from_bytes(data) {
            if let PacketHeaderType::ConnectionRequest = header.packet {
                return Some(Conn::new(address, time));
            }
        }
        None
    }

    /// Determines if local connection can be accepted.
    /// If connection is accepted, then `after_remote_accepted` will be invoked on it.
    fn should_accept_local(
        &mut self,
        time: Instant,
        address: SocketAddr,
        event: &<Conn as Connection>::UserEvent,
    ) -> Option<Conn> {
        if let UserEvent::Connect = event.1 {
            Some(Conn::new(address, time))
        } else {
            None
        }
    }
}

impl ConnectionFactory for Factory {
    type Connection = Conn;

    fn process_packet(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<<Self::Connection as Connection>::ConnectionEvent>,
        address: &SocketAddr,
        payload: &[u8],
    ) {
        if let Some(conn) = self.connections.get_mut(&address) {
            conn.process_packet(time, messenger, payload);
        } else if let Some(mut conn) = self.should_accept_remote(time, *address, payload) {
            conn.after_remote_accepted(time, messenger, payload);
            self.connections.insert(*address, conn);
        }
    }

    fn process_event(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<<Self::Connection as Connection>::ConnectionEvent>,
        event: <Self::Connection as Connection>::UserEvent,
    ) {
        if let Some(conn) = self.connections.get_mut(&event.0) {
            conn.process_event(time, messenger, event);
        } else {
            let address = event.0;
            if let Some(mut conn) = self.should_accept_local(time, address, &event) {
                conn.after_local_accepted(time, messenger, event);
                self.connections.insert(address, conn);
            }
        }
    }

    fn update_connections(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<<Self::Connection as Connection>::ConnectionEvent>,
    ) {
        for conn in self.connections.values_mut() {
            conn.update(time, messenger);
        }

        self.connections
            .retain(|_, conn| !conn.should_discard(time, messenger));
    }
}
