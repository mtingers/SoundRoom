import CoreMIDI
import SoundroomCore
import SoundroomRemote
import XCTest

@testable import Soundroom

/// The native GUI client driving the portable Rust host over verified loopback
/// WSS. The host uses its test-only simulated callback: no audio device opens.
/// Set SOUNDROOM_RUST_SERVER to the built `soundroom-server` executable.
@MainActor final class RustServerGUITests: XCTestCase {
    private var directory: URL!
    private var store: SessionStore!
    private var remote: RemoteGUIController!

    override func setUp() async throws {
        guard let server = ProcessInfo.processInfo.environment["SOUNDROOM_RUST_SERVER"] else {
            throw XCTSkip("Set SOUNDROOM_RUST_SERVER to run native GUI ↔ Rust host acceptance")
        }
        let environment = ProcessInfo.processInfo.environment
        directory =
            environment["SOUNDROOM_RUST_TEST_DIRECTORY"].map { URL(fileURLWithPath: $0) }
            ?? FileManager.default.temporaryDirectory.appendingPathComponent("SoundroomRust-\(UUID().uuidString)")
        let port = environment["SOUNDROOM_RUST_TEST_PORT"].flatMap(UInt16.init) ?? UInt16.random(in: 50_000...59_000)
        // Hardware tests open the real default output (silent unless a test plays audio).
        if name.contains("HourLong") && environment["SOUNDROOM_RUST_LONG"] != "1" {
            throw XCTSkip("Set SOUNDROOM_RUST_LONG=1 to measure hour-long seek on the Rust host")
        }
        if name.contains("Microphone") && environment["SOUNDROOM_RUST_MIC"] != "1" {
            throw XCTSkip("Set SOUNDROOM_RUST_MIC=1 to record from the real default input")
        }
        let hardware = name.contains("Hardware")
        if hardware && environment["SOUNDROOM_RUST_HARDWARE"] != "1" {
            throw XCTSkip("Set SOUNDROOM_RUST_HARDWARE=1 to open real Core Audio devices")
        }
        let configuration = ServerLaunchConfiguration(
            arguments: [
                "Soundroom", "--rust-server", server,
                "--rust-data-directory", directory.path, "--rust-port", String(port),
            ] + (hardware ? [] : ["--rust-server-flag", "--simulated-callback"]))
        store = SessionStore(headless: true)
        remote = try await ServerBootstrap.connectRust(
            store: store, configuration: configuration, executable: URL(fileURLWithPath: server))
        try await until("authenticated snapshot") { self.remote.connected && self.remote.authoritativeSession != nil }
    }
    override func tearDown() async throws {
        if let remote {
            if store.isPlaying || store.isRecording { remote.route(.init(operation: "transport.stop")) }
            remote.route(.init(operation: "server.shutdown"))
            let port = remote.port
            let deadline = Date().addingTimeInterval(3)
            while ServerBootstrap.loopbackAccepts(port: port) && Date() < deadline {
                try? await Task.sleep(for: .milliseconds(50))
            }
            remote.disconnect()
            // A host that refused or missed the shutdown never outlives its test.
            if let child = ServerBootstrap.launchedRustServer, child.isRunning {
                child.terminate()
                child.waitUntilExit()
            }
        }
        if let directory, ProcessInfo.processInfo.environment["SOUNDROOM_RUST_TEST_DIRECTORY"] == nil {
            try? FileManager.default.removeItem(at: directory)
        }
    }
    private func until(_ label: String, timeout: Double = 10, _ done: @escaping () -> Bool) async throws {
        let deadline = Date().addingTimeInterval(timeout)
        while !done() {
            guard Date() < deadline else {
                let log =
                    (try? String(contentsOf: directory.appendingPathComponent("server.log"), encoding: .utf8)) ?? ""
                XCTFail(
                    "Timed out: \(label) · devices \(store.devices.count) out \(store.outputDevice) · status: \(store.status) · state: \(String(describing: remote?.state)) connected=\(remote?.connected == true) session=\(remote?.authoritativeSession != nil) · error: \(remote?.lastError ?? "") · tracks: \(remote?.authoritativeSession?.tracks.map { "\($0.name)=\($0.gain)" } ?? []) · canUndo: \(remote?.canUndo as Any) · log: \(log.suffix(800))"
                )
                throw XCTSkip(label)
            }
            try await Task.sleep(for: .milliseconds(10))
        }
    }

