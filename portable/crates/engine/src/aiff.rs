//! Bounded streamed AIFF/AIFF-C PCM and IEEE float decoding, and stereo export.
//! File workers only; no device, AudioUnit or platform codec calls.
use crate::{Error, Result};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
};
const BLOCK: usize = 8192;
const AIFC_VERSION: u32 = 0xa2805140;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiffEncoding {
    SignedBigEndian,
    SignedLittleEndian,
    FloatBigEndian,
}
#[derive(Clone, Debug)]
pub struct AiffInfo {
    pub channels: u16,
    pub sample_rate: f64,
    pub frames: u64,
    pub bits: u16,
    pub block_align: u16,
    pub data_offset: u64,
    pub encoding: AiffEncoding,
    pub compressed_container: bool,
}
pub struct AiffReader {
    file: File,
    pub info: AiffInfo,
    bytes: Vec<u8>,
}
fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes(b[at..at + 2].try_into().unwrap())
}
fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(b[at..at + 4].try_into().unwrap())
}
/// AIFF's explicit-integer-bit IEEE80 rate; no host-endian extended float casts.
fn extended_rate(bytes: &[u8]) -> Result<f64> {
    let sign = be16(bytes, 0);
    let exponent = sign & 0x7fff;
    let mantissa = u64::from_be_bytes(bytes[2..10].try_into().unwrap());
    if sign & 0x8000 != 0 || exponent == 0 || exponent == 0x7fff || mantissa & (1 << 63) == 0 {
        return Err(Error::Invalid("Invalid AIFF extended sample rate".into()));
    }
    let rate = mantissa as f64 * 2f64.powi(i32::from(exponent) - 16383 - 63);
    if !rate.is_finite() || !(8000. ..=384000.).contains(&rate) {
        return Err(Error::Unsupported(
            "AIFF sample rate exceeds 8–384 kHz admission".into(),
        ));
    }
    Ok(rate)
}
fn encode_rate(rate: u32) -> [u8; 10] {
    let exponent = 31 - rate.leading_zeros();
    let mantissa = u64::from(rate) << (63 - exponent);
    let mut bytes = [0; 10];
    bytes[..2].copy_from_slice(&(16383 + exponent as u16).to_be_bytes());
    bytes[2..].copy_from_slice(&mantissa.to_be_bytes());
    bytes
}
impl AiffReader {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_file(File::open(path)?)
    }
    /// Consume an already admitted source handle without reopening its path.
    pub fn from_file(mut file: File) -> Result<Self> {
        file.seek(SeekFrom::Start(0))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(Error::Invalid(
                "AIFF source requires a regular file handle".into(),
            ));
        }
        let length = metadata.len();
        let mut header = [0; 12];
        file.read_exact(&mut header)?;
        let compressed = match &header[8..] {
            b"AIFF" => false,
            b"AIFC" => true,
            _ => return Err(Error::Unsupported("Expected AIFF/AIFF-C FORM audio".into())),
        };
        if &header[..4] != b"FORM" {
            return Err(Error::Unsupported("Expected AIFF FORM audio".into()));
        }
        let declared = u64::from(be32(&header, 4)) + 8;
        if declared > length || declared < 12 {
            return Err(Error::Invalid("Truncated AIFF FORM".into()));
        }
        let mut cursor = 12u64;
        let mut chunks = 0;
        let mut common = None;
        let mut sound = None;
        let mut version = false;
        while cursor < declared {
            chunks += 1;
            if chunks > 4096 || declared - cursor < 8 {
                return Err(Error::Invalid(
                    "AIFF chunk count or trailing header exceeds bound".into(),
                ));
            }
            file.seek(SeekFrom::Start(cursor))?;
            let mut h = [0; 8];
            file.read_exact(&mut h)?;
            let size = u64::from(be32(&h, 4));
            let start = cursor + 8;
            let next = start
                .checked_add(size)
                .and_then(|v| v.checked_add(size % 2))
                .filter(|&v| v <= declared)
                .ok_or_else(|| Error::Invalid("Truncated AIFF chunk/padding".into()))?;
            match &h[..4] {
                b"FVER" => {
                    if !compressed || version || size != 4 {
                        return Err(Error::Invalid(
                            "Unexpected or duplicate AIFF-C version chunk".into(),
                        ));
                    }
                    let mut v = [0; 4];
                    file.read_exact(&mut v)?;
                    if be32(&v, 0) != AIFC_VERSION {
                        return Err(Error::Unsupported(
                            "Unsupported AIFF-C format version".into(),
                        ));
                    }
                    version = true;
                }
                b"COMM" => {
                    if common.is_some() || !(18..=4096).contains(&size) {
                        return Err(Error::Invalid(
                            "Invalid or duplicate AIFF common chunk".into(),
                        ));
                    }
                    let mut b = vec![0; size as usize];
                    file.read_exact(&mut b)?;
                    let channels = be16(&b, 0);
                    let frames = u64::from(be32(&b, 2));
                    let bits = be16(&b, 6);
                    let sample_rate = extended_rate(&b[8..18])?;
                    if !(1..=64).contains(&channels) {
                        return Err(Error::Unsupported(
                            "AIFF channels exceed 1–64 admission".into(),
                        ));
                    }
                    let encoding = if compressed {
                        if size < 23 {
                            return Err(Error::Invalid(
                                "Missing AIFF-C compression type/name".into(),
                            ));
                        }
                        let name = b[22] as usize;
                        let required = 23 + name;
                        let padded = required + required % 2;
                        if b.len() != padded {
                            return Err(Error::Invalid(
                                "Invalid AIFF-C Pascal compression name/padding".into(),
                            ));
                        }
                        match &b[18..22] {
                            b"NONE" | b"twos" => AiffEncoding::SignedBigEndian,
                            b"sowt" => AiffEncoding::SignedLittleEndian,
                            b"fl32" | b"FL32" if bits == 32 => AiffEncoding::FloatBigEndian,
                            b"fl64" | b"FL64" if bits == 64 => AiffEncoding::FloatBigEndian,
                            _ => {
                                return Err(Error::Unsupported(
                                    "AIFF-C compression is not supported PCM/IEEE float".into(),
                                ))
                            }
                        }
                    } else {
                        if size != 18 {
                            return Err(Error::Invalid(
                                "AIFF common chunk must have 18 bytes".into(),
                            ));
                        }
                        AiffEncoding::SignedBigEndian
                    };
                    if encoding != AiffEncoding::FloatBigEndian && !(1..=32).contains(&bits) {
                        return Err(Error::Unsupported(
                            "AIFF integer sample depth exceeds 1–32 bits".into(),
                        ));
                    }
                    let block_align = channels * bits.div_ceil(8);
                    common = Some(AiffInfo {
                        channels,
                        sample_rate,
                        frames,
                        bits,
                        block_align,
                        data_offset: 0,
                        encoding,
                        compressed_container: compressed,
                    });
                }
                b"SSND" => {
                    if sound.is_some() || size < 8 {
                        return Err(Error::Invalid(
                            "Invalid or duplicate AIFF sound chunk".into(),
                        ));
                    }
                    let mut b = [0; 8];
                    file.read_exact(&mut b)?;
                    let offset = u64::from(be32(&b, 0));
                    if offset > size - 8 {
                        return Err(Error::Invalid("AIFF sound offset exceeds chunk".into()));
                    }
                    sound = Some((start + 8 + offset, size - 8 - offset));
                }
                _ => {}
            }
            cursor = next;
        }
        let mut info = common.ok_or_else(|| Error::Invalid("Missing AIFF common chunk".into()))?;
        if compressed && !version {
            return Err(Error::Invalid("Missing AIFF-C format version".into()));
        }
        match sound {
            Some((offset, size)) => {
                if info.frames * info.block_align as u64 > size {
                    return Err(Error::Invalid(
                        "AIFF sample frames exceed sound payload".into(),
                    ));
                }
                info.data_offset = offset;
            }
            None if info.frames == 0 => info.data_offset = declared,
            None => return Err(Error::Invalid("Missing AIFF sound data".into())),
        }
        Ok(Self {
            file,
            bytes: vec![0; BLOCK * info.block_align as usize],
            info,
        })
    }
    fn read_bytes(&mut self, start: u64, frames: usize) -> Result<()> {
        if frames > BLOCK || start > self.info.frames || frames as u64 > self.info.frames - start {
            return Err(Error::Invalid(
                "AIFF read outside bounded source range".into(),
            ));
        }
        self.file.seek(SeekFrom::Start(
            self.info.data_offset + start * self.info.block_align as u64,
        ))?;
        self.file
            .read_exact(&mut self.bytes[..frames * self.info.block_align as usize])?;
        Ok(())
    }
    pub fn read_stereo(&mut self, start: u64, left: &mut [f32], right: &mut [f32]) -> Result<()> {
        if left.len() != right.len() {
            return Err(Error::Invalid("AIFF stereo buffers differ".into()));
        }
        self.read_bytes(start, left.len())?;
        let size = self.info.bits.div_ceil(8) as usize;
        for i in 0..left.len() {
            let at = i * self.info.block_align as usize;
            left[i] = decode(
                &self.bytes[at..at + size],
                self.info.encoding,
                self.info.bits,
            );
            right[i] = if self.info.channels == 1 {
                left[i]
            } else {
                decode(
                    &self.bytes[at + size..at + 2 * size],
                    self.info.encoding,
                    self.info.bits,
                )
            };
        }
        Ok(())
    }
    pub fn read_planar(&mut self, start: u64, channels: &mut [&mut [f32]]) -> Result<()> {
        let frames = channels.first().map_or(0, |v| v.len());
        if channels.len() != self.info.channels as usize
            || channels.iter().any(|v| v.len() != frames)
        {
            return Err(Error::Invalid(
                "AIFF planar buffers differ from source".into(),
            ));
        }
        self.read_bytes(start, frames)?;
        let size = self.info.bits.div_ceil(8) as usize;
        for (channel, target) in channels.iter_mut().enumerate() {
            for (frame, sample) in target.iter_mut().enumerate() {
                let at = frame * self.info.block_align as usize + channel * size;
                *sample = decode(
                    &self.bytes[at..at + size],
                    self.info.encoding,
                    self.info.bits,
                );
            }
        }
        Ok(())
    }
}
fn decode(bytes: &[u8], encoding: AiffEncoding, bits: u16) -> f32 {
    let sample = if encoding == AiffEncoding::FloatBigEndian {
        match bytes.len() {
            4 => f32::from_be_bytes(bytes.try_into().unwrap()),
            8 => f64::from_be_bytes(bytes.try_into().unwrap()) as f32,
            _ => unreachable!(),
        }
    } else {
        let mut integer = 0i32;
        if encoding == AiffEncoding::SignedLittleEndian {
            for &byte in bytes.iter().rev() {
                integer = (integer << 8) | i32::from(byte);
            }
        } else {
            for &byte in bytes {
                integer = (integer << 8) | i32::from(byte);
            }
        }
        let padding = bytes.len() * 8 - bits as usize;
        integer &= !((1i32 << padding) - 1);
        let shift = 32 - bytes.len() * 8;
        (integer << shift) as f32 / 2147483648.
    };
    if sample.is_finite() {
        sample
    } else {
        0.
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AiffExport {
    Pcm16,
    Pcm24,
    Float32,
}
/// Known-length streamed stereo export; integer modes round/clamp to format
/// bounds and float mode preserves finite headroom. Dither is an explicit DSP
/// choice; the container writer never applies a hidden second dither stage.
pub struct AiffWriter {
    file: File,
    frames: u64,
    expected: u64,
    format: AiffExport,
}
impl AiffWriter {
    pub fn create(path: &Path, frames: u64, sample_rate: u32, format: AiffExport) -> Result<Self> {
        Self::layout(frames, sample_rate, format)?;
        Self::from_file(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?,
            frames,
            sample_rate,
            format,
        )
    }
    fn layout(frames: u64, sample_rate: u32, format: AiffExport) -> Result<(u16, u64, u64)> {
        if !(8000..=384000).contains(&sample_rate) || frames > u32::MAX as u64 {
            return Err(Error::Invalid(
                "AIFF export rate/frame bound exceeded".into(),
            ));
        }
        let (bits, header_size) = match format {
            AiffExport::Pcm16 => (16u16, 54u64),
            AiffExport::Pcm24 => (24, 54),
            AiffExport::Float32 => (32, 92),
        };
        let data = frames * 2 * u64::from(bits / 8);
        if data + header_size - 8 > u32::MAX as u64 {
            return Err(Error::Unsupported("AIFF FORM exceeds 4 GiB".into()));
        }
        Ok((bits, header_size, data))
    }
    /// New empty read/write handle from a private artifact provider.
    pub fn from_file(
        mut file: File,
        frames: u64,
        sample_rate: u32,
        format: AiffExport,
    ) -> Result<Self> {
        let (bits, header_size, data) = Self::layout(frames, sample_rate, format)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != 0 {
            return Err(Error::Conflict(
                "AIFF writer requires a new empty artifact handle".into(),
            ));
        }
        file.seek(SeekFrom::Start(0))?;
        file.write_all(b"FORM")?;
        file.write_all(&((header_size + data - 8) as u32).to_be_bytes())?;
        if format == AiffExport::Float32 {
            file.write_all(b"AIFCFVER")?;
            file.write_all(&4u32.to_be_bytes())?;
            file.write_all(&AIFC_VERSION.to_be_bytes())?;
        } else {
            file.write_all(b"AIFF")?;
        }
        file.write_all(b"COMM")?;
        file.write_all(
            &(if format == AiffExport::Float32 {
                44u32
            } else {
                18
            })
            .to_be_bytes(),
        )?;
        file.write_all(&2u16.to_be_bytes())?;
        file.write_all(&(frames as u32).to_be_bytes())?;
        file.write_all(&bits.to_be_bytes())?;
        file.write_all(&encode_rate(sample_rate))?;
        if format == AiffExport::Float32 {
            file.write_all(b"fl32\x1532-bit floating point")?;
        }
        file.write_all(b"SSND")?;
        file.write_all(&((data + 8) as u32).to_be_bytes())?;
        file.write_all(&[0; 8])?;
        Ok(Self {
            file,
            frames: 0,
            expected: frames,
            format,
        })
    }
    pub fn write(&mut self, left: &[f32], right: &[f32]) -> Result<()> {
        if left.len() != right.len()
            || left.len() > BLOCK
            || left.len() as u64 > self.expected - self.frames
        {
            return Err(Error::Invalid("Invalid AIFF export block".into()));
        }
        let mut bytes = [0u8; BLOCK * 8];
        let size = match self.format {
            AiffExport::Pcm16 => 2,
            AiffExport::Pcm24 => 3,
            AiffExport::Float32 => 4,
        };
        for (frame, (&l, &r)) in left.iter().zip(right).enumerate() {
            for (channel, sample) in [l, r].into_iter().enumerate() {
                let sample = if sample.is_finite() { sample } else { 0. };
                let at = (frame * 2 + channel) * size;
                match self.format {
                    AiffExport::Float32 => bytes[at..at + 4].copy_from_slice(&sample.to_be_bytes()),
                    AiffExport::Pcm16 => {
                        let value = (sample as f64 * 32768.).round().clamp(-32768., 32767.) as i16;
                        bytes[at..at + 2].copy_from_slice(&value.to_be_bytes());
                    }
                    AiffExport::Pcm24 => {
                        let value = (sample as f64 * 8388608.)
                            .round()
                            .clamp(-8388608., 8388607.) as i32;
                        bytes[at..at + 3].copy_from_slice(&value.to_be_bytes()[1..]);
                    }
                }
            }
        }
        self.file.write_all(&bytes[..left.len() * 2 * size])?;
        self.frames += left.len() as u64;
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        if self.frames != self.expected {
            return Err(Error::Invalid("Incomplete AIFF export".into()));
        }
        self.file.sync_all()?;
        Ok(())
    }
}
