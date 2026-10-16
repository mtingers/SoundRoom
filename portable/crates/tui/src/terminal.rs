use std::{
    io::{self, Read, Write},
    sync::atomic::{AtomicBool, Ordering},
};
static EXIT: AtomicBool = AtomicBool::new(false);
#[cfg(unix)]
extern "C" fn signal_exit(_: libc::c_int) {
    EXIT.store(true, Ordering::Relaxed);
}
#[cfg(windows)]
unsafe extern "system" fn console_exit(_: u32) -> i32 {
    EXIT.store(true, Ordering::Relaxed);
    1
}
const ENTER: &str = "\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[?2004h";
const LEAVE: &str = "\x1b[?2004l\x1b[?1006l\x1b[?1000l\x1b[0m\x1b[?25h\x1b[?1049l";
pub struct Terminal {
    #[cfg(unix)]
    saved: libc::termios,
    #[cfg(unix)]
    signals: [libc::sighandler_t; 3],
    #[cfg(windows)]
    saved_input: u32,
    #[cfg(windows)]
    saved_output: u32,
}
impl Terminal {
    pub fn enter() -> io::Result<Self> {
        #[cfg(unix)]
        let value = unsafe {
            if libc::isatty(0) != 1 || libc::isatty(1) != 1 {
                return Err(io::Error::other("A real interactive terminal is required"));
            }
            let mut saved = std::mem::zeroed();
            if libc::tcgetattr(0, &mut saved) != 0 {
                return Err(io::Error::last_os_error());
            }
            let mut raw = saved;
            libc::cfmakeraw(&mut raw);
            raw.c_cc[libc::VMIN] = 0;
            raw.c_cc[libc::VTIME] = 0;
            if libc::tcsetattr(0, libc::TCSANOW, &raw) != 0 {
                return Err(io::Error::last_os_error());
            }
            let signals = [
                libc::signal(
                    libc::SIGTERM,
                    signal_exit as *const () as libc::sighandler_t,
                ),
                libc::signal(libc::SIGHUP, signal_exit as *const () as libc::sighandler_t),
                libc::signal(libc::SIGINT, signal_exit as *const () as libc::sighandler_t),
            ];
            EXIT.store(false, Ordering::Relaxed);
            Self { saved, signals }
        };
        #[cfg(windows)]
        let value = unsafe {
            let mut input = 0;
            let mut output = 0;
            if GetConsoleMode(GetStdHandle(-10), &mut input) == 0
                || GetConsoleMode(GetStdHandle(-11), &mut output) == 0
            {
                return Err(io::Error::last_os_error());
            }
            if SetConsoleMode(GetStdHandle(-10), (input & !0x0007) | 0x0200) == 0
                || SetConsoleMode(GetStdHandle(-11), output | 0x0004) == 0
            {
                SetConsoleMode(GetStdHandle(-10), input);
                return Err(io::Error::last_os_error());
            }
            SetConsoleCtrlHandler(Some(console_exit), 1);
            Self {
                saved_input: input,
                saved_output: output,
            }
        };
        print!("{ENTER}");
        io::stdout().flush()?;
        Ok(value)
    }
    pub fn exit_requested(&self) -> bool {
        EXIT.load(Ordering::Relaxed)
    }
    pub fn size(&self) -> (usize, usize) {
        #[cfg(unix)]
        unsafe {
            let mut s: libc::winsize = std::mem::zeroed();
            if libc::ioctl(1, libc::TIOCGWINSZ, &mut s) == 0 {
                return (
                    (s.ws_col as usize).clamp(20, 500),
                    (s.ws_row as usize).clamp(8, 200),
                );
            }
        }
        #[cfg(windows)]
        unsafe {
            let mut s: ScreenInfo = std::mem::zeroed();
            if GetConsoleScreenBufferInfo(GetStdHandle(-11), &mut s) != 0 {
                return (
                    (s.window.right - s.window.left + 1).clamp(20, 500) as usize,
                    (s.window.bottom - s.window.top + 1).clamp(8, 200) as usize,
                );
            }
        }
        (100, 30)
    }
    pub fn read(&self) -> io::Result<Vec<u8>> {
        #[cfg(unix)]
        unsafe {
            let mut fd = libc::pollfd {
                fd: 0,
                events: libc::POLLIN,
                revents: 0,
            };
            let ready = libc::poll(&mut fd, 1, 30);
            if ready < 0 {
                let error = io::Error::last_os_error();
                if error.kind() == io::ErrorKind::Interrupted {
                    return Ok(vec![]);
                }
                return Err(error);
            }
            if ready == 0 {
                return Ok(vec![]);
            }
        }
        #[cfg(windows)]
        unsafe {
            if WaitForSingleObject(GetStdHandle(-10), 30) != 0 {
                return Ok(vec![]);
            }
        }
        let mut bytes = [0u8; 4096];
        let n = io::stdin().read(&mut bytes)?;
        Ok(bytes[..n].to_vec())
    }
}
impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = io::stdout().write_all(LEAVE.as_bytes());
        let _ = io::stdout().flush();
        #[cfg(unix)]
        unsafe {
            libc::tcsetattr(0, libc::TCSANOW, &self.saved);
            for (signal, handler) in [libc::SIGTERM, libc::SIGHUP, libc::SIGINT]
                .into_iter()
                .zip(self.signals)
            {
                libc::signal(signal, handler);
            }
        }
        #[cfg(windows)]
        unsafe {
            SetConsoleCtrlHandler(Some(console_exit), 0);
            SetConsoleMode(GetStdHandle(-10), self.saved_input);
            SetConsoleMode(GetStdHandle(-11), self.saved_output);
        }
    }
}
#[cfg(windows)]
#[repr(C)]
struct Coord {
    x: i16,
    y: i16,
}
#[cfg(windows)]
#[repr(C)]
struct Rect {
    left: i16,
    top: i16,
    right: i16,
    bottom: i16,
}
#[cfg(windows)]
#[repr(C)]
struct ScreenInfo {
    size: Coord,
    cursor: Coord,
    attributes: u16,
    window: Rect,
    max_size: Coord,
}
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn SetConsoleCtrlHandler(
        handler: Option<unsafe extern "system" fn(u32) -> i32>,
        add: i32,
    ) -> i32;
    fn GetStdHandle(n: i32) -> *mut std::ffi::c_void;
    fn GetConsoleMode(h: *mut std::ffi::c_void, m: *mut u32) -> i32;
    fn SetConsoleMode(h: *mut std::ffi::c_void, m: u32) -> i32;
    fn GetConsoleScreenBufferInfo(h: *mut std::ffi::c_void, s: *mut ScreenInfo) -> i32;
    fn WaitForSingleObject(h: *mut std::ffi::c_void, t: u32) -> u32;
}
