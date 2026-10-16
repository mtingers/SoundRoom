//! Opt-in macOS output adapter. DefaultOutput is qualified by an earlier muted
//! smoke; explicit UID/HAL output remains hardware-unqualified. No device is
//! enumerated or opened automatically, and system default routes are never set.
use crate::{realtime::AudioHost, Error, Result};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::{cell::UnsafeCell, ffi::c_void, marker::PhantomData, rc::Rc};
#[path = "core_audio_devices.rs"]
mod devices;
pub use devices::{core_audio_devices, CoreAudioDevice};
type Unit = *mut c_void;
#[repr(C)]
struct ComponentDescription {
    kind: u32,
    subtype: u32,
    manufacturer: u32,
    flags: u32,
    mask: u32,
}
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
type Render =
    unsafe extern "C" fn(*mut c_void, *mut u32, *const c_void, u32, u32, *mut BufferList) -> i32;
#[repr(C)]
struct Callback {
    function: Render,
    context: *mut c_void,
}
#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioComponentFindNext(previous: Unit, description: *const ComponentDescription) -> Unit;
    fn AudioComponentInstanceNew(component: Unit, unit: *mut Unit) -> i32;
    fn AudioComponentInstanceDispose(unit: Unit) -> i32;
    fn AudioUnitSetProperty(
        unit: Unit,
        property: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        size: u32,
    ) -> i32;
    fn AudioUnitInitialize(unit: Unit) -> i32;
    fn AudioUnitUninitialize(unit: Unit) -> i32;
    fn AudioOutputUnitStart(unit: Unit) -> i32;
    fn AudioOutputUnitStop(unit: Unit) -> i32;
}
struct Context {
    host: UnsafeCell<AudioHost>,
    gate: CallbackGate,
    callbacks: AtomicU64,
    rejected_callbacks: AtomicU64,
    closed_callbacks: AtomicU64,
}
/// Validates the native planar stereo list and returns non-overlapping slices.
/// The list belongs to this callback invocation, never to the AudioHost.
unsafe fn output_slices<'a>(
    list: *mut BufferList,
    frames: u32,
) -> Option<(&'a mut [f32], &'a mut [f32])> {
    // DefaultOutput promises a serialized render callback and valid AudioBufferList
    // storage for its negotiated two planar Float32 channels. Validate dimensions
    // before constructing non-overlapping Rust slices.
    let list = unsafe { &mut *list };
    if list.count != 2 {
        return None;
    }
    let first = std::ptr::addr_of_mut!(list.first);
    let second = unsafe { first.add(1) };
    let l = unsafe { &mut *first };
    let r = unsafe { &mut *second };
    let size = frames as usize * 4;
    let a = l.data as usize;
    let b = r.data as usize;
    if l.channels != 1
        || r.channels != 1
        || (l.bytes as usize) < size
        || (r.bytes as usize) < size
        || a == 0
        || b == 0
        || a % 4 != 0
        || b % 4 != 0
        || a.abs_diff(b) < size
    {
        return None;
    }
    Some(unsafe {
        (
            std::slice::from_raw_parts_mut(l.data.cast::<f32>(), frames as usize),
            std::slice::from_raw_parts_mut(r.data.cast::<f32>(), frames as usize),
        )
    })
}
unsafe extern "C" fn render(
    context: *mut c_void,
    _flags: *mut u32,
    time: *const c_void,
    _bus: u32,
    frames: u32,
    list: *mut BufferList,
) -> i32 {
    if context.is_null() || list.is_null() || frames > 8192 {
        return -50;
    }
    let Some((left, right)) = (unsafe { output_slices(list, frames) }) else {
        return -50;
    };
    let context = unsafe { &*context.cast::<Context>() };
    // Admit before forming any reference into the AudioHost. A late callback
    // after close, or an overlapping one, renders silence and never touches it.
    let Some(scope) = context.gate.enter() else {
        context.closed_callbacks.fetch_add(1, Ordering::Relaxed);
        left.fill(0.);
        right.fill(0.);
        return 0;
    };
    if !scope.exclusive {
        context.rejected_callbacks.fetch_add(1, Ordering::Relaxed);
        left.fill(0.);
        right.fill(0.);
        return 0;
    }
    context.callbacks.fetch_add(1, Ordering::Relaxed);
    let timing = if time.is_null() {
        crate::capture::CaptureTiming::default()
    } else {
        let timestamp = unsafe { &*time.cast::<TimeStamp>() };
        crate::capture::CaptureTiming {
            sample_time: timestamp.sample_time,
            host_time: timestamp.host_time,
            flags: timestamp.flags & 3,
        }
    };
    unsafe { &mut *context.host.get() }.render_timed(left, right, timing);
    0
}
fn checked(status: i32, operation: &str) -> Result<()> {
    if status == 0 {
        Ok(())
    } else {
        Err(Error::Io(std::io::Error::other(format!(
            "Core Audio {operation} failed ({status})"
        ))))
    }
}
/// Active and uncertain-retained native output contexts share a fixed bound.
/// The permit is reserved before any native work so safe retention is bounded.
pub const MAX_NATIVE_OUTPUT_CONTEXTS: usize = 2;
struct OutputAdmission {
    reserved: AtomicUsize,
    quarantined: AtomicUsize,
}
static OUTPUT_ADMISSION: OutputAdmission = OutputAdmission {
    reserved: AtomicUsize::new(0),
    quarantined: AtomicUsize::new(0),
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreAudioOutputAdmission {
    pub reserved_contexts: usize,
    pub quarantined_contexts: usize,
    pub maximum_contexts: usize,
}
/// Advisory bounded coherent read; reservation itself is the admission fence.
pub fn output_admission_status() -> Option<CoreAudioOutputAdmission> {
    for _ in 0..4 {
        let reserved = OUTPUT_ADMISSION.reserved.load(Ordering::Acquire);
        let quarantined = OUTPUT_ADMISSION.quarantined.load(Ordering::Acquire);
        if reserved == OUTPUT_ADMISSION.reserved.load(Ordering::Acquire) && quarantined <= reserved
        {
            return Some(CoreAudioOutputAdmission {
                reserved_contexts: reserved,
                quarantined_contexts: quarantined,
                maximum_contexts: MAX_NATIVE_OUTPUT_CONTEXTS,
            });
        }
    }
    None
}
struct OutputPermit {
    admission: &'static OutputAdmission,
    quarantined: bool,
}
impl OutputAdmission {
    fn reserve(&'static self) -> Result<OutputPermit> {
        self.reserved
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |n| {
                (n < MAX_NATIVE_OUTPUT_CONTEXTS).then_some(n + 1)
            })
            .map_err(|_| {
                Error::Conflict(format!(
                    "Native output context limit reached ({MAX_NATIVE_OUTPUT_CONTEXTS} active or quarantined); uncertain retained units require server restart"
                ))
            })?;
        Ok(OutputPermit {
            admission: self,
            quarantined: false,
        })
    }
}
impl OutputPermit {
    fn quarantine(&mut self) {
        if !self.quarantined {
            self.quarantined = true;
            self.admission.quarantined.fetch_add(1, Ordering::Release);
        }
    }
}
impl Drop for OutputPermit {
    fn drop(&mut self) {
        if !self.quarantined {
            self.admission.reserved.fetch_sub(1, Ordering::Release);
        }
    }
}
#[derive(Clone, Copy)]
struct OutputOperations {
    start: unsafe extern "C" fn(Unit) -> i32,
    stop: unsafe extern "C" fn(Unit) -> i32,
    uninitialize: unsafe extern "C" fn(Unit) -> i32,
    dispose: unsafe extern "C" fn(Unit) -> i32,
}
impl Default for OutputOperations {
    fn default() -> Self {
        Self {
            start: AudioOutputUnitStart,
            stop: AudioOutputUnitStop,
            uninitialize: AudioUnitUninitialize,
            dispose: AudioComponentInstanceDispose,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreAudioOutputHealth {
    pub running: bool,
    pub callback_count: u64,
    /// Overlapping callbacks rendered as silence without entering the host.
    pub rejected_callbacks: u64,
    /// Late callbacks after close rendered as silence without entering the host.
    pub closed_callbacks: u64,
    pub stop_failures: u64,
    pub last_stop_status: i32,
    pub closing: bool,
    pub quiescent: bool,
}
/// Why a closed output kept its native unit and callback storage. Retention is
/// counted against the fixed admission bound until process exit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreAudioOutputRetention {
    /// Native Stop failed; the unit may still call back.
    StopFailed(i32),
    /// Stop succeeded but an admitted callback had not left the gate.
    CallbackActive,
    UninitializeFailed(i32),
    DisposeFailed(i32),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CoreAudioOutputClose {
    /// Stopped, quiescent, uninitialized and disposed; storage was freed.
    Closed,
    /// Uncertain teardown; unit and AudioHost are retained, never freed.
    Retained(CoreAudioOutputRetention),
}
/// Control-thread lifecycle owner. Its render context is never accessed by the
/// control thread while callbacks may run. Stop/quiescence/dispose all precede
/// context destruction; any uncertainty retains storage rather than free it.
pub struct CoreAudioOutput {
    unit: Unit,
    context: *mut Context,
    running: bool,
    initialized: bool,
    closed: bool,
    stop_failures: u64,
    last_stop_status: i32,
    lifecycle: crate::realtime::OutputLifecycleControl,
    permit: Option<OutputPermit>,
    operations: OutputOperations,
    _control_thread: PhantomData<Rc<()>>,
}
impl CoreAudioOutput {
    pub fn open_default(host: AudioHost) -> Result<Self> {
        Self::open_unit(host, None).map_err(|(error, _)| error)
    }
    /// Opens `uid` (or the system default) and, when opening fails after the
    /// host was accepted, returns the recovered host so the caller can retry.
    pub fn open_recoverable(
        host: AudioHost,
        uid: Option<&str>,
    ) -> std::result::Result<Self, (Error, Option<Box<AudioHost>>)> {
        let device = match uid {
            Some(uid) => match devices::output_device(uid) {
                Ok(device) => Some(device.id),
                Err(error) => return Err((error, Some(Box::new(host)))),
            },
            None => None,
        };
        Self::open_unit(host, device)
    }
    /// Closes the unit and returns the AudioHost for another device. Uncertain
    /// teardown retains the host (never freed) and reports why.
    pub fn close_into_host(mut self) -> std::result::Result<AudioHost, CoreAudioOutputRetention> {
        let context = self.teardown_take()?;
        Ok(context.host.into_inner())
    }
    /// Resolve a stable device UID immediately before opening an output-only HAL
    /// unit. Does not change the system default or hardware rate/buffer settings.
    /// Explicit device routing is pending actual hardware qualification.
    pub fn open_device(host: AudioHost, uid: &str) -> Result<Self> {
        let device = devices::output_device(uid)?;
        Self::open_unit(host, Some(device.id)).map_err(|(error, _)| error)
    }
    fn wrap(host: AudioHost, unit: Unit, permit: Option<OutputPermit>) -> Self {
        let lifecycle = host.output_lifecycle_control();
        // A host that already rendered elsewhere starts a new output boundary.
        lifecycle.attach();
        Self {
            unit,
            context: Box::into_raw(Box::new(Context {
                host: UnsafeCell::new(host),
                gate: CallbackGate(AtomicUsize::new(0)),
                callbacks: AtomicU64::new(0),
                rejected_callbacks: AtomicU64::new(0),
                closed_callbacks: AtomicU64::new(0),
            })),
            running: false,
            initialized: false,
            closed: false,
            stop_failures: 0,
            last_stop_status: 0,
            lifecycle,
            permit,
            operations: OutputOperations::default(),
            _control_thread: PhantomData,
        }
    }
    fn open_unit(
        host: AudioHost,
        device: Option<u32>,
    ) -> std::result::Result<Self, (Error, Option<Box<AudioHost>>)> {
        let permit = match OUTPUT_ADMISSION.reserve() {
            Ok(permit) => permit,
            Err(error) => return Err((error, Some(Box::new(host)))),
        };
        let description = ComponentDescription {
            kind: u32::from_be_bytes(*b"auou"),
            subtype: u32::from_be_bytes(if device.is_some() { *b"ahal" } else { *b"def " }),
            manufacturer: u32::from_be_bytes(*b"appl"),
            flags: 0,
            mask: 0,
        };
        let component = unsafe { AudioComponentFindNext(std::ptr::null_mut(), &description) };
        if component.is_null() {
            return Err((
                Error::Unsupported("Core Audio DefaultOutput unavailable".into()),
                Some(Box::new(host)),
            ));
        }
        let mut unit = std::ptr::null_mut();
        if let Err(error) = checked(
            unsafe { AudioComponentInstanceNew(component, &mut unit) },
            "create output",
        ) {
            return Err((error, Some(Box::new(host))));
        }
        // From here Drop owns the unit: a configuration failure disposes it
        // (never started, so no callback can be admitted) before freeing storage.
        let mut output = Self::wrap(host, unit, Some(permit));
        match output.configure(device) {
            Ok(()) => Ok(output),
            Err(error) => Err((error, output.close_into_host().ok().map(Box::new))),
        }
    }
    fn configure(&mut self, device: Option<u32>) -> Result<()> {
        let output = self;
        let unit = output.unit;
        if let Some(device) = device {
            output.property_on(2003, 1, 1, &0_u32)?; // input disabled
            output.property_on(2003, 2, 0, &1_u32)?; // output enabled
            output.property(2000, 0, &device)?;
        }
        let format = StreamDescription {
            rate: 48000.,
            format: u32::from_be_bytes(*b"lpcm"),
            flags: 1 | 8 | 32,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels: 2,
            bits: 32,
            reserved: 0,
        };
        // Property/scope constants and C layouts are from the installed SDK's
        // AudioUnitProperties.h, AUComponent.h and CoreAudioBaseTypes.h.
        output.property(8, 1, &format)?;
        output.property(14, 0, &8192_u32)?;
        output.property(
            23,
            1,
            &Callback {
                function: render,
                context: output.context.cast(),
            },
        )?;
        checked(unsafe { AudioUnitInitialize(unit) }, "initialize output")?;
        output.initialized = true;
        Ok(())
    }
    fn property<T>(&self, property: u32, scope: u32, value: &T) -> Result<()> {
        self.property_on(property, scope, 0, value)
    }
    fn property_on<T>(&self, property: u32, scope: u32, element: u32, value: &T) -> Result<()> {
        checked(
            unsafe {
                AudioUnitSetProperty(
                    self.unit,
                    property,
                    scope,
                    element,
                    (value as *const T).cast(),
                    std::mem::size_of::<T>() as u32,
                )
            },
            "configure output",
        )
    }
    /// Reversible after a successful stop. Restart marks the output available
    /// again; the next callback starts a new audio-clock epoch. A unit left
    /// running by a failed Stop is re-admitted without another native start.
    pub fn start(&mut self) -> Result<()> {
        if self.closed || unsafe { &*self.context }.gate.state() & GATE_CLOSED != 0 {
            return Err(Error::Conflict(
                "Output is closed; open a new output unit".into(),
            ));
        }
        if !self.initialized {
            return Err(Error::Conflict("Output is not initialized".into()));
        }
        if !self.lifecycle.can_start() {
            return Err(Error::Conflict(
                "Output boundary counter is exhausted; restart the server".into(),
            ));
        }
        if !self.running {
            checked(
                unsafe { (self.operations.start)(self.unit) },
                "start output",
            )?;
            self.running = true;
        }
        self.lifecycle.available();
        Ok(())
    }
    /// Marks the output unavailable before the native Stop, so an in-flight
    /// callback already sees the boundary: anchors, open Write leases and
    /// handoffs from this run are invalidated. A failed native Stop keeps the
    /// unit running and unavailable; retry Stop or start() to re-admit it.
    pub fn stop(&mut self) -> Result<()> {
        if self.running {
            self.lifecycle.unavailable();
            let status = unsafe { (self.operations.stop)(self.unit) };
            if status != 0 {
                self.stop_failures = self.stop_failures.saturating_add(1);
                self.last_stop_status = status;
            }
            checked(status, "stop output")?;
            self.running = false;
        }
        Ok(())
    }
    pub fn health(&self) -> CoreAudioOutputHealth {
        let context = unsafe { &*self.context };
        let state = context.gate.state();
        CoreAudioOutputHealth {
            running: self.running,
            callback_count: context.callbacks.load(Ordering::Relaxed),
            rejected_callbacks: context.rejected_callbacks.load(Ordering::Relaxed),
            closed_callbacks: context.closed_callbacks.load(Ordering::Relaxed),
            stop_failures: self.stop_failures,
            last_stop_status: self.last_stop_status,
            closing: state & GATE_CLOSED != 0,
            quiescent: state & !GATE_CLOSED == 0,
        }
    }
    /// Explicit teardown with a truthful result. Only a successful native Stop,
    /// a quiescent closed gate, Uninitialize and Dispose free the AudioHost.
    pub fn close(mut self) -> CoreAudioOutputClose {
        self.teardown()
    }
    fn teardown(&mut self) -> CoreAudioOutputClose {
        match self.teardown_take() {
            Ok(context) => {
                drop(context);
                CoreAudioOutputClose::Closed
            }
            Err(reason) => CoreAudioOutputClose::Retained(reason),
        }
    }
    /// On proven exclusive teardown, hands back the callback context.
    fn teardown_take(&mut self) -> std::result::Result<Box<Context>, CoreAudioOutputRetention> {
        if self.closed {
            return Err(CoreAudioOutputRetention::CallbackActive);
        }
        self.closed = true;
        self.lifecycle.unavailable();
        let stopped = self.stop();
        let context = unsafe { &*self.context };
        context.gate.close();
        let retained = if stopped.is_err() {
            Some(CoreAudioOutputRetention::StopFailed(self.last_stop_status))
        } else if context.gate.state() != GATE_CLOSED {
            Some(CoreAudioOutputRetention::CallbackActive)
        } else {
            None
        };
        if let Some(reason) = retained {
            return self.retain(reason);
        }
        // Exclusive access is proven: no callback can enter the closed gate and
        // none is admitted. Release open leases with truthful terminal receipts.
        unsafe { &mut *context.host.get() }.release_output_writes();
        if self.initialized {
            let status = unsafe { (self.operations.uninitialize)(self.unit) };
            if status != 0 {
                return self.retain(CoreAudioOutputRetention::UninitializeFailed(status));
            }
            self.initialized = false;
        }
        let status = unsafe { (self.operations.dispose)(self.unit) };
        if status != 0 {
            return self.retain(CoreAudioOutputRetention::DisposeFailed(status));
        }
        if context.gate.state() != GATE_CLOSED {
            return self.retain(CoreAudioOutputRetention::CallbackActive);
        }
        let context = unsafe { Box::from_raw(self.context) };
        self.context = std::ptr::null_mut();
        Ok(context)
    }
    fn retain(
        &mut self,
        reason: CoreAudioOutputRetention,
    ) -> std::result::Result<Box<Context>, CoreAudioOutputRetention> {
        // Never free callback-owned storage under uncertainty; a late system
        // callback finds a closed gate and renders silence. Process exit reclaims.
        if let Some(permit) = &mut self.permit {
            permit.quarantine();
        }
        Err(reason)
    }
}
impl Drop for CoreAudioOutput {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.teardown();
        }
    }
}

#[repr(C)]
struct TimeStamp {
    sample_time: f64,
    host_time: u64,
    rate_scalar: f64,
    word_clock: u64,
    smpte: [u8; 24],
    flags: u32,
    reserved: u32,
}
#[repr(C)]
struct InputBuffers {
    count: u32,
    buffers: [AudioBuffer; 64],
}
type InputRender =
    unsafe extern "C" fn(Unit, *mut u32, *const TimeStamp, u32, u32, *mut BufferList) -> i32;
#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    fn AudioUnitRender(
        unit: Unit,
        flags: *mut u32,
        time: *const TimeStamp,
        bus: u32,
        frames: u32,
        data: *mut BufferList,
    ) -> i32;
}
struct InputContext {
    unit: Unit,
    render: InputRender,
    writer: UnsafeCell<crate::capture::CaptureWriter>,
    audio: UnsafeCell<Vec<Vec<f32>>>,
    buffers: UnsafeCell<InputBuffers>,
    control: crate::capture::CaptureControl,
    gate: CallbackGate,
    callbacks: AtomicU64,
    accepted: AtomicU64,
    timestamped: AtomicU64,
    rejected_callbacks: AtomicU64,
    rejected_frames: AtomicU64,
}
const GATE_CLOSED: usize = 1;
const GATE_ACTIVE: usize = 2;
struct CallbackGate(AtomicUsize);
struct CallbackScope<'a> {
    gate: &'a CallbackGate,
    exclusive: bool,
}
impl CallbackGate {
    fn enter(&self) -> Option<CallbackScope<'_>> {
        self.0
            .fetch_update(Ordering::Acquire, Ordering::Relaxed, |state| {
                if state & GATE_CLOSED != 0 {
                    None
                } else {
                    state.checked_add(GATE_ACTIVE)
                }
            })
            .ok()
            .map(|previous| CallbackScope {
                gate: self,
                exclusive: previous == 0,
            })
    }
    fn close(&self) {
        self.0.fetch_or(GATE_CLOSED, Ordering::AcqRel);
    }
    fn state(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}
