//! CoreMIDI input for live instruments: source inventory, one connected
//! source, and a running-status byte parser (port of Swift `DesignerMIDIParser`).
//! The CoreMIDI read callback only copies packet bytes into a bounded channel;
//! parsing and dispatch happen on the consumer's thread.
use crate::{designer::ControllerKind, Error, Result};
use std::{
    ffi::{c_char, c_void, CStr, CString},
    sync::mpsc::SyncSender,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Event {
    Note { pitch: i32, velocity: i32, on: bool },
    Controller(ControllerKind, f64),
    AllNotesOff,
}
/// Keeps running status across packets; ignores realtime and SysEx bytes.
#[derive(Default)]
pub struct Parser {
    status: u8,
    data: Vec<u8>,
    sysex: bool,
}
impl Parser {
    pub fn receive(&mut self, bytes: &[u8]) -> Vec<Event> {
        let mut output = vec![];
        for &byte in bytes {
            if byte >= 0xF8 {
                continue;
            }
            if byte >= 0x80 {
                self.data.clear();
                match byte {
                    0xF0 => {
                        self.sysex = true;
                        self.status = 0
                    }
                    0xF7 => {
                        self.sysex = false;
                        self.status = 0
                    }
                    b if b < 0xF0 => {
                        self.sysex = false;
                        self.status = b
                    }
                    _ => self.status = 0,
                }
                continue;
            }
            if self.sysex || !(0x80..0xF0).contains(&self.status) {
                continue;
            }
            self.data.push(byte);
            let command = self.status & 0xF0;
            let needed = if command == 0xC0 || command == 0xD0 {
                1
            } else {
                2
            };
            if self.data.len() != needed {
                continue;
            }
            let (a, b) = (self.data[0], self.data.get(1).copied().unwrap_or(0));
            match command {
                0x90 | 0x80 => output.push(Event::Note {
                    pitch: a as i32,
                    velocity: b as i32,
                    on: command == 0x90 && b > 0,
                }),
                0xB0 => {
                    let kind = match a {
                        1 => Some(ControllerKind::Modulation),
                        7 | 11 => Some(ControllerKind::Expression),
                        64 => Some(ControllerKind::Sustain),
                        120 | 123 => {
                            output.push(Event::AllNotesOff);
                            None
                        }
                        _ => None,
                    };
                    if let Some(kind) = kind {
                        output.push(Event::Controller(kind, b as f64 / 127.));
                    }
                }
                0xE0 => {
                    let raw = a as i32 | (b as i32) << 7;
                    let value = if raw <= 8192 {
                        raw as f64 / 16384.
                    } else {
                        0.5 + (raw - 8192) as f64 / 16382.
                    };
                    output.push(Event::Controller(ControllerKind::PitchBend, value));
                }
                _ => {}
            }
            self.data.clear();
        }
        output
    }
}

type CfString = *const c_void;
type ReadProc = extern "C" fn(*const c_void, *mut c_void, *mut c_void);
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFStringCreateWithCString(
        allocator: *const c_void,
        text: *const c_char,
        encoding: u32,
    ) -> CfString;
    fn CFStringGetCString(text: CfString, buffer: *mut u8, size: isize, encoding: u32) -> u8;
    fn CFRelease(value: *const c_void);
}
#[link(name = "CoreMIDI", kind = "framework")]
extern "C" {
    static kMIDIPropertyDisplayName: CfString;
    fn MIDIClientCreate(
        name: CfString,
        notify: *const c_void,
        refcon: *mut c_void,
        client: *mut u32,
    ) -> i32;
    fn MIDIInputPortCreate(
        client: u32,
        name: CfString,
        read: ReadProc,
        refcon: *mut c_void,
        port: *mut u32,
    ) -> i32;
    fn MIDIGetNumberOfSources() -> usize;
    fn MIDIGetSource(index: usize) -> u32;
    fn MIDIObjectGetStringProperty(object: u32, property: CfString, value: *mut CfString) -> i32;
    fn MIDIPortConnectSource(port: u32, source: u32, refcon: *mut c_void) -> i32;
    fn MIDIPortDisconnectSource(port: u32, source: u32) -> i32;
    fn MIDIPortDispose(port: u32) -> i32;
    fn MIDIClientDispose(client: u32) -> i32;
}
const UTF8: u32 = 0x0800_0100;
fn cf(text: &str) -> CfString {
    let text = CString::new(text).unwrap_or_default();
    unsafe { CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8) }
}
/// Copies each packet's bytes to the consumer. Runs on a CoreMIDI thread.
extern "C" fn read(list: *const c_void, refcon: *mut c_void, _: *mut c_void) {
    if list.is_null() || refcon.is_null() {
        return;
    }
    let sender = unsafe { &*(refcon as *const SyncSender<Vec<u8>>) };
    unsafe {
        let count = *(list as *const u32);
        // MIDIPacketList { UInt32 numPackets; MIDIPacket packet[] } under
        // #pragma pack(4): MIDIPacket { UInt64 timeStamp; UInt16 length; Byte data[] }.
        let mut packet = (list as *const u8).add(4);
        for _ in 0..count.min(4096) {
            let length = (packet.add(8) as *const u16).read_unaligned() as usize;
            let data = std::slice::from_raw_parts(packet.add(10), length);
            let _ = sender.try_send(data.to_vec());
            let next = packet.add(10 + length);
            packet = if cfg!(target_arch = "aarch64") {
                ((next as usize + 3) & !3) as *const u8
            } else {
                next
            };
        }
    }
}
/// One client and input port; at most one connected source.
pub struct MidiInput {
    client: u32,
    port: u32,
    connected: u32,
    sender: *mut SyncSender<Vec<u8>>,
}
unsafe impl Send for MidiInput {}
impl MidiInput {
    pub fn new(sender: SyncSender<Vec<u8>>) -> Result<Self> {
        let (mut client, mut port) = (0, 0);
        let name = cf("Soundroom server");
        let status =
            unsafe { MIDIClientCreate(name, std::ptr::null(), std::ptr::null_mut(), &mut client) };
        unsafe { CFRelease(name) };
        if status != 0 {
            return Err(Error::Unsupported(format!(
                "MIDI could not start ({status})"
            )));
        }
        let sender = Box::into_raw(Box::new(sender));
        let name = cf("Designer notes");
        let status = unsafe { MIDIInputPortCreate(client, name, read, sender.cast(), &mut port) };
        unsafe { CFRelease(name) };
        if status != 0 {
            unsafe {
                MIDIClientDispose(client);
                drop(Box::from_raw(sender));
            }
            return Err(Error::Unsupported(format!(
                "MIDI input could not open ({status})"
            )));
        }
        Ok(Self {
            client,
            port,
            connected: 0,
            sender,
        })
    }
    /// (endpoint, display name) for every current source.
    pub fn sources(&self) -> Vec<(u32, String)> {
        let count = unsafe { MIDIGetNumberOfSources() }.min(256);
        (0..count)
            .filter_map(|index| {
                let endpoint = unsafe { MIDIGetSource(index) };
                if endpoint == 0 {
                    return None;
                }
                let mut value: CfString = std::ptr::null();
                let mut name = format!("MIDI input {}", index + 1);
                if unsafe {
                    MIDIObjectGetStringProperty(endpoint, kMIDIPropertyDisplayName, &mut value)
                } == 0
                    && !value.is_null()
                {
                    let mut buffer = [0u8; 256];
                    if unsafe { CFStringGetCString(value, buffer.as_mut_ptr(), 256, UTF8) } != 0 {
                        name = unsafe { CStr::from_ptr(buffer.as_ptr().cast()) }
                            .to_string_lossy()
                            .into_owned();
                    }
                    unsafe { CFRelease(value) };
                }
                Some((endpoint, name))
            })
            .collect()
    }
    /// Connects `endpoint` (0 disconnects).
    pub fn connect(&mut self, endpoint: u32) -> Result<()> {
        if self.connected != 0 {
            unsafe { MIDIPortDisconnectSource(self.port, self.connected) };
            self.connected = 0;
        }
        if endpoint == 0 {
            return Ok(());
        }
        let status = unsafe { MIDIPortConnectSource(self.port, endpoint, std::ptr::null_mut()) };
        if status != 0 {
            return Err(Error::Unsupported(format!(
                "MIDI input could not connect ({status})"
            )));
        }
        self.connected = endpoint;
        Ok(())
    }
    pub fn connected(&self) -> u32 {
        self.connected
    }
}
impl Drop for MidiInput {
    fn drop(&mut self) {
        unsafe {
            if self.connected != 0 {
                MIDIPortDisconnectSource(self.port, self.connected);
            }
            MIDIPortDispose(self.port);
            MIDIClientDispose(self.client);
            // The port is disposed, so no read callback can still use the sender.
            drop(Box::from_raw(self.sender));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn running_status_notes_controllers_and_bend() {
        let mut parser = Parser::default();
        let events = parser.receive(&[
            0x90, 60, 100, 62, 0, 0xF8, 0xB0, 64, 127, 0xE0, 0, 64, 0xB0, 123, 0,
        ]);
        assert_eq!(
            events,
            vec![
                Event::Note {
                    pitch: 60,
                    velocity: 100,
                    on: true
                },
                Event::Note {
                    pitch: 62,
                    velocity: 0,
                    on: false
                },
                Event::Controller(ControllerKind::Sustain, 1.),
                Event::Controller(ControllerKind::PitchBend, 0.5),
                Event::AllNotesOff,
            ]
        );
        assert!(parser.receive(&[0xF0, 1, 2, 3, 0xF7]).is_empty());
    }
}
