//! Open Sound Control 1.0 packets.
//!
//! This is deliberately one small module rather than a dependency: the desk
//! needs addresses, `i`, `f`, `s`, `b` and bundles, and nothing else. Every
//! length is checked before it is used, so a malformed datagram is an error and
//! never a panic.

use std::fmt;

const MAX_BUNDLE_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq)]
pub enum OscValue {
    Int(i32),
    Float(f32),
    String(String),
    Blob(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct OscMessage {
    pub address: String,
    pub values: Vec<OscValue>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OscError(pub String);

impl fmt::Display for OscError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for OscError {}

fn error(message: impl Into<String>) -> OscError {
    OscError(message.into())
}

/// Decode one datagram. A bundle is flattened in order; timetags are parsed so
/// the packet is validated but are otherwise ignored (the engine tick owns
/// scheduling).
pub fn decode(packet: &[u8]) -> Result<Vec<OscMessage>, OscError> {
    let mut messages = Vec::new();
    decode_packet(packet, 0, &mut messages)?;
    Ok(messages)
}

fn decode_packet(
    packet: &[u8],
    depth: usize,
    messages: &mut Vec<OscMessage>,
) -> Result<(), OscError> {
    if packet.starts_with(b"#bundle\0") {
        if depth >= MAX_BUNDLE_DEPTH {
            return Err(error("OSC bundle nesting is too deep"));
        }
        if packet.len() < 16 {
            return Err(error("truncated OSC bundle header"));
        }
        let mut position = 16;
        while position < packet.len() {
            if position + 4 > packet.len() {
                return Err(error("truncated OSC bundle element size"));
            }
            let size = read_u32(packet, position)? as usize;
            position += 4;
            let end = position
                .checked_add(size)
                .filter(|end| *end <= packet.len())
                .ok_or_else(|| error("OSC bundle element runs past the packet"))?;
            decode_packet(&packet[position..end], depth + 1, messages)?;
            position = end;
        }
        return Ok(());
    }
    messages.push(decode_message(packet)?);
    Ok(())
}

fn decode_message(packet: &[u8]) -> Result<OscMessage, OscError> {
    let mut position = 0;
    let address = read_string(packet, &mut position, "address")?;
    if !address.starts_with('/') {
        return Err(error("OSC address must start with `/`"));
    }
    if position == packet.len() {
        return Ok(OscMessage {
            address,
            values: Vec::new(),
        });
    }
    let tags = read_string(packet, &mut position, "type tag")?;
    let Some(tags) = tags.strip_prefix(',') else {
        return Err(error("OSC type tag must start with `,`"));
    };
    let mut values = Vec::with_capacity(tags.len());
    for tag in tags.chars() {
        match tag {
            'i' => {
                let value = read_u32(packet, position)?;
                position += 4;
                values.push(OscValue::Int(value as i32));
            }
            'f' => {
                let value = read_u32(packet, position)?;
                position += 4;
                values.push(OscValue::Float(f32::from_bits(value)));
            }
            's' => values.push(OscValue::String(read_string(
                packet,
                &mut position,
                "string argument",
            )?)),
            'b' => {
                let size = read_u32(packet, position)? as usize;
                position += 4;
                let end = position
                    .checked_add(size)
                    .filter(|end| *end <= packet.len())
                    .ok_or_else(|| error("OSC blob runs past the packet"))?;
                values.push(OscValue::Blob(packet[position..end].to_vec()));
                position = align4(end)?;
            }
            other => return Err(error(format!("unsupported OSC type tag `{other}`"))),
        }
    }
    if position != packet.len() {
        return Err(error("OSC arguments do not fill the packet"));
    }
    Ok(OscMessage { address, values })
}

/// Encode one message. Used by tests and by any future reply surface.
pub fn encode(message: &OscMessage) -> Vec<u8> {
    let mut bytes = Vec::new();
    write_string(&mut bytes, &message.address);
    let mut tags = String::from(",");
    for value in &message.values {
        tags.push(match value {
            OscValue::Int(_) => 'i',
            OscValue::Float(_) => 'f',
            OscValue::String(_) => 's',
            OscValue::Blob(_) => 'b',
        });
    }
    write_string(&mut bytes, &tags);
    for value in &message.values {
        match value {
            OscValue::Int(value) => bytes.extend_from_slice(&value.to_be_bytes()),
            OscValue::Float(value) => bytes.extend_from_slice(&value.to_bits().to_be_bytes()),
            OscValue::String(value) => write_string(&mut bytes, value),
            OscValue::Blob(value) => {
                bytes.extend_from_slice(&(value.len() as u32).to_be_bytes());
                bytes.extend_from_slice(value);
                while bytes.len() % 4 != 0 {
                    bytes.push(0);
                }
            }
        }
    }
    bytes
}

fn read_u32(packet: &[u8], position: usize) -> Result<u32, OscError> {
    let bytes = packet
        .get(position..position + 4)
        .ok_or_else(|| error("truncated OSC integer"))?;
    Ok(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

fn read_string(packet: &[u8], position: &mut usize, what: &str) -> Result<String, OscError> {
    let rest = packet
        .get(*position..)
        .ok_or_else(|| error(format!("missing OSC {what}")))?;
    let Some(length) = rest.iter().position(|byte| *byte == 0) else {
        return Err(error(format!("unterminated OSC {what}")));
    };
    let text = std::str::from_utf8(&rest[..length])
        .map_err(|_| error(format!("OSC {what} is not UTF-8")))?
        .to_string();
    *position = align4(*position + length + 1)?;
    if *position > packet.len() {
        return Err(error(format!("OSC {what} padding runs past the packet")));
    }
    Ok(text)
}

fn align4(value: usize) -> Result<usize, OscError> {
    value
        .checked_add(3)
        .map(|value| value & !3)
        .ok_or_else(|| error("OSC size overflow"))
}

fn write_string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(value.as_bytes());
    bytes.push(0);
    while bytes.len() % 4 != 0 {
        bytes.push(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_supported_values_round_trip() {
        let message = OscMessage {
            address: "/openlight/executor/1/fader".into(),
            values: vec![
                OscValue::Int(7),
                OscValue::Float(0.75),
                OscValue::String("hello".into()),
                OscValue::Blob(vec![1, 2, 3]),
            ],
        };
        assert_eq!(decode(&encode(&message)).unwrap(), vec![message]);
    }

    #[test]
    fn bundles_are_flattened_in_order() {
        let first = encode(&OscMessage {
            address: "/openlight/grand".into(),
            values: vec![OscValue::Float(0.5)],
        });
        let second = encode(&OscMessage {
            address: "/openlight/blackout".into(),
            values: vec![OscValue::Int(1)],
        });
        let mut bundle = b"#bundle\0".to_vec();
        bundle.extend_from_slice(&1u64.to_be_bytes());
        for message in [&first, &second] {
            bundle.extend_from_slice(&(message.len() as u32).to_be_bytes());
            bundle.extend_from_slice(message);
        }
        let decoded = decode(&bundle).unwrap();
        assert_eq!(decoded.len(), 2);
        assert_eq!(decoded[0].address, "/openlight/grand");
        assert_eq!(decoded[1].address, "/openlight/blackout");
    }

    #[test]
    fn a_truncated_packet_is_an_error_not_a_panic() {
        let message = OscMessage {
            address: "/openlight/grand".into(),
            values: vec![OscValue::Float(0.5)],
        };
        let bytes = encode(&message);
        for end in 0..bytes.len() {
            let _ = decode(&bytes[..end]);
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(decode(&trailing).is_err());
        assert!(decode(&[0, 1, 2]).is_err());
    }

    #[test]
    fn unknown_type_tags_are_rejected() {
        let mut bytes = b"/x\0\0".to_vec();
        bytes.extend_from_slice(b",d\0\0");
        bytes.extend_from_slice(&0u64.to_be_bytes());
        assert!(decode(&bytes).is_err());
    }

    #[test]
    fn future_timetags_are_ignored_not_scheduled() {
        let message = encode(&OscMessage {
            address: "/openlight/grand".into(),
            values: vec![OscValue::Float(1.0)],
        });
        let mut bundle = b"#bundle\0".to_vec();
        bundle.extend_from_slice(&u64::MAX.to_be_bytes());
        bundle.extend_from_slice(&(message.len() as u32).to_be_bytes());
        bundle.extend_from_slice(&message);
        assert_eq!(decode(&bundle).unwrap().len(), 1);
    }
}
