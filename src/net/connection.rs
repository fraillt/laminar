use std::{self, collections::HashMap, fmt::Debug, net::SocketAddr, time::Instant};

use crate::config::Config;

/// Allows connection to send packet, send event and get global configuration.
pub trait ConnectionMessenger<ConnectionEvent: Debug> {
    /// Returns a buffer that can be used to fill data, when using `send_packet_from_buffer` function.
    fn buffer(&mut self) -> &mut [u8];
    /// Sends a packet, and uses data written to `buffer`.
    fn send_packet_from_buffer(&mut self, address: &SocketAddr, packet_size: usize);
    /// Returns global configuration.
    fn config(&self) -> &Config;
    /// Sends a connection event.
    fn send_event(&mut self, event: ConnectionEvent);
    /// Sends a packet.
    fn send_packet(&mut self, address: &SocketAddr, payload: &[u8]);
}

/// Allows to implement actual connection.
/// Defines types of user and connection events that will be used by a connection.
pub trait Connection: Debug {
    /// Defines a user event type.
    type UserEvent: Debug;
    /// Defines a connection event type.
    type ConnectionEvent: Debug;

    /// Initial call with a payload, when connection is created by accepting remote packet.
    fn after_remote_accepted(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        packet: &[u8],
    );

    /// Initial call with a event, when connection is created by accepting user event.
    fn after_local_accepted(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        event: Self::UserEvent,
    );

    /// Processes a received packet: parse it and emit an event.
    fn process_packet(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        packet: &[u8],
    );

    /// Processes a received event and send a packet.
    fn process_event(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
        event: Self::UserEvent,
    );

    /// Processes various connection-related tasks: resend dropped packets, send heartbeat packet, etc...
    /// This function gets called frequently.
    fn update(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
    );

    /// Last call before connection is destroyed.
    fn before_discarded(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
    );

    fn should_discard(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<Self::ConnectionEvent>,
    ) -> bool;
}

/// Decides when to create and destroy connections, and provides a way for `ConnectionManager` to get connection from an user event.
pub trait ConnectionFactory: Debug {
    /// An actual connection type that is created by a factory.
    type Connection: Connection;

    fn process_packet(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<<Self::Connection as Connection>::ConnectionEvent>,
        address: &SocketAddr,
        data: &[u8],
    );

    fn process_event(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<<Self::Connection as Connection>::ConnectionEvent>,
        event: <Self::Connection as Connection>::UserEvent,
    );

    fn update_connections(
        &mut self,
        time: Instant,
        messenger: &mut impl ConnectionMessenger<<Self::Connection as Connection>::ConnectionEvent>,
    );
}
