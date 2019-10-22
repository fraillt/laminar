use std::fmt;
use std::time::Instant;

use crate::{
    config::Config,
    error::{ErrorKind, PacketErrorKind, Result},
    infrastructure::{
        arranging::{Arranging, ArrangingSystem, OrderingSystem, SequencingSystem},
        AcknowledgmentHandler, CongestionHandler, Fragmentation, SentPacket,
    },
    net::constants::{
        ACKED_PACKET_HEADER, DEFAULT_ORDERING_STREAM, DEFAULT_SEQUENCING_STREAM,
        STANDARD_HEADER_SIZE,
    },
    packet::{
        DeliveryGuarantee, IncomingPackets, OrderingGuarantee, OutgoingPacketBuilder,
        OutgoingPackets, Packet, PacketInfo, PacketReader, PacketType, SequenceNumber,
    },
};

/// Contains the information about a certain 'virtual connection' over udp.
/// This connections also keeps track of network quality, processing packets, buffering data related to connection etc.
pub struct ReliabilitySystem {
    config: Config,
    ordering_system: OrderingSystem<(Box<[u8]>, PacketType)>,
    sequencing_system: SequencingSystem<Box<[u8]>>,
    acknowledge_handler: AcknowledgmentHandler,
    congestion_handler: CongestionHandler,
    fragmentation: Fragmentation,
}

// TODO make everything derive(Debug)
impl fmt::Debug for ReliabilitySystem {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "<ReliabilitySystem>",)
    }
}

