import Darwin
import Foundation

/// One owner for terminal mode, signal interception, and restoration. SIGINT/TERM/HUP
/// become main-queue shutdown requests rather than terminating in raw mode.
final class Terminal {
    private var original = termios()
    private var active = false
    private var input: DispatchSourceRead?
    private var signals: [DispatchSourceSignal] = []
    var onKey: ((Character) -> Void)?
    var onPaste: ((String) -> Void)?
    var onMouse: ((TUIMouseEvent) -> Void)?
    var mouseEnabled = true
    private var decoder = TerminalInputDecoder()
    var onQuit: (() -> Void)?
    var onResize: (() -> Void)?
    private var escapeTimer: DispatchWorkItem?
    var dimensions: (width: Int, height: Int) {
        var size = winsize()
        guard ioctl(STDOUT_FILENO, TIOCGWINSZ, &size) == 0 else { return (100, 28) }
        return (size.ws_col == 0 ? 100 : Int(size.ws_col), size.ws_row == 0 ? 28 : Int(size.ws_row))
    }
    func start() throws {
        guard isatty(STDIN_FILENO) == 1, isatty(STDOUT_FILENO) == 1 else { throw TerminalError.notTTY }
        guard tcgetattr(STDIN_FILENO, &original) == 0 else { throw TerminalError.mode }
        var raw = original
        cfmakeraw(&raw)
        // Keep signal generation for Ctrl-C; dispatch handlers perform restoration.
        raw.c_lflag |= UInt(ISIG)
        guard tcsetattr(STDIN_FILENO, TCSAFLUSH, &raw) == 0 else { throw TerminalError.mode }
        active = true
        let source = DispatchSource.makeReadSource(fileDescriptor: STDIN_FILENO, queue: .main)
        source.setEventHandler { [weak self] in self?.readInput() }
        source.resume()
        input = source
        for number in [SIGINT, SIGTERM, SIGHUP] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
            source.setEventHandler { [weak self] in self?.onQuit?() }
            source.resume()
            signals.append(source)
        }
        signal(SIGWINCH, SIG_IGN)
        let resize = DispatchSource.makeSignalSource(signal: SIGWINCH, queue: .main)
        resize.setEventHandler { [weak self] in self?.onResize?() }
        resize.resume()
        signals.append(resize)
        // The first frame is visible only after input/signal cleanup is ready.
        output("\u{1b}[?1049h\u{1b}[?25l\u{1b}[2J\u{1b}[?2004h" + (mouseEnabled ? Self.enableMouse : ""))
    }
    func stop() {
        guard active else { return }
        active = false
        escapeTimer?.cancel()
        input?.cancel()
        input = nil
        signals.forEach { $0.cancel() }
        signals = []
        _ = tcsetattr(STDIN_FILENO, TCSAFLUSH, &original)
        output(Self.disableMouse + "\u{1b}[?2004l\u{1b}[0m\u{1b}[?25h\u{1b}[?1049l")
        for number in [SIGINT, SIGTERM, SIGHUP, SIGWINCH] { signal(number, SIG_DFL) }
    }
    deinit { stop() }
    func output(_ text: String) {
        let data = Array(text.utf8)
        data.withUnsafeBytes { pointer in
            var written = 0
            while written < pointer.count {
                let amount = Darwin.write(
                    STDOUT_FILENO, pointer.baseAddress!.advanced(by: written), pointer.count - written)
                if amount <= 0 { break }
                written += amount
            }
        }
    }
    private func readInput() {
        var bytes = [UInt8](repeating: 0, count: 4096)
        let count = Darwin.read(STDIN_FILENO, &bytes, bytes.count)
        guard count > 0 else {
            onQuit?()
            return
        }
        escapeTimer?.cancel()
        deliver(decoder.feed(Array(bytes.prefix(count))))
        if decoder.hasPendingEscape {
            let timer = DispatchWorkItem { [weak self] in
                guard let self else { return }
                self.deliver(self.decoder.expireEscape())
            }
            escapeTimer = timer
            DispatchQueue.main.asyncAfter(deadline: .now() + .milliseconds(80), execute: timer)
        }
    }
    private func deliver(_ events: [TerminalInputEvent]) {
        for event in events {
            switch event {
            case .key(let key): onKey?(key)
            case .mouse(let mouse): onMouse?(mouse)
            case .paste(let text): onPaste?(text)
            }
        }
    }
    static let enableMouse = "\u{1b}[?1002h\u{1b}[?1006h"
    static let disableMouse = "\u{1b}[?1002l\u{1b}[?1006l"
    enum TerminalError: Error, CustomStringConvertible {
        case notTTY, mode
        var description: String {
            self == .notTTY
                ? "Interactive mode requires a terminal. Use --status or --send for scripts."
                : "Could not enter terminal mode."
        }
    }
}
