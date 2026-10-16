//! Bounded RFC 6455 framing. No compression/extensions are negotiated.
//! SHA-1 is used exclusively for the public Upgrade nonce checksum.
use crate::{validation::MAX_FRAME_BYTES, Error};
use base64::Engine;
use std::collections::VecDeque;

#[derive(Debug, PartialEq)]
pub enum FrameEvent {
    Message(Vec<u8>),
    Ping(Vec<u8>),
    Pong(Vec<u8>),
    Close(Vec<u8>),
}
#[derive(Default)]
pub struct Decoder {
    bytes: Vec<u8>,
    offset: usize,
    fragment: Option<(u8, Vec<u8>)>,
    events: VecDeque<FrameEvent>,
    closed: bool,
    expect_mask: bool,
}
impl Decoder {
    /// Server endpoint: accepts only masked client frames.
    pub fn server() -> Self {
        Self {
            expect_mask: true,
            ..Self::default()
        }
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if self.closed {
            return Err(protocol("Data received after Close"));
        }
        if bytes.len() > 65536
            || self.bytes.len() - self.offset > MAX_FRAME_BYTES + 14 - bytes.len()
        {
            return Err(protocol("WebSocket buffer exceeds its limit"));
        }
        if self.offset > 65536 || self.offset == self.bytes.len() {
            self.bytes.drain(..self.offset);
            self.offset = 0;
        }
        self.bytes.extend_from_slice(bytes);
        while self.parse()? {}
        Ok(())
    }
    fn emit(&mut self, event: FrameEvent) -> Result<(), Error> {
        if self.events.len() >= 64 {
            return Err(Error::QueueFull);
        }
        self.events.push_back(event);
        Ok(())
    }
    fn parse(&mut self) -> Result<bool, Error> {
        let bytes = &self.bytes[self.offset..];
        if bytes.len() < 2 {
            return Ok(false);
        }
        let fin = bytes[0] & 128 != 0;
        let opcode = bytes[0] & 15;
        if bytes[0] & 112 != 0
            || (bytes[1] & 128 != 0) != self.expect_mask
            || ![0, 1, 2, 8, 9, 10].contains(&opcode)
        {
            return Err(protocol("Invalid frame flags/opcode/masking"));
        }
        let short = bytes[1] & 127;
        let length_header = match short {
            126 => 4,
            127 => 10,
            _ => 2,
        };
        let header = length_header + if self.expect_mask { 4 } else { 0 };
        if bytes.len() < header {
            return Ok(false);
        }
        let length = match short {
            126 => u16::from_be_bytes([bytes[2], bytes[3]]) as u64,
            127 => u64::from_be_bytes(bytes[2..10].try_into().unwrap()),
            _ => short as u64,
        };
        if (short == 126 && length < 126)
            || (short == 127 && (length <= 65535 || length >> 63 != 0))
            || length > MAX_FRAME_BYTES as u64
        {
            return Err(protocol("Noncanonical or oversized WebSocket length"));
        }
        if opcode >= 8 && (!fin || length > 125) {
            return Err(protocol("Fragmented or oversized control frame"));
        }
        if bytes.len() < header + length as usize {
            return Ok(false);
        }
        let mut payload = bytes[header..header + length as usize].to_vec();
        if self.expect_mask {
            let mask = &bytes[length_header..header];
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= mask[i % 4];
            }
        }
        self.offset += header + length as usize;
        match opcode {
            8 => {
                validate_close(&payload)?;
                self.closed = true;
                self.emit(FrameEvent::Close(payload))?;
                return Ok(false);
            }
            9 => self.emit(FrameEvent::Ping(payload))?,
            10 => self.emit(FrameEvent::Pong(payload))?,
            0 => {
                let (kind, fragment) = self
                    .fragment
                    .as_mut()
                    .ok_or_else(|| protocol("Continuation without a message"))?;
                if fragment.len() > MAX_FRAME_BYTES - payload.len() {
                    return Err(protocol("Fragmented message exceeds its limit"));
                }
                fragment.extend_from_slice(&payload);
                if fin {
                    let (kind, message) = self.fragment.take().unwrap();
                    validate_message(kind, &message)?;
                    self.emit(FrameEvent::Message(message))?;
                } else {
                    let _ = kind;
                }
            }
            1 | 2 => {
                if self.fragment.is_some() {
                    return Err(protocol("New message before continuation completes"));
                }
                if fin {
                    validate_message(opcode, &payload)?;
                    self.emit(FrameEvent::Message(payload))?;
                } else {
                    self.fragment = Some((opcode, payload));
                }
            }
            _ => unreachable!(),
        }
        Ok(self.offset < self.bytes.len())
    }
}
fn validate_message(kind: u8, bytes: &[u8]) -> Result<(), Error> {
    if kind == 1 && std::str::from_utf8(bytes).is_err() {
        return Err(protocol("Invalid UTF-8 text message"));
    }
    Ok(())
}
pub fn validate_close(bytes: &[u8]) -> Result<(), Error> {
    if bytes.len() == 1 || bytes.len() > 125 {
        return Err(protocol("Invalid Close length"));
    }
    if bytes.len() >= 2 {
        let code = u16::from_be_bytes([bytes[0], bytes[1]]);
        if !([
            1000, 1001, 1002, 1003, 1007, 1008, 1009, 1010, 1011, 1012, 1013, 1014,
        ]
        .contains(&code)
            || (3000..=4999).contains(&code))
            || std::str::from_utf8(&bytes[2..]).is_err()
        {
            return Err(protocol("Invalid Close status/reason"));
        }
    }
    Ok(())
}
pub fn client_frame(opcode: u8, payload: &[u8]) -> Result<Vec<u8>, Error> {
    if ![1, 2, 8, 9, 10].contains(&opcode)
        || payload.len() > MAX_FRAME_BYTES
        || (opcode >= 8 && payload.len() > 125)
    {
        return Err(protocol("Invalid outgoing frame"));
    }
    if opcode == 1 {
        validate_message(opcode, payload)?;
    }
    if opcode == 8 {
        validate_close(payload)?;
    }
    let mut mask = [0u8; 4];
    getrandom::getrandom(&mut mask)
        .map_err(|_| protocol("Operating-system randomness is unavailable"))?;
    let mut out = Vec::with_capacity(payload.len() + 14);
    out.push(128 | opcode);
    match payload.len() {
        0..=125 => out.push(128 | payload.len() as u8),
        126..=65535 => {
            out.push(128 | 126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        _ => {
            out.push(128 | 127);
            out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(&mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    Ok(out)
}
pub fn upgrade_accept(key: &str) -> String {
    let mut input = key.as_bytes().to_vec();
    input.extend_from_slice(b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11");
    base64::engine::general_purpose::STANDARD.encode(sha1_public_nonce(&input))
}
fn sha1_public_nonce(bytes: &[u8]) -> [u8; 20] {
    let mut data = bytes.to_vec();
    let bitlen = (data.len() as u64) * 8;
    data.push(128);
    while data.len() % 64 != 56 {
        data.push(0);
    }
    data.extend_from_slice(&bitlen.to_be_bytes());
    let mut state = [
        0x67452301u32,
        0xEFCDAB89,
        0x98BADCFE,
        0x10325476,
        0xC3D2E1F0,
    ];
    for block in data.chunks_exact(64) {
        let mut words = [0u32; 80];
        for (i, part) in block.chunks_exact(4).enumerate() {
            words[i] = u32::from_be_bytes(part.try_into().unwrap());
        }
        for i in 16..80 {
            words[i] = (words[i - 3] ^ words[i - 8] ^ words[i - 14] ^ words[i - 16]).rotate_left(1);
        }
        let [mut a, mut b, mut c, mut d, mut e] = state;
        for (i, w) in words.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999u32),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*w);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }
        for (i, v) in [a, b, c, d, e].iter().enumerate() {
            state[i] = state[i].wrapping_add(*v);
        }
    }
    let mut out = [0u8; 20];
    for (i, v) in state.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
    }
    out
}
fn protocol(message: &str) -> Error {
    Error::Protocol(message.into())
}

impl Iterator for Decoder {
    type Item = FrameEvent;
    fn next(&mut self) -> Option<Self::Item> {
        self.events.pop_front()
    }
}

/// Unmasked server frame; extensions/compression are never enabled.
pub fn server_frame(opcode: u8, payload: &[u8]) -> Result<Vec<u8>, Error> {
    if ![1, 2, 8, 9, 10].contains(&opcode)
        || payload.len() > MAX_FRAME_BYTES
        || (opcode >= 8 && payload.len() > 125)
    {
        return Err(protocol("Invalid outbound server frame"));
    }
    if opcode == 8 {
        validate_close(payload)?;
    }
    if opcode == 1 {
        validate_message(opcode, payload)?;
    }
    let mut out = Vec::with_capacity(payload.len() + 10);
    out.push(128 | opcode);
    match payload.len() {
        0..=125 => out.push(payload.len() as u8),
        126..=65535 => {
            out.push(126);
            out.extend_from_slice(&(payload.len() as u16).to_be_bytes());
        }
        _ => {
            out.push(127);
            out.extend_from_slice(&(payload.len() as u64).to_be_bytes());
        }
    }
    out.extend_from_slice(payload);
    Ok(out)
}