impl Drop for CallbackScope<'_> {
    fn drop(&mut self) {
        self.gate.0.fetch_sub(GATE_ACTIVE, Ordering::Release);
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreAudioInputHealth {
    pub running: bool,
    pub callback_count: u64,
    pub accepted_frames: u64,
    pub timestamped_frames: u64,
    pub rejected_callbacks: u64,
    pub rejected_frames: u64,
    pub stop_failures: u64,
    pub last_stop_status: i32,
    pub closing: bool,
    pub quiescent: bool,
    pub capture: crate::capture::CaptureStats,
}
/// Constructed only after successful native Stop (if started), callback scope
/// quiescence and irreversible queue sealing. Pending PCM still requires worker
/// drain/archive completion before raw storage custody can be transferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreAudioInputStopped {
    started: bool,
    health: CoreAudioInputHealth,
    scope_id: u64,
    device_uid_fingerprint: [u8; 32],
    host_ticks_bits: u64,
}
fn input_device_fingerprint(uid: &str) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    Sha256::digest(uid.as_bytes()).into()
}
impl CoreAudioInputStopped {
    pub fn matches_host_ticks_per_second(&self, value: f64) -> bool {
        self.host_ticks_bits == value.to_bits()
    }
    pub(crate) fn capture_scope_id(&self) -> u64 {
        self.scope_id
    }
    pub fn matches_device_uid(&self, uid: &str) -> bool {
        self.device_uid_fingerprint == input_device_fingerprint(uid)
    }
    pub fn had_started(&self) -> bool {
        self.started
    }
    pub fn health(&self) -> CoreAudioInputHealth {
        self.health
    }
    /// Match the actual stopped native source to its drained queue, preventing
    /// accidental cross-device receipt reuse at the archive custody boundary.
    pub fn matches_capture(&self, receipt: &crate::capture::CaptureSealReceipt) -> bool {
        self.scope_id == receipt.scope_id && self.health.capture == receipt.stats()
    }
}
impl InputContext {
    fn new(unit: Unit, writer: crate::capture::CaptureWriter) -> Self {
        let control = writer.control();
        let config = writer.config();
        let mut audio: Vec<Vec<f32>> = (0..config.channels)
            .map(|_| vec![0.; config.maximum_frames])
            .collect();
        let mut buffers = InputBuffers {
            count: config.channels as u32,
            buffers: std::array::from_fn(|_| AudioBuffer {
                channels: 1,
                bytes: 0,
                data: std::ptr::null_mut(),
            }),
        };
        for (buffer, channel) in buffers.buffers.iter_mut().zip(&mut audio) {
            buffer.data = channel.as_mut_ptr().cast();
        }
        Self {
            unit,
            render: AudioUnitRender,
            writer: UnsafeCell::new(writer),
            audio: UnsafeCell::new(audio),
            buffers: UnsafeCell::new(buffers),
            control,
            gate: CallbackGate(AtomicUsize::new(0)),
            callbacks: AtomicU64::new(0),
            accepted: AtomicU64::new(0),
            timestamped: AtomicU64::new(0),
            rejected_callbacks: AtomicU64::new(0),
            rejected_frames: AtomicU64::new(0),
        }
    }
}
unsafe extern "C" fn input_render(
    context: *mut c_void,
    flags: *mut u32,
    time: *const c_void,
    _bus: u32,
    frames: u32,
    _list: *mut BufferList,
) -> i32 {
    if context.is_null() {
        return -50;
    }
    let context = unsafe { &*context.cast::<InputContext>() };
    // Admit before forming any mutable references into callback-owned storage.
    // Late/closed/reentrant callbacks cannot race the worker's stop receipt.
    let Some(scope) = context.gate.enter() else {
        return 0;
    };
    if !scope.exclusive {
        context.rejected_callbacks.fetch_add(1, Ordering::Relaxed);
        context
            .rejected_frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        return -50;
    }
    context.callbacks.fetch_add(1, Ordering::Relaxed);
    let writer = unsafe { &mut *context.writer.get() };
    if flags.is_null() || time.is_null() {
        context.rejected_callbacks.fetch_add(1, Ordering::Relaxed);
        context
            .rejected_frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        writer.device_fault(-50);
        return -50;
    }
    let config = writer.config();
    if frames as usize > config.maximum_frames {
        context.rejected_callbacks.fetch_add(1, Ordering::Relaxed);
        context
            .rejected_frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        writer.device_fault(-10874);
        return -10874;
    }
    if frames == 0 {
        return 0;
    }
    let buffers = unsafe { &mut *context.buffers.get() };
    buffers.count = config.channels as u32;
    let audio = unsafe { &mut *context.audio.get() };
    for (buffer, channel) in buffers.buffers.iter_mut().zip(audio.iter_mut()) {
        buffer.channels = 1;
        buffer.bytes = frames * 4;
        buffer.data = channel.as_mut_ptr().cast();
    }
    let time = time.cast::<TimeStamp>();
    let status = unsafe {
        (context.render)(
            context.unit,
            flags,
            time,
            1,
            frames,
            (buffers as *mut InputBuffers).cast(),
        )
    };
    if status != 0 {
        context.rejected_callbacks.fetch_add(1, Ordering::Relaxed);
        context
            .rejected_frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        writer.device_fault(status);
        return status;
    }
    if buffers.count as usize != config.channels
        || buffers.buffers.iter().zip(audio.iter()).any(|(b, a)| {
            b.channels != 1 || b.bytes < frames * 4 || b.data != a.as_ptr().cast_mut().cast()
        })
    {
        context.rejected_callbacks.fetch_add(1, Ordering::Relaxed);
        context
            .rejected_frames
            .fetch_add(frames as u64, Ordering::Relaxed);
        writer.device_fault(-50);
        return -50;
    }
    let mut channels: [&[f32]; 64] = [&[]; 64];
    for (destination, source) in channels.iter_mut().zip(audio.iter()) {
        *destination = &source[..frames as usize];
    }
    let time = unsafe { &*time };
    let timing = crate::capture::CaptureTiming {
        sample_time: time.sample_time,
        host_time: time.host_time,
        flags: time.flags & 3,
    };
    match writer.push(&channels[..config.channels], timing) {
        crate::capture::CaptureStatus::InvalidBlock => {
            context.rejected_callbacks.fetch_add(1, Ordering::Relaxed);
            context
                .rejected_frames
                .fetch_add(frames as u64, Ordering::Relaxed);
            writer.device_fault(-50);
            return -50;
        }
        crate::capture::CaptureStatus::Captured => {
            context.accepted.fetch_add(frames as u64, Ordering::Relaxed);
            if timing.flags == 3 && timing.host_time != 0 && timing.sample_time.is_finite() {
                context
                    .timestamped
                    .fetch_add(frames as u64, Ordering::Relaxed);
            }
        }
        crate::capture::CaptureStatus::Sealed => {
            context.rejected_callbacks.fetch_add(1, Ordering::Relaxed);
            context
                .rejected_frames
                .fetch_add(frames as u64, Ordering::Relaxed);
        }
        crate::capture::CaptureStatus::Overflow => {}
    }
    // Queue overflow is counted with an explicit source-frame gap. The device
    // continues; authority must report/abort recording rather than hide dropouts.
    0
}
/// Explicit input-only HAL capture. This preparatory adapter is not yet hardware
/// qualified or wired to recording authority. Opening requires macOS microphone
/// authorization and must be invoked only after the hardware gate is permitted.
/// Multiple devices have independent clocks; use the source worker's timestamp
/// fit. Physical route/timestamp/deadline qualification remains separate.
/// Active and uncertain-retained native input contexts share a fixed admission
/// bound. Reserving before any native discovery/open makes safe retention bounded.
pub const MAX_NATIVE_INPUT_CONTEXTS: usize = 4;
struct InputAdmission {
    reserved: AtomicUsize,
    quarantined: AtomicUsize,
}
static INPUT_ADMISSION: InputAdmission = InputAdmission {
    reserved: AtomicUsize::new(0),
    quarantined: AtomicUsize::new(0),
};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CoreAudioInputAdmission {
    pub reserved_contexts: usize,
    pub quarantined_contexts: usize,
    pub maximum_contexts: usize,
}
/// Advisory bounded coherent read; reservation itself is the admission fence.
pub fn input_admission_status() -> Option<CoreAudioInputAdmission> {
    for _ in 0..4 {
        let reserved = INPUT_ADMISSION.reserved.load(Ordering::Acquire);
        let quarantined = INPUT_ADMISSION.quarantined.load(Ordering::Acquire);
        if reserved == INPUT_ADMISSION.reserved.load(Ordering::Acquire) && quarantined <= reserved {
            return Some(CoreAudioInputAdmission {
                reserved_contexts: reserved,
                quarantined_contexts: quarantined,
                maximum_contexts: MAX_NATIVE_INPUT_CONTEXTS,
            });
        }
    }
    None
}

