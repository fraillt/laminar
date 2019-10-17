use std::io::{Cursor, Write};

use byteorder::{LittleEndian, ReadBytesExt, WriteBytesExt};
use crc;
use lazy_static::lazy_static;

use crate::error::{DecodingErrorKind, ErrorKind, PacketErrorKind, Result};
use crate::packet::{DeliveryGuarantee, OrderingGuarantee, PacketInfo, PacketType};
use crate::protocol_version::ProtocolVersion;

// save some bytes, instead of having 3 bytes, put everything in 1 byte.
const PACKET_TYPE_MASK: u8 = 0b00000111u8;
const RELIABILITY_TYPE_MASK: u8 = 0b01111000u8;
const FRAGMENTATION_TYPE_MASK: u8 = 0b10000000u8;

const HEADER_SIZE: usize = 15;

#[derive(Debug, PartialEq, Clone)]
pub enum PacketHeaderType<'a> {
    Disconnect,
    ConnectionRequest,
    Challenge(u64),
    ChallengeResponse,
    Payload(PacketInfo<'a>),
}

/// Packet header structure.
/// * [0-1] - Protocol version.
/// * [2-5] - CRC32 (range [6..]).
/// * 6 - Packet type (same byte is used to store reliability and fragmentation info).
/// * [7..] - Packet related information.
#[derive(Debug)]
pub struct PacketHeader<'a> {
    pub identity: u64,
    pub packet: PacketHeaderType<'a>,
}

lazy_static! {
    // this is used to check and to write zeroed payload
    static ref ZERO_BUFFER: Vec<u8> = vec![0; 1000 - HEADER_SIZE];
}

impl<'a> PacketHeader<'a> {
    pub fn from_bytes(value: &'a [u8]) -> Result<Self> {
        let mut rdr = Cursor::new(value);
        let protocol_version = rdr.read_u16::<LittleEndian>()?;
        if !ProtocolVersion::valid_version(protocol_version) {
            return Err(ErrorKind::ProtocolVersionMismatch);
        }
        let checksum = rdr.read_u32::<LittleEndian>()?;
        // checksum after reading protocol and checksum itself
        if checksum != crc::crc32::checksum_koopman(&value[6..]) {
            return Err(ErrorKind::ProtocolVersionMismatch);
        }
        let packet_type_byte = rdr.read_u8()?;
        let identity = rdr.read_u64::<LittleEndian>()?;
        let packet_type = packet_type_byte & PACKET_TYPE_MASK;
        let payload: &[u8] = &value[rdr.position() as usize..];
        let packet = match packet_type {
            0 => PacketHeaderType::Disconnect,
            1 => {
                if *ZERO_BUFFER != payload {
                    return Err(ErrorKind::DecodingError(DecodingErrorKind::PacketType));
                }
                PacketHeaderType::ConnectionRequest
            }
            2 => PacketHeaderType::Challenge(rdr.read_u64::<LittleEndian>()?),
            3 => {
                if *ZERO_BUFFER != payload {
                    return Err(ErrorKind::DecodingError(DecodingErrorKind::PacketType));
                }
                PacketHeaderType::ChallengeResponse
            }
            4 => PacketHeaderType::Payload(get_packet_info(packet_type_byte, payload)?),
            _ => return Err(ErrorKind::DecodingError(DecodingErrorKind::PacketType)),
        };
        Ok(Self { identity, packet })
    }

    pub fn into_bytes(self, buf: &mut [u8]) -> Result<usize> {
        // skip all main fields, and write packet data
        let (packet_type, written_bytes) =
            self.write_packet_data_and_get_packet_type(&mut buf[HEADER_SIZE..])?;
        let packet_size = written_bytes + HEADER_SIZE;
        // split buffer after checksum field
        let (mut before_buf, after_buf) = buf.split_at_mut(6);
        // write fields before calculating checksum, we need `tmp` variable, because after write_* slice will be changed.
        let mut tmp = &mut *after_buf;
        tmp.write_u8(packet_type)?;
        tmp.write_u64::<LittleEndian>(self.identity)?;

        let checksum = crc::crc32::checksum_koopman(&after_buf[..packet_size - 6]);
        // write skipped fields
        before_buf.write_u16::<LittleEndian>(ProtocolVersion::get_crc16())?;
        before_buf.write_u32::<LittleEndian>(checksum)?;

        Ok(packet_size)
    }

