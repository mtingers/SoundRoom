import XCTest
import SoundroomCore
import SoundroomRemote
@testable import Soundroom

final class RemoteWaveformCacheTests: XCTestCase {
    func testSeparateChannelsDecodeAndMalformedOrLegacyChannelsStayCombined() throws {
        var asset = RemoteAsset(id: UUID(), frames: 4, channels: 2, peaks: [-0.5, 0.8, -0.2, 0.6], framesPerBin: 2,
                                channelPeaks: [[-0.5, 0, -0.2, 0], [0, 0.8, 0, 0.6]])
        let decoded = try JSONDecoder().decode(RemoteAsset.self, from: JSONEncoder().encode(asset))
        let envelope = try XCTUnwrap(WaveformEnvelope(asset: decoded))
        XCTAssertEqual(envelope.channelValues[0].map(\.1), [0, 0])
        XCTAssertEqual(envelope.channelValues[1].map(\.0), [0, 0])
        XCTAssertEqual(envelope.channelValues[1].map(\.1), [0.8, 0.6])
        for malformed in [nil, [[0, 1]], [[0, 1], [0, 1]]] as [[[Float]]?] {
            asset.channelPeaks = malformed
            XCTAssertTrue(try XCTUnwrap(WaveformEnvelope(asset: asset)).channelValues.isEmpty)
        }
        asset.peaks.append(1)
        XCTAssertNil(WaveformEnvelope(asset: asset))
    }

    func testStereoUpgradeIsCachedAndAllThreeEnvelopesCountAgainstBudget() throws {
        let id = UUID(); var cache = RemoteWaveformCache(maximumBytes: 500)
        let legacy = WaveformEnvelope(framesPerBin: 1, values: Array(repeating: (-0.2, 0.8), count: 10))
        let stereo = WaveformEnvelope.measured(framesPerBin: 1, channels: [Array(repeating: (-0.2, 0), count: 10), Array(repeating: (0, 0.8), count: 10)])
        XCTAssertTrue(cache.install(id: id, frames: 10, envelope: legacy))
        XCTAssertTrue(cache.install(id: id, frames: 10, envelope: stereo))
        XCTAssertEqual(cache.residentBytes, stereo.byteCount + 128)
        XCTAssertEqual(cache.envelope(id: id, range: 0..<10)?.channelValues.count, 2)
        XCTAssertFalse(cache.install(id: id, frames: 10, envelope: legacy))
        var tooSmall = RemoteWaveformCache(maximumBytes: 300)
        XCTAssertFalse(tooSmall.install(id: id, frames: 10, envelope: stereo))
    }

    func testFullRangeStereoRefinementsNeverEvictOtherClipsOverviews() throws {
        // Twinkle has 48 distinct stereo assets. The old full-range detail
        // replacement evicted the first six, exactly matching the blank pad.
        let ids = (0..<48).map { _ in UUID() }, frames = 1_048_576
        let overview = WaveformEnvelope.measured(framesPerBin: 512,
            channels: Array(repeating: Array(repeating: (-0.4, 0.6), count: 2048), count: 2))
        let detail = WaveformEnvelope.measured(framesPerBin: 128,
            channels: Array(repeating: Array(repeating: (-0.8, 0.9), count: 8192), count: 2))
        var cache = RemoteWaveformCache()
        for id in ids {
            cache.install(id: id, frames: frames, envelope: overview)
            cache.install(id: id, frames: frames, envelope: detail)
            XCTAssertLessThanOrEqual(cache.residentBytes, cache.maximumBytes)
            XCTAssertLessThanOrEqual(cache.count, cache.maximumEntries)
        }
        for _ in 0..<3 {
            for id in ids {
                let peaks = try XCTUnwrap(cache.envelope(id: id, range: 0..<frames))
                XCTAssertEqual(peaks.channelValues.count, 2)
                XCTAssertNil(RemoteWaveformRequest.refinement(id: id, frames: frames, range: 0..<frames, pixels: 128, envelope: peaks))
            }
        }
    }

    func testOverviewArrivesBeforeDetailAndOnlyCloseZoomRefines() throws {
        let id = UUID(), frames = 1_048_576
        let overview = WaveformEnvelope(framesPerBin: 512, values: Array(repeating: (-0.3, 0.4), count: 2048))
        XCTAssertNil(RemoteWaveformRequest.refinement(id: id, frames: frames, range: 0..<frames, pixels: 128, envelope: nil))
        XCTAssertNil(RemoteWaveformRequest.refinement(id: id, frames: frames, range: 0..<frames, pixels: 128, envelope: overview))
        let visible = 100_000..<148_000
        let request = try XCTUnwrap(RemoteWaveformRequest.refinement(id: id, frames: frames, range: visible, pixels: 1000, envelope: overview))
        XCTAssertLessThanOrEqual(request.start, visible.lowerBound)
        XCTAssertGreaterThanOrEqual(request.end, visible.upperBound)
        let bin = (request.end - request.start + request.points - 1) / request.points
        let fine = WaveformEnvelope(framesPerBin: bin, firstFrame: request.start,
            values: Array(repeating: (-0.6, 0.7), count: request.points))
        XCTAssertNil(RemoteWaveformRequest.refinement(id: id, frames: frames, range: visible, pixels: 1000, envelope: fine))
    }

