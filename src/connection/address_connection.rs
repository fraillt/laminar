use std::net::SocketAddr;
use std::time::{Duration, Instant};

use log::error;
use rand::RngCore;

use crate::{
    config::Config,
    net::{ActivitySystem, ContextWithSender, ReliabilitySystem},
    packet::{DeliveryGuarantee, Packet, PacketInfo},
};

use super::events::{ConnectionEvent, DestroyReason, DisconnectReason};
use super::packet_header::{PacketHeader, PacketHeaderType};

const SEND_INTERVAL_DURING_HANDSHAKE: Duration = Duration::from_millis(200); // 5 packets per second

#[derive(Debug)]
struct PacketToSend {
    send_packet: PacketHeaderType<'static>,
    remote_identity: u64,
    send_at: Instant,
    send_times_left: u8,
}

#[derive(Debug)]
struct ConnectionState {
    client_salt: u64,
    server_salt: u64,
}

impl ConnectionState {
    fn get_identity(&self) -> u64 {
        self.client_salt ^ self.server_salt
    }
}

#[derive(Debug)]
pub struct AddressConnection {
    address: SocketAddr,
    active: Option<ConnectionState>,
    pending: Vec<ConnectionState>,
    packet_to_send: Option<PacketToSend>,
    destroy_reason: Option<DestroyReason>,
    reliability: ReliabilitySystem,
    activity: ActivitySystem,
}

impl AddressConnection {
    pub fn new(address: SocketAddr, config: &Config, time: Instant) -> Self {
        Self {
            address,
            active: None,
            pending: Vec::default(),
            packet_to_send: None,
            destroy_reason: None,
            reliability: ReliabilitySystem::new(config),
            activity: ActivitySystem::new(config.idle_connection_timeout, time),
        }
    }

    pub fn process_payload(
        &mut self,
        identity: u64,
        payload: &[u8],
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        if let Some(state) = &self.active {
            if state.get_identity() == identity {
                self.activity.received(messenger.time());
                match self.reliability.process_incoming(payload, messenger.time()) {
                    Ok(packets) => {
                        for incoming in packets {
                            messenger
                                .send_event((self.address, ConnectionEvent::Packet(incoming.0)));
                        }
                    }
                    Err(err) => error!("Error occured processing incomming packet: {:?}", err),
                }
            }
        }
    }

    pub fn process_disconnect(
        &mut self,
        identity: u64,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        if let Some(state) = &self.active {
            if state.get_identity() == identity {
                self.disconnect(identity, messenger, DisconnectReason::ClosedByRemoteHost);
            }
        }
    }

    pub fn process_connection_request(
        &mut self,
        client_salt: u64,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
        max_pending_connections: usize,
    ) {
        let mut server_salt = None;
        if let Some(state) = self.pending.iter().find(|p| p.client_salt == client_salt) {
            server_salt = Some(state.server_salt);
        } else if self.pending.len() < max_pending_connections {
            let mut salt;
            // generate such a salt, that connection `identity` doesn't collide with existing pending connections
            loop {
                salt = rand::thread_rng().next_u64();
                let identity = client_salt ^ salt;

                if self
                    .active
                    .iter()
                    .chain(self.pending.iter())
                    .find(|info| info.client_salt ^ info.server_salt == identity)
                    .is_none()
                {
                    break;
                }
            }
            server_salt = Some(salt);
            self.pending.push(ConnectionState {
                client_salt: client_salt,
                server_salt: salt,
            });
            self.activity.received(messenger.time());
        }
        if let Some(salt) = server_salt {
            self.send_packet(client_salt, PacketHeaderType::Challenge(salt), messenger);
        } else {
            self.send_packet(client_salt, PacketHeaderType::ConnectionDenied, messenger);
        }
    }

