import SoundroomCore
import XCTest

@testable import SoundroomRemote

final class RemoteSnapshotEncodingPoolTests: XCTestCase {
    func testAuthenticationReceiptsAndHeartbeatBypassOrdinaryWorker() {
        let id = UUID()
        for message in [
            RemoteWireMessage.challenge(.init(serverID: id, nonce: Data(repeating: 1, count: 32))),
            .welcome(.init(serverID: id, name: "Host", role: .operator)),
            .failure("Connection rejected"), .reply(.init(requestID: id, revision: 1)),
            .heartbeat(id), .heartbeatReply(id),
        ] {
            XCTAssertTrue(message.usesPriorityWebEncoding)
        }
        XCTAssertFalse(RemoteWireMessage.event(.delta(.init(revision: 1))).usesPriorityWebEncoding)
        XCTAssertFalse(
            RemoteWireMessage.event(.snapshot(.init(serverID: id, serverName: "Host", session: Session())))
                .usesPriorityWebEncoding)
    }
    func testSixteenClientsShareLargeMetadataAndKeepDistinctLiveStateAndTransfers() async throws {
        let pool = RemoteSnapshotEncodingPool()
        var session = Session()
        var pattern = SoundDesignPattern()
        pattern.notes = Array(repeating: SoundDesignNote(pitch: 60, startBeat: 0), count: 100_000)
        var clip = Clip(assetID: UUID(), name: "Large MIDI", start: 0, length: 48_000)
        clip.soundDesign = pattern
        session.tracks[0].clips = [clip]
        let serverID = UUID()
        var leases: [RemoteSnapshotLease] = []
        for index in 0..<16 {
            let snapshot = RemoteSnapshot(
                serverID: serverID, serverName: "Host", session: session, revision: 12,
                transport: .init(frame: index * 128, isPlaying: true, hostTime: Double(index)),
                status: "Client \(index)",
                peers: [.init(id: UUID(), name: "Peer \(index)")])
            let result = try await pool.encode(.event(.snapshot(snapshot)), state: .init())
            guard case .snapshot(let lease) = result.output else { return XCTFail() }
            leases.append(lease)
            if index == 0 || index == 15 {
                var stream = RemoteWebSnapshotStream(lease: lease)
                var client = RemoteV3ClientStream()
                var installed: RemoteSnapshot?
                while let bytes = try stream.next() {
                    XCTAssertLessThan(bytes.count, RemoteProtocol.maximumFrameBytes)
                    for message in try client.accept(bytes) {
                        switch message {
                        case .event(.snapshot(let value)): installed = value
                        case .event(.transport(let value)): installed?.transport = value
                        case .event(.presence(let value)): installed?.peers = value
                        case .event(.status(let value)): installed?.status = value
                        default: break
                        }
                    }
                }
                XCTAssertEqual(installed, snapshot)
            }
        }
        let stats = await pool.statistics()
        XCTAssertEqual(stats.documentEncodes, 1)
        XCTAssertEqual(stats.activeLeases, 16)
        XCTAssertEqual(stats.entries, 1)
        XCTAssertEqual(stats.retainedBytes, leases[0].pages.bytes.count)
        XCTAssertGreaterThan(stats.retainedBytes, RemoteProtocol.maximumFrameBytes)
        XCTAssertEqual(Set(leases.map { $0.pages.transferID }).count, 16)
        leases.removeAll()
        try await waitForRelease(pool)
    }

