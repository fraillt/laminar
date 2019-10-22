use std::{self, fmt::Debug, net::SocketAddr, time::Instant};

use crate::config::Config;

/// Allows connection to send packet, send event and get global configuration.
pub trait ContextWithSender<ConnectionEvent: Debug> {
    /// Returns current time `Instant`.
    fn time(&self) -> Instant;
    /// Returns global configuration.
    fn config(&self) -> &Config;
    /// Returns a buffer that can be used to fill data, when using `send_packet_from_buffer` function.
    fn buffer(&mut self) -> &mut [u8];
    /// Sends a packet, and uses data written to `buffer`.
    fn send_packet_from_buffer(&mut self, address: &SocketAddr, packet_size: usize);
    /// Sends a connection event.
    fn send_event(&mut self, event: ConnectionEvent);
    /// Sends a packet.
    fn send_packet(&mut self, address: &SocketAddr, payload: &[u8]);
}

/// Decides when to create and destroy connections, and provides a way for `ConnectionManager` to get connection from an user event.
pub trait ConnectionManager: Debug {
    /// Defines a user event type.
    type UserEvent: Debug;
    /// Defines a connection event type.
    type ConnectionEvent: Debug;

    fn process_packet(
        &mut self,
        messenger: &mut impl ContextWithSender<Self::ConnectionEvent>,
        address: &SocketAddr,
        data: &[u8],
    );

    fn process_event(
        &mut self,
        messenger: &mut impl ContextWithSender<Self::ConnectionEvent>,
        event: Self::UserEvent,
    );

    fn update_connections(&mut self, messenger: &mut impl ContextWithSender<Self::ConnectionEvent>);
}