    func testUnsolicitedFullDetailKeepsStereoFallbackWithinSmallBudgets() throws {
        let id = UUID(), frames = 1_048_576
        let detail = WaveformEnvelope.measured(framesPerBin: 128,
            channels: [Array(repeating: (-0.8, 0), count: 8192), Array(repeating: (0, 0.9), count: 8192)])
        for cacheLimits in [(60_000, 128), (300_000, 1)] {
            var cache = RemoteWaveformCache(maximumBytes: cacheLimits.0, maximumEntries: cacheLimits.1)
            XCTAssertTrue(cache.install(id: id, frames: frames, envelope: detail), "Installing a fallback must trigger display invalidation")
            let fallback = try XCTUnwrap(cache.envelope(id: id, range: 0..<frames))
            XCTAssertEqual(fallback.framesPerBin, 512)
            XCTAssertEqual(fallback.count, 2048)
            XCTAssertEqual(fallback.channelValues[0][0].0, -0.8)
            XCTAssertEqual(fallback.channelValues[1][0].1, 0.9)
            XCTAssertFalse(cache.install(id: id, frames: frames, envelope: detail))
            XCTAssertLessThanOrEqual(cache.residentBytes, cacheLimits.0)
            XCTAssertEqual(cache.count, 1)
        }
    }

    func testThousandAssetDocumentRetainsEveryStereoOverviewAndFineZoom() throws {
        let ids = (0..<1000).map { _ in UUID() }, frames = 1_048_576
        var cache = RemoteWaveformCache()
        cache.configure(assetCount: ids.count)
        XCTAssertEqual(cache.overviewPoints, 128)
        let overview = WaveformEnvelope.measured(framesPerBin: frames / cache.overviewPoints,
            channels: [Array(repeating: (-0.4, 0), count: cache.overviewPoints),
                       Array(repeating: (0, 0.6), count: cache.overviewPoints)])
        for id in ids { XCTAssertTrue(cache.install(id: id, frames: frames, envelope: overview)) }
        let detail = WaveformEnvelope.measured(framesPerBin: 128,
            channels: Array(repeating: Array(repeating: (-0.8, 0.9), count: 8192), count: 2))
        for id in ids.prefix(50) { cache.install(id: id, frames: frames, envelope: detail) }
        for _ in 0..<3 { for id in ids {
            let peaks = try XCTUnwrap(cache.envelope(id: id, range: 0..<frames))
            XCTAssertEqual(peaks.channelValues.count, 2)
            XCTAssertNil(RemoteWaveformRequest.refinement(id: id, frames: frames, range: 0..<frames, pixels: 100, envelope: peaks))
        } }
        XCTAssertLessThanOrEqual(cache.residentBytes, cache.maximumBytes)
        let fine = try XCTUnwrap(cache.envelope(id: ids[49], range: 0..<frames))
        XCTAssertEqual(fine.framesPerBin, 128)
        XCTAssertLessThanOrEqual(cache.count, cache.maximumEntries)
    }

    func testGrowingDocumentRecompactsRetainedOverviewsAndShortFineAssetsStayFine() throws {
        let id = UUID(), frames = 1_048_576
        var cache = RemoteWaveformCache()
        let overview = WaveformEnvelope.measured(framesPerBin: 512,
            channels: [Array(repeating: (-0.7, 0), count: 2048), Array(repeating: (0, 0.9), count: 2048)])
        cache.install(id: id, frames: frames, envelope: overview)
        cache.configure(assetCount: 1000)
        let compact = try XCTUnwrap(cache.envelope(id: id, range: 0..<frames))
        XCTAssertEqual(compact.count, 128)
        XCTAssertEqual(compact.channelValues[0][0].0, -0.7)
        XCTAssertEqual(compact.channelValues[1][0].1, 0.9)
        let short = UUID()
        cache.install(id: short, frames: 1000, envelope: .init(framesPerBin: 1, values: Array(repeating: (-0.4, 0.5), count: 1000)))
        XCTAssertEqual(cache.envelope(id: short, range: 0..<1000)?.framesPerBin, 1,
                       "A full-range fine request with fewer than 2048 bins must retain its detail")
    }