struct InputPermit {
    admission: &'static InputAdmission,
    quarantined: bool,
}
impl InputAdmission {
    fn reserve(&'static self) -> Result<InputPermit> {
        self.reserved.fetch_update(Ordering::AcqRel,Ordering::Relaxed,|n|(n<MAX_NATIVE_INPUT_CONTEXTS).then_some(n+1))
            .map_err(|_|Error::Conflict("Native input context limit reached (4 active or quarantined); uncertain retained units require server restart".into()))?;
        Ok(InputPermit {
            admission: self,
            quarantined: false,
        })
    }
}
impl InputPermit {
    fn quarantine(&mut self) {
        if !self.quarantined {
            self.quarantined = true;
            self.admission.quarantined.fetch_add(1, Ordering::Release);
        }
    }
}
impl Drop for InputPermit {
    fn drop(&mut self) {
        if !self.quarantined {
            self.admission.reserved.fetch_sub(1, Ordering::Release);
        }
    }
}
pub struct CoreAudioInput {
    unit: Unit,
    context: *mut InputContext,
    running: bool,
    started: bool,
    initialized: bool,
    stop_failures: u64,
    last_stop_status: i32,
    permit: Option<InputPermit>,
    device_uid_fingerprint: [u8; 32],
    host_ticks_bits: u64,
    operations: InputOperations,
    _control_thread: PhantomData<Rc<()>>,
}
#[derive(Clone, Copy)]
struct InputOperations {
    start: unsafe extern "C" fn(Unit) -> i32,
    stop: unsafe extern "C" fn(Unit) -> i32,
    uninitialize: unsafe extern "C" fn(Unit) -> i32,
    dispose: unsafe extern "C" fn(Unit) -> i32,
}
impl Default for InputOperations {
    fn default() -> Self {
        Self {
            start: AudioOutputUnitStart,
            stop: AudioOutputUnitStop,
            uninitialize: AudioUnitUninitialize,
            dispose: AudioComponentInstanceDispose,
        }
    }
}
impl CoreAudioInput {
    pub fn open_device(writer: crate::capture::CaptureWriter, uid: &str) -> Result<Self> {
        let permit = INPUT_ADMISSION.reserve()?;
        if uid.is_empty() || uid.len() > 32768 {
            return Err(Error::Invalid(
                "Input device UID is outside admission bound".into(),
            ));
        }
        let device = core_audio_devices()?
            .into_iter()
            .find(|d| d.uid == uid)
            .ok_or_else(|| {
                Error::Conflict("Selected input device is no longer available".into())
            })?;
        let host_ticks_bits = host_ticks_per_second()?.to_bits();
        let config = writer.config();
        if device.input_channels == 0
            || device.input_channels > 64
            || config.channels != device.input_channels as usize
            || config.sample_rate as f64 != device.nominal_sample_rate
            || config.maximum_frames < device.buffer_frames as usize
        {
            return Err(Error::Unsupported(
                "Capture queue must match the device's native channels/rate and callback size"
                    .into(),
            ));
        }
        let description = ComponentDescription {
            kind: u32::from_be_bytes(*b"auou"),
            subtype: u32::from_be_bytes(*b"ahal"),
            manufacturer: u32::from_be_bytes(*b"appl"),
            flags: 0,
            mask: 0,
        };
        let component = unsafe { AudioComponentFindNext(std::ptr::null_mut(), &description) };
        if component.is_null() {
            return Err(Error::Unsupported(
                "Core Audio input HAL unavailable".into(),
            ));
        }
        let mut unit = std::ptr::null_mut();
        checked(
            unsafe { AudioComponentInstanceNew(component, &mut unit) },
            "create input",
        )?;
        let context = Box::into_raw(Box::new(InputContext::new(unit, writer)));
        let mut input = Self {
            unit,
            context,
            running: false,
            started: false,
            initialized: false,
            stop_failures: 0,
            last_stop_status: 0,
            permit: Some(permit),
            device_uid_fingerprint: input_device_fingerprint(uid),
            host_ticks_bits,
            operations: InputOperations::default(),
            _control_thread: PhantomData,
        };
        input.property(2003, 2, 0, &0_u32)?; // output disabled
        input.property(2003, 1, 1, &1_u32)?; // input enabled
        input.property(2000, 0, 0, &device.id)?;
        let format = StreamDescription {
            rate: config.sample_rate as f64,
            format: u32::from_be_bytes(*b"lpcm"),
            flags: 1 | 8 | 32,
            bytes_per_packet: 4,
            frames_per_packet: 1,
            bytes_per_frame: 4,
            channels: config.channels as u32,
            bits: 32,
            reserved: 0,
        };
        input.property(8, 2, 1, &format)?;
        input.property(14, 0, 0, &(config.maximum_frames as u32))?;
        input.property(51, 2, 1, &0_u32)?;
        input.property(
            2005,
            0,
            0,
            &Callback {
                function: input_render,
                context: context.cast(),
            },
        )?;
        checked(unsafe { AudioUnitInitialize(unit) }, "initialize input")?;
        input.initialized = true;
        Ok(input)
    }
    fn property<T>(&self, property: u32, scope: u32, element: u32, value: &T) -> Result<()> {
        checked(
            unsafe {
                AudioUnitSetProperty(
                    self.unit,
                    property,
                    scope,
                    element,
                    (value as *const T).cast(),
                    std::mem::size_of::<T>() as u32,
                )
            },
            "configure input",
        )
    }
    pub fn start(&mut self) -> Result<()> {
        if unsafe { &*self.context }.gate.state() & GATE_CLOSED != 0
            || unsafe { &*self.context }.control.is_sealing()
        {
            return Err(Error::Conflict(
                "Input capture scope is sealed; prepare a new queue/unit for the next take".into(),
            ));
        }
        if !self.running {
            checked(unsafe { (self.operations.start)(self.unit) }, "start input")?;
            self.running = true;
            self.started = true;
        }
        Ok(())
    }
    pub fn stop(&mut self) -> Result<()> {
        if self.running {
            let status = unsafe { (self.operations.stop)(self.unit) };
            if status != 0 {
                self.stop_failures = self.stop_failures.saturating_add(1);
                self.last_stop_status = status;
            }
            checked(status, "stop input")?;
            self.running = false;
        }
        Ok(())
    }
    pub fn health(&self) -> CoreAudioInputHealth {
        let context = unsafe { &*self.context };
        let state = context.gate.state();
        CoreAudioInputHealth {
            running: self.running,
            callback_count: context.callbacks.load(Ordering::Relaxed),
            accepted_frames: context.accepted.load(Ordering::Relaxed),
            timestamped_frames: context.timestamped.load(Ordering::Relaxed),
            rejected_callbacks: context.rejected_callbacks.load(Ordering::Relaxed),
            rejected_frames: context.rejected_frames.load(Ordering::Relaxed),
            stop_failures: self.stop_failures,
            last_stop_status: self.last_stop_status,
            closing: state & GATE_CLOSED != 0,
            quiescent: state & !GATE_CLOSED == 0,
            capture: context.control.stats(),
        }
    }
    /// Native Stop errors propagate and produce no sealed-stop receipt. After
    /// success, None means an already admitted callback is finishing; poll on the
    /// control worker. No sleep/spin or disk work occurs on an audio callback.
    pub fn stop_and_seal(&mut self) -> Result<Option<CoreAudioInputStopped>> {
        self.stop()?;
        let context = unsafe { &*self.context };
        context.gate.close();
        if context.gate.state() != GATE_CLOSED {
            return Ok(None);
        }
        context.control.seal();
        if context.control.final_stats().is_none() {
            return Ok(None);
        }
        Ok(Some(CoreAudioInputStopped {
            started: self.started,
            health: self.health(),
            scope_id: context.control.scope_id(),
            device_uid_fingerprint: self.device_uid_fingerprint,
            host_ticks_bits: self.host_ticks_bits,
        }))
    }
}
impl Drop for CoreAudioInput {
    fn drop(&mut self) {
        let stopped = self.stop().is_ok();
        unsafe { &*self.context }.gate.close();
        // Native teardown must never race an admitted AudioUnitRender call. If
        // Stop/quiescence cannot be established, retain unit and callback storage
        // until process exit rather than destroy callback-owned native state.
        if !stopped || unsafe { &*self.context }.gate.state() != GATE_CLOSED {
            if let Some(p) = &mut self.permit {
                p.quarantine();
            }
            return;
        }
        if self.initialized && unsafe { (self.operations.uninitialize)(self.unit) } != 0 {
            if let Some(p) = &mut self.permit {
                p.quarantine();
            }
            return;
        }
        if unsafe { (self.operations.dispose)(self.unit) } == 0
            && unsafe { &*self.context }.gate.state() == GATE_CLOSED
        {
            unsafe { &*self.context }.control.seal();
            unsafe {
                drop(Box::from_raw(self.context));
            }
        } else if let Some(p) = &mut self.permit {
            p.quarantine();
        }
    }
}