    fn write_packet_data_and_get_packet_type(&self, mut buf: &mut [u8]) -> Result<(u8, usize)> {
        // save buffer size
        let buff_size = buf.len();

        let packet_type = match &self.packet {
            PacketHeaderType::Disconnect => 0,
            PacketHeaderType::ConnectionRequest => {
                buf.write(ZERO_BUFFER.as_ref())?;
                1
            }
            PacketHeaderType::Challenge(server_salt) => {
                buf.write_u64::<LittleEndian>(*server_salt)?;
                2
            }
            PacketHeaderType::ChallengeResponse => {
                buf.write(ZERO_BUFFER.as_ref())?;
                3
            }
            PacketHeaderType::Payload(packet_info) => {
                buf.write(packet_info.payload)?;
                let fragment_byte = if packet_info.packet_type == PacketType::Fragment {
                    1
                } else {
                    0
                };
                let reliability_byte: u8 = match (packet_info.delivery, packet_info.ordering) {
                    (DeliveryGuarantee::Unreliable, OrderingGuarantee::None) => 0,
                    (DeliveryGuarantee::Unreliable, OrderingGuarantee::Sequenced(_)) => 1,
                    (DeliveryGuarantee::Reliable, OrderingGuarantee::None) => 2,
                    (DeliveryGuarantee::Reliable, OrderingGuarantee::Sequenced(_)) => 3,
                    (DeliveryGuarantee::Reliable, OrderingGuarantee::Ordered(_)) => 4,
                    _ => panic!(
                        "Unsupported reliability combination {:?}-{:?}",
                        packet_info.delivery, packet_info.ordering
                    ),
                };
                4 | (reliability_byte << 3) | (fragment_byte << 7)
            }
        };
        // we must be sure, that write buffer must be always bigger
        Ok((packet_type, buff_size - buf.len()))
    }
}