    @MainActor func testLargeSnapshotPublishesItsAssetInventoryOnce() throws {
        let store = SessionStore(headless: true)
        let remote = RemoteGUIController(store: store, credential: try RemoteCredential.generate(), host: "fixture")
        let assets = (0..<1000).map { _ in RemoteAsset(id: UUID(), frames: 100_000, channels: 2) }
        var publications = 0
        let observation = store.$assets.dropFirst().sink { _ in publications += 1 }
        remote.receive(.snapshot(.init(serverID: UUID(), serverName: "Fixture", session: Session(), assets: assets)))
        XCTAssertEqual(publications, 1)
        XCTAssertEqual(Set(store.assets.keys), Set(assets.map(\.id)))
        XCTAssertTrue(store.assets.values.allSatisfy { $0.count == 100_000 && $0.channelCount == 2 })
        withExtendedLifetime(observation) {}
    }

    @MainActor func testWaveformArrivalBurstRepaintsLanesWithoutPublishingDocument() async throws {
        let store = SessionStore()
        let remote = RemoteGUIController(store: store, credential: try RemoteCredential.generate(), host: "127.0.0.1")
        let id = UUID()
        remote.receive(.snapshot(.init(serverID: UUID(), serverName: "Fixture", session: Session(),
                                       assets: [.init(id: id, frames: 100_000, channels: 2)])))
        var documents = 0, drawings = 0
        let document = store.objectWillChange.sink { documents += 1 }
        let drawing = store.waveformDisplay.objectWillChange.sink { drawings += 1 }
        for first in 0..<100 {
            remote.receive(.waveform(.init(id: id, frames: 100_000, channels: 2,
                peaks: [-0.4, 0.6], waveformStartFrame: first * 100, framesPerBin: 100)))
        }
        XCTAssertEqual(documents, 0)
        try await Task.sleep(nanoseconds: 60_000_000)
        XCTAssertEqual(drawings, 1)
        XCTAssertEqual(documents, 0)
        withExtendedLifetime((document, drawing)) {}
    }

