//! Shared bounded file decoder/export boundary. Format detection uses headers,
//! not extensions; callers own storage custody, source import and authority.
use crate::{
    aiff::{AiffEncoding, AiffExport, AiffReader, AiffWriter},
    wave::{WaveReader, WaveWriter},
    Error, Result,
};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::Path,
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioContainer {
    Wave,
    Aiff,
    AiffC,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioEncoding {
    Integer,
    Float,
}
#[derive(Clone, Debug)]
pub struct AudioInfo {
    pub container: AudioContainer,
    pub channels: u16,
    pub sample_rate: f64,
    pub frames: u64,
    pub bits: u16,
    pub encoding: AudioEncoding,
    pub block_align: u16,
}
enum Decoder {
    Wave(WaveReader),
    Aiff(AiffReader),
}
pub struct AudioReader {
    decoder: Decoder,
    pub info: AudioInfo,
}
impl AudioReader {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_file(File::open(path)?)
    }
    pub fn from_file(mut file: File) -> Result<Self> {
        if !file.metadata()?.is_file() {
            return Err(Error::Invalid(
                "Audio source requires a regular file handle".into(),
            ));
        }
        file.seek(SeekFrom::Start(0))?;
        let mut header = [0; 12];
        file.read_exact(&mut header)?;
        file.seek(SeekFrom::Start(0))?;
        let (decoder, info) = match (&header[..4], &header[8..]) {
            (b"RIFF", b"WAVE") => {
                let reader = WaveReader::from_file(file)?;
                let info = AudioInfo {
                    container: AudioContainer::Wave,
                    channels: reader.info.channels,
                    sample_rate: reader.info.sample_rate as f64,
                    frames: reader.info.frames,
                    bits: reader.info.bits,
                    encoding: if reader.info.float {
                        AudioEncoding::Float
                    } else {
                        AudioEncoding::Integer
                    },
                    block_align: reader.info.block_align,
                };
                (Decoder::Wave(reader), info)
            }
            (b"FORM", b"AIFF" | b"AIFC") => {
                let reader = AiffReader::from_file(file)?;
                let info = AudioInfo {
                    container: if reader.info.compressed_container {
                        AudioContainer::AiffC
                    } else {
                        AudioContainer::Aiff
                    },
                    channels: reader.info.channels,
                    sample_rate: reader.info.sample_rate,
                    frames: reader.info.frames,
                    bits: reader.info.bits,
                    encoding: if reader.info.encoding == AiffEncoding::FloatBigEndian {
                        AudioEncoding::Float
                    } else {
                        AudioEncoding::Integer
                    },
                    block_align: reader.info.block_align,
                };
                (Decoder::Aiff(reader), info)
            }
            _ => {
                return Err(Error::Unsupported(
                    "Audio container is not supported WAV/AIFF/AIFF-C".into(),
                ))
            }
        };
        Ok(Self { decoder, info })
    }
    pub fn scratch_bytes(&self) -> usize {
        8192 * self.info.block_align as usize
    }
    pub fn read_stereo(&mut self, start: u64, left: &mut [f32], right: &mut [f32]) -> Result<()> {
        match &mut self.decoder {
            Decoder::Wave(reader) => reader.read_stereo(start, left, right),
            Decoder::Aiff(reader) => reader.read_stereo(start, left, right),
        }
    }
    pub fn read_planar(&mut self, start: u64, channels: &mut [&mut [f32]]) -> Result<()> {
        match &mut self.decoder {
            Decoder::Wave(reader) => reader.read_planar(start, channels),
            Decoder::Aiff(reader) => reader.read_planar(start, channels),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioExport {
    WaveFloat32,
    AiffPcm16,
    AiffPcm24,
    AiffFloat32,
}
pub enum AudioWriter {
    Wave(WaveWriter),
    Aiff(AiffWriter),
}
impl AudioWriter {
    pub fn from_file(
        file: File,
        frames: u64,
        sample_rate: u32,
        format: AudioExport,
    ) -> Result<Self> {
        match format {
            AudioExport::WaveFloat32 => {
                if sample_rate != 48000 {
                    return Err(Error::Unsupported(
                        "Current WAV writer requires 48 kHz".into(),
                    ));
                }
                Ok(Self::Wave(WaveWriter::from_file(file, frames)?))
            }
            _ => Ok(Self::Aiff(AiffWriter::from_file(
                file,
                frames,
                sample_rate,
                match format {
                    AudioExport::AiffPcm16 => AiffExport::Pcm16,
                    AudioExport::AiffPcm24 => AiffExport::Pcm24,
                    AudioExport::AiffFloat32 => AiffExport::Float32,
                    _ => unreachable!(),
                },
            )?)),
        }
    }
    pub fn create(path: &Path, frames: u64, sample_rate: u32, format: AudioExport) -> Result<Self> {
        match format {
            AudioExport::WaveFloat32 => {
                if sample_rate != 48000 {
                    return Err(Error::Unsupported(
                        "Current WAV session writer requires 48 kHz; resample before export".into(),
                    ));
                }
                Ok(Self::Wave(WaveWriter::create(path, frames)?))
            }
            _ => {
                let encoding = match format {
                    AudioExport::AiffPcm16 => AiffExport::Pcm16,
                    AudioExport::AiffPcm24 => AiffExport::Pcm24,
                    AudioExport::AiffFloat32 => AiffExport::Float32,
                    _ => unreachable!(),
                };
                Ok(Self::Aiff(AiffWriter::create(
                    path,
                    frames,
                    sample_rate,
                    encoding,
                )?))
            }
        }
    }
    pub fn write(&mut self, left: &[f32], right: &[f32]) -> Result<()> {
        match self {
            Self::Wave(writer) => writer.write(left, right),
            Self::Aiff(writer) => writer.write(left, right),
        }
    }
    pub fn finish(self) -> Result<()> {
        match self {
            Self::Wave(writer) => writer.finish(),
            Self::Aiff(writer) => writer.finish(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImportedAudio {
    pub source: AudioInfo,
    pub frames: u64,
}
struct PartialImport<'a> {
    path: &'a Path,
    finished: bool,
}
impl Drop for PartialImport<'_> {
    fn drop(&mut self) {
        if !self.finished {
            let _ = std::fs::remove_file(self.path);
        }
    }
}
/// Stream a selected native mono/stereo route into a new 48kHz project WAV.
/// The source remains unchanged; caller registers the artifact in authority.
/// No device/wall clock is involved: these are file sample coordinates only.
pub fn import_audio(
    source: &Path,
    target: &Path,
    route: &[usize],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<ImportedAudio> {
    check_cancel(cancel)?;
    let reader = AudioReader::open(source)?;
    let mut partial = PartialImport {
        path: target,
        finished: true,
    };
    let result = import_reader(
        reader,
        || {
            let writer = WaveWriter::stream(target)?;
            partial.finished = false;
            Ok(writer)
        },
        route,
        cancel,
    )?;
    partial.finished = true;
    Ok(result)
}
/// Source and new destination handles from custody. On error the caller retains
/// or removes its own unpublished artifact; no source/path is reopened/deleted.
pub fn import_audio_from_files(
    source: File,
    target: File,
    route: &[usize],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<ImportedAudio> {
    check_cancel(cancel)?;
    import_reader(
        AudioReader::from_file(source)?,
        || WaveWriter::stream_file(target),
        route,
        cancel,
    )
}
fn check_cancel(cancel: &std::sync::atomic::AtomicBool) -> Result<()> {
    if cancel.load(std::sync::atomic::Ordering::Relaxed) {
        Err(Error::Conflict(
            "Audio import cancelled; source retained".into(),
        ))
    } else {
        Ok(())
    }
}
fn import_reader(
    mut reader: AudioReader,
    create: impl FnOnce() -> Result<WaveWriter>,
    route: &[usize],
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<ImportedAudio> {
    use crate::capture_resample::{CaptureResampler, ResampleConfig};
    let check = || check_cancel(cancel);
    check()?;
    if ![1, 2].contains(&route.len()) || route.iter().any(|&i| i >= reader.info.channels as usize) {
        return Err(Error::Invalid(
            "Audio import route must select valid mono/stereo channels".into(),
        ));
    }
    let info = reader.info.clone();
    let selected: Vec<usize> = route
        .iter()
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let left_index = selected.iter().position(|&v| v == route[0]).unwrap();
    let right_index = selected
        .iter()
        .position(|&v| v == *route.get(1).unwrap_or(&route[0]))
        .unwrap();
    let mut converter = if info.sample_rate == 48000. {
        None
    } else {
        Some(CaptureResampler::new_fractional(
            ResampleConfig {
                channels: selected.len(),
                input_rate: info.sample_rate.round() as u32,
                output_rate: 48000,
                maximum_drift_ppm: 2000.,
            },
            info.sample_rate,
            0,
            0.,
        )?)
    };
    let mut input = vec![vec![0.; 8192]; info.channels as usize];
    let mut output = vec![vec![0.; 8192]; selected.len()];
    let mut writer = create()?;
    for frame in (0..info.frames).step_by(8192) {
        check()?;
        let count = (info.frames - frame).min(8192) as usize;
        let mut refs: Vec<&mut [f32]> = input.iter_mut().map(|v| &mut v[..count]).collect();
        reader.read_planar(frame, &mut refs)?;
        if let Some(converter) = &mut converter {
            let channels: Vec<&[f32]> = selected.iter().map(|&i| &input[i][..count]).collect();
            converter.push(frame, &channels)?;
            loop {
                check()?;
                let mut refs: Vec<&mut [f32]> =
                    output.iter_mut().map(|v| v.as_mut_slice()).collect();
                let block = converter.drain(&mut refs)?;
                if block.frames == 0 {
                    break;
                }
                writer.write(
                    &output[left_index][..block.frames],
                    &output[right_index][..block.frames],
                )?;
            }
        } else {
            writer.write(
                &input[route[0]][..count],
                &input[*route.get(1).unwrap_or(&route[0])][..count],
            )?;
        }
    }
    if let Some(converter) = &mut converter {
        converter.finish();
        loop {
            check()?;
            let mut refs: Vec<&mut [f32]> = output.iter_mut().map(|v| v.as_mut_slice()).collect();
            let block = converter.drain(&mut refs)?;
            if block.frames == 0 {
                break;
            }
            writer.write(
                &output[left_index][..block.frames],
                &output[right_index][..block.frames],
            )?;
        }
    }
    let frames = writer.frames();
    check()?;
    writer.finish()?;
    Ok(ImportedAudio {
        source: info,
        frames,
    })
}
