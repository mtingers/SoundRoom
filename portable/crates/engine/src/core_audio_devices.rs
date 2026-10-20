//! Worker-side Core Audio inventory. Calling these APIs enumerates hardware;
//! there is no automatic enumeration, device opening or default-route mutation.
//! The only device setting written is an explicitly requested input volume.
use crate::{Error, Result};
use std::ffi::c_void;

const GLOBAL: u32 = u32::from_be_bytes(*b"glob");
const INPUT: u32 = u32::from_be_bytes(*b"inpt");
const OUTPUT: u32 = u32::from_be_bytes(*b"outp");
const VOLUME: u32 = u32::from_be_bytes(*b"volm");
const MAX_PROPERTY: usize = 1024 * 1024;
#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}
#[link(name = "CoreAudio", kind = "framework")]
extern "C" {
    fn AudioObjectGetPropertyDataSize(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectHasProperty(object: u32, address: *const Address) -> u8;
    fn AudioObjectIsPropertySettable(
        object: u32,
        address: *const Address,
        settable: *mut u8,
    ) -> i32;
    fn AudioObjectSetPropertyData(
        object: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: u32,
        data: *const c_void,
    ) -> i32;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringGetLength(string: *const c_void) -> isize;
    fn CFStringGetCString(string: *const c_void, buffer: *mut u8, size: isize, encoding: u32)
        -> u8;
    fn CFRelease(value: *const c_void);
}
/// Object IDs are ephemeral. Select by UID and resolve again immediately before
/// opening; never persist the numeric ID as the user's routing choice.
#[derive(Clone, Debug, PartialEq)]
pub struct CoreAudioDevice {
    pub uid: String,
    pub name: String,
    pub input_channels: u32,
    pub output_channels: u32,
    pub nominal_sample_rate: f64,
    pub buffer_frames: u32,
    pub is_default_input: bool,
    pub is_default_output: bool,
    pub(crate) id: u32,
}
impl CoreAudioDevice {
    /// Session-local HAL object ID (not stable across reboots; use `uid`).
    pub fn id(&self) -> u32 {
        self.id
    }
}
fn error(status: i32) -> Error {
    Error::Io(std::io::Error::other(format!(
        "Core Audio device property failed ({status})"
    )))
}
fn property(object: u32, selector: [u8; 4], scope: u32) -> Result<Vec<u8>> {
    let address = Address {
        selector: u32::from_be_bytes(selector),
        scope,
        element: 0,
    };
    let mut size = 0;
    let status =
        unsafe { AudioObjectGetPropertyDataSize(object, &address, 0, std::ptr::null(), &mut size) };
    if status != 0 {
        return Err(error(status));
    }
    if size as usize > MAX_PROPERTY {
        return Err(Error::Invalid(
            "Core Audio property exceeds 1 MiB inventory bound".into(),
        ));
    }
    // Vec<u64> provides the alignment required by every queried SDK property,
    // including AudioBufferList and CFStringRef. No external data is dereferenced.
    let mut words = vec![0_u64; (size as usize).div_ceil(8).max(1)];
    let capacity = size;
    let status = unsafe {
        AudioObjectGetPropertyData(
            object,
            &address,
            0,
            std::ptr::null(),
            &mut size,
            words.as_mut_ptr().cast(),
        )
    };
    if status != 0 {
        return Err(error(status));
    }
    if size > capacity {
        return Err(Error::Conflict(
            "Core Audio inventory changed while reading".into(),
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(words.as_ptr().cast::<u8>(), size as usize) };
    Ok(bytes.to_vec())
}
fn number(object: u32, selector: [u8; 4]) -> Result<u32> {
    let bytes = property(object, selector, GLOBAL)?;
    Ok(u32::from_ne_bytes(bytes.try_into().map_err(|_| {
        Error::Invalid("Core Audio UInt32 property has invalid size".into())
    })?))
}
fn text(object: u32, selector: [u8; 4]) -> Result<String> {
    let bytes = property(object, selector, GLOBAL)?;
    let bits = usize::from_ne_bytes(
        bytes
            .try_into()
            .map_err(|_| Error::Invalid("Core Audio CFString property has invalid size".into()))?,
    );
    let string = bits as *const c_void;
    if string.is_null() {
        return Err(Error::Invalid("Core Audio returned a null string".into()));
    }
    // AudioObject CFString properties return a retained reference; SDK requires
    // the caller to release it. The guard also covers all validation failures.
    struct Owned(*const c_void);
    impl Drop for Owned {
        fn drop(&mut self) {
            unsafe {
                CFRelease(self.0);
            }
        }
    }
    let _owned = Owned(string);
    let length = unsafe { CFStringGetLength(string) };
    if !(0..=8192).contains(&length) {
        return Err(Error::Invalid(
            "Core Audio device string exceeds inventory bound".into(),
        ));
    }
    let mut utf8 = vec![0; length as usize * 4 + 1];
    if unsafe { CFStringGetCString(string, utf8.as_mut_ptr(), utf8.len() as isize, 0x08000100) }
        == 0
    {
        return Err(Error::Invalid(
            "Core Audio device string cannot be read as UTF-8".into(),
        ));
    }
    utf8.truncate(utf8.iter().position(|&v| v == 0).unwrap_or(utf8.len()));
    let value = String::from_utf8(utf8)
        .map_err(|_| Error::Invalid("Core Audio device string contains invalid UTF-8".into()))?;
    if value.encode_utf16().count() != length as usize {
        return Err(Error::Invalid(
            "Core Audio device string contains an embedded NUL".into(),
        ));
    }
    Ok(value)
}
fn channels(bytes: &[u8]) -> Result<u32> {
    // AudioBufferList on 64-bit macOS has eight bytes before 16-byte buffers.
    if bytes.len() < 8 {
        return Err(Error::Invalid(
            "Core Audio channel configuration is truncated".into(),
        ));
    }
    let count = u32::from_ne_bytes(bytes[..4].try_into().unwrap()) as usize;
    if count > 4096 || bytes.len() < 8 + count * 16 {
        return Err(Error::Invalid(
            "Core Audio channel configuration has invalid dimensions".into(),
        ));
    }
    let mut total = 0_u32;
    for buffer in bytes[8..8 + count * 16].chunks_exact(16) {
        total = total
            .checked_add(u32::from_ne_bytes(buffer[..4].try_into().unwrap()))
            .filter(|&v| v <= 1024)
            .ok_or_else(|| {
                Error::Invalid("Core Audio device exceeds 1024-channel inventory bound".into())
            })?;
    }
    Ok(total)
}
/// Explicit control-worker call. Does not change sample rates, buffer sizes or
/// default devices. Device disappearance returns an error for a fresh retry.
fn describe(id: u32, default_input: u32, default_output: u32) -> Result<Option<CoreAudioDevice>> {
    if number(id, *b"livn")? == 0 {
        return Ok(None);
    }
    let rate_bytes = property(id, *b"nsrt", GLOBAL)?;
    let rate =
        f64::from_ne_bytes(rate_bytes.try_into().map_err(|_| {
            Error::Invalid("Core Audio sample-rate property has invalid size".into())
        })?);
    if !rate.is_finite() || !(1. ..=1536000.).contains(&rate) {
        return Err(Error::Invalid(
            "Core Audio nominal sample rate is outside admission bound".into(),
        ));
    }
    let device = CoreAudioDevice {
        uid: text(id, *b"uid ")?,
        name: text(id, *b"lnam")?,
        input_channels: channels(&property(id, *b"slay", INPUT)?)?,
        output_channels: channels(&property(id, *b"slay", OUTPUT)?)?,
        nominal_sample_rate: rate,
        buffer_frames: number(id, *b"fsiz")?,
        is_default_input: id == default_input,
        is_default_output: id == default_output,
        id,
    };
    if device.uid.is_empty() || device.buffer_frames == 0 {
        return Err(Error::Invalid(
            "Core Audio device identity/buffer size is outside admission bound".into(),
        ));
    }
    Ok(Some(device))
}
pub fn core_audio_devices() -> Result<Vec<CoreAudioDevice>> {
    let bytes = property(1, *b"dev#", GLOBAL)?;
    if bytes.len() % 4 != 0 || bytes.len() / 4 > 4096 {
        return Err(Error::Invalid(
            "Core Audio device list exceeds inventory bound".into(),
        ));
    }
    let default_input = number(1, *b"dIn ")?;
    let default_output = number(1, *b"dOut")?;
    let mut devices = Vec::new();
    for bytes in bytes.chunks_exact(4) {
        let id = u32::from_ne_bytes(bytes.try_into().unwrap());
        // One device with unusual or unreadable properties (a virtual bridge, a
        // device mid-reconfiguration) is left out; it never hides the others.
        let device = match describe(id, default_input, default_output) {
            Ok(Some(device)) => device,
            Ok(None) | Err(_) => continue,
        };
        if devices
            .iter()
            .any(|d: &CoreAudioDevice| d.id == device.id || d.uid == device.uid)
        {
            return Err(Error::Invalid(
                "Core Audio inventory contains a duplicate device identity".into(),
            ));
        }
        devices.push(device);
    }
    Ok(devices)
}
/// Input-scope elements that carry the hardware volume System Settings shows:
/// the main element when the device has one, otherwise each input channel.
fn input_volume_elements(id: u32, channels: u32) -> Vec<Address> {
    let settable = |address: &Address| {
        let mut settable = 0_u8;
        unsafe {
            AudioObjectHasProperty(id, address) != 0
                && AudioObjectIsPropertySettable(id, address, &mut settable) == 0
                && settable != 0
        }
    };
    let address = |element| Address {
        selector: VOLUME,
        scope: INPUT,
        element,
    };
    if settable(&address(0)) {
        return vec![address(0)];
    }
    (1..=channels.min(64))
        .map(address)
        .filter(settable)
        .collect()
}
fn read_volume(id: u32, address: &Address) -> Result<f32> {
    let mut value = 0_f32;
    let mut size = std::mem::size_of::<f32>() as u32;
    let status = unsafe {
        AudioObjectGetPropertyData(
            id,
            address,
            0,
            std::ptr::null(),
            &mut size,
            (&mut value as *mut f32).cast(),
        )
    };
    if status != 0 {
        return Err(error(status));
    }
    Ok(value.clamp(0., 1.))
}
/// Resolves an input device by UID without describing the whole inventory:
/// one UID read per device, then the input layout of the match only.
/// An empty `uid` is the system default input. Returns (UID, ID, channels).
fn input_device(uid: &str) -> Result<(String, u32, u32)> {
    let id = if uid.is_empty() {
        Some(number(1, *b"dIn ")?).filter(|&id| id != 0)
    } else {
        let bytes = property(1, *b"dev#", GLOBAL)?;
        bytes
            .chunks_exact(4)
            .map(|bytes| u32::from_ne_bytes(bytes.try_into().unwrap()))
            .find(|&id| text(id, *b"uid ").is_ok_and(|device| device == uid))
    };
    let id =
        id.ok_or_else(|| Error::Conflict("That input device is no longer available".into()))?;
    let channels = channels(&property(id, *b"slay", INPUT)?)?;
    if channels == 0 {
        return Err(Error::Conflict("That device has no inputs".into()));
    }
    let uid = if uid.is_empty() {
        text(id, *b"uid ")?
    } else {
        uid.to_owned()
    };
    Ok((uid, id, channels))
}
fn average_volume(id: u32, elements: &[Address]) -> Result<Option<f32>> {
    if elements.is_empty() {
        return Ok(None);
    }
    let mut total = 0.;
    for address in elements {
        total += read_volume(id, address)?;
    }
    Ok(Some(total / elements.len() as f32))
}
/// The input device's UID and hardware input volume (0…1), averaged across
/// channels when it has no main control; the volume is `None` when the device
/// offers no adjustable input level. An empty `uid` is the default input.
pub fn input_volume(uid: &str) -> Result<(String, Option<f32>)> {
    let (uid, id, channels) = input_device(uid)?;
    let volume = average_volume(id, &input_volume_elements(id, channels))?;
    Ok((uid, volume))
}
/// Sets every input volume control of the device and returns its UID and the
/// level the hardware reports afterwards (drivers may quantize it).
pub fn set_input_volume(uid: &str, volume: f32) -> Result<(String, Option<f32>)> {
    if !volume.is_finite() {
        return Err(Error::Invalid("Input volume must be a number".into()));
    }
    let (uid, id, channels) = input_device(uid)?;
    let elements = input_volume_elements(id, channels);
    let value = volume.clamp(0., 1.);
    for address in &elements {
        let status = unsafe {
            AudioObjectSetPropertyData(
                id,
                address,
                0,
                std::ptr::null(),
                std::mem::size_of::<f32>() as u32,
                (&value as *const f32).cast(),
            )
        };
        if status != 0 {
            return Err(error(status));
        }
    }
    Ok((uid, average_volume(id, &elements)?))
}
pub(crate) fn output_device(uid: &str) -> Result<CoreAudioDevice> {
    if uid.is_empty() || uid.len() > 32768 {
        return Err(Error::Invalid(
            "Output device UID is outside admission bound".into(),
        ));
    }
    let device = core_audio_devices()?
        .into_iter()
        .find(|d| d.uid == uid)
        .ok_or_else(|| Error::Conflict("Selected output device is no longer available".into()))?;
    if device.buffer_frames > 8192 {
        return Err(Error::Unsupported(
            "Selected output device exceeds the 8192-frame callback bound".into(),
        ));
    }
    if device.output_channels < 2 {
        return Err(Error::Unsupported(
            "Selected output device requires at least two output channels".into(),
        ));
    }
    Ok(device)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sdk_property_address_and_bounded_channel_parser() {
        assert_eq!(std::mem::size_of::<Address>(), 12);
        let mut bytes = vec![0; 8 + 2 * 16];
        bytes[..4].copy_from_slice(&2_u32.to_ne_bytes());
        bytes[8..12].copy_from_slice(&1_u32.to_ne_bytes());
        bytes[24..28].copy_from_slice(&2_u32.to_ne_bytes());
        assert_eq!(channels(&bytes).unwrap(), 3);
        assert!(channels(&bytes[..bytes.len() - 1]).is_err());
        bytes[24..28].copy_from_slice(&u32::MAX.to_ne_bytes());
        assert!(channels(&bytes).is_err());
        assert!(channels(&[]).is_err());
        assert_eq!(channels(&[0; 8]).unwrap(), 0);
        assert_eq!(channels(&[0; 24]).unwrap(), 0);
    }
}
