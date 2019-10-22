//! This module provides the logic between the low-level abstract types and the types that the user will be interacting with.
//! You can think of the socket, connection management, congestion control.

pub use self::activity_system::ActivitySystem;
pub use self::connection_impl::ConnectionImpl;
pub use self::connection_manager::{ConnectionManager, ContextWithSender};
pub use self::events::SocketEvent;
pub use self::link_conditioner::LinkConditioner;
pub use self::manager_impl::ManagerImpl;
pub use self::quality::{NetworkQuality, RttMeasurer};
pub use self::reliability_system::ReliabilitySystem;
pub use self::socket::Socket;
pub use self::socket_with_conditioner::SocketWithConditioner;
pub use self::socket_with_connections::{DatagramSocket, SocketWithConnections};
pub use self::virtual_connection::VirtualConnection;

mod activity_system;
mod connection_impl;
mod connection_manager;
mod events;
mod link_conditioner;
mod manager_impl;
mod quality;
mod reliability_system;
mod socket;
mod socket_with_conditioner;
mod socket_with_connections;
mod virtual_connection;

pub mod constants;
