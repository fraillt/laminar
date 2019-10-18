use crate::packet::Packet;

#[derive(Debug)]
pub enum UserEvent {
    Connect,
    Packet(Packet),
    Disconnect,
}

#[derive(Debug)]
pub enum ConnectionEvent {
    Created,
    Connected,
    Reconnected,
    Packet(Packet),
    Disconnected(DisconnectReason),
    Destroyed(DestroyReason),
}

/// Provides a reason why the connection was destroyed.
#[derive(Debug)]
pub enum DestroyReason {
    /// When `SocketManager` decided to destroy a connection for error that arrived from `ConnectionManager`.
    ConnectionError(String),
    /// After `Config.idle_connection_timeout` connection had no activity.
    Timeout,
    /// If there are too many non-acked packets in flight `Config.max_packets_in_flight`.
    TooManyPacketsInFlight,
    /// When `ConnectionManager` changed to `Disconnected` state.
    GracefullyDisconnected,
}

/// Disconnect reason, received by connection
#[derive(Debug)]
pub enum DisconnectReason {
    /// Disconnect was initiated by the local or remote host
    ClosedByLocalHost,
    ClosedByRemoteHost,
    /// Socket manager decided to destroy connection for provided reason
    Destroying(DestroyReason),
}
