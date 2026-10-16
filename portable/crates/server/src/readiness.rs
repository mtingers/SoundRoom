//! One TLS owner waits for socket readiness or bounded outbox notifications.
//! Unix uses a private socketpair; Windows uses loopback sockets and WSAPoll.
#[cfg(unix)]
use std::os::unix::{io::AsRawFd, net::UnixStream};
#[cfg(windows)]
use std::os::windows::io::AsRawSocket;
use std::{
    cell::Cell,
    io::{self, Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    time::Instant,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Interest {
    Read,
    Write,
}
#[derive(Debug)]
pub(crate) struct SocketIo {
    tcp: TcpStream,
    blocked: Cell<Option<Interest>>,
}
impl SocketIo {
    pub(crate) fn new(tcp: TcpStream) -> Self {
        Self {
            tcp,
            blocked: Cell::new(None),
        }
    }
    pub(crate) fn reset(&self) {
        self.blocked.set(None);
    }
    pub(crate) fn blocked(&self, fallback: Interest) -> Interest {
        self.blocked.get().unwrap_or(fallback)
    }
}
impl std::ops::Deref for SocketIo {
    type Target = TcpStream;
    fn deref(&self) -> &TcpStream {
        &self.tcp
    }
}
impl Read for SocketIo {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let result = self.tcp.read(bytes);
        if result
            .as_ref()
            .is_err_and(|e| e.kind() == io::ErrorKind::WouldBlock)
        {
            self.blocked.set(Some(Interest::Read));
        }
        result
    }
}
impl Write for SocketIo {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let result = self.tcp.write(bytes);
        if result
            .as_ref()
            .is_err_and(|e| e.kind() == io::ErrorKind::WouldBlock)
        {
            self.blocked.set(Some(Interest::Write));
        }
        result
    }
    fn flush(&mut self) -> io::Result<()> {
        self.tcp.flush()
    }
}
#[cfg(unix)]
type WakeSocket = UnixStream;
#[cfg(windows)]
type WakeSocket = TcpStream;
pub(crate) struct Signal {
    socket: WakeSocket,
    #[cfg(all(test, unix))]
    waits: std::sync::atomic::AtomicU64,
    #[cfg(all(test, unix))]
    wakes: std::sync::atomic::AtomicU64,
}
impl Signal {
    #[cfg(all(test, unix))]
    pub(crate) fn counts(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering;
        (
            self.waits.load(Ordering::Relaxed),
            self.wakes.load(Ordering::Relaxed),
        )
    }
    pub(crate) fn notify(&self) {
        // A full socket already has unread notifications, so WouldBlock cannot
        // lose the wake. Do not wait under the authority's lock.
        let mut socket = &self.socket;
        for _ in 0..4 {
            match socket.write(&[1]) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                _ => break,
            }
        }
    }
}
pub(crate) struct Waiter {
    signal: Arc<Signal>,
    receiver: WakeSocket,
}
#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct Ready {
    pub read: bool,
    pub write: bool,
    pub wake: bool,
}
impl Waiter {
    pub(crate) fn new() -> io::Result<Self> {
        #[cfg(unix)]
        let (sender, receiver) = UnixStream::pair()?;
        #[cfg(windows)]
        let (sender, receiver) = {
            let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
            let sender = TcpStream::connect_timeout(
                &listener.local_addr()?,
                std::time::Duration::from_secs(1),
            )?;
            let (receiver, peer) = listener.accept()?;
            if peer != sender.local_addr()? {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "Unexpected local wake peer",
                ));
            }
            sender.set_nodelay(true)?;
            receiver.set_nodelay(true)?;
            (sender, receiver)
        };
        sender.set_nonblocking(true)?;
        receiver.set_nonblocking(true)?;
        Ok(Self {
            signal: Arc::new(Signal {
                socket: sender,
                #[cfg(all(test, unix))]
                waits: std::sync::atomic::AtomicU64::new(0),
                #[cfg(all(test, unix))]
                wakes: std::sync::atomic::AtomicU64::new(0),
            }),
            receiver,
        })
    }
    pub(crate) fn signal(&self) -> Arc<Signal> {
        self.signal.clone()
    }
    pub(crate) fn wait(
        &self,
        socket: &TcpStream,
        read: bool,
        write: bool,
        deadline: Instant,
    ) -> io::Result<Ready> {
        #[cfg(unix)]
        let descriptors = (socket.as_raw_fd(), self.receiver.as_raw_fd());
        #[cfg(windows)]
        let descriptors = (
            usize::try_from(socket.as_raw_socket()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "Socket width mismatch")
            })?,
            usize::try_from(self.receiver.as_raw_socket()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "Wake socket width mismatch")
            })?,
        );
        #[cfg(all(test, unix))]
        self.signal
            .waits
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let result = wait(descriptors.0, descriptors.1, read, write, deadline)?;
        #[cfg(all(test, unix))]
        if result.wake {
            self.signal
                .wakes
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        if result.wake {
            self.drain()?;
        }
        Ok(result)
    }
    pub(crate) fn wait_listener(
        &self,
        listener: &TcpListener,
        deadline: Instant,
    ) -> io::Result<()> {
        #[cfg(unix)]
        let descriptor = listener.as_raw_fd();
        #[cfg(windows)]
        let descriptor = usize::try_from(listener.as_raw_socket()).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "Listener socket width mismatch",
            )
        })?;
        #[cfg(unix)]
        let wake = self.receiver.as_raw_fd();
        #[cfg(windows)]
        let wake = usize::try_from(self.receiver.as_raw_socket()).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "Wake socket width mismatch")
        })?;
        let ready = wait(descriptor, wake, true, false, deadline)?;
        if ready.wake {
            self.drain()?;
        }
        Ok(())
    }
    fn drain(&self) -> io::Result<()> {
        let mut bytes = [0; 4096];
        let mut receiver = &self.receiver;
        // Bounded even when writers continue notifying during drainage.
        for _ in 0..64 {
            match receiver.read(&mut bytes) {
                Ok(0) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "Wake channel closed",
                    ))
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}
fn milliseconds(deadline: Instant) -> i32 {
    let remaining = deadline.saturating_duration_since(Instant::now());
    let ceil = remaining
        .as_millis()
        .saturating_add(u128::from(remaining.subsec_nanos() % 1_000_000 != 0));
    ceil.min(i32::MAX as u128) as i32
}
#[cfg(unix)]
fn wait(socket: i32, wake: i32, read: bool, write: bool, deadline: Instant) -> io::Result<Ready> {
    loop {
        let mut fds = [
            libc::pollfd {
                fd: socket,
                events: if read { libc::POLLIN } else { 0 } | if write { libc::POLLOUT } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: wake,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // Both handles stay owned for the entire call; two initialized entries.
        let result = unsafe { libc::poll(fds.as_mut_ptr(), 2, milliseconds(deadline)) };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if fds.iter().any(|fd| fd.revents & libc::POLLNVAL != 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid readiness descriptor",
            ));
        }
        let fault = libc::POLLERR | libc::POLLHUP;
        return Ok(Ready {
            read: fds[0].revents & (libc::POLLIN | fault) != 0,
            write: fds[0].revents & (libc::POLLOUT | fault) != 0,
            wake: fds[1].revents & (libc::POLLIN | fault) != 0,
        });
    }
}
#[cfg(windows)]
fn wait(
    socket: usize,
    wake: usize,
    read: bool,
    write: bool,
    deadline: Instant,
) -> io::Result<Ready> {
    // Matches WinSock's SOCKET / SHORT / SHORT WSAPOLLFD ABI. Constants match
    // windows-sys0.36.1 and Microsoft's WSAPoll documentation (no POLLPRI).
    #[repr(C)]
    struct PollFd {
        fd: usize,
        events: i16,
        revents: i16,
    }
    #[link(name = "Ws2_32")]
    extern "system" {
        fn WSAPoll(fds: *mut PollFd, count: u32, timeout: i32) -> i32;
        fn WSAGetLastError() -> i32;
    }
    const READ: i16 = 256;
    const WRITE: i16 = 16;
    const FAULT: i16 = 1 | 2;
    const INVALID: i16 = 4;
    loop {
        let mut fds = [
            PollFd {
                fd: socket,
                events: if read { READ } else { 0 } | if write { WRITE } else { 0 },
                revents: 0,
            },
            PollFd {
                fd: wake,
                events: READ,
                revents: 0,
            },
        ];
        let result = unsafe { WSAPoll(fds.as_mut_ptr(), 2, milliseconds(deadline)) };
        if result < 0 {
            let code = unsafe { WSAGetLastError() };
            if code == 10004 {
                continue;
            }
            return Err(io::Error::from_raw_os_error(code));
        }
        if fds.iter().any(|fd| fd.revents & INVALID != 0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid readiness socket",
            ));
        }
        return Ok(Ready {
            read: fds[0].revents & (READ | FAULT) != 0,
            write: fds[0].revents & (WRITE | FAULT) != 0,
            wake: fds[1].revents & (READ | FAULT) != 0,
        });
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn wake_interrupts_a_long_wait_and_idle_wait_uses_its_deadline() {
        let waiter = Waiter::new().unwrap();
        let signal = waiter.signal();
        let (socket, _peer) = UnixStream::pair().unwrap();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(10));
            signal.notify();
        });
        let start = Instant::now();
        let result = wait(
            socket.as_raw_fd(),
            waiter.receiver.as_raw_fd(),
            true,
            false,
            start + Duration::from_secs(2),
        )
        .unwrap();
        assert!(result.wake && !result.read);
        assert!(start.elapsed() < Duration::from_secs(1));
        thread.join().unwrap();
        waiter.drain().unwrap();
        let start = Instant::now();
        let result = wait(
            socket.as_raw_fd(),
            waiter.receiver.as_raw_fd(),
            true,
            false,
            start + Duration::from_millis(30),
        )
        .unwrap();
        assert!(!result.wake && !result.read && !result.write);
        assert!(start.elapsed() >= Duration::from_millis(25));
    }
    #[test]
    fn notification_flood_is_nonblocking_and_close_is_readiness() {
        let waiter = Waiter::new().unwrap();
        let signal = waiter.signal();
        for _ in 0..100000 {
            signal.notify();
        }
        let (socket, peer) = UnixStream::pair().unwrap();
        drop(peer);
        let result = wait(
            socket.as_raw_fd(),
            waiter.receiver.as_raw_fd(),
            true,
            false,
            Instant::now() + Duration::from_secs(1),
        )
        .unwrap();
        assert!(result.read && result.wake);
        waiter.drain().unwrap();
    }
}