    func testOversizedDeltaEncodesOncePerRevisionAndMixerChangesRemainIncremental() async throws {
        let pool = RemoteSnapshotEncodingPool()
        let initial = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: Session(), revision: 5)
        var track = initial.session.tracks[0]
        var pattern = SoundDesignPattern()
        pattern.notes = Array(repeating: SoundDesignNote(pitch: 60, startBeat: 0), count: 100_000)
        var clip = Clip(assetID: UUID(), name: "Large MIDI", start: 0, length: 48_000)
        clip.soundDesign = pattern
        track.clips = [clip]
        let delta = RemoteSessionDelta(revision: 6, tracks: [track], trackRevisions: [track.id: 6])
        var leases: [RemoteSnapshotLease] = []
        var lastState = RemoteWebDocumentState()
        for _ in 0..<16 {
            var state = RemoteWebDocumentState()
            _ = try state.encode(.event(.snapshot(initial)))
            let result = try await pool.encode(.event(.delta(delta)), state: state)
            guard case .snapshot(let lease) = result.output else { return XCTFail() }
            leases.append(lease)
            lastState = result.state
        }
        let stats = await pool.statistics()
        XCTAssertEqual(stats.documentEncodes, 1)
        XCTAssertEqual(stats.messageEncodes, 1, "Only the first peer should attempt the oversized delta envelope")
        var controls = RemoteTrackControls(track: track)
        controls.gain = 0.25
        let result = try await pool.encode(
            .event(.delta(.init(revision: 7, controls: [controls], trackRevisions: [track.id: 7]))), state: lastState)
        guard case .frame(let bytes) = result.output else { return XCTFail() }
        XCTAssertLessThan(bytes.count, 8192)
        XCTAssertEqual(result.state.snapshot?.session.tracks[0].clips[0].soundDesign?.notes.count, 100_000)
        XCTAssertEqual(result.state.snapshot?.session.tracks[0].gain, 0.25)
        leases.removeAll()
        try await waitForRelease(pool)
    }

    func testActiveBudgetProtectionIdleEvictionAndCancellation() async throws {
        let initial = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: Session())
        var normalized = initial
        normalized.transport = .init(hostTime: 0)
        let bytes = try RemoteSnapshotPages(snapshot: normalized).bytes.count
        let pool = RemoteSnapshotEncodingPool(
            maximumSerializedBytes: bytes * 2, documentReservationBytes: bytes, maximumEntries: 2)
        var held: [RemoteSnapshotLease] = []
        for name in ["Host", "Next"] {
            var snapshot = initial
            snapshot.serverName = name
            let result = try await pool.encode(.event(.snapshot(snapshot)), state: .init())
            guard case .snapshot(let lease) = result.output else { return XCTFail() }
            held.append(lease)
        }
        var third = initial
        third.serverName = "Last"
        do {
            _ = try await pool.encode(.event(.snapshot(third)), state: .init())
            XCTFail("Active documents must remain protected")
        } catch { XCTAssertEqual(error as? RemoteError, .queueFull) }
        let before = await pool.statistics()
        XCTAssertEqual(before.documentEncodes, 2)
        XCTAssertLessThanOrEqual(before.retainedBytes, bytes * 2)
        held.removeAll()
        try await waitForRelease(pool)
        do {
            let result = try await pool.encode(.event(.snapshot(third)), state: .init())
            guard case .snapshot = result.output else { return XCTFail() }
            XCTAssertEqual(result.state.snapshot, third)
        }
        try await waitForRelease(pool)
        let after = await pool.statistics()
        XCTAssertEqual(after.documentEncodes, 3)
        XCTAssertLessThanOrEqual(after.retainedBytes, bytes * 2)
        let worker = Task {
            withUnsafeCurrentTask { $0?.cancel() }
            _ = try await pool.encode(.event(.snapshot(initial)), state: .init())
        }
        do {
            try await worker.value
            XCTFail("Cancelled preparation must not lease or encode")
        } catch { XCTAssertTrue(error is CancellationError) }
        let cancelled = await pool.statistics()
        XCTAssertEqual(cancelled.documentEncodes, after.documentEncodes)
        XCTAssertEqual(cancelled.activeLeases, 0)
    }

    func testStreamCopiesKeepOneLeaseUntilTheFinalCursorIsReleased() async throws {
        let pool = RemoteSnapshotEncodingPool()
        var result: RemoteSnapshotEncodingPool.Result? = try await pool.encode(
            .event(.snapshot(.init(serverID: UUID(), serverName: "Host", session: Session()))), state: .init())
        var stream: RemoteWebSnapshotStream?
        if case .snapshot(let lease) = result?.output { stream = .init(lease: lease) } else { return XCTFail() }
        var copy = stream
        result = nil
        XCTAssertNotNil(try stream?.next())
        stream = nil
        XCTAssertGreaterThan(copy?.retainedBytes ?? 0, 0)
        let retained = await pool.statistics()
        XCTAssertEqual(retained.activeLeases, 1)
        copy = nil
        try await waitForRelease(pool)
        let released = await pool.statistics()
        XCTAssertEqual(released.activeLeases, 0)
    }

    private func waitForRelease(_ pool: RemoteSnapshotEncodingPool) async throws {
        for _ in 0..<1000 {
            if await pool.statistics().activeLeases == 0 { return }
            try await Task.sleep(nanoseconds: 1_000_000)
        }
        XCTFail("The final stream copy must release its lease")
    }
}