    func testEditsUndoRedoAndTransportThroughTheRustHost() async throws {
        let before = store.session.tracks.count
        store.addTrack()
        try await until("added track is authoritative") { self.remote.authoritativeSession?.tracks.count == before + 1 }
        let id = try XCTUnwrap(store.session.tracks.last?.id)
        store.renameTrack(id, to: "Rust vocal")
        try await until("rename") { self.remote.authoritativeSession?.tracks.last?.name == "Rust vocal" }
        store.updateTrack(id, actionName: "Gain") { $0.gain = 0.5 }
        try await until("gain") { self.remote.authoritativeSession?.tracks.last?.gain == 0.5 }
        store.undo()
        try await until("undo gain") { self.remote.authoritativeSession?.tracks.last?.gain == 1 }
        store.redo()
        try await until("redo gain") { self.remote.authoritativeSession?.tracks.last?.gain == 0.5 }
        store.togglePlay()
        try await until("callback-confirmed play") { self.store.isPlaying }
        store.togglePlay()
        try await until("stop") { !self.store.isPlaying }
    }

    func testImportedAudioAndEditsSurviveAServerRestart() async throws {
        let wav = directory.appendingPathComponent("tone.wav")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let tone = AudioSamples(channels: [(0..<48_000).map { Float(sin(Double($0) * 0.05)) * 0.2 }])
        try WaveFile.encode(tone, float: true).write(to: wav)
        store.addTrack()
        try await until("track") { self.remote.authoritativeSession?.tracks.count == 1 }
        store.selectedTrack = store.session.tracks.first?.id
        remote.importAudio(urls: [wav])
        try await until("imported clip", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.count == 1
        }
        let id = try XCTUnwrap(store.session.tracks.first?.id)
        store.renameTrack(id, to: "Kept")
        try await until("rename") { self.remote.authoritativeSession?.tracks.first?.name == "Kept" }
        try await Task.sleep(for: .milliseconds(1500))  // debounced autosave
        remote.route(.init(operation: "server.shutdown"))
        try await until("server stopped") { !ServerBootstrap.loopbackAccepts(port: self.remote.port) }
        remote.disconnect()
        let server = try XCTUnwrap(ProcessInfo.processInfo.environment["SOUNDROOM_RUST_SERVER"])
        let configuration = ServerLaunchConfiguration(arguments: [
            "Soundroom", "--rust-server", server,
            "--rust-data-directory", directory.path, "--rust-port", String(remote.port), "--rust-server-flag",
            "--simulated-callback",
        ])
        store = SessionStore(headless: true)
        remote = try await ServerBootstrap.connectRust(
            store: store, configuration: configuration, executable: URL(fileURLWithPath: server))
        try await until("restored snapshot") { self.remote.authoritativeSession?.tracks.first?.name == "Kept" }
        let clip = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first)
        XCTAssertEqual(clip.length, 48_000)
        store.togglePlay()
        try await until("restored media plays") { self.store.isPlaying }
        store.togglePlay()
    }

    func testCompressedAndResampledImportIsDecodedByTheRustHost() async throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let wav = directory.appendingPathComponent("tone44.wav")
        let m4a = directory.appendingPathComponent("tone.m4a")
        // One second, resampled to 44.1 kHz AAC by the system tool.
        let tone = AudioSamples(channels: [(0..<48_000).map { Float(sin(Double($0) * 0.05)) * 0.2 }])
        try WaveFile.encode(tone, float: true).write(to: wav)
        let convert = Process()
        convert.executableURL = URL(fileURLWithPath: "/usr/bin/afconvert")
        convert.arguments = ["-f", "m4af", "-d", "aac@44100", wav.path, m4a.path]
        try convert.run()
        convert.waitUntilExit()
        XCTAssertEqual(convert.terminationStatus, 0)
        remote.importAudio(urls: [m4a])
        try await until("decoded import", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.count == 1
        }
        let clip = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first)
        // ≈ one second at 48 kHz (AAC priming/padding allowed).
        XCTAssertEqual(Double(clip.length), 48_000, accuracy: 3_000)
    }

    func testLoopPlaybackStaysInsideTheCycleAndWraps() async throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let wav = directory.appendingPathComponent("loop.wav")
        try WaveFile.encode(
            AudioSamples(channels: [(0..<96_000).map { Float(sin(Double($0) * 0.05)) * 0.2 }]), float: true
        ).write(to: wav)
        remote.importAudio(urls: [wav])
        try await until("clip", timeout: 20) { self.remote.authoritativeSession?.tracks.first?.clips.count == 1 }
        store.setLoopRange(12_000..<24_000)
        store.togglePlay()
        try await until("playing") { self.store.isPlaying }
        var frames: [Int] = []
        for _ in 0..<150 {
            frames.append(store.playbackPosition.frame)
            try await Task.sleep(for: .milliseconds(10))
        }
        store.togglePlay()
        let moving = frames.filter { $0 > 0 }
        XCTAssertFalse(moving.isEmpty)
        XCTAssertTrue(
            moving.allSatisfy { (12_000..<24_000).contains($0) }, "\(moving.min() ?? -1)...\(moving.max() ?? -1)")
        XCTAssertTrue(zip(moving, moving.dropFirst()).contains { $1 < $0 }, "a quarter-second cycle wraps within 1.5 s")
    }

    func testRecordingAnArmedTrackCommitsOneAlignedTake() async throws {
        store.addTrack()
        try await until("track") { self.remote.authoritativeSession?.tracks.count == 1 }
        let id = try XCTUnwrap(store.session.tracks.first?.id)
        store.updateTrack(id, actionName: "Arm track") { $0.armed = true }
        try await until("armed") { self.remote.authoritativeSession?.tracks.first?.armed == true }
        // The fixture supplies simulated input; avoid a microphone permission
        // prompt from xctest. Finish still exercises the real GUI toggle.
        remote.route(.init(operation: "transport.record", context: store.remoteContext))
        try await until("recording", timeout: 15) { self.store.isRecording }
        try await Task.sleep(for: .milliseconds(1200))
        // Exercise the same toggle used by the toolbar and R shortcut.
        let stopRequested = ProcessInfo.processInfo.systemUptime
        store.record()
        try await until("recording stop visible before take publication", timeout: 2) {
            !self.store.isRecording && !self.store.isPlaying
        }
        XCTAssertLessThan(
            ProcessInfo.processInfo.systemUptime - stopRequested, 1, "Stop must not wait for archive finalization")
        try await until("take committed", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.count == 1
        }
        try await until("transport stopped") { !self.store.isRecording && !self.store.isPlaying }
        let clip = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first)
        // About 1.2 s of the simulated 440 Hz input, placed near the record start.
        XCTAssertGreaterThan(clip.length, 36_000)
        XCTAssertLessThan(clip.length, 96_000, "Capture must not continue for seconds after Stop")
        XCTAssertLessThan(clip.start, 24_000)
        // One authored action: Undo removes the whole take.
        store.undo()
        try await until("undo take") { self.remote.authoritativeSession?.tracks.first?.clips.isEmpty == true }
    }

    private func importTone(frames: Int = 48_000) async throws -> UUID {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let wav = directory.appendingPathComponent("tone-\(UUID().uuidString).wav")
        try WaveFile.encode(
            AudioSamples(channels: [(0..<frames).map { Float(sin(Double($0) * 0.05)) * 0.2 }]), float: true
        ).write(to: wav)
        remote.importAudio(urls: [wav])
        try await until("imported clip", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.count == 1
        }
        return try XCTUnwrap(remote.authoritativeSession?.tracks.first?.id)
    }

    func testSelectionEffectsAndCombineRenderOnTheRustHost() async throws {
        let track = try await importTone()
        store.selectedTrack = track
        store.selection = EditSelection(trackID: track, range: 12_000..<24_000)
        store.effect = .reverse
        store.effectParameters = [:]
        store.applyEffect()
        try await until("reversed selection", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.count == 3
        }
        let clips = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips).sorted { $0.start < $1.start }
        XCTAssertEqual(clips.map(\.start), [0, 12_000, 24_000])
        XCTAssertEqual(clips.map(\.length), [12_000, 12_000, 24_000])
        XCTAssertTrue(clips[1].name.hasSuffix("Reverse"), clips[1].name)
        // A parameterized realtime effect with its tail extends past the selection.
        store.selection = EditSelection(trackID: track, range: 30_000..<36_000)
        store.effect = .echo
        store.effectParameters = ["time": 100, "feedback": 0.3, "mix": 0.5, "damping": 12_000]
        store.includeEffectTail = true
        store.applyEffect()
        try await until("echo", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.contains { $0.name.hasSuffix("Echo") } == true
        }
        let echo = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first { $0.name.hasSuffix("Echo") })
        XCTAssertEqual(echo.start, 30_000)
        XCTAssertGreaterThan(echo.length, 6_000)
        store.undo()
        try await until("undo echo") { self.remote.authoritativeSession?.tracks.first?.clips.count == 3 }
        // Replacing a whole clip remembers it; a span across clips keeps its dry render.
        let reversed = try XCTUnwrap(
            remote.authoritativeSession?.tracks.first?.clips.first { $0.name.hasSuffix("Reverse") })
        XCTAssertEqual(reversed.previousStates?.last?.operation, "Reverse")
        store.selection = EditSelection(trackID: track, range: 6_000..<18_000)
        store.effect = .gain
        store.effectParameters = [:]
        store.includeEffectTail = false
        store.applyEffect()
        try await until("gain across clips", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.contains { $0.name.hasSuffix("Gain") } == true
        }
        let spanning = try XCTUnwrap(
            remote.authoritativeSession?.tracks.first?.clips.first { $0.name.hasSuffix("Gain") })
        let original = try XCTUnwrap(spanning.previousStates?.last)
        XCTAssertEqual(original.name, "Original selection")
        XCTAssertEqual(original.length, 12_000)
        store.undo()
        try await until("undo gain") { self.remote.authoritativeSession?.tracks.first?.clips.count == 3 }
        store.selection = nil
        store.selectedClips = Set(remote.authoritativeSession?.tracks.first?.clips.map(\.id) ?? [])
        store.combineClips()
        try await until("combined", timeout: 20) { self.remote.authoritativeSession?.tracks.first?.clips.count == 1 }
        let combined = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first)
        XCTAssertEqual(combined.start, 0)
        XCTAssertEqual(combined.length, 48_000)
        XCTAssertFalse(combined.previousStates?.isEmpty ?? true)
        store.undo()
        try await until("undo combine") { self.remote.authoritativeSession?.tracks.first?.clips.count == 3 }
    }

    func testPitchEditorAnalyzesTargetsAndAppliesOnTheRustHost() async throws {
        let track = try await importTone()
        store.selectedTrack = track
        let editor = store.pitchEditor
        editor.synchronize(track: store.session.tracks.first, effectID: nil, range: 0..<48_000, assets: store.assets)
        editor.analyze()
        try await until("contour", timeout: 20) { !editor.before.isEmpty }
        XCTAssertTrue(editor.before.contains(where: \.isVoiced), editor.error ?? "")
        XCTAssertFalse(editor.visibleRegions.isEmpty)
        XCTAssertTrue(editor.targetsAreDraft)
        // Choosing a preset authors an edits-only pitch insert through ordinary track edits.
        editor.resetTargets(.tight)
        try await until("authored targets", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.realtimeEffects.first { $0.kind == .pitchCorrection }?
                .pitchEdits.isEmpty == false
        }
        let effect = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.realtimeEffects.first)
        editor.synchronize(
            track: store.session.tracks.first, effectID: effect.id, range: 0..<48_000, assets: store.assets)
        editor.render(preview: false)
        try await until("measured output", timeout: 20) { editor.after != nil }
        editor.bake()
        try await until("baked", timeout: 30) {
            self.remote.authoritativeSession?.tracks.first?.realtimeEffects.isEmpty == true
        }
        XCTAssertTrue(remote.authoritativeSession?.tracks.first?.clips.contains { $0.name.hasSuffix("Pitch") } == true)
        store.undo()
        try await until("undo bake") { self.remote.authoritativeSession?.tracks.first?.realtimeEffects.count == 1 }
        store.selection = EditSelection(trackID: track, range: 0..<48_000)
        store.applyManualPitchTune(
            trackID: track, selectionRange: 0..<48_000, region: 12_000..<24_000, target: 67, parameters: [:])
        try await until("manual tune", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.contains { $0.name.hasSuffix("Tuned") } == true
        }
        let tuned = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first { $0.name.hasSuffix("Tuned") })
        XCTAssertEqual(tuned.start, 12_000)
        XCTAssertEqual(tuned.length, 12_000)
    }

    private func reply(_ intent: RemoteEditorIntent) async throws -> RemoteReply {
        var result: RemoteReply?
        remote.route(intent) { result = $0 }
        try await until("\(intent.operation) reply") { result != nil }
        return result!
    }

    func testAuditionsAndLiveNotesPlayThroughTheRustHostPreviewMixer() async throws {
        let track = try await importTone()
        // Drummer loop preview: playing state arrives as an owned audition event.
        store.drumDesigner.preview(DrumPattern())
        try await until("drums previewing", timeout: 15) { self.store.drumDesigner.previewing }
        store.drumDesigner.stop()
        try await until("drums stopped") { !self.store.drumDesigner.previewing }
        // Designer loop preview, then live notes on the same lease.
        let pattern = SoundDesignPattern(notes: [.init(pitch: 69, startBeat: 0, durationBeats: 1)], bpm: 120, beats: 2)
        store.previewSoundDesign(pattern)
        try await until("designer previewing", timeout: 15) { self.store.designerPreviewing }
        let data = try JSONEncoder().encode(pattern)
        let on = try await reply(
            .init(operation: "designer.noteOn", integers: ["pitch": 64], numbers: ["velocity": 0.9], payload: data))
        XCTAssertTrue(on.accepted, on.message ?? "")
        let off = try await reply(.init(operation: "designer.noteOff", integers: ["pitch": 64]))
        XCTAssertTrue(off.accepted, off.message ?? "")
        store.stopSoundDesignPreview()
        try await until("designer stopped") { !self.store.designerPreviewing }
        // Selection effect preview renders and plays without authoring anything.
        let revision = remote.revision
        store.selectedTrack = track
        store.selection = EditSelection(trackID: track, range: 0..<24_000)
        let effect = try await reply(
            .init(
                operation: "effects.preview", context: store.remoteContext, numbers: ["amount": 0.5],
                strings: ["kind": "reverse"]))
        XCTAssertTrue(effect.accepted, effect.message ?? "")
        XCTAssertEqual(remote.revision, revision)
        // Single-track audition runs the transport with only that track audible.
        let audition = try await reply(
            .init(operation: "transport.audition", context: store.remoteContext, strings: ["trackID": track.uuidString])
        )
        XCTAssertTrue(audition.accepted, audition.message ?? "")
        try await until("auditioning") { self.store.isPlaying }
        store.togglePlay()
        try await until("stopped") { !self.store.isPlaying }
    }

    func testMasteringReviewAnalyzesProposesAppliesAndUndoesThroughTheRustHost() async throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let wav = directory.appendingPathComponent("master.wav")
        let tone = AudioSamples(channels: [
            (0..<96_000).map { Float(sin(Double($0) * 0.05)) * 0.3 },
            (0..<96_000).map { Float(sin(Double($0) * 0.31)) * 0.1 },
        ])
        try WaveFile.encode(tone, float: true).write(to: wav)
        remote.importAudio(urls: [wav])
        try await until("clip", timeout: 20) { self.remote.authoritativeSession?.tracks.first?.clips.count == 1 }
        // The Master assistant's own review controller, exactly as the GUI drives it.
        let review = MasteringReviewController(owner: store)
        review.analyze()
        try await until("analysis", timeout: 30) {
            self.remote.authoritativeSession?.masteringAnalysis != nil && !review.working
        }
        await review.waitForSourceVerification()
        try await until("verified review") { review.ready }
        let document = try XCTUnwrap(remote.authoritativeSession)
        let report = try XCTUnwrap(document.masteringAnalysis)
        XCTAssertTrue(report.matches(document), "the Rust fingerprint must equal the native re-encoding")
        XCTAssertEqual(report.analyzedFrames, 96_000)
        XCTAssertFalse(report.isSilent)
        XCTAssertEqual(report.sections.count, 1)
        XCTAssertLessThan(report.integratedLUFS, -5)
        XCTAssertGreaterThan(report.integratedLUFS, -40)
        // The host's proposal is the native proposal for the stored analysis.
        var reply: RemoteReply?
        remote.route(.init(operation: "master.proposal", numbers: ["targetLUFS": -14, "intensity": 0.65])) {
            reply = $0
        }
        try await until("proposal") { reply != nil }
        XCTAssertEqual(reply?.accepted, true, reply?.message ?? "")
        let proposal = try JSONDecoder().decode([RealtimeEffect].self, from: XCTUnwrap(reply?.payload))
        let local = report.proposal(targetLUFS: -14, intensity: 0.65)
        let neutral = UUID()
        XCTAssertEqual(
            proposal.map {
                var e = $0
                e.id = neutral
                return e
            },
            local.map {
                var e = $0
                e.id = neutral
                return e
            })
        XCTAssertEqual(proposal.map(\.kind), [.equalizer, .resonance, .warmth, .limiter])
        let opening = document.masterTrack.realtimeEffects
        review.apply()
        try await until("applied") { review.completedApplyID != nil }
        XCTAssertEqual(remote.authoritativeSession?.masterTrack.realtimeEffects, review.proposed)
        XCTAssertEqual(remote.authoritativeSession?.masterTrack.realtimeEffects.count, 4)
        // One authored action: Undo restores the opening Master rack.
        store.undo()
        try await until("undo apply") { self.remote.authoritativeSession?.masterTrack.realtimeEffects == opening }
        XCTAssertNotNil(remote.authoritativeSession?.masteringAnalysis)
        store.undo()
        try await until("undo analysis") { self.remote.authoritativeSession?.masteringAnalysis == nil }
    }

    func testHardwareOutputSwitchesToAnotherDeviceAndBack() async throws {
        remote.route(.init(operation: "devices.refresh"))
        try await until("devices") { !self.store.devices.isEmpty && self.store.outputDevice != 0 }
        let original = store.outputDevice
        let outputs = store.devices.filter { $0.outputChannels > 0 && $0.id != original }
        // Prefer a silent virtual bridge for the round trip.
        guard let other = outputs.first(where: { $0.name.contains("Audio Bridge") }) ?? outputs.first else {
            throw XCTSkip("Only one output device is available")
        }
        var reply: RemoteReply?
        remote.send(.setDevices(inputID: nil, outputID: other.id)) { reply = $0 }
        try await until("switch reply") { reply != nil }
        XCTAssertTrue(reply!.accepted, reply!.message ?? "")
        try await until("output switched") { self.store.outputDevice == other.id }
        // Callback-confirmed Play proves the reopened unit renders on the new device.
        store.addTrack()
        try await until("track") { self.remote.authoritativeSession?.tracks.count == 1 }
        store.togglePlay()
        try await until("playing on \(other.name)") { self.store.isPlaying }
        store.togglePlay()
        try await until("stopped") { !self.store.isPlaying }
        reply = nil
        remote.send(.setDevices(inputID: nil, outputID: original)) { reply = $0 }
        try await until("switch back reply") { reply != nil }
        XCTAssertTrue(reply!.accepted, reply!.message ?? "")
        try await until("output restored") { self.store.outputDevice == original }
        store.togglePlay()
        try await until("playing on the original device") { self.store.isPlaying }
        store.togglePlay()
    }

    func testPunchRecordingWithPreRollCountInAndMetronomeFinishesItself() async throws {
        store.addTrack()
        try await until("track") { self.remote.authoritativeSession?.tracks.count == 1 }
        let id = try XCTUnwrap(store.session.tracks.first?.id)
        store.updateTrack(id, actionName: "Arm track") { $0.armed = true }
        try await until("armed") { self.remote.authoritativeSession?.tracks.first?.armed == true }
        store.selectedTrack = id
        store.selection = EditSelection(trackID: id, range: 24_000..<48_000)
        store.punchRecording = true
        store.preRollSeconds = 0.25
        store.countInBars = 1
        store.recordingMetronome = true
        store.record()
        // One 4/4 bar at 120 BPM of count-in, a quarter second of pre-roll, then
        // the half-second punch; the host stops and commits on its own.
        try await until("punch take", timeout: 30) { self.remote.authoritativeSession?.tracks.first?.clips.count == 1 }
        try await until("transport stopped") { !self.store.isRecording && !self.store.isPlaying }
        let clip = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first)
        XCTAssertEqual(clip.start, 24_000)
        XCTAssertEqual(clip.length, 24_000)
        XCTAssertEqual(remote.authoritativeSession?.tracks.count, 1, "the metronome is never authored")
    }

    func testSelectedInsertReceivesLiveMetersWhilePlaying() async throws {
        let track = try await importTone(frames: 96_000)
        store.selectedTrack = track
        // Selecting the new insert subscribes the GUI to its meters.
        store.addRealtimeEffect(.compressor)
        try await until("insert") { self.remote.authoritativeSession?.tracks.first?.realtimeEffects.count == 1 }
        let id = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.realtimeEffects.first?.id)
        store.changeRealtimeEffect(id, name: "Threshold") { $0.parameters["threshold"] = -40 }
        try await until("threshold") {
            self.remote.authoritativeSession?.tracks.first?.realtimeEffects.first?.parameters["threshold"] == -40
        }
        store.selectedRealtimeEffect = nil
        store.selectedRealtimeEffect = id
        store.togglePlay()
        try await until("playing") { self.store.isPlaying }
        try await until("insert meters", timeout: 5) { (self.remote.effectMeters[id]?.outputPeak ?? 0) > 0 }
        let meter = try XCTUnwrap(remote.effectMeters[id])
        XCTAssertGreaterThan(meter.inputPeak, 0.15)
        XCTAssertGreaterThan(meter.gainReductionDB, 3, "a -40 dB threshold compresses the -14 dB tone")
        store.togglePlay()
        try await until("stopped") { !self.store.isPlaying }
        // A limiter also reports true peak and momentary/short-term loudness.
        store.addRealtimeEffect(.limiter)
        try await until("limiter") { self.remote.authoritativeSession?.tracks.first?.realtimeEffects.count == 2 }
        let limiter = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.realtimeEffects.last?.id)
        store.selectedRealtimeEffect = nil
        store.selectedRealtimeEffect = limiter
        store.togglePlay()
        try await until("loudness", timeout: 5) { (self.remote.effectMeters[limiter]?.momentaryLUFS ?? -120) > -60 }
        let reading = try XCTUnwrap(remote.effectMeters[limiter])
        XCTAssertGreaterThan(reading.truePeak, 0.01)
        XCTAssertLessThan(reading.momentaryLUFS, 0)
        store.togglePlay()
    }

    func testLocalProjectPackageUploadsAndOpensOnTheRustHost() async throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        var session = Session()
        let asset = UUID()
        var track = Track(name: "From disk", color: 2)
        track.clips = [Clip(assetID: asset, name: "Tone", start: 4_800, length: 24_000)]
        session.tracks = [track]
        let audio = AudioSamples(channels: [(0..<24_000).map { Float(sin(Double($0) * 0.05)) * 0.2 }])
        let package = directory.appendingPathComponent("Local Song.soundroom")
        _ = try ProjectPersistence.saveSynchronously(session: session, assets: [asset: audio], to: package)
        remote.openProject(url: package)
        try await until("opened package", timeout: 30) {
            self.remote.authoritativeSession?.tracks.first?.name == "From disk"
        }
        let clip = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first)
        XCTAssertEqual(clip.start, 4_800)
        XCTAssertEqual(clip.length, 24_000)
        store.togglePlay()
        try await until("opened media plays") { self.store.isPlaying }
        store.togglePlay()
    }

    func testServerMIDISourcePlaysTheLiveDesignerInstrument() async throws {
        var client = MIDIClientRef()
        var source = MIDIEndpointRef()
        guard MIDIClientCreate("Soundroom test" as CFString, nil, nil, &client) == noErr,
            MIDISourceCreate(client, "Soundroom Test Keys" as CFString, &source) == noErr
        else {
            throw XCTSkip("CoreMIDI is unavailable")
        }
        defer {
            MIDIEndpointDispose(source)
            MIDIClientDispose(client)
        }
        let listed = try await reply(.init(operation: "designer.midiSources"))
        XCTAssertTrue(listed.accepted, listed.message ?? "")
        let sources = try JSONDecoder().decode([RemoteMIDISource].self, from: try XCTUnwrap(listed.payload))
        let keys = try XCTUnwrap(sources.first { $0.name == "Soundroom Test Keys" }, "\(sources)")
        let pattern = SoundDesignPattern(notes: [.init(pitch: 69, startBeat: 0, durationBeats: 1)], bpm: 120, beats: 2)
        let connected = try await reply(
            .init(
                operation: "designer.midiConnect", integers: ["sourceID": Int(keys.id)],
                payload: try JSONEncoder().encode(pattern)))
        XCTAssertTrue(connected.accepted, connected.message ?? "")
        _ = try await reply(.init(operation: "meters.subscribe", payload: try JSONEncoder().encode([UUID]())))
        try await Task.sleep(for: .milliseconds(200))
        XCTAssertEqual(store.meter, 0)
        func send(_ bytes: [UInt8]) {
            var list = MIDIPacketList()
            let packet = MIDIPacketListInit(&list)
            _ = MIDIPacketListAdd(&list, MemoryLayout<MIDIPacketList>.size, packet, 0, bytes.count, bytes)
            MIDIReceived(source, &list)
        }
        send([0x90, 64, 110])
        try await until("live note is audible", timeout: 5) { self.store.meter > 0.01 }
        send([0x80, 64, 0])
        let stop = try await reply(.init(operation: "designer.stop"))
        XCTAssertTrue(stop.accepted, stop.message ?? "")
    }

    func testInputMonitoringPlaysArmedInputWhileRecording() async throws {
        store.addTrack()
        try await until("track") { self.remote.authoritativeSession?.tracks.count == 1 }
        let id = try XCTUnwrap(store.session.tracks.first?.id)
        store.updateTrack(id, actionName: "Arm track") { $0.armed = true }
        try await until("armed") { self.remote.authoritativeSession?.tracks.first?.armed == true }
        _ = try await reply(.init(operation: "meters.subscribe", payload: try JSONEncoder().encode([UUID]())))
        store.record()
        try await until("recording", timeout: 15) { self.store.isRecording }
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertLessThan(store.meter, 0.001, "nothing is audible without monitoring")
        // Turning monitoring on mid-take routes the armed input to the output.
        store.inputMonitoring = true
        try await until("monitored input", timeout: 5) { self.store.meter > 0.1 }
        store.inputMonitoring = false
        store.finishRecording()
        try await until("take committed", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.count == 1
        }
    }

    func testHardwareMicrophoneRecordsAnAlignedTake() async throws {
        store.addTrack()
        try await until("track") { self.remote.authoritativeSession?.tracks.count == 1 }
        let id = try XCTUnwrap(store.session.tracks.first?.id)
        store.updateTrack(id, actionName: "Arm track") { $0.armed = true }
        try await until("armed") { self.remote.authoritativeSession?.tracks.first?.armed == true }
        store.record()
        try await until("recording from the microphone", timeout: 15) { self.store.isRecording }
        try await Task.sleep(for: .milliseconds(1500))
        store.finishRecording()
        try await until("microphone take", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.count == 1
        }
        let clip = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first)
        XCTAssertGreaterThan(clip.length, 48_000, "about 1.5 s of native input, resampled and aligned")
        XCTAssertLessThan(clip.start, 48_000)
    }

    func testSingleTrackAuditionSurvivesEditsUntilStopped() async throws {
        let toneTrack = try await importTone(frames: 480_000)
        store.addTrack()
        try await until("empty track") { self.remote.authoritativeSession?.tracks.count == 2 }
        let empty = try XCTUnwrap(remote.authoritativeSession?.tracks.last?.id)
        _ = try await reply(.init(operation: "meters.subscribe", payload: try JSONEncoder().encode([UUID]())))
        let audition = try await reply(
            .init(operation: "transport.audition", context: store.remoteContext, strings: ["trackID": empty.uuidString])
        )
        XCTAssertTrue(audition.accepted, audition.message ?? "")
        try await until("auditioning") { self.store.isPlaying }
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertLessThan(store.meter, 0.001, "only the empty track is audible")
        // An authored edit during the audition must not bring back the full mix.
        store.renameTrack(toneTrack, to: "Renamed while auditioning")
        try await until("rename") {
            self.remote.authoritativeSession?.tracks.first?.name == "Renamed while auditioning"
        }
        try await Task.sleep(for: .milliseconds(400))
        XCTAssertLessThan(store.meter, 0.001, "the audition survives the edit")
        store.togglePlay()
        try await until("stopped") { !self.store.isPlaying }
        // Normal playback afterwards plays every track again.
        store.togglePlay()
        try await until("tone audible", timeout: 5) { self.store.meter > 0.05 }
        store.togglePlay()
    }

    func testCombiningContiguousPiecesOfOneSourceIsLossless() async throws {
        let track = try await importTone()
        let original = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first)
        // Two contiguous pieces of the same source with no fades or cut boundaries.
        store.updateTrack(track, actionName: "Pieces") { t in
            var a = original
            var b = original
            a.length = 20_000
            a.edgeFadeFrames = 0
            a.edgeProtectionDisabled = true
            b.id = UUID()
            b.start = original.start + 20_000
            b.offset = original.offset + 20_000
            b.length = 28_000
            b.edgeFadeFrames = 0
            b.edgeProtectionDisabled = true
            t.clips = [a, b]
        }
        try await until("pieces") { self.remote.authoritativeSession?.tracks.first?.clips.count == 2 }
        store.selection = nil
        store.selectedClips = Set(remote.authoritativeSession?.tracks.first?.clips.map(\.id) ?? [])
        store.combineClips()
        try await until("rejoined", timeout: 20) { self.remote.authoritativeSession?.tracks.first?.clips.count == 1 }
        let joined = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first)
        XCTAssertEqual(joined.assetID, original.assetID, "no new media for a provably identical rejoin")
        XCTAssertEqual(joined.length, 48_000)
        XCTAssertEqual(joined.start, original.start)
        store.undo()
        try await until("undo rejoin") { self.remote.authoritativeSession?.tracks.first?.clips.count == 2 }
    }

    func testHourLongSessionSeekAndPlayLatency() async throws {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        // Ten minutes of 16-bit mono, placed every ten minutes across an hour.
        let frames = 48_000 * 600
        let wav = directory.appendingPathComponent("ten-minutes.wav")
        var data = Data(capacity: 44 + frames * 2)
        func le<T: FixedWidthInteger>(_ v: T) { withUnsafeBytes(of: v.littleEndian) { data.append(contentsOf: $0) } }
        data.append(contentsOf: Array("RIFF".utf8))
        le(UInt32(36 + frames * 2))
        data.append(contentsOf: Array("WAVEfmt ".utf8))
        le(UInt32(16))
        le(UInt16(1))
        le(UInt16(1))
        le(UInt32(48_000))
        le(UInt32(96_000))
        le(UInt16(2))
        le(UInt16(16))
        data.append(contentsOf: Array("data".utf8))
        le(UInt32(frames * 2))
        var samples = [Int16](repeating: 0, count: frames)
        for i in 0..<frames { samples[i] = Int16(sin(Double(i) * 0.03) * 6000) }
        samples.withUnsafeBytes { data.append(contentsOf: $0) }
        try data.write(to: wav)
        remote.importAudio(urls: [wav])
        let importStart = Date()
        try await until("long import", timeout: 900) {
            self.remote.authoritativeSession?.tracks.first?.clips.count == 1
        }
        let importSeconds = Date().timeIntervalSince(importStart)
        let track = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.id)
        let source = try XCTUnwrap(remote.authoritativeSession?.tracks.first?.clips.first)
        store.updateTrack(track, actionName: "Hour") { t in
            t.clips = (0..<6).map { n in
                var c = source
                c.id = UUID()
                c.start = n * frames
                return c
            }
            t.realtimeEffects = [RealtimeEffect(kind: .room, parameters: RealtimeEffectKind.room.defaultParameters)]
        }
        try await until("hour timeline", timeout: 20) {
            self.remote.authoritativeSession?.tracks.first?.clips.count == 6
        }
        func seekAndPlay(_ frame: Int) async throws -> Double {
            var reply: RemoteReply?
            let started = Date()
            remote.send(.seek(frame: frame)) { reply = $0 }
            try await until("seek reply", timeout: 120) { reply != nil }
            XCTAssertTrue(reply!.accepted, reply!.message ?? "")
            store.togglePlay()
            try await until("playing", timeout: 120) { self.store.isPlaying }
            let elapsed = Date().timeIntervalSince(started)
            store.togglePlay()
            try await until("stopped") { !self.store.isPlaying }
            return elapsed
        }
        let first = try await seekAndPlay(58 * 60 * 48_000)
        let cached = try await seekAndPlay(58 * 60 * 48_000 + 48_000)
        print(
            String(
                format:
                    "SOUNDROOM_RUST_HOUR_BENCHMARK upload_import_57MiB_seconds=%.3f first_seek_to_playing_seconds=%.3f nearby_seek_to_playing_seconds=%.3f",
                importSeconds, first, cached))
        XCTAssertLessThan(first, 2, "bounded warm-start seek")
    }
}
