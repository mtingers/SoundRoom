import XCTest
import Foundation
import SoundroomCore
@testable import SoundroomRemote

final class RemoteSnapshotPagesTests: XCTestCase {
    func testMultiPageSnapshotInstallsOnlyAfterMatchingDigestAndRevision() throws {
        var session = Session(); var pattern = SoundDesignPattern()
        pattern.notes = (0..<20000).map { SoundDesignNote(pitch: 60, startBeat: Double($0) / 4) }
        var clip = Clip(assetID: UUID(), name: "Large MIDI", start: 0, length: 48000)
        clip.soundDesign = pattern; session.tracks[0].clips = [clip]
        let snapshot = RemoteSnapshot(serverID: UUID(), serverName: "Host", session: session, revision: 22)
        let transfer = try RemoteSnapshotPages(snapshot: snapshot)
        XCTAssertGreaterThan(transfer.pageCount, 1)
        var assembler = RemoteSnapshotAssembler()
        XCTAssertNil(try assembler.accept(transfer.begin()))
        for index in 0..<transfer.pageCount {
            let page = try transfer.page(at: index)
            XCTAssertLessThan(try JSONEncoder().encode(page).count, 1024 * 1024)
            XCTAssertNil(try assembler.accept(page))
        }
        XCTAssertEqual(try assembler.accept(transfer.end()), snapshot)
        XCTAssertEqual(assembler.nextOffset, 0)
    }
    func testRejectsMixedRevisionOutOfOrderAndTampering() throws {
        let transfer = try RemoteSnapshotPages(snapshot: .init(serverID: UUID(), serverName: "Host", session: Session(), revision: 22))
        var assembler = RemoteSnapshotAssembler()
        XCTAssertNil(try assembler.accept(transfer.begin()))
        var bad = try transfer.page(at: 0); bad.revision += 1
        XCTAssertThrowsError(try assembler.accept(bad)); XCTAssertEqual(assembler.nextOffset, 0)
        bad = try transfer.page(at: 0); bad.offset = 1
        XCTAssertThrowsError(try assembler.accept(bad)); XCTAssertEqual(assembler.nextOffset, 0)
        bad = try transfer.page(at: 0); bad.data![0] ^= 1
        XCTAssertNil(try assembler.accept(bad))
        XCTAssertThrowsError(try assembler.accept(transfer.end()))
        XCTAssertEqual(assembler.nextOffset, 0)
    }
    func testLimitsUnknownVersionAndCancellation() throws {
        var assembler = RemoteSnapshotAssembler()
        let transferID = UUID()
        let oversize = RemoteSnapshotPage(kind: .snapshotBegin, transferID: transferID, revision: 0,
                                          totalBytes: RemoteSnapshotPages.maximumSnapshotBytes + 1, sha256: String(repeating: "a", count: 64))
        XCTAssertThrowsError(try assembler.accept(oversize))
        var bad = oversize; bad.version = 4
        XCTAssertThrowsError(try assembler.accept(bad))
        let transfer = try RemoteSnapshotPages(snapshot: .init(serverID: UUID(), serverName: "Host", session: Session()))
        _ = try assembler.accept(transfer.begin()); assembler.cancel()
        XCTAssertThrowsError(try assembler.accept(transfer.page(at: 0)))
    }
    func testCapabilitiesEnvelopeContainsCompleteFactoryMetadataWithoutClaimingUnavailableFeatures() throws {
        let capability = RemoteCapabilities(deviceBackends: ["coreaudio"], codecs: ["wav", "aiff"], features: ["transport", "effects"])
        XCTAssertEqual(capability.maxTimelineFrame, 48000 * 60 * 60 * 24)
        XCTAssertEqual(capability.designerPresets.count, SoundDesignPreset.allCases.count + OrganicPatch.allCases.count)
        XCTAssertEqual(capability.drumKits.count, DrumKit.allCases.count)
        XCTAssertEqual(capability.drumPresets.count, DrumPatternPreset.allCases.count)
        for (effect, kind) in zip(capability.effects, RealtimeEffectKind.allCases) {
            XCTAssertEqual(effect.factoryPresets.map(\.values), kind.presets.map(\.values))
        }
        let wire = try RemoteV3Codec.encodeCapabilities(capability)
        if let output = ProcessInfo.processInfo.environment["SOUNDROOM_EXPORT_CAPABILITIES"] {
            // Explicit fixture generation for portable hosts; this never opens
            // hardware and the consumer must filter its own supported features.
            try RemoteV3Codec.encodeCapabilities(RemoteCapabilities()).write(to: URL(fileURLWithPath: output), options: .atomic)
        }
        XCTAssertLessThan(wire.count, 256 * 1024)
        XCTAssertEqual(try RemoteV3Codec.decodeCapabilities(wire), capability)
        var bad = try XCTUnwrap(JSONSerialization.jsonObject(with: wire) as? [String: Any]); bad["kind"] = "event"
        XCTAssertThrowsError(try RemoteV3Codec.decodeCapabilities(JSONSerialization.data(withJSONObject: bad)))
        XCTAssertTrue(RemoteCapabilities().features.isEmpty)
    }
    func testCapabilityMetadataMatchesDSPParameterRangesAndDefaults() throws {
        let capability = RemoteCapabilities(sampleRate: 48000, deviceBackends: ["coreaudio"], codecs: ["wav", "aiff"])
        XCTAssertFalse(capability.snapshotPagination, "Capabilities must not advertise unintegrated features")
        XCTAssertEqual(capability.effects.map(\.id), RealtimeEffectKind.allCases.map(\.rawValue))
        for (effect, kind) in zip(capability.effects, RealtimeEffectKind.allCases) {
            XCTAssertEqual(effect.parameters.map(\.defaultValue), kind.parameters.map(\.defaultValue))
            XCTAssertEqual(effect.parameters.map(\.logarithmic), kind.parameters.map(\.logarithmic))
            XCTAssertTrue(effect.parameters.allSatisfy { $0.minimum <= $0.defaultValue && $0.defaultValue <= $0.maximum })
        }
        XCTAssertEqual(try JSONDecoder().decode(RemoteCapabilities.self, from: JSONEncoder().encode(capability)), capability)
    }
}