    pub fn process_challenge(
        &mut self,
        client_salt: u64,
        server_salt: u64,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        if self.active.is_none() && self.pending.len() == 1 {
            let pending = &mut self.pending[0];
            if pending.client_salt == client_salt {
                self.activity.received(messenger.time());
                pending.server_salt = server_salt;
                self.packet_to_send = Some(PacketToSend {
                    send_packet: PacketHeaderType::ChallengeResponse,
                    remote_identity: pending.client_salt ^ pending.server_salt,
                    send_at: messenger.time(),
                    send_times_left: 10,
                });
            }
        }
    }

    /// Returns true only if accepted and previously wasn't connected.
    pub fn process_challenge_response(
        &mut self,
        identity: u64,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
        can_accept_new: bool,
    ) -> bool {
        if let Some(active) = &self.active {
            if active.get_identity() == identity {
                self.activity.received(messenger.time());
                self.send_packet(identity, PacketHeaderType::ConnectionAccepted, messenger);
                return false;
            }
        }
        if let Some(index) = self
            .pending
            .iter()
            .position(|p| p.client_salt ^ p.server_salt == identity)
        {
            let is_new_connection;
            if self.active.is_some() {
                messenger.send_event((self.address, ConnectionEvent::Reconnected));
                is_new_connection = false;
            } else if can_accept_new {
                messenger.send_event((self.address, ConnectionEvent::Connected));
                is_new_connection = true;
            } else {
                self.send_packet(identity, PacketHeaderType::ConnectionDenied, messenger);
                return false;
            }
            self.active = Some(self.pending.swap_remove(index));
            self.activity.received(messenger.time());
            self.reliability.reset();
            self.destroy_reason = None;
            self.send_packet(identity, PacketHeaderType::ConnectionAccepted, messenger);
            return is_new_connection;
        } else {
            self.send_packet(identity, PacketHeaderType::ConnectionDenied, messenger);
        }
        false
    }

    pub fn process_connection_accepted(
        &mut self,
        identity: u64,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        if self.active.is_none()
            && self.pending.len() == 1
            && self.pending[0].get_identity() == identity
        {
            self.activity.received(messenger.time());
            self.active = Some(self.pending.swap_remove(0));
            self.packet_to_send = None;
            self.destroy_reason = None;
            messenger.send_event((self.address, ConnectionEvent::Connected));
        }
    }

    pub fn process_connection_denied(
        &mut self,
        client_salt: u64,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        if self.active.is_none()
            && self.pending.len() == 1
            && self.pending[0].client_salt == client_salt
        {
            self.pending.clear();
            self.packet_to_send = None;
            self.destroy_reason = Some(DestroyReason::Declined);
        }
    }

    pub fn user_packet(
        &mut self,
        packet: Packet,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        if let Some(active) = &self.active {
            let packet = PacketInfo::user_packet(
                packet.payload().as_ref(),
                packet.delivery_guarantee(),
                packet.order_guarantee(),
            );

            match self
                .reliability
                .process_outgoing(packet, None, messenger.time())
            {
                Ok(packets) => {
                    self.activity.sent(messenger.time());
                    for outgoing in packets {
                        self.send_packet(
                            active.get_identity(),
                            PacketHeaderType::Payload(outgoing.contents().as_ref()),
                            messenger,
                        );
                    }
                }
                Err(err) => error!("Error occured processing incomming packet: {:?}", err),
            }
        }
    }

    /// Initiates a new connection if not already connected, by removing all previous handshaking state.
    pub fn user_connect(
        &mut self,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        if self.active.is_none() {
            let salt = rand::thread_rng().next_u64();
            self.pending.clear();
            self.reliability.reset();
            self.pending.push(ConnectionState {
                client_salt: salt,
                server_salt: 0,
            });

            self.packet_to_send = Some(PacketToSend {
                send_packet: PacketHeaderType::ConnectionRequest,
                remote_identity: salt,
                send_at: messenger.time(),
                send_times_left: 100,
            });
        }
    }

