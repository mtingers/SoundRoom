import Foundation
import Darwin
import SoundroomRemote

struct TUIOptions {
    var host = "127.0.0.1"
    var port = RemoteProtocol.defaultPort
    var websocketURL: URL?
    var origin: URL?
    var credential = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Application Support/Soundroom/Server/operator.json")
    var status = false
    var send: String?
    var track: String?
    var clientID: UUID?
    var clientName: String?
    var importFile: URL?
    var download: UUID?
    var output: URL?
    var help = false
    var color = ProcessInfo.processInfo.environment["NO_COLOR"] == nil && ProcessInfo.processInfo.environment["TERM"] != "dumb"
    var mouse = true
    var beats = false
    static func parse(_ arguments: [String]) throws -> TUIOptions {
        var options = TUIOptions(), index = 0
        while index < arguments.count {
            let argument = arguments[index]
            func value() throws -> String {
                index += 1; guard index < arguments.count else { throw OptionError("Missing value after \(argument).") }; return arguments[index]
            }
            switch argument {
            case "--host": options.host = try value()
            case "--wss": guard let url = URL(string: try value()) else { throw OptionError("Invalid WSS URL.") }; options.websocketURL = url
            case "--origin": guard let url = URL(string: try value()) else { throw OptionError("Invalid HTTPS origin.") }; options.origin = url
            case "--port": guard let port = UInt16(try value()), port > 0 else { throw OptionError("Port must be 1…65535.") }; options.port = port
            case "--credential": options.credential = URL(fileURLWithPath: NSString(string: try value()).expandingTildeInPath)
            case "--status": options.status = true
            case "--send": options.send = try value()
            case "--track": options.track = try value()
            case "--client-id": guard let id = UUID(uuidString: try value()) else { throw OptionError("Client ID must be a UUID.") }; options.clientID = id
            case "--client-name": options.clientName = try value(); try RemoteClientCredential.validateName(options.clientName!)
            case "--import": options.importFile = URL(fileURLWithPath: NSString(string: try value()).expandingTildeInPath)
            case "--download": guard let id = UUID(uuidString: try value()) else { throw OptionError("Artifact ID must be a UUID.") }; options.download = id
            case "--output": options.output = URL(fileURLWithPath: NSString(string: try value()).expandingTildeInPath)
            case "--beats": options.beats = true
            case "--no-color": options.color = false
            case "--no-mouse": options.mouse = false
            case "--help", "-h": options.help = true
            default: throw OptionError("Unknown option \(argument). Use --help.")
            }
            index += 1
        }
        let actions = (options.status ? 1 : 0) + (options.send == nil ? 0 : 1) + (options.importFile == nil ? 0 : 1) + (options.download == nil ? 0 : 1)
        guard actions <= 1 else { throw OptionError("Choose one of --status, --send, --import or --download.") }
        guard options.download == nil || options.output != nil else { throw OptionError("Use --output FILE with --download ID.") }
        guard options.output == nil || options.download != nil else { throw OptionError("--output applies only to --download.") }
        if options.websocketURL != nil || options.origin != nil {
            guard let url = options.websocketURL, let origin = options.origin else { throw OptionError("Use --wss URL with --origin HTTPS_ORIGIN.") }
            guard !arguments.contains("--host"), !arguments.contains("--port") else { throw OptionError("Choose --wss/--origin or --host/--port.") }
            try RemoteClient.validateWebEndpoint(websocketURL: url, origin: origin)
            options.host = url.host!; options.port = UInt16(url.port ?? 443)
        }
        return options
    }
    struct OptionError: Error, CustomStringConvertible { let description: String; init(_ text: String) { description = text } }
}