    func testLabelsAndSplitGeometryAtEveryTrackHeight() {
        XCTAssertEqual(AudioChannelPresentation.label(channels: 1), "Mono")
        XCTAssertEqual(AudioChannelPresentation.label(channels: 2), "Stereo")
        XCTAssertNil(AudioChannelPresentation.label(channels: nil))
        for size in TrackSize.allCases {
            let lane = AudioLaneGeometry(height: size.height)
            let geometry = WaveformChannelGeometry(top: 36, bottom: lane.bottom, channels: 2)
            XCTAssertGreaterThanOrEqual(geometry.amplitude, 14)
            XCTAssertGreaterThan(geometry.center(0) - geometry.amplitude, 33, "Keep audio below the clip header/drag region")
            XCTAssertLessThan(geometry.center(0) + geometry.amplitude, geometry.center(1) - geometry.amplitude)
            XCTAssertLessThan(geometry.center(1) + geometry.amplitude, lane.bottom)
        }
    }
    func testAdjacentScrollsUseStableExpandedBuckets() {
        let id = UUID(), frames = 10_000_000
        let first = RemoteWaveformRequest.detail(id: id, frames: frames, range: 100_000..<148_000)
        for offset in 1...500 {
            let next = RemoteWaveformRequest.detail(id: id, frames: frames, range: (100_000 + offset)..<(148_000 + offset))
            XCTAssertEqual(next, first)
        }
        XCTAssertLessThan(first.start, 100_000)
        XCTAssertGreaterThan(first.end, 148_500)
        let boundary = RemoteWaveformRequest.detail(id: id, frames: frames, range: 131_073..<179_073)
        XCTAssertGreaterThan(first.end, boundary.start, "Neighboring buckets overlap for continuous handoff")
    }
    func testContinuousOverviewAndFinestCompleteTileDuringScroll() throws {
        let id = UUID(); var cache = RemoteWaveformCache()
        let overview = WaveformEnvelope(framesPerBin: 100, values: Array(repeating: (-0.3, 0.4), count: 1000))
        let detail = WaveformEnvelope(framesPerBin: 10, firstFrame: 10_000, values: Array(repeating: (-0.6, 0.7), count: 2000))
        cache.install(id: id, frames: 100_000, envelope: overview)
        cache.install(id: id, frames: 100_000, envelope: detail)
        XCTAssertEqual(cache.envelope(id: id, range: 15_000..<25_000)?.framesPerBin, 10)
        for start in stride(from: 25_000, through: 80_000, by: 499) {
            let visible = start..<(start + 10_000)
            let peaks = try XCTUnwrap(cache.envelope(id: id, range: visible))
            XCTAssertLessThanOrEqual(peaks.firstFrame, visible.lowerBound)
            XCTAssertGreaterThanOrEqual(peaks.firstFrame + peaks.count * peaks.framesPerBin, visible.upperBound)
            XCTAssertEqual(peaks.values[0].0, -0.3)
        }
        var partialOnly = RemoteWaveformCache()
        partialOnly.install(id: id, frames: 100_000, envelope: detail)
        XCTAssertNil(partialOnly.envelope(id: id, range: 29_000..<40_000), "Partial tiles must never draw a misleading disappearing portion")
    }
    func testScrollQueueReplacesStaleWorkAndKeepsOverview() throws {
        let id = UUID(), frames = 10_000_000; var queue = RemoteWaveformQueue()
        let overview = RemoteWaveformRequest.overview(id: id, frames: frames)
        queue.enqueue(overview)
        for offset in stride(from: 0, through: 5_000_000, by: 4096) {
            queue.enqueue(.detail(id: id, frames: frames, range: offset..<(offset + 48_000)))
        }
        XCTAssertEqual(queue.pending.count, 2)
        XCTAssertEqual(queue.next(), overview)
        let latest = try XCTUnwrap(queue.next())
        XCTAssertGreaterThan(latest.start, 4_000_000)
        XCTAssertEqual(queue.active.count, 2)
        queue.enqueue(latest); XCTAssertTrue(queue.pending.isEmpty)
        queue.enqueue(.detail(id: id, frames: frames, range: 1_000_000..<1_048_000))
        queue.enqueue(latest)
        XCTAssertTrue(queue.pending.isEmpty, "Returning to an in-flight view cancels obsolete waiting views")
        queue.complete(overview); queue.complete(latest); XCTAssertTrue(queue.active.isEmpty)
    }
    func testCacheBudgetProtectsOverviewAndDuplicateDoesNotDowngrade() throws {
        let id = UUID(); var cache = RemoteWaveformCache(maximumBytes: 1000, maximumEntries: 3)
        let overview = WaveformEnvelope(framesPerBin: 1000, values: Array(repeating: (-0.3, 0.4), count: 20))
        let detail = WaveformEnvelope(framesPerBin: 10, firstFrame: 1000, values: Array(repeating: (-0.6, 0.7), count: 20))
        XCTAssertTrue(cache.install(id: id, frames: 20_000, envelope: overview))
        XCTAssertFalse(cache.install(id: id, frames: 20_000, envelope: overview))
        for index in 0..<40 {
            cache.install(id: id, frames: 20_000, envelope: .init(framesPerBin: 10, firstFrame: 1000 + index * 300, values: detail.values))
            XCTAssertLessThanOrEqual(cache.residentBytes, 1000); XCTAssertLessThanOrEqual(cache.count, 3)
        }
        XCTAssertNotNil(cache.envelope(id: id, range: 0..<20_000))
        cache.retainAssets([]); XCTAssertEqual(cache.count, 0); XCTAssertEqual(cache.residentBytes, 0)
        var quality = RemoteWaveformCache()
        quality.install(id: id, frames: 100, envelope: .init(framesPerBin: 1, values: Array(repeating: (-0.5, 0.9), count: 100)))
        XCTAssertFalse(quality.install(id: id, frames: 100, envelope: .init(framesPerBin: 10, values: Array(repeating: (-1, 1), count: 10))))
        XCTAssertEqual(quality.envelope(id: id, range: 0..<100)?.framesPerBin, 1)
    }
    @MainActor func testReconnectPreservesWaveformAndDuplicateEventAvoidsInvalidation() throws {
        let store = SessionStore()
        let remote = RemoteGUIController(store: store, credential: try RemoteCredential.generate(), host: "127.0.0.1")
        let id = UUID(), serverID = UUID()
        let asset = RemoteAsset(id: id, frames: 100_000, channels: 2, peaks: [-0.4, 0.6, -0.2, 0.3])
        remote.receive(.waveform(asset))
        let previous = try XCTUnwrap(remote.waveform(id, range: 40_000..<90_000))
        remote.receive(.snapshot(.init(serverID: serverID, serverName: "Host", session: Session(), assets: [.init(id: id, frames: 100_000, channels: 2)])))
        XCTAssertEqual(remote.waveform(id, range: 40_000..<90_000)?.framesPerBin, previous.framesPerBin)
        var changes = 0
        let observation = store.objectWillChange.sink { changes += 1 }
        remote.receive(.waveform(asset)); XCTAssertEqual(changes, 0)
        withExtendedLifetime(observation) {}
        remote.receive(.snapshot(.init(serverID: serverID, serverName: "Host", session: Session())))
        XCTAssertNil(remote.waveform(id))
    }
}