    pub fn user_disconnect(
        &mut self,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        if let Some(state) = &self.active {
            self.disconnect(
                state.get_identity(),
                messenger,
                DisconnectReason::ClosedByLocalHost,
            );
        }
    }

    pub fn should_drop(&self) -> Option<DestroyReason> {
        if let Some(reason) = &self.destroy_reason {
            if self.packet_to_send.is_none() {
                return Some(reason.clone());
            }
        }
        None
    }

    pub fn update(
        &mut self,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        if self.packet_to_send.is_some() {
            let mut packet = self.packet_to_send.as_mut().unwrap(); // we can unwrap, we checked it before
            if messenger.time() > packet.send_at {
                packet.send_at = messenger.time() + SEND_INTERVAL_DURING_HANDSHAKE;
                if packet.send_times_left > 1 {
                    packet.send_times_left = packet.send_times_left - 1;
                    let packet_to_send = packet.send_packet.clone();
                    let identity = packet.remote_identity;
                    self.activity.sent(messenger.time());
                    self.send_packet(identity, packet_to_send, messenger);
                } else {
                    self.packet_to_send = None;
                }
            }
        }
        if self.destroy_reason.is_none() && !self.activity.is_active(messenger.time()) {
            self.prepare_drop(messenger, DestroyReason::Timeout);
        }
        if self.reliability.packets_in_flight() > messenger.config().max_packets_in_flight {
            self.prepare_drop(messenger, DestroyReason::TooManyPacketsInFlight);
        }

        if let Some(state) = &self.active {
            for dropped in self.reliability.gather_dropped_packets().into_iter() {
                match self.reliability.process_outgoing(
                    PacketInfo {
                        packet_type: dropped.packet_type,
                        payload: &dropped.payload,
                        // because a delivery guarantee is only sent with reliable packets
                        delivery: DeliveryGuarantee::Reliable,
                        // this is stored with the dropped packet because they could be mixed
                        ordering: dropped.ordering_guarantee,
                    },
                    dropped.item_identifier,
                    messenger.time(),
                ) {
                    Ok(packets) => {
                        self.activity.sent(messenger.time());
                        for outgoing in packets {
                            self.send_packet(
                                state.get_identity(),
                                PacketHeaderType::Payload(outgoing.contents().as_ref()),
                                messenger,
                            );
                        }
                    }
                    Err(err) => error!("Error occured processing incomming packet: {:?}", err),
                }
            }
        }
    }

    pub fn is_connected(&self) -> bool {
        self.active.is_some()
    }

    fn send_packet(
        &self,
        identity: u64,
        packet: PacketHeaderType,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
    ) {
        let header = PacketHeader { identity, packet };
        let written = header
            .into_bytes(messenger.buffer())
            .expect("Do not fail, when ");
        messenger.send_packet_from_buffer(&self.address, written);
    }

    fn disconnect(
        &mut self,
        identity: u64,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
        reason: DisconnectReason,
    ) {
        self.pending.clear();
        self.destroy_reason = Some(DestroyReason::GracefullyDisconnected);
        self.active = None;
        if let DisconnectReason::ClosedByLocalHost = reason {
            self.packet_to_send = Some(PacketToSend {
                send_packet: PacketHeaderType::Disconnect,
                remote_identity: identity,
                send_at: messenger.time(),
                send_times_left: 10,
            });
        } else {
            self.packet_to_send = None;
        }
        messenger.send_event((self.address, ConnectionEvent::Disconnected(reason)));
    }

    fn prepare_drop(
        &mut self,
        messenger: &mut impl ContextWithSender<(SocketAddr, ConnectionEvent)>,
        reason: DestroyReason,
    ) {
        if self.active.is_some() {
            messenger.send_event((
                self.address,
                ConnectionEvent::Disconnected(DisconnectReason::Destroying(reason.clone())),
            ));
        }
        self.reliability.reset();
        self.active = None;
        self.packet_to_send = None;
        self.destroy_reason = Some(reason);
    }
}