impl ReliabilitySystem {
    /// Creates and returns a new Connection that wraps the provided socket address
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.to_owned(),
            ordering_system: OrderingSystem::new(),
            sequencing_system: SequencingSystem::new(),
            acknowledge_handler: AcknowledgmentHandler::new(),
            congestion_handler: CongestionHandler::new(config),
            fragmentation: Fragmentation::new(config),
        }
    }

    pub fn reset(&mut self) {
        self.ordering_system = OrderingSystem::new();
        self.sequencing_system = SequencingSystem::new();
        self.acknowledge_handler = AcknowledgmentHandler::new();
        self.congestion_handler = CongestionHandler::new(&self.config);
        self.fragmentation = Fragmentation::new(&self.config);
    }

    pub fn packets_in_flight(&self) -> u16 {
        self.acknowledge_handler.packets_in_flight()
    }

    /// Pre-processes the given buffer to be sent over the network.
    pub fn process_outgoing<'a>(
        &mut self,
        packet: PacketInfo<'a>,
        last_item_identifier: Option<SequenceNumber>,
        time: Instant,
    ) -> Result<OutgoingPackets<'a>> {
        match packet.delivery {
            DeliveryGuarantee::Unreliable => {
                if packet.payload.len() <= self.config.receive_buffer_max_size {
                    if packet.packet_type == PacketType::Heartbeat {
                        self.congestion_handler
                            .process_outgoing(self.acknowledge_handler.local_sequence_num(), time);
                    }

                    let mut builder = OutgoingPacketBuilder::new(packet.payload)
                        .with_default_header(packet.packet_type, packet.delivery, packet.ordering);

                    if let OrderingGuarantee::Sequenced(stream_id) = packet.ordering {
                        let item_identifier = self
                            .sequencing_system
                            .get_or_create_stream(stream_id.unwrap_or(DEFAULT_SEQUENCING_STREAM))
                            .new_item_identifier();

                        builder = builder.with_sequencing_header(item_identifier as u16, stream_id);
                    };

                    Ok(OutgoingPackets::one(builder.build()))
                } else {
                    Err(PacketErrorKind::ExceededMaxPacketSize.into())
                }
            }
            DeliveryGuarantee::Reliable => {
                let payload_length = packet.payload.len() as u16;

                let mut item_identifier_value = None;
                let outgoing = {
                    // spit the packet if the payload length is greater than the allowed fragment size.
                    if payload_length <= self.config.fragment_size {
                        let mut builder = OutgoingPacketBuilder::new(packet.payload)
                            .with_default_header(
                                packet.packet_type,
                                packet.delivery,
                                packet.ordering,
                            );

                        builder = builder.with_acknowledgment_header(
                            self.acknowledge_handler.local_sequence_num(),
                            self.acknowledge_handler.remote_sequence_num(),
                            self.acknowledge_handler.ack_bitfield(),
                        );

                        if let OrderingGuarantee::Ordered(stream_id) = packet.ordering {
                            let item_identifier =
                                if let Some(item_identifier) = last_item_identifier {
                                    item_identifier
                                } else {
                                    self.ordering_system
                                        .get_or_create_stream(
                                            stream_id.unwrap_or(DEFAULT_ORDERING_STREAM),
                                        )
                                        .new_item_identifier()
                                };

                            item_identifier_value = Some(item_identifier);

                            builder = builder.with_ordering_header(item_identifier, stream_id);
                        };

                        if let OrderingGuarantee::Sequenced(stream_id) = packet.ordering {
                            let item_identifier =
                                if let Some(item_identifier) = last_item_identifier {
                                    item_identifier
                                } else {
                                    self.sequencing_system
                                        .get_or_create_stream(
                                            stream_id.unwrap_or(DEFAULT_SEQUENCING_STREAM),
                                        )
                                        .new_item_identifier()
                                };

                            item_identifier_value = Some(item_identifier);

                            builder = builder.with_sequencing_header(item_identifier, stream_id);
                        };

                        OutgoingPackets::one(builder.build())
                    } else {
                        if packet.packet_type != PacketType::Packet {
                            return Err(PacketErrorKind::PacketCannotBeFragmented.into());
                        }
                        OutgoingPackets::many(
                            Fragmentation::spit_into_fragments(packet.payload, &self.config)?
                                .into_iter()
                                .enumerate()
                                .map(|(fragment_id, fragment)| {
                                    let fragments_needed = Fragmentation::fragments_needed(
                                        payload_length,
                                        self.config.fragment_size,
                                    )
                                        as u8;

                                    let mut builder = OutgoingPacketBuilder::new(fragment)
                                        .with_default_header(
                                            PacketType::Fragment, // change from Packet to Fragment type, it only matters when assembling/dissasembling packet header.
                                            packet.delivery,
                                            packet.ordering,
                                        );

                                    builder = builder.with_fragment_header(
                                        self.acknowledge_handler.local_sequence_num(),
                                        fragment_id as u8,
                                        fragments_needed,
                                    );

                                    if fragment_id == 0 {
                                        builder = builder.with_acknowledgment_header(
                                            self.acknowledge_handler.local_sequence_num(),
                                            self.acknowledge_handler.remote_sequence_num(),
                                            self.acknowledge_handler.ack_bitfield(),
                                        );
                                    }

                                    builder.build()
                                })
                                .collect(),
                        )
                    }
                };

                self.congestion_handler
                    .process_outgoing(self.acknowledge_handler.local_sequence_num(), time);
                self.acknowledge_handler.process_outgoing(
                    packet.packet_type,
                    packet.payload,
                    packet.ordering,
                    item_identifier_value,
                );

                Ok(outgoing)
            }
        }
    }

    /// Processes the incoming data and returns a packet once the data is complete.
    pub fn process_incoming(
        &mut self,
        received_data: &[u8],
        time: Instant,
    ) -> Result<IncomingPackets> {
        let mut packet_reader = PacketReader::new(received_data);

        let header = packet_reader.read_standard_header()?;

        match header.delivery_guarantee() {
            DeliveryGuarantee::Unreliable => {
                if let OrderingGuarantee::Sequenced(_id) = header.ordering_guarantee() {
                    let arranging_header =
                        packet_reader.read_arranging_header(u16::from(STANDARD_HEADER_SIZE))?;

                    let payload = packet_reader.read_payload();

                    let stream = self
                        .sequencing_system
                        .get_or_create_stream(arranging_header.stream_id());

                    if let Some(packet) = stream.arrange(arranging_header.arranging_id(), payload) {
                        return Ok(IncomingPackets::one(
                            Packet::new(
                                packet,
                                header.delivery_guarantee(),
                                OrderingGuarantee::Sequenced(Some(arranging_header.stream_id())),
                            ),
                            header.packet_type(),
                        ));
                    }

                    return Ok(IncomingPackets::zero());
                }

                return Ok(IncomingPackets::one(
                    Packet::new(
                        packet_reader.read_payload(),
                        header.delivery_guarantee(),
                        header.ordering_guarantee(),
                    ),
                    header.packet_type(),
                ));
            }
            DeliveryGuarantee::Reliable => {
                if header.is_fragment() {
                    if let Ok((fragment_header, acked_header)) = packet_reader.read_fragment() {
                        let payload = packet_reader.read_payload();

                        match self.fragmentation.handle_fragment(
                            fragment_header,
                            &payload,
                            acked_header,
                        ) {
                            Ok(Some((payload, acked_header))) => {
                                self.congestion_handler
                                    .process_incoming(acked_header.sequence());
                                self.acknowledge_handler.process_incoming(
                                    acked_header.sequence(),
                                    acked_header.ack_seq(),
                                    acked_header.ack_field(),
                                );

                                return Ok(IncomingPackets::one(
                                    Packet::new(
                                        payload.into_boxed_slice(),
                                        header.delivery_guarantee(),
                                        header.ordering_guarantee(),
                                    ),
                                    PacketType::Packet, // change from Fragment to Packet type, it only matters when assembling/dissasembling packet header.
                                ));
                            }
                            Ok(None) => return Ok(IncomingPackets::zero()),
                            Err(e) => return Err(e),
                        };
                    }
                } else {
                    let acked_header = packet_reader.read_acknowledge_header()?;

                    self.congestion_handler
                        .process_incoming(acked_header.sequence());
                    self.acknowledge_handler.process_incoming(
                        acked_header.sequence(),
                        acked_header.ack_seq(),
                        acked_header.ack_field(),
                    );

                    if let OrderingGuarantee::Sequenced(_) = header.ordering_guarantee() {
                        let arranging_header = packet_reader.read_arranging_header(u16::from(
                            STANDARD_HEADER_SIZE + ACKED_PACKET_HEADER,
                        ))?;

                        let payload = packet_reader.read_payload();

                        let stream = self
                            .sequencing_system
                            .get_or_create_stream(arranging_header.stream_id());

                        if let Some(packet) =
                            stream.arrange(arranging_header.arranging_id(), payload)
                        {
                            return Ok(IncomingPackets::one(
                                Packet::new(
                                    packet,
                                    header.delivery_guarantee(),
                                    OrderingGuarantee::Sequenced(Some(
                                        arranging_header.stream_id(),
                                    )),
                                ),
                                header.packet_type(),
                            ));
                        }
                    } else if let OrderingGuarantee::Ordered(_id) = header.ordering_guarantee() {
                        let arranging_header = packet_reader.read_arranging_header(u16::from(
                            STANDARD_HEADER_SIZE + ACKED_PACKET_HEADER,
                        ))?;

                        let payload = packet_reader.read_payload();

                        let stream = self
                            .ordering_system
                            .get_or_create_stream(arranging_header.stream_id());
                        return Ok(IncomingPackets::many(
                            stream
                                .arrange(
                                    arranging_header.arranging_id(),
                                    (payload, header.packet_type()),
                                )
                                .into_iter()
                                .chain(stream.iter_mut())
                                .map(|(packet, packet_type)| {
                                    (
                                        Packet::new(
                                            packet,
                                            header.delivery_guarantee(),
                                            OrderingGuarantee::Ordered(Some(
                                                arranging_header.stream_id(),
                                            )),
                                        ),
                                        packet_type,
                                    )
                                })
                                .collect(),
                        ));
                    } else {
                        let payload = packet_reader.read_payload();
                        return Ok(IncomingPackets::one(
                            Packet::new(
                                payload,
                                header.delivery_guarantee(),
                                header.ordering_guarantee(),
                            ),
                            header.packet_type(),
                        ));
                    }
                }
            }
        }
        Ok(IncomingPackets::zero())
    }

    /// Gathers dropped packets from the acknowledgment handler.
    ///
    /// Note that after requesting dropped packets the dropped packets will be removed from this client.
    pub fn gather_dropped_packets(&mut self) -> Vec<SentPacket> {
        self.acknowledge_handler.dropped_packets()
    }
}