/// Native host-tick scale queried by an explicit control worker, independent of
/// audio devices. Input/output timestamp alignment must use this same domain.
pub fn host_ticks_per_second() -> Result<f64> {
    #[repr(C)]
    struct Timebase {
        numerator: u32,
        denominator: u32,
    }
    extern "C" {
        fn mach_timebase_info(info: *mut Timebase) -> i32;
    }
    let mut info = Timebase {
        numerator: 0,
        denominator: 0,
    };
    let status = unsafe { mach_timebase_info(&mut info) };
    checked(status, "host timebase")?;
    if info.numerator == 0 || info.denominator == 0 {
        return Err(Error::Invalid("Invalid native host timebase".into()));
    }
    Ok(1e9 * f64::from(info.denominator) / f64::from(info.numerator))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    thread_local! {
        static TRACK_HEAP:Cell<bool>=const {Cell::new(false)};
        static HEAP_CALLS:Cell<usize>=const {Cell::new(0)};
        static FREE_CALLS:Cell<usize>=const {Cell::new(0)};
    }
    struct Probe;
    unsafe impl GlobalAlloc for Probe {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            if TRACK_HEAP.try_with(Cell::get).unwrap_or(false) {
                let _ = HEAP_CALLS.try_with(|v| v.set(v.get() + 1));
            }
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            if TRACK_HEAP.try_with(Cell::get).unwrap_or(false) {
                let _ = FREE_CALLS.try_with(|v| v.set(v.get() + 1));
            }
            unsafe { System.dealloc(pointer, layout) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: Probe = Probe;
    fn no_heap<T>(body: impl FnOnce() -> T) -> T {
        HEAP_CALLS.with(|v| v.set(0));
        FREE_CALLS.with(|v| v.set(0));
        TRACK_HEAP.with(|v| v.set(true));
        let result = body();
        TRACK_HEAP.with(|v| v.set(false));
        assert_eq!(HEAP_CALLS.with(Cell::get), 0, "native callback allocated");
        assert_eq!(
            FREE_CALLS.with(Cell::get),
            0,
            "native callback freed storage"
        );
        result
    }
    unsafe extern "C" fn custody_success(_unit: Unit) -> i32 {
        0
    }
    unsafe extern "C" fn custody_failure(_unit: Unit) -> i32 {
        -10851
    }
    unsafe extern "C" fn custody_pcm(
        _unit: Unit,
        _flags: *mut u32,
        _time: *const TimeStamp,
        _bus: u32,
        frames: u32,
        list: *mut BufferList,
    ) -> i32 {
        let buffers = unsafe { &mut *list.cast::<InputBuffers>() };
        for buffer in &buffers.buffers[..buffers.count as usize] {
            unsafe { std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), frames as usize) }
                .fill(0.25);
        }
        0
    }
    fn custody_unit(writer: crate::capture::CaptureWriter) -> CoreAudioInput {
        let mut context = InputContext::new(std::ptr::null_mut(), writer);
        context.render = custody_pcm;
        CoreAudioInput {
            unit: std::ptr::null_mut(),
            context: Box::into_raw(Box::new(context)),
            running: false,
            started: false,
            initialized: true,
            stop_failures: 0,
            last_stop_status: 0,
            permit: None,
            device_uid_fingerprint: input_device_fingerprint("simulated-only"),
            host_ticks_bits: 1e9f64.to_bits(),
            operations: InputOperations {
                start: custody_success,
                stop: custody_success,
                uninitialize: custody_success,
                dispose: custody_success,
            },
            _control_thread: PhantomData,
        }
    }
    #[test]
    fn native_custody_matches_stop_drain_and_durable_raw_and_keeps_unplaced_loss_explicit() {
        use crate::{
            capture::{capture_channel, CaptureConfig},
            capture_archive::{
                ArchiveEventRead, ArchiveManifest, CaptureArchive, CaptureEvent, CaptureEventReader,
            },
            capture_clock::{CaptureAligner, ClockConfig, OutputClockAnchor},
            capture_worker::RecordingSourceWorker,
        };
        for failure in [
            "clean",
            "reentry",
            "untimestamped",
            "stopRetry",
            "queueOverrun",
            "foreignScope",
            "wrongUID",
            "wrongTimebase",
            "missingPCM",
            "aliasCompletion",
            "aliasManifest",
        ] {
            let path = std::env::temp_dir().canonicalize().unwrap().join(format!(
                "soundroom-native-custody-{}-{failure}",
                std::process::id()
            ));
            let config = CaptureConfig {
                channels: 1,
                maximum_frames: 64,
                blocks: 4,
                sample_rate: 48000,
            };
            let (writer, mut reader) = capture_channel(config).unwrap();
            let control = writer.control();
            let mut input = custody_unit(writer);
            let archive = CaptureArchive::create(
                &path,
                ArchiveManifest {
                    version: 1,
                    take_id: "11223344-5566-7788-9900-aabbccddeeff".into(),
                    device_uid: if failure == "wrongUID" {
                        "another-device"
                    } else {
                        "simulated-only"
                    }
                    .into(),
                    channels: 1,
                    sample_rate: 48000,
                    maximum_frames: 64,
                    host_ticks_per_second: if failure == "wrongTimebase" { 1e6 } else { 1e9 },
                    calibration_offset_frames: 0,
                },
            )
            .unwrap();
            let clock = ClockConfig {
                sample_rate: 48000,
                host_ticks_per_second: if failure == "wrongTimebase" { 1e6 } else { 1e9 },
                maximum_drift_ppm: 2000.,
            };
            let mut worker = RecordingSourceWorker::with_capture(
                archive,
                CaptureAligner::new(clock, clock, 0).unwrap(),
                2000.,
                &control,
            )
            .unwrap();
            input.start().unwrap();
            let mut time = TimeStamp {
                sample_time: 0.,
                host_time: 1_000_000_000,
                rate_scalar: 0.,
                word_clock: 0,
                smpte: [0; 24],
                flags: if failure == "untimestamped" { 1 } else { 3 },
                reserved: 0,
            };
            worker
                .observe_output(OutputClockAnchor {
                    epoch: 1,
                    playback_clock: 0,
                    frames: 512,
                    timing: crate::capture::CaptureTiming {
                        sample_time: 0.,
                        host_time: 1_000_000_000,
                        flags: 3,
                    },
                })
                .unwrap();
            let mut flags = 0;
            let packets = if failure == "queueOverrun" { 5 } else { 1 };
            for i in 0..packets {
                time.sample_time = (i * 64) as f64;
                time.host_time = 1_000_000_000 + ((i * 64) as f64 / 48000. * 1e9).round() as u64;
                assert_eq!(
                    no_heap(|| unsafe {
                        input_render(
                            input.context.cast(),
                            &mut flags,
                            (&time as *const TimeStamp).cast(),
                            1,
                            64,
                            std::ptr::null_mut(),
                        )
                    }),
                    0
                );
            }
            if failure == "reentry" {
                let active = unsafe { &*input.context }.gate.enter().unwrap();
                assert_eq!(
                    no_heap(|| unsafe {
                        input_render(
                            input.context.cast(),
                            &mut flags,
                            (&time as *const TimeStamp).cast(),
                            1,
                            17,
                            std::ptr::null_mut(),
                        )
                    }),
                    -50
                );
                drop(active);
            }
            if failure == "stopRetry" {
                input.operations.stop = custody_failure;
                assert!(input.stop_and_seal().is_err());
                input.operations.stop = custody_success;
            }
            let mut stopped = input.stop_and_seal().unwrap().unwrap();
            if failure == "foreignScope" {
                let (other_writer, _other_reader) = capture_channel(config).unwrap();
                let mut other = custody_unit(other_writer);
                other.start().unwrap();
                assert_eq!(
                    unsafe {
                        input_render(
                            other.context.cast(),
                            &mut flags,
                            (&time as *const TimeStamp).cast(),
                            1,
                            64,
                            std::ptr::null_mut(),
                        )
                    },
                    0
                );
                stopped = other.stop_and_seal().unwrap().unwrap();
            }
            assert!(reader.seal_receipt().is_none());
            let mut pcm = [0.; 64];
            while let Some(block) = reader.read_planar(&mut [&mut pcm]).unwrap() {
                if failure != "missingPCM" {
                    let _ = worker.ingest(block, &[&pcm[..block.frames]]);
                }
            }
            let sealed = reader.seal_receipt().unwrap();
            if failure.starts_with("alias") {
                let name = if failure == "aliasCompletion" {
                    "completion.json"
                } else {
                    "manifest.json"
                };
                std::fs::rename(path.join(name), path.join(format!("old-{name}"))).unwrap();
                soundroom_platform::PrivateDirectory::open_root(&path)
                    .unwrap()
                    .create_file(name)
                    .unwrap();
            }
            let result = worker.finish_core_audio_capture(stopped, sealed);
            if [
                "foreignScope",
                "wrongUID",
                "wrongTimebase",
                "missingPCM",
                "aliasCompletion",
                "aliasManifest",
            ]
            .contains(&failure)
            {
                assert!(result.is_err(), "unexpected custody for {failure}");
                drop(input);
                std::fs::remove_dir_all(path).unwrap();
                continue;
            }
            let custody = result.unwrap();
            assert_eq!(custody.is_complete(), failure == "clean");
            assert_eq!(
                custody.stats().retained_frames,
                if failure == "queueOverrun" { 256 } else { 64 }
            );
            assert_eq!(
                custody.native_health().accepted_frames,
                custody.stats().retained_frames
            );
            let mut events = CaptureEventReader::from_directory(custody.directory()).unwrap();
            let mut final_health = None;
            while let ArchiveEventRead::Event(event) = events.read_event().unwrap() {
                if let CaptureEvent::NativeInputStopped {
                    rejected_frames,
                    timestamped_frames,
                    stop_failures,
                    ..
                } = event
                {
                    final_health = Some((rejected_frames, timestamped_frames, stop_failures));
                }
            }
            let health = final_health.unwrap();
            assert_eq!(health.0, if failure == "reentry" { 17 } else { 0 });
            assert_eq!(
                health.1,
                if failure == "untimestamped" {
                    0
                } else {
                    custody.stats().retained_frames
                }
            );
            assert_eq!(health.2, u64::from(failure == "stopRetry"));
            use std::io::Read;
            for seal in custody.artifacts() {
                let mut file = custody.directory().read_file(seal.name).unwrap();
                assert_eq!(
                    soundroom_platform::file_identity(&file).unwrap(),
                    seal.identity
                );
                let mut bytes = vec![];
                file.read_to_end(&mut bytes).unwrap();
                assert_eq!(bytes.len() as u64, seal.bytes);
                use sha2::{Digest, Sha256};
                assert_eq!(<[u8; 32]>::from(Sha256::digest(&bytes)), seal.sha256);
            }
            assert!(
                custody
                    .directory()
                    .read_file("completion.json")
                    .unwrap()
                    .metadata()
                    .unwrap()
                    .len()
                    > 0
            );
            use crate::capture_recovery::{
                recover_take_in, RecoveryOutcome, RecoveryPlan, RecoveryRoute,
            };
            let recovery_path = path.with_file_name(format!(
                "soundroom-native-recovery-{}-{failure}",
                std::process::id()
            ));
            let parent = soundroom_platform::PrivateDirectory::create_root(&recovery_path).unwrap();
            let mut plan = RecoveryPlan {
                version: 1,
                name: "Native fixture".into(),
                routes: vec![RecoveryRoute {
                    track_id: "00112233-4455-6677-8899-AABBCCDDEEFF".into(),
                    channels: vec![0],
                }],
                range: None,
                loop_range: None,
                maximum_drift_ppm: 2000.,
                allow_incomplete: false,
            };
            let result = recover_take_in(
                custody.directory(),
                &parent,
                "take",
                &plan,
                &std::sync::atomic::AtomicBool::new(false),
            );
            let outcome = if failure == "clean" {
                result.unwrap()
            } else {
                assert!(result.is_err(), "loss accepted without review: {failure}");
                plan.allow_incomplete = true;
                recover_take_in(
                    custody.directory(),
                    &parent,
                    "take",
                    &plan,
                    &std::sync::atomic::AtomicBool::new(false),
                )
                .unwrap()
            };
            let report = match outcome {
                RecoveryOutcome::Committed(v) => v,
                other => panic!("{other:?}"),
            };
            assert_eq!(report.requires_review, failure != "clean");
            assert_eq!(
                report.retained_native_frames,
                custody.stats().retained_frames
            );
            if ["reentry", "untimestamped", "stopRetry"].contains(&failure) {
                assert!(report
                    .issues
                    .iter()
                    .any(|v| v.kind == "nativeInputHealth" && v.requires_review));
            }
            drop(parent);
            std::fs::remove_dir_all(recovery_path).unwrap();
            drop(custody);
            drop(input);
            std::fs::remove_dir_all(path).unwrap();
        }
    }
    #[test]
    fn uncertain_native_teardowns_exhaust_a_fixed_budget_and_clean_teardown_releases() {
        use crate::capture::{capture_channel, CaptureConfig};
        let admission = Box::leak(Box::new(InputAdmission {
            reserved: AtomicUsize::new(0),
            quarantined: AtomicUsize::new(0),
        }));
        let config = CaptureConfig {
            channels: 1,
            maximum_frames: 64,
            blocks: 4,
            sample_rate: 48000,
        };
        let (writer, _reader) = capture_channel(config).unwrap();
        let mut input = custody_unit(writer);
        input.permit = Some(admission.reserve().unwrap());
        drop(input);
        assert_eq!(admission.reserved.load(Ordering::Relaxed), 0);
        for index in 0..MAX_NATIVE_INPUT_CONTEXTS {
            let (writer, reader) = capture_channel(config).unwrap();
            let mut input = custody_unit(writer);
            input.permit = Some(admission.reserve().unwrap());
            input.running = true;
            input.started = true;
            let raw = input.context;
            let active = if index == 1 {
                unsafe { &*raw }.gate.enter()
            } else {
                None
            };
            if index == 0 {
                input.operations.stop = custody_failure;
            } else if index == 2 {
                input.operations.uninitialize = custody_failure;
            } else if index == 3 {
                input.operations.dispose = custody_failure;
            }
            drop(input);
            assert_eq!(admission.quarantined.load(Ordering::Acquire), index + 1);
            assert_eq!(admission.reserved.load(Ordering::Acquire), index + 1);
            assert!(!reader.control().is_sealing());
            drop(active);
            // Exclusively-owned mock unit has no OS callbacks; test-only cleanup.
            unsafe {
                drop(Box::from_raw(raw));
            }
        }
        assert!(admission.reserve().is_err());
        assert_eq!(
            admission.reserved.load(Ordering::Acquire),
            MAX_NATIVE_INPUT_CONTEXTS
        );
    }
    #[test]
    fn native_input_stop_receipt_waits_for_callbacks_and_preserves_stop_failure() {
        use crate::capture::{capture_channel, CaptureConfig};
        unsafe extern "C" fn success(_: Unit) -> i32 {
            0
        }
        unsafe extern "C" fn failure(_: Unit) -> i32 {
            -10851
        }
        unsafe extern "C" fn fake_render(
            _: Unit,
            _: *mut u32,
            _: *const TimeStamp,
            _: u32,
            frames: u32,
            list: *mut BufferList,
        ) -> i32 {
            let list = unsafe { &mut *list.cast::<InputBuffers>() };
            let pcm = unsafe {
                std::slice::from_raw_parts_mut(list.buffers[0].data.cast::<f32>(), frames as usize)
            };
            pcm.fill(0.25);
            0
        }
        let (writer, mut reader) = capture_channel(CaptureConfig {
            channels: 1,
            maximum_frames: 128,
            blocks: 4,
            sample_rate: 48000,
        })
        .unwrap();
        let mut context = Box::new(InputContext::new(std::ptr::null_mut(), writer));
        context.render = fake_render;
        let mut input = CoreAudioInput {
            unit: std::ptr::null_mut(),
            context: Box::into_raw(context),
            running: false,
            started: false,
            initialized: true,
            stop_failures: 0,
            last_stop_status: 0,
            permit: None,
            device_uid_fingerprint: input_device_fingerprint("simulated-only"),
            host_ticks_bits: 1e9f64.to_bits(),
            operations: InputOperations {
                start: success,
                stop: failure,
                uninitialize: success,
                dispose: success,
            },
            _control_thread: PhantomData,
        };
        input.start().unwrap();
        assert!(input.health().running);
        assert_eq!(input.health().timestamped_frames, 0);
        let timestamp = TimeStamp {
            sample_time: 500.,
            host_time: 700,
            rate_scalar: 1.,
            word_clock: 0,
            smpte: [0; 24],
            flags: 3,
            reserved: 0,
        };
        let mut flags = 0;
        assert_eq!(
            unsafe {
                input_render(
                    input.context.cast(),
                    &mut flags,
                    (&timestamp as *const TimeStamp).cast(),
                    1,
                    64,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(input.health().accepted_frames, 64);
        assert_eq!(input.health().timestamped_frames, 64);
        assert!(input.stop_and_seal().is_err());
        assert!(input.health().running);
        assert!(!input.health().closing);
        assert!(!reader.control().is_sealing());
        assert!(reader.seal_receipt().is_none());
        input.operations.stop = success;
        let context = unsafe { &*input.context };
        let in_flight = context.gate.enter().unwrap();
        assert!(input.stop_and_seal().unwrap().is_none());
        assert!(!input.health().running);
        assert!(input.health().closing);
        assert!(!input.health().quiescent);
        assert!(
            !reader.control().is_sealing(),
            "preserve already admitted final callback"
        );
        drop(in_flight);
        let stopped = input.stop_and_seal().unwrap().unwrap();
        assert!(stopped.had_started());
        assert!(stopped.health().quiescent);
        assert!(reader.control().final_stats().is_some());
        assert!(
            reader.seal_receipt().is_none(),
            "accepted PCM must still drain to raw worker"
        );
        let mut audio = [0.; 128];
        assert_eq!(
            reader
                .read_planar(&mut [&mut audio])
                .unwrap()
                .unwrap()
                .frames,
            64
        );
        assert_eq!(audio[..64], [0.25; 64]);
        let sealed = reader.seal_receipt().unwrap();
        assert_eq!(sealed.stats().received_frames, 64);
        assert!(stopped.matches_capture(&sealed));
        let (_, other) = capture_channel(CaptureConfig {
            channels: 1,
            maximum_frames: 128,
            blocks: 4,
            sample_rate: 48000,
        })
        .unwrap();
        other.control().seal();
        assert!(!stopped.matches_capture(&other.seal_receipt().unwrap()));
        assert!(input.start().is_err());
        let before = input.health();
        assert_eq!(
            unsafe {
                input_render(
                    input.context.cast(),
                    &mut flags,
                    (&timestamp as *const TimeStamp).cast(),
                    1,
                    64,
                    std::ptr::null_mut(),
                )
            },
            0
        );
        assert_eq!(
            input.health(),
            before,
            "late callback cannot alter sealed evidence"
        );
    }
    #[test]
    fn input_callback_gate_rejects_reentry_without_mutable_alias_or_hidden_loss() {
        use crate::capture::{capture_channel, CaptureConfig};
        unsafe extern "C" fn forbidden(
            _: Unit,
            _: *mut u32,
            _: *const TimeStamp,
            _: u32,
            _: u32,
            _: *mut BufferList,
        ) -> i32 {
            panic!("reentrant callback touched unit-owned buffers")
        }
        let (writer, reader) = capture_channel(CaptureConfig {
            channels: 1,
            maximum_frames: 128,
            blocks: 4,
            sample_rate: 48000,
        })
        .unwrap();
        let mut context = InputContext::new(std::ptr::null_mut(), writer);
        context.render = forbidden;
        let scope = context.gate.enter().unwrap();
        assert!(scope.exclusive);
        let timestamp = TimeStamp {
            sample_time: 0.,
            host_time: 1,
            rate_scalar: 1.,
            word_clock: 0,
            smpte: [0; 24],
            flags: 3,
            reserved: 0,
        };
        let mut flags = 0;
        assert_eq!(
            unsafe {
                input_render(
                    (&context as *const InputContext).cast_mut().cast(),
                    &mut flags,
                    (&timestamp as *const TimeStamp).cast(),
                    1,
                    64,
                    std::ptr::null_mut(),
                )
            },
            -50
        );
        assert_eq!(context.rejected_callbacks.load(Ordering::Relaxed), 1);
        assert_eq!(context.rejected_frames.load(Ordering::Relaxed), 64);
        context.gate.close();
        assert_ne!(context.gate.state(), GATE_CLOSED);
        assert_eq!(reader.stats().received_frames, 0);
        drop(scope);
        assert_eq!(context.gate.state(), GATE_CLOSED);
        assert!(context.gate.enter().is_none());
    }
    #[test]
    fn invalid_native_callback_arguments_retain_explicit_fault_and_rejected_frame_evidence() {
        use crate::capture::{capture_channel, CaptureConfig};
        let (writer, reader) = capture_channel(CaptureConfig {
            channels: 1,
            maximum_frames: 64,
            blocks: 4,
            sample_rate: 48000,
        })
        .unwrap();
        let context = InputContext::new(std::ptr::null_mut(), writer);
        let timestamp = TimeStamp {
            sample_time: 0.,
            host_time: 1,
            rate_scalar: 1.,
            word_clock: 0,
            smpte: [0; 24],
            flags: 3,
            reserved: 0,
        };
        let mut flags = 0;
        assert_eq!(
            no_heap(|| unsafe {
                input_render(
                    (&context as *const InputContext).cast_mut().cast(),
                    std::ptr::null_mut(),
                    (&timestamp as *const TimeStamp).cast(),
                    1,
                    16,
                    std::ptr::null_mut(),
                )
            }),
            -50
        );
        assert_eq!(
            no_heap(|| unsafe {
                input_render(
                    (&context as *const InputContext).cast_mut().cast(),
                    &mut flags,
                    (&timestamp as *const TimeStamp).cast(),
                    1,
                    65,
                    std::ptr::null_mut(),
                )
            }),
            -10874
        );
        assert_eq!(context.rejected_callbacks.load(Ordering::Relaxed), 2);
        assert_eq!(context.rejected_frames.load(Ordering::Relaxed), 81);
        context.gate.close();
        context.control.seal();
        let receipt = reader.seal_receipt().unwrap();
        assert_eq!(receipt.stats().received_frames, 0);
        assert_eq!(receipt.stats().device_faults, 2);
        assert_eq!(receipt.stats().last_device_error, -10874);
    }
    #[test]
    fn input_teardown_preserves_callback_owned_state_when_stop_or_quiescence_fails() {
        use crate::capture::{capture_channel, CaptureConfig};
        unsafe extern "C" fn success(_: Unit) -> i32 {
            0
        }
        unsafe extern "C" fn failure(_: Unit) -> i32 {
            -10851
        }
        unsafe extern "C" fn teardown(unit: Unit) -> i32 {
            unsafe { &*unit.cast::<AtomicUsize>() }.fetch_add(1, Ordering::Relaxed);
            0
        }
        for failed_stop in [true, false] {
            let calls = Box::new(AtomicUsize::new(0));
            let (writer, reader) = capture_channel(CaptureConfig {
                channels: 1,
                maximum_frames: 64,
                blocks: 4,
                sample_rate: 48000,
            })
            .unwrap();
            let raw = Box::into_raw(Box::new(InputContext::new(std::ptr::null_mut(), writer)));
            let active = if failed_stop {
                None
            } else {
                unsafe { &*raw }.gate.enter()
            };
            let input = CoreAudioInput {
                unit: (&*calls as *const AtomicUsize).cast_mut().cast(),
                context: raw,
                running: true,
                started: true,
                initialized: true,
                stop_failures: 0,
                last_stop_status: 0,
                permit: None,
                device_uid_fingerprint: input_device_fingerprint("simulated-only"),
                host_ticks_bits: 1e9f64.to_bits(),
                operations: InputOperations {
                    start: success,
                    stop: if failed_stop { failure } else { success },
                    uninitialize: teardown,
                    dispose: teardown,
                },
                _control_thread: PhantomData,
            };
            drop(input);
            assert_eq!(
                calls.load(Ordering::Relaxed),
                0,
                "do not dispose unit while native stop/callback quiescence is uncertain"
            );
            assert!(unsafe { &*raw }.gate.state() & GATE_CLOSED != 0);
            assert!(
                !reader.control().is_sealing(),
                "uncertain teardown cannot issue a sealed stop receipt"
            );
            // Fixture-only cleanup: fake native calls own no hardware, and this
            // test exclusively owns the simulated in-flight scope.
            drop(active);
            unsafe {
                drop(Box::from_raw(raw));
            }
        }
    }
    #[test]
    fn input_callback_uses_prepared_pcm_actual_timestamp_fields_and_fault_counters() {
        use crate::capture::{capture_channel, CaptureConfig};
        unsafe extern "C" fn fake_render(
            _unit: Unit,
            _flags: *mut u32,
            _time: *const TimeStamp,
            bus: u32,
            frames: u32,
            list: *mut BufferList,
        ) -> i32 {
            assert_eq!(bus, 1);
            let list = unsafe { &mut *list.cast::<InputBuffers>() };
            for (index, buffer) in list.buffers[..list.count as usize].iter_mut().enumerate() {
                let audio = unsafe {
                    std::slice::from_raw_parts_mut(buffer.data.cast::<f32>(), frames as usize)
                };
                audio.fill(if index == 0 { 0.2 } else { -0.3 });
            }
            0
        }
        unsafe extern "C" fn fault(
            _unit: Unit,
            _flags: *mut u32,
            _time: *const TimeStamp,
            _bus: u32,
            _frames: u32,
            _list: *mut BufferList,
        ) -> i32 {
            -10851
        }
        assert_eq!(std::mem::size_of::<TimeStamp>(), 64);
        assert_eq!(std::mem::size_of::<InputBuffers>(), 1032);
        let (writer, mut reader) = capture_channel(CaptureConfig {
            channels: 2,
            maximum_frames: 128,
            blocks: 4,
            sample_rate: 48000,
        })
        .unwrap();
        let mut context = InputContext::new(std::ptr::null_mut(), writer);
        context.render = fake_render;
        let timestamp = TimeStamp {
            sample_time: 1234.5,
            host_time: 98765,
            rate_scalar: 1.,
            word_clock: 0,
            smpte: [0; 24],
            flags: 3,
            reserved: 0,
        };
        assert_eq!(
            (&timestamp.flags as *const u32 as usize) - (&timestamp as *const TimeStamp as usize),
            56
        );
        let mut flags = 0;
        let status = no_heap(|| unsafe {
            input_render(
                (&mut context as *mut InputContext).cast(),
                &mut flags,
                (&timestamp as *const TimeStamp).cast(),
                1,
                64,
                std::ptr::null_mut(),
            )
        });
        assert_eq!(status, 0);
        let mut left = [0.; 128];
        let mut right = left;
        let block = reader
            .read_planar(&mut [&mut left, &mut right])
            .unwrap()
            .unwrap();
        assert_eq!(block.frames, 64);
        assert_eq!(block.timing.sample_time, timestamp.sample_time);
        assert_eq!(block.timing.host_time, timestamp.host_time);
        assert_eq!(block.timing.flags, 3);
        assert_eq!(left[..64], [0.2; 64]);
        assert_eq!(right[..64], [-0.3; 64]);
        context.render = fault;
        assert_eq!(
            unsafe {
                input_render(
                    (&mut context as *mut InputContext).cast(),
                    &mut flags,
                    (&timestamp as *const TimeStamp).cast(),
                    1,
                    64,
                    std::ptr::null_mut(),
                )
            },
            -10851
        );
        assert_eq!(reader.stats().device_faults, 1);
        assert_eq!(reader.stats().last_device_error, -10851);
        assert_eq!(reader.stats().received_frames, 64);
        assert!(reader
            .read_planar(&mut [&mut left, &mut right])
            .unwrap()
            .is_none());
    }

    #[test]
    fn sdk_64_bit_c_abi_layout() {
        assert_eq!(std::mem::size_of::<ComponentDescription>(), 20);
        assert_eq!(std::mem::size_of::<StreamDescription>(), 40);
        assert_eq!(std::mem::size_of::<AudioBuffer>(), 16);
        let list = BufferList {
            count: 0,
            first: AudioBuffer {
                channels: 0,
                bytes: 0,
                data: std::ptr::null_mut(),
            },
        };
        assert_eq!(
            (&list.first as *const AudioBuffer as usize) - (&list as *const BufferList as usize),
            8
        );
        assert_eq!(std::mem::size_of::<Callback>(), 16);
    }

    // Output lifecycle: mock native operations only; no device is opened.
    mod output_lifecycle {
        use super::*;
        use crate::{
            automation_write::{ReleaseReason, WriteReceipt, WriteTarget},
            project::Project,
            realtime::{channel, GraphControl},
            render::PreparedGraph,
        };
        #[repr(C)]
        struct Stereo {
            count: u32,
            buffers: [AudioBuffer; 2],
        }
        struct Buffers {
            left: Vec<f32>,
            right: Vec<f32>,
            list: Stereo,
        }
        impl Buffers {
            fn new(frames: usize) -> Self {
                let mut left = vec![0.5; frames];
                let mut right = vec![0.5; frames];
                let list = Stereo {
                    count: 2,
                    buffers: [
                        AudioBuffer {
                            channels: 1,
                            bytes: frames as u32 * 4,
                            data: left.as_mut_ptr().cast(),
                        },
                        AudioBuffer {
                            channels: 1,
                            bytes: frames as u32 * 4,
                            data: right.as_mut_ptr().cast(),
                        },
                    ],
                };
                Self { left, right, list }
            }
            fn silent(&self) -> bool {
                self.left.iter().chain(&self.right).all(|v| *v == 0.)
            }
        }
        unsafe extern "C" fn ok(_: Unit) -> i32 {
            0
        }
        unsafe extern "C" fn fail(_: Unit) -> i32 {
            -10851
        }
        fn graph() -> (Project, PreparedGraph) {
            let project = Project::open(
                &std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("../../fixtures/legacy-gain.soundroom"),
            )
            .unwrap();
            let graph = PreparedGraph::prepare_resident(
                &project,
                16 * 1024 * 1024,
                &std::sync::atomic::AtomicBool::new(false),
            )
            .unwrap();
            (project, graph)
        }
        fn mock(host: AudioHost, permit: Option<OutputPermit>) -> CoreAudioOutput {
            let mut output = CoreAudioOutput::wrap(host, std::ptr::null_mut(), permit);
            output.initialized = true;
            output.operations = OutputOperations {
                start: ok,
                stop: ok,
                uninitialize: ok,
                dispose: ok,
            };
            output
        }
        /// Invoke the real native render entry point with timestamped buffers.
        fn callback(context: *mut Context, native: &mut u64, frames: usize) -> Buffers {
            let mut buffers = Buffers::new(frames);
            let time = TimeStamp {
                sample_time: *native as f64,
                host_time: 1_000_000_000_000 + *native * 20_833,
                rate_scalar: 1.,
                word_clock: 0,
                smpte: [0; 24],
                flags: 3,
                reserved: 0,
            };
            let status = no_heap(|| unsafe {
                render(
                    context.cast(),
                    std::ptr::null_mut(),
                    (&time as *const TimeStamp).cast(),
                    0,
                    frames as u32,
                    (&mut buffers.list as *mut Stereo).cast(),
                )
            });
            assert_eq!(status, 0);
            *native += frames as u64;
            buffers
        }
        fn drain(
            control: &mut GraphControl,
            lease: crate::automation_write::WriteLease,
        ) -> Vec<WriteReceipt> {
            let mut receipts = vec![];
            while let Ok(Some(receipt)) = control.poll_write(lease) {
                receipts.push(receipt);
            }
            receipts
        }
        fn playing(graph: PreparedGraph) -> (GraphControl, AudioHost) {
            let (mut control, host) = channel();
            control.publish(Box::new(graph)).ok().unwrap();
            control.request_playing(true).unwrap();
            (control, host)
        }

        #[test]
        fn restart_invalidates_stale_anchor_handoff_and_open_write_without_heap_work() {
            let (project, graph) = graph();
            let target = graph
                .resolve_write_target(&WriteTarget::EffectAmount {
                    track_id: project.session.tracks[0].id.clone(),
                    effect_id: project.session.tracks[0].realtime_effects[0].id.clone(),
                })
                .unwrap();
            let candidate = graph.fork();
            let (mut control, host) = playing(graph);
            let mut output = mock(host, None);
            let mut native = 0;
            // Not started: no anchor, and the host is never entered.
            assert!(control.output_anchor().is_none());
            output.start().unwrap();
            callback(output.context, &mut native, 256);
            let first = control.output_anchor().expect("timed callback anchor");
            let lease = control.begin_write(7, target).unwrap();
            callback(output.context, &mut native, 128);
            let began = drain(&mut control, lease);
            let began_epoch = match began.as_slice() {
                [WriteReceipt::Began { stamp, .. }] => stamp.transport_epoch,
                other => panic!("{other:?}"),
            };
            assert_eq!(began_epoch, first.epoch);
            // A handoff prepared in this output run, still pending at Stop.
            let token = control.publish(Box::new(candidate)).ok().unwrap();
            output.stop().unwrap();
            assert!(!output.health().running);
            assert!(
                control.output_anchor().is_none(),
                "stopped output keeps no anchor"
            );
            output.start().unwrap();
            assert!(
                control.output_anchor().is_none(),
                "the pre-stop anchor stays stale until a new callback"
            );
            let audio = callback(output.context, &mut native, 128);
            assert!(audio.left.iter().all(|v| v.is_finite()));
            let telemetry = control.telemetry().unwrap();
            assert_eq!(telemetry.rejected_handoff, token, "stale output handoff");
            assert_ne!(telemetry.handoff_token, token);
            assert!(control.reclaim_retired());
            match drain(&mut control, lease).as_slice() {
                [WriteReceipt::Released {
                    reason: ReleaseReason::Discontinuity,
                    stamp,
                    ..
                }] => assert!(stamp.transport_epoch > began_epoch),
                other => panic!("{other:?}"),
            }
            let after = control.output_anchor().expect("fresh anchor after restart");
            assert!(after.epoch > first.epoch);
            assert_eq!(output.health().callback_count, 3);
            assert_eq!(output.close(), CoreAudioOutputClose::Closed);
        }

        #[test]
        fn failed_stop_is_reversible_and_exhausted_boundaries_refuse_start() {
            let (_, graph) = graph();
            let (control, host) = playing(graph);
            let mut output = mock(host, None);
            let mut native = 0;
            output.start().unwrap();
            callback(output.context, &mut native, 64);
            assert!(control.output_anchor().is_some());
            output.operations.stop = fail;
            assert!(output.stop().is_err());
            let health = output.health();
            assert!(health.running, "failed Stop leaves the unit running");
            assert_eq!((health.stop_failures, health.last_stop_status), (1, -10851));
            assert!(
                control.output_anchor().is_none(),
                "uncertain run is unavailable"
            );
            // Re-admit the still-running unit without another native start.
            output.operations.start = fail;
            output.start().unwrap();
            callback(output.context, &mut native, 64);
            assert!(control.output_anchor().is_some());
            output.operations.stop = ok;
            output.stop().unwrap();
            output.lifecycle.exhaust_boundary_for_test();
            assert!(output.start().is_err());
            assert!(!output.health().running);
            assert_eq!(output.close(), CoreAudioOutputClose::Closed);
        }

        #[test]
        fn late_and_reentrant_callbacks_render_silence_without_entering_host() {
            let (_, graph) = graph();
            let (control, host) = playing(graph);
            let mut output = mock(host, None);
            let mut native = 0;
            output.start().unwrap();
            callback(output.context, &mut native, 64);
            let context = output.context;
            let entered = control.telemetry().unwrap().callback_count;
            // An admitted callback is still inside the host.
            let active = unsafe { &*context }.gate.enter().unwrap();
            let overlap = callback(context, &mut native, 64);
            assert!(overlap.silent());
            assert_eq!(output.health().rejected_callbacks, 1);
            assert_eq!(control.telemetry().unwrap().callback_count, entered);
            // Teardown cannot prove quiescence: retained, never freed.
            assert_eq!(
                output.close(),
                CoreAudioOutputClose::Retained(CoreAudioOutputRetention::CallbackActive)
            );
            drop(active);
            // A late system callback after close finds the closed gate.
            let late = callback(context, &mut native, 64);
            assert!(late.silent());
            let context = unsafe { &*context };
            assert_eq!(context.closed_callbacks.load(Ordering::Relaxed), 1);
            assert_eq!(control.telemetry().unwrap().callback_count, entered);
            // Exclusively-owned mock unit has no OS callbacks; test-only cleanup.
            unsafe {
                drop(Box::from_raw((context as *const Context).cast_mut()));
            }
        }

        #[test]
        fn uncertain_teardowns_exhaust_a_fixed_budget_and_clean_close_releases_writes() {
            for failures in [["stop", "callback"], ["uninitialize", "dispose"]] {
                let admission = Box::leak(Box::new(OutputAdmission {
                    reserved: AtomicUsize::new(0),
                    quarantined: AtomicUsize::new(0),
                }));
                for (index, failure) in failures.into_iter().enumerate() {
                    let (_, graph) = graph();
                    let (_control, host) = playing(graph);
                    let mut output = mock(host, Some(admission.reserve().unwrap()));
                    output.start().unwrap();
                    let context = output.context;
                    let active =
                        (failure == "callback").then(|| unsafe { &*context }.gate.enter().unwrap());
                    match failure {
                        "stop" => output.operations.stop = fail,
                        "uninitialize" => output.operations.uninitialize = fail,
                        "dispose" => output.operations.dispose = fail,
                        _ => {}
                    }
                    let expected = match failure {
                        "stop" => CoreAudioOutputRetention::StopFailed(-10851),
                        "callback" => CoreAudioOutputRetention::CallbackActive,
                        "uninitialize" => CoreAudioOutputRetention::UninitializeFailed(-10851),
                        _ => CoreAudioOutputRetention::DisposeFailed(-10851),
                    };
                    assert_eq!(output.close(), CoreAudioOutputClose::Retained(expected));
                    assert_eq!(admission.quarantined.load(Ordering::Acquire), index + 1);
                    assert_eq!(admission.reserved.load(Ordering::Acquire), index + 1);
                    drop(active);
                    unsafe {
                        drop(Box::from_raw(context));
                    }
                }
                assert!(admission.reserve().is_err(), "retention budget is fixed");
            }
            // Clean close frees the permit and releases open/pending writes truthfully.
            let admission = Box::leak(Box::new(OutputAdmission {
                reserved: AtomicUsize::new(0),
                quarantined: AtomicUsize::new(0),
            }));
            let (project, graph) = graph();
            let targets: Vec<_> = (0..2)
                .map(|i| {
                    let track = &project.session.tracks[0];
                    graph
                        .resolve_write_target(&if i == 0 {
                            WriteTarget::EffectAmount {
                                track_id: track.id.clone(),
                                effect_id: track.realtime_effects[0].id.clone(),
                            }
                        } else {
                            WriteTarget::Volume {
                                track_id: track.id.clone(),
                            }
                        })
                        .unwrap()
                })
                .collect();
            let (mut control, host) = playing(graph);
            let mut output = mock(host, Some(admission.reserve().unwrap()));
            let mut native = 0;
            output.start().unwrap();
            callback(output.context, &mut native, 256);
            let open = control.begin_write(1, targets[0]).unwrap();
            callback(output.context, &mut native, 64);
            let epoch = match drain(&mut control, open).as_slice() {
                [WriteReceipt::Began { stamp, .. }] => stamp.transport_epoch,
                other => panic!("{other:?}"),
            };
            let clock = control.telemetry().unwrap().playback_clock;
            let unclaimed = control.begin_write(2, targets[1]).unwrap();
            assert_eq!(output.close(), CoreAudioOutputClose::Closed);
            assert_eq!(admission.reserved.load(Ordering::Acquire), 0);
            assert_eq!(admission.quarantined.load(Ordering::Acquire), 0);
            match drain(&mut control, open).as_slice() {
                [WriteReceipt::Released {
                    reason: ReleaseReason::Discontinuity,
                    stamp,
                    ..
                }] => {
                    assert_eq!(stamp.transport_epoch, epoch, "own epoch at teardown");
                    assert_eq!(stamp.playback_clock, clock, "actual last rendered clock");
                }
                other => panic!("{other:?}"),
            }
            assert!(matches!(
                drain(&mut control, unclaimed).as_slice(),
                [WriteReceipt::Rejected { sequence: None, .. }]
            ));
            assert!(control.output_anchor().is_none());
        }
    }
}
