use std::net::SocketAddr;
use std::time::{Duration, Instant};

use log::error;
use rand::RngCore;

use crate::{
    config::Config,
    net::{ActivitySystem, ContextWithSender, ReliabilitySystem},
    packet::{DeliveryGuarantee, Packet, PacketInfo},
};

use super::connection_manager2::Connection;
use super::events::{ConnectionEvent, DestroyReason, DisconnectReason};
use super::packet_header::{PacketHeader, PacketHeaderType};

const SEND_INTERVAL_DURING_HANDSHAKE: Duration = Duration::from_millis(200); // 5 packets per second


#[derive(Debug)]
struct ConnectingState {
    client_salt: u64,
    send_packet: PacketHeaderType<'static>,
    hashed_salt: u64,
    send_at: Instant,
    send_times_left: u8,
}

#[derive(Debug)]
struct ConnectedState {
    identity: u64,
    activity: ActivitySystem,
    reliability: ReliabilitySystem,
}

#[derive(Debug)]
struct DisconnectedState {
    immediately: bool,
    destroy_reason: DestroyReason,
}

#[derive(Debug)]
enum ConnectionState {
    Connecting(ConnectingState),
    Connected(ConnectedState),
    Disconnected(DisconnectedState),
}

#[derive(Debug)]
pub struct ClientConnection {
    address: SocketAddr,
    state: ConnectionState,
}

impl ClientConnection {
    fn send_packet(
        &self,
        identity: u64,
        packet: PacketHeaderType,
        messenger: &mut dyn ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        let header = PacketHeader { identity, packet };
        let written = header
            .into_bytes(messenger.buffer())
            .expect("Do not fail, when ");
        messenger.send_packet_from_buffer(&self.address, written);
    }

    pub fn connect(&mut self, messenger: &mut dyn ContextWithSender<(SocketAddr, ConnectionEvent)>) {

    }
}

impl Connection for ClientConnection {
    fn process_packet(
        &mut self,
        messenger: &mut dyn ContextWithSender<(SocketAddr, ConnectionEvent)>,
        packet: PacketHeader,
    ) {
        match (packet.packet, &mut self.state) {
            (PacketHeaderType::Payload(payload), ConnectionState::Connected(state)) if state.identity == packet.identity => {
                state.activity.received(messenger.time());
                match state.reliability.process_incoming(payload, messenger.time()) {
                    Ok(packets) => {
                        for incoming in packets {
                            messenger
                                .send_event((self.address, ConnectionEvent::Packet(incoming.0)));
                        }
                    }
                    Err(err) => error!("Error occured processing incomming packet: {:?}", err),
                }
            }
            (PacketHeaderType::Challenge(server_salt), ConnectionState::Connecting(state)) if state.client_salt == packet.identity => {
                state.hashed_salt = state.client_salt ^ server_salt;
            }
            (PacketHeaderType::ConnectionAccepted, ConnectionState::Connecting(state)) if state.hashed_salt == packet.identity => {
                self.state = ConnectionState::Connected(ConnectedState{
                    identity: packet.identity,
                    reliability: ReliabilitySystem::new(messenger.config()),
                    activity: ActivitySystem::new(messenger.config().idle_connection_timeout, messenger.time()),
                });
            }
            (PacketHeaderType::ConnectionDenied, ConnectionState::Connecting(state)) if state.client_salt == packet.identity => {
                self.state = ConnectionState::Disconnected(DisconnectedState{
                    immediately: true,
                    destroy_reason: DestroyReason::Declined
                });
            }
            _ => {}
        }

        // match (packet.packet, &mut self.state) {
        //     (PacketHeaderType::Payload(payload), ConnectionState::Connected(state))
        //         if self.client_server_salt == packet.identity =>
        //     {
        //         match self.reliability.process_incoming(payload, messenger.time()) {
        //             Ok(packets) => {
        //                 for incoming in packets {
        //                     messenger
        //                         .send_event((self.address, ConnectionEvent::Packet(incoming.0)));
        //                 }
        //             }
        //             Err(err) => error!("Error occured processing incomming packet: {:?}", err),
        //         }
        //     }
        //     (PacketHeaderType::Challenge(server_salt), ConnectionState::Connecting)
        //         if self.client_salt == packet.identity =>
        //     {
        //         self.client_server_salt = self.client_salt ^ server_salt;
        //     }
        //     (PacketHeaderType::ConnectionAccepted, ConnectionState::Connecting)
        //         if self.client_server_salt == packet.identity =>
        //     {
        //         self.state = ConnectionState::Connected;
        //         messenger.send_event((self.address, ConnectionEvent::Connected));
        //     }
        //     (PacketHeaderType::ConnectionDenied, ConnectionState::Connecting)
        //         if self.client_salt == packet.identity =>
        //     {
        //         self.state = ConnectionState::Disconnected;
        //         self.destroy_reason = Some(DestroyReason::Declined);
        //     }
        //     (PacketHeaderType::Disconnect, ConnectionState::Connected)
        //         if self.client_server_salt == packet.identity =>
        //     {
        //         self.state = ConnectionState::Disconnected;
        //         self.destroy_reason = Some(DestroyReason::GracefullyDisconnected);
        //         messenger.send_event((
        //             self.address,
        //             ConnectionEvent::Disconnected(DisconnectReason::ClosedByRemoteHost),
        //         ));
        //     }
        //     _ => return,
        // }
        // self.activity.received(messenger.time());
    }

    fn user_packet(
        &mut self,
        messenger: &mut dyn ContextWithSender<(SocketAddr, ConnectionEvent)>,
        packet: Packet,
    ) {
        // if let ConnectionState::Connected = self.state {
        //     let packet = PacketInfo::user_packet(
        //         packet.payload().as_ref(),
        //         packet.delivery_guarantee(),
        //         packet.order_guarantee(),
        //     );

        //     match self
        //         .reliability
        //         .process_outgoing(packet, None, messenger.time())
        //     {
        //         Ok(packets) => {
        //             self.activity.sent(messenger.time());
        //             for outgoing in packets {
        //                 self.send_packet(
        //                     self.client_server_salt,
        //                     PacketHeaderType::Payload(outgoing.contents().as_ref()),
        //                     messenger,
        //                 );
        //             }
        //         }
        //         Err(err) => error!("Error occured processing incomming packet: {:?}", err),
        //     }
        // }
    }

    fn disconnect(&mut self, messenger: &mut dyn ContextWithSender<(SocketAddr, ConnectionEvent)>) {
        // if let ConnectionState::Connected = self.state {
        //     self.state = ConnectionState::Disconnected;
        //     messenger.send_event((
        //         self.address,
        //         ConnectionEvent::Disconnected(DisconnectReason::ClosedByLocalHost),
        //     ));
        // }
    }

    fn update(&mut self, messenger: &mut dyn ContextWithSender<(SocketAddr, ConnectionEvent)>) {
        // if let ConnectionState::Connecting = self.state {
        //     if self.client_server_salt == 0 {

        //     } else {

        //     }
        // }
    }
    fn is_connected(&self) -> bool {
        false
        // match self.state {
        //     ConnectionState::Connected => true,
        //     _ => false,
        // }
    }

    fn should_drop(&self) -> Option<DestroyReason> {
        None
        // match &self.destroy_reason {
        //     Some(reason) => Some(reason.clone()),
        //     None => None,
        // }
    }
}
