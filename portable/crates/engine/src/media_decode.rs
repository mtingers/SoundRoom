//! macOS import decoding: any format AudioToolbox reads (WAV/AIFF at any rate,
//! MP3, AAC/M4A, CAF, FLAC, ALAC) is converted by ExtAudioFile to 48 kHz
//! interleaved float stereo and streamed into a new WAV. Bounded memory; a
//! worker-only operation that never runs on an audio callback.
use crate::{wave::WaveWriter, Error, Result};
use std::{
    ffi::c_void,
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};
type ExtAudioFile = *mut c_void;
type CfUrl = *const c_void;
#[repr(C)]
struct StreamDescription {
    rate: f64,
    format: u32,
    flags: u32,
    bytes_per_packet: u32,
    frames_per_packet: u32,
    bytes_per_frame: u32,
    channels: u32,
    bits: u32,
    reserved: u32,
}
#[repr(C)]
struct AudioBuffer {
    channels: u32,
    bytes: u32,
    data: *mut c_void,
}
#[repr(C)]
struct BufferList {
    count: u32,
    first: AudioBuffer,
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: *const c_void,
        buffer: *const u8,
        length: isize,
        directory: u8,
    ) -> CfUrl;
    fn CFRelease(value: *const c_void);
}
#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn ExtAudioFileOpenURL(url: CfUrl, file: *mut ExtAudioFile) -> i32;
    fn ExtAudioFileGetProperty(
        file: ExtAudioFile,
        id: u32,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn ExtAudioFileSetProperty(file: ExtAudioFile, id: u32, size: u32, data: *const c_void) -> i32;
    fn ExtAudioFileRead(file: ExtAudioFile, frames: *mut u32, data: *mut BufferList) -> i32;
    fn ExtAudioFileDispose(file: ExtAudioFile) -> i32;
}
const FILE_FORMAT: u32 = u32::from_be_bytes(*b"ffmt");
const CLIENT_FORMAT: u32 = u32::from_be_bytes(*b"cfmt");
const BLOCK: usize = 4096;
struct Handle(ExtAudioFile);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            ExtAudioFileDispose(self.0);
        }
    }
}
fn checked(status: i32, what: &str) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error::Unsupported(format!(
            "This audio file could not be decoded ({what}, {status})"
        )))
    }
}
/// Source channel count and sample rate as AudioToolbox reports them.
pub struct DecodedSource {
    pub source_channels: u32,
    pub source_rate: f64,
    pub frames: u64,
}
/// Decodes `input` into a new WAV at `output` (which must not exist).
pub fn decode_to_wav(input: &Path, output: &Path, cancel: &AtomicBool) -> Result<DecodedSource> {
    let bytes = input.as_os_str().as_bytes();
    let url = unsafe {
        CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            0,
        )
    };
    if url.is_null() {
        return Err(Error::Invalid("Invalid import path".into()));
    }
    let mut file = std::ptr::null_mut();
    let opened = unsafe { ExtAudioFileOpenURL(url, &mut file) };
    unsafe { CFRelease(url) };
    checked(opened, "open")?;
    let handle = Handle(file);
    let mut source = StreamDescription {
        rate: 0.,
        format: 0,
        flags: 0,
        bytes_per_packet: 0,
        frames_per_packet: 0,
        bytes_per_frame: 0,
        channels: 0,
        bits: 0,
        reserved: 0,
    };
    let mut size = std::mem::size_of::<StreamDescription>() as u32;
    checked(
        unsafe {
            ExtAudioFileGetProperty(
                handle.0,
                FILE_FORMAT,
                &mut size,
                (&mut source as *mut StreamDescription).cast(),
            )
        },
        "format",
    )?;
    if source.channels == 0 || !(source.rate.is_finite() && source.rate > 0.) {
        return Err(Error::Unsupported(
            "The audio file reports no channels or rate".into(),
        ));
    }
    let client = StreamDescription {
        rate: 48000.,
        format: u32::from_be_bytes(*b"lpcm"),
        flags: 1 | 8, // float, packed (interleaved)
        bytes_per_packet: 8,
        frames_per_packet: 1,
        bytes_per_frame: 8,
        channels: 2,
        bits: 32,
        reserved: 0,
    };
    checked(
        unsafe {
            ExtAudioFileSetProperty(
                handle.0,
                CLIENT_FORMAT,
                std::mem::size_of::<StreamDescription>() as u32,
                (&client as *const StreamDescription).cast(),
            )
        },
        "convert",
    )?;
    let mut writer = WaveWriter::stream(output)?;
    let result = (|| {
        let mut interleaved = vec![0f32; BLOCK * 2];
        let (mut left, mut right) = (vec![0f32; BLOCK], vec![0f32; BLOCK]);
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err(Error::Invalid("Import cancelled".into()));
            }
            let mut list = BufferList {
                count: 1,
                first: AudioBuffer {
                    channels: 2,
                    bytes: (BLOCK * 8) as u32,
                    data: interleaved.as_mut_ptr().cast(),
                },
            };
            let mut frames = BLOCK as u32;
            checked(
                unsafe { ExtAudioFileRead(handle.0, &mut frames, &mut list) },
                "read",
            )?;
            let frames = frames as usize;
            if frames == 0 {
                break;
            }
            for i in 0..frames {
                left[i] = interleaved[i * 2];
                right[i] = interleaved[i * 2 + 1];
            }
            writer.write(&left[..frames], &right[..frames])?;
        }
        Ok(writer.frames())
    })();
    match result {
        Ok(frames) if frames > 0 => {
            writer.finish()?;
            Ok(DecodedSource {
                source_channels: source.channels,
                source_rate: source.rate,
                frames,
            })
        }
        Ok(_) => {
            drop(writer);
            let _ = std::fs::remove_file(output);
            Err(Error::Unsupported(
                "The audio file contains no audio".into(),
            ))
        }
        Err(error) => {
            drop(writer);
            let _ = std::fs::remove_file(output);
            Err(error)
        }
    }
}