fn get_packet_info(type_byte: u8, payload: &[u8]) -> Result<PacketInfo> {
    let reliability_type = (type_byte & RELIABILITY_TYPE_MASK) >> 3;
    let fragmentation_type = (type_byte & FRAGMENTATION_TYPE_MASK) >> 7;
    let packet_type = if fragmentation_type == 0 {
        PacketType::Packet
    } else {
        PacketType::Fragment
    };
    let (delivery, ordering) = match reliability_type {
        0 => (DeliveryGuarantee::Unreliable, OrderingGuarantee::None),
        1 => (
            DeliveryGuarantee::Unreliable,
            OrderingGuarantee::Sequenced(None),
        ),
        2 => (DeliveryGuarantee::Reliable, OrderingGuarantee::None),
        3 => (
            DeliveryGuarantee::Reliable,
            OrderingGuarantee::Sequenced(None),
        ),
        4 => (
            DeliveryGuarantee::Reliable,
            OrderingGuarantee::Ordered(None),
        ),
        _ => return Err(ErrorKind::DecodingError(DecodingErrorKind::PacketType)),
    };
    Ok(PacketInfo {
        packet_type,
        payload,
        delivery,
        ordering,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_and_read<'a>(header: PacketHeader, buf: &'a mut [u8]) -> (PacketHeader<'a>, usize) {
        let written = header.into_bytes(buf).unwrap();
        (PacketHeader::from_bytes(&buf[..written]).unwrap(), written)
    }

    #[test]
    fn write_read_packet_type_disconnect() {
        let mut buffer = [0; 1500];
        let (res, written) = write_and_read(
            PacketHeader {
                identity: 654818944169854,
                packet: PacketHeaderType::Disconnect,
            },
            &mut buffer,
        );
        assert_eq!(written, HEADER_SIZE);
        assert_eq!(res.identity, 654818944169854);
        assert_eq!(res.packet, PacketHeaderType::Disconnect);
    }

    #[test]
    fn write_read_packet_type_connection_request() {
        let mut buffer = [0; 1500];
        let (res, written) = write_and_read(
            PacketHeader {
                identity: 8945186514,
                packet: PacketHeaderType::ConnectionRequest,
            },
            &mut buffer,
        );
        assert_eq!(written, 1000);
        assert_eq!(res.identity, 8945186514);
        assert_eq!(res.packet, PacketHeaderType::ConnectionRequest);
    }

    #[test]
    fn write_read_packet_type_challenge() {
        let mut buffer = [0; 1500];
        let (res, written) = write_and_read(
            PacketHeader {
                identity: 8716546546878,
                packet: PacketHeaderType::Challenge(98548465498746),
            },
            &mut buffer,
        );
        assert_eq!(written, HEADER_SIZE + 8);
        assert_eq!(res.identity, 8716546546878);
        assert_eq!(res.packet, PacketHeaderType::Challenge(98548465498746));
    }

    #[test]
    fn write_read_packet_type_challenge_response() {
        let mut buffer = [0; 1500];
        let (res, written) = write_and_read(
            PacketHeader {
                identity: 8716546546878,
                packet: PacketHeaderType::ChallengeResponse,
            },
            &mut buffer,
        );
        assert_eq!(written, 1000);
        assert_eq!(res.identity, 8716546546878);
        assert_eq!(res.packet, PacketHeaderType::ChallengeResponse);
    }

    #[test]
    fn write_read_packet_type_payload1() {
        let mut buffer = [0; 1500];
        let payload = [54, 1, 98, 94, 37, 159];
        let (res, written) = write_and_read(
            PacketHeader {
                identity: 8716546546878,
                packet: PacketHeaderType::Payload(PacketInfo {
                    packet_type: PacketType::Packet,
                    payload: &payload,
                    delivery: DeliveryGuarantee::Unreliable,
                    ordering: OrderingGuarantee::None,
                }),
            },
            &mut buffer,
        );
        assert_eq!(written, HEADER_SIZE + payload.len());
        assert_eq!(res.identity, 8716546546878);
        assert_eq!(
            res.packet,
            PacketHeaderType::Payload(PacketInfo {
                packet_type: PacketType::Packet,
                payload: &payload,
                delivery: DeliveryGuarantee::Unreliable,
                ordering: OrderingGuarantee::None
            })
        );
    }

    #[test]
    fn write_read_packet_type_payload2() {
        let mut buffer = [0; 1500];
        let payload = [];
        let (res, written) = write_and_read(
            PacketHeader {
                identity: 3216576541897651987,
                packet: PacketHeaderType::Payload(PacketInfo {
                    packet_type: PacketType::Fragment,
                    payload: &payload,
                    delivery: DeliveryGuarantee::Reliable,
                    ordering: OrderingGuarantee::Sequenced(None),
                }),
            },
            &mut buffer,
        );
        assert_eq!(written, HEADER_SIZE + payload.len());
        assert_eq!(res.identity, 3216576541897651987);
        assert_eq!(
            res.packet,
            PacketHeaderType::Payload(PacketInfo {
                packet_type: PacketType::Fragment,
                payload: &payload,
                delivery: DeliveryGuarantee::Reliable,
                ordering: OrderingGuarantee::Sequenced(None)
            })
        );
    }

    #[test]
    fn all_malformed_packets_should_be_rejected() {
        let mut buf = [0; 1000];
        // create valid packet, so we could reuse data in it, without breaking early
        let valid_packet = PacketHeader {
            identity: 3216576541897651987,
            packet: PacketHeaderType::Payload(PacketInfo {
                packet_type: PacketType::Fragment,
                payload: &[123, 54, 98, 67, 98, 4],
                delivery: DeliveryGuarantee::Reliable,
                ordering: OrderingGuarantee::Sequenced(None),
            }),
        };
        let written = valid_packet.into_bytes(&mut buf).unwrap();

        for i in 0..30 {
            match PacketHeader::from_bytes(&buf[0..i]) {
                Ok(_) if i == written => (),
                Err(_) if i != written => (),
                _ => panic!("something is wrong"),
            }
        }
    }
}
