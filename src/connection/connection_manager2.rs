use std::collections::HashMap;
use std::net::SocketAddr;

use crate::net::{ConnectionManager, ContextWithSender};
use crate::packet::Packet;

use super::{
    events::{ConnectionEvent, DestroyReason, UserEvent},
    packet_header::{PacketHeader, PacketHeaderType},
};

pub trait Connection: std::fmt::Debug {
    fn process_packet(
        &mut self,
        messenger: &mut dyn ContextWithSender<(SocketAddr, ConnectionEvent)>,
        packet: PacketHeader,
    );

    fn user_packet(
        &mut self,
        messenger: &mut dyn ContextWithSender<(SocketAddr, ConnectionEvent)>,
        packet: Packet,
    );

    fn disconnect(&mut self, messenger: &mut dyn ContextWithSender<(SocketAddr, ConnectionEvent)>);

    fn update(&mut self, messenger: &mut dyn ContextWithSender<(SocketAddr, ConnectionEvent)>);
    fn is_connected(&self) -> bool;
    fn should_drop(&self) -> Option<DestroyReason>;
}

#[derive(Debug)]
pub struct Manager {
    connections: HashMap<SocketAddr, Box<dyn Connection>>,
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
                conn.process_packet(messenger, header);
            } else if let PacketHeaderType::ConnectionRequest = header.packet {
            }
        }
    }

    fn process_event(
        &mut self,
        messenger: &mut impl ContextWithSender<Self::ConnectionEvent>,
        event: Self::UserEvent,
    ) {
        match (event.1, self.connections.get_mut(&event.0)) {
            (UserEvent::Packet(packet), Some(conn)) => conn.user_packet(messenger, packet),
            (UserEvent::Disconnect, Some(conn)) => {
                conn.disconnect(messenger);
            }
            (UserEvent::Connect, None) => {}
            _ => {}
        }
    }

    fn update_connections(
        &mut self,
        messenger: &mut impl ContextWithSender<Self::ConnectionEvent>,
    ) {
        for conn in self.connections.values_mut() {
            conn.update(messenger);
        }

        // self.connected_connections = self
        //     .connections
        //     .iter()
        //     .filter(|(_, conn)| conn.is_connected())
        //     .count();

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