@main struct SoundroomTUI {
    static let help = """
    SoundroomTUI — control Soundroom hardware from a terminal; no client audio I/O.

    Usage: SoundroomTUI [--host HOST] [--port PORT] [--credential FILE] [--no-color] [--no-mouse] [--beats]
           SoundroomTUI --wss WSS_URL --origin HTTPS_ORIGIN [--credential FILE] [--client-name NAME]
           SoundroomTUI [connection options] --status
           SoundroomTUI [connection options] --track NAME|NUMBER|UUID --send 'mute on'
           SoundroomTUI [connection options] --import AUDIO_OR_PROJECT
           SoundroomTUI [connection options] --download ARTIFACT_UUID --output FILE

    Launch Soundroom.app first. The Mac hosts the authoritative audio server.
    For another computer, copy a paired credential securely from the host;
    it must be a regular file owned by you with permissions 0600 (chmod 600).
    Credentials are read from a file, never passed as a secret command argument.
    --client-id UUID optionally preserves authored Undo across sequential CLI calls.
    Default: ~/Library/Application Support/Soundroom/Server/operator.json
    Host defaults to 127.0.0.1; port defaults to 49152.
    --wss uses the common v3 endpoint and system certificate/hostname trust.
    Portable credentials require --client-name with the exact operator-approved name.

    B switches Time/Beats; --beats opens a musical timeline.
    :tempo BPM and :meter 4/4 set shared project timing; :grid 1/16 sets local snap.
    Beat-mode positions use bar.beat[.tick]; nudge amounts use meter beats.
    Navigation: j/k tracks, h/l cursor, counts (10l), w/b clip edges, gg/G.
    Your selection, cursor and zoom stay local. Enter explicitly seeks the server.
    Space play/pause, r record/stop, v range, V clip, y copy, p paste, dd delete.
    u your Undo, Ctrl-R your Redo, +/- zoom, : commands with Tab completion.
    Mouse: click tracks/clips/controls, Shift-click adds clips, drag selects a range.
    Wheel scrolls; Shift-wheel pans; Ctrl-wheel zooms. Tab switches pane focus.
    m mute, s solo, a arm, f effects, x split, C combine, L loop, F fit, [/] nudge.
    Mixer, effects (all parameters + presets), and routing inspectors are clickable.
    --no-color / NO_COLOR disables color; --no-mouse restores terminal selection.
    ? opens help. q / :quit detaches without stopping server audio or recording.

    Commands: play, stop, record, seek SECONDS, arm|mute|solo on|off,
      volume 0…4, pan -1…1, rename NAME, new-track NAME, split, combine, delete, move SECONDS, nudge SECONDS,
      trim START END, duplicate-track [automation], loop [START END|off], play-track,
      inputs, outputs, input ID|UID|default [CHANNEL] [stereo], output ID|UID,
      effect [add KIND|remove NUMBER|bypass NUMBER on|off|set NUMBER PARAM VALUE|presets NUMBER|preset NUMBER NAME],
      projects, load ID, save NAME, export NAME, takeover, cancel, undo, redo.
      web-pair ROLE NAME, web-clients, web-revoke ID (operator only).
    Project and export names refer to the server's managed storage.
    --status writes a JSON snapshot. --send writes the acknowledgement or error.
    --import streams local audio or a .soundroom package into server storage.
    --download saves an export artifact to a new local file without overwriting.
    """
    @MainActor static func main() async {
        do {
            let options = try TUIOptions.parse(Array(CommandLine.arguments.dropFirst()))
            if options.help { print(help); return }
            let credential = try RemoteClientCredential.load(from: options.credential)
            if options.websocketURL == nil { _ = try credential.requireNativeCredential() }
            let name = try credential.connectionName(options.clientName, nativeDefault: "Soundroom TUI")
            let connection = TUIConnection(credential: credential, clientID: options.clientID ?? UUID(), clientName: name)
            if let url = options.websocketURL, let origin = options.origin { try await connection.connect(websocketURL: url, origin: origin) }
            else { try await connection.connect(host: options.host, port: options.port) }
            if options.status {
                guard let snapshot = connection.snapshot else { throw RemoteError.connectionLost }
                let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
                let data = try encoder.encode(snapshot)
                FileHandle.standardOutput.write(data); FileHandle.standardOutput.write(Data([10]))
                connection.client.disconnect(); return
            }
            var model = TUIModel(); if options.beats { model.timebase = .beats }; if let state = connection.projection { model.update(state) }
                if let selector = options.track {
                    let selected = model.session.tracks.first { $0.id.uuidString.lowercased() == selector.lowercased() || $0.name == selector }
                    if let selected { model.trackID = selected.id }
                    else if let number = Int(selector), number >= 1 && number <= model.session.tracks.count { model.trackID = model.session.tracks[number - 1].id }
                    else { throw TUIOptions.OptionError("Unknown track '\(selector)'. Track numbers start at 1.") }
                }
            if let file = options.importFile {
                print(try await TUIFileTransfer.upload(file, connection: connection, trackID: options.track == nil ? nil : model.trackID, frame: 0))
                connection.client.disconnect(); return
            }
            if let id = options.download, let output = options.output {
                try await TUIFileTransfer.download(id, to: output, connection: connection)
                print("Downloaded " + output.path); connection.client.disconnect(); return
            }
            if let line = options.send {
                let intents = model.execute(line)
                guard !intents.isEmpty else { print(TUIRenderer.safe(model.status)); connection.client.disconnect(); return }
                for intent in intents { print(TUIRenderer.safe(try await connection.perform(intent, model: model))) }
                connection.client.disconnect(); return
            }
            let controller = TUIController(connection: connection, host: "\(options.host):\(options.port)", color: options.color, mouse: options.mouse, model: model)
            try await controller.run()
        } catch {
            let text = "SoundroomTUI: \(String(describing: error))\n"
            FileHandle.standardError.write(Data(text.utf8)); Darwin.exit(1)
        }
    }
}
