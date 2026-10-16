//! Bounded seekable PCM decoder and streaming float WAV writer. No session-sized PCM allocation.
use crate::{Error, Result};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
const MAX_BLOCK: usize = 8192;
#[derive(Clone, Debug)]
pub struct WaveInfo {
    pub channels: u16,
    pub sample_rate: u32,
    pub frames: u64,
    pub bits: u16,
    pub float: bool,
    pub data_offset: u64,
    pub block_align: u16,
}
pub struct WaveReader {
    file: File,
    pub info: WaveInfo,
    bytes: Vec<u8>,
}
fn u16at(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn u32at(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes(b[i..i + 4].try_into().unwrap())
}
impl WaveReader {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_file(File::open(path)?)
    }
    /// Consume an already admitted worker handle; never reopen its source path.
    pub fn from_file(mut file: File) -> Result<Self> {
        file.seek(SeekFrom::Start(0))?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(Error::Invalid(
                "WAV source requires a regular file handle".into(),
            ));
        }
        let length = metadata.len();
        let mut header = [0; 12];
        file.read_exact(&mut header)?;
        if &header[..4] != b"RIFF" || &header[8..] != b"WAVE" {
            return Err(Error::Unsupported("Expected RIFF/WAVE audio".into()));
        }
        let declared = u32at(&header, 4) as u64 + 8;
        if declared > length || declared < 12 {
            return Err(Error::Invalid("Truncated RIFF file".into()));
        }
        let mut format = None;
        let mut data = None;
        let mut cursor = 12u64;
        let mut chunks = 0;
        while cursor + 8 <= declared {
            chunks += 1;
            if chunks > 4096 {
                return Err(Error::Invalid("Too many WAV chunks".into()));
            }
            file.seek(SeekFrom::Start(cursor))?;
            let mut h = [0; 8];
            file.read_exact(&mut h)?;
            let size = u32at(&h, 4) as u64;
            let start = cursor + 8;
            if size > declared - start {
                return Err(Error::Invalid("Truncated WAV chunk".into()));
            }
            if &h[..4] == b"fmt " {
                if !(16..=4096).contains(&size) {
                    return Err(Error::Invalid("Invalid WAV format chunk".into()));
                }
                let mut b = vec![0; size as usize];
                file.read_exact(&mut b)?;
                let mut code = u16at(&b, 0);
                if code == 0xfffe {
                    if size < 40
                        || u16at(&b, 16) < 22
                        || b[26..28] != [0, 0]
                        || b[28..40] != [0, 0, 16, 0, 128, 0, 0, 170, 0, 56, 155, 113]
                    {
                        return Err(Error::Unsupported(
                            "Unsupported extensible WAV format".into(),
                        ));
                    }
                    code = u16at(&b, 24)
                }
                let channels = u16at(&b, 2);
                let rate = u32at(&b, 4);
                let align = u16at(&b, 12);
                let bits = u16at(&b, 14);
                if !(1..=64).contains(&channels)
                    || !(8000..=384000).contains(&rate)
                    || !((code == 1 && [8, 16, 24, 32].contains(&bits))
                        || (code == 3 && [32, 64].contains(&bits)))
                    || align != channels * (bits / 8)
                    || u32at(&b, 8) != rate * align as u32
                {
                    return Err(Error::Unsupported(
                        "Unsupported or invalid WAV format".into(),
                    ));
                }
                format = Some((channels, rate, align, bits, code == 3));
            } else if &h[..4] == b"data" {
                if data.is_some() {
                    return Err(Error::Invalid("Multiple WAV data chunks".into()));
                }
                data = Some((start, size));
            }
            cursor = start + size + size % 2;
        }
        let (channels, sample_rate, block_align, bits, float) =
            format.ok_or_else(|| Error::Invalid("Missing WAV format".into()))?;
        let (data_offset, size) = data.ok_or_else(|| Error::Invalid("Missing WAV data".into()))?;
        if size % block_align as u64 != 0 {
            return Err(Error::Invalid("Partial WAV frame".into()));
        }
        let info = WaveInfo {
            channels,
            sample_rate,
            frames: size / block_align as u64,
            bits,
            float,
            data_offset,
            block_align,
        };
        Ok(Self {
            file,
            bytes: vec![0; MAX_BLOCK * block_align as usize],
            info,
        })
    }
    /// Worker-only all-channel read for import routing; mix readers use stereo.
    pub fn read_planar(&mut self, start: u64, channels: &mut [&mut [f32]]) -> Result<()> {
        let frames = channels.first().map_or(0, |v| v.len());
        if channels.len() != self.info.channels as usize
            || channels.iter().any(|v| v.len() != frames)
            || frames > MAX_BLOCK
            || start > self.info.frames
            || frames as u64 > self.info.frames - start
        {
            return Err(Error::Invalid(
                "WAV planar read outside source/channel bounds".into(),
            ));
        }
        let count = frames * self.info.block_align as usize;
        self.file.seek(SeekFrom::Start(
            self.info.data_offset + start * self.info.block_align as u64,
        ))?;
        self.file.read_exact(&mut self.bytes[..count])?;
        let size = self.info.bits as usize / 8;
        for (channel, target) in channels.iter_mut().enumerate() {
            for (frame, sample) in target.iter_mut().enumerate() {
                let at = frame * self.info.block_align as usize + channel * size;
                *sample = decode(&self.bytes[at..at + size], self.info.float);
            }
        }
        Ok(())
    }
    pub fn read_stereo(&mut self, start: u64, left: &mut [f32], right: &mut [f32]) -> Result<()> {
        if left.len() != right.len()
            || left.len() > MAX_BLOCK
            || start > self.info.frames
            || left.len() as u64 > self.info.frames - start
        {
            return Err(Error::Invalid(
                "WAV read outside bounded source range".into(),
            ));
        }
        let count = left.len() * self.info.block_align as usize;
        self.file.seek(SeekFrom::Start(
            self.info.data_offset + start * self.info.block_align as u64,
        ))?;
        self.file.read_exact(&mut self.bytes[..count])?;
        let bytes = self.info.bits as usize / 8;
        for i in 0..left.len() {
            let base = i * self.info.block_align as usize;
            left[i] = decode(&self.bytes[base..base + bytes], self.info.float);
            right[i] = if self.info.channels == 1 {
                left[i]
            } else {
                decode(&self.bytes[base + bytes..base + 2 * bytes], self.info.float)
            }
        }
        Ok(())
    }
}
fn decode(b: &[u8], float: bool) -> f32 {
    let x = if float {
        match b.len() {
            4 => f32::from_le_bytes(b.try_into().unwrap()),
            8 => f64::from_le_bytes(b.try_into().unwrap()) as f32,
            _ => unreachable!(),
        }
    } else {
        match b.len() {
            1 => (b[0] as f32 - 128.) / 128.,
            2 => i16::from_le_bytes(b.try_into().unwrap()) as f32 / 32768.,
            3 => {
                (((b[0] as i32) | ((b[1] as i32) << 8) | ((b[2] as i32) << 16)) << 8) as f32
                    / 2147483648.
            }
            4 => i32::from_le_bytes(b.try_into().unwrap()) as f32 / 2147483648.,
            _ => 0.,
        }
    };
    if x.is_finite() {
        x
    } else {
        0.
    }
}
pub struct WaveWriter {
    file: File,
    frames: u64,
    expected: u64,
    streaming: bool,
}
impl WaveWriter {
    pub fn create(path: &Path, frames: u64) -> Result<Self> {
        Self::output_bytes(frames)?;
        Self::from_file(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?,
            frames,
        )
    }
    fn output_bytes(frames: u64) -> Result<u64> {
        let bytes = frames
            .checked_mul(8)
            .filter(|n| *n <= u32::MAX as u64 - 36)
            .ok_or_else(|| {
                Error::Unsupported(
                    "RIFF output exceeds 4 GiB; RF64 output is not implemented yet".into(),
                )
            })?;
        Ok(bytes)
    }
    /// New, empty, exclusively owned read/write handle from a storage provider.
    pub fn from_file(mut file: File, frames: u64) -> Result<Self> {
        let bytes = Self::output_bytes(frames)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != 0 {
            return Err(Error::Conflict(
                "WAV writer requires a new empty artifact handle".into(),
            ));
        }
        file.seek(SeekFrom::Start(0))?;
        file.write_all(b"RIFF")?;
        file.write_all(&(36 + bytes as u32).to_le_bytes())?;
        file.write_all(b"WAVEfmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&3u16.to_le_bytes())?;
        file.write_all(&2u16.to_le_bytes())?;
        file.write_all(&48000u32.to_le_bytes())?;
        file.write_all(&384000u32.to_le_bytes())?;
        file.write_all(&8u16.to_le_bytes())?;
        file.write_all(&32u16.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&(bytes as u32).to_le_bytes())?;
        Ok(Self {
            file,
            frames: 0,
            expected: frames,
            streaming: false,
        })
    }
    /// Worker-only unknown-length artifact. Keep it in an unpublished staging
    /// directory until finish patches lengths and syncs the complete file.
    pub fn stream(path: &Path) -> Result<Self> {
        Self::stream_file(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?,
        )
    }
    pub fn stream_file(file: File) -> Result<Self> {
        let mut writer = Self::from_file(file, 0)?;
        writer.expected = (u32::MAX as u64 - 36) / 8;
        writer.streaming = true;
        Ok(writer)
    }
    pub fn frames(&self) -> u64 {
        self.frames
    }
    pub fn write(&mut self, left: &[f32], right: &[f32]) -> Result<()> {
        if left.len() != right.len()
            || left.len() > MAX_BLOCK
            || left.len() as u64 > self.expected - self.frames
        {
            return Err(Error::Invalid("Invalid output block".into()));
        }
        let mut block = [0u8; MAX_BLOCK * 8];
        for (i, (&l, &r)) in left.iter().zip(right).enumerate() {
            block[i * 8..i * 8 + 4]
                .copy_from_slice(&if l.is_finite() { l } else { 0. }.to_le_bytes());
            block[i * 8 + 4..i * 8 + 8]
                .copy_from_slice(&if r.is_finite() { r } else { 0. }.to_le_bytes())
        }
        self.file.write_all(&block[..left.len() * 8])?;
        self.frames += left.len() as u64;
        Ok(())
    }
    pub fn finish(mut self) -> Result<()> {
        if self.streaming {
            let bytes = self.frames * 8;
            self.file.seek(SeekFrom::Start(4))?;
            self.file.write_all(&(36 + bytes as u32).to_le_bytes())?;
            self.file.seek(SeekFrom::Start(40))?;
            self.file.write_all(&(bytes as u32).to_le_bytes())?;
        } else if self.frames != self.expected {
            return Err(Error::Invalid("Incomplete WAV output".into()));
        }
        self.file.sync_all()?;
        Ok(())
    }
}
