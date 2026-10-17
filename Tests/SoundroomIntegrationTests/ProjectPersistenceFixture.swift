import Foundation
import SoundroomCore
import SoundroomEngineControl

@testable import Soundroom

typealias LoadedProject = SoundroomEngineControl.LoadedProject
typealias ProjectSaveReport = SoundroomEngineControl.ProjectSaveReport
typealias RecoverySnapshot = SoundroomEngineControl.RecoverySnapshot
typealias RecoveryStore = SoundroomEngineControl.RecoveryStore

/// The Mac host supplies its existing streaming codec. Document storage and
/// recovery belong to the engine library, independently of editor observation.
enum ProjectPersistence {
    static let media = ClosureProjectMediaIO(
        read: { try FixtureAudioFile.read($0) }, write: { try StreamingAudioFile.writeFloat($0, to: $1) })
    static func save(session: Session, assets: [UUID: AudioSamples], to target: URL) async throws -> ProjectSaveReport {
        try await SoundroomEngineControl.ProjectPersistence.save(
            session: session, assets: assets, to: target, media: media)
    }
    static func load(from target: URL) async throws -> LoadedProject {
        try await SoundroomEngineControl.ProjectPersistence.load(from: target, media: media)
    }
    @discardableResult static func saveSynchronously(
        session: Session, assets: [UUID: AudioSamples], to target: URL, recoveryMetadata: Data? = nil
    ) throws -> ProjectSaveReport {
        try SoundroomEngineControl.ProjectPersistence.saveSynchronously(
            session: session, assets: assets, to: target, recoveryMetadata: recoveryMetadata, media: media)
    }
    static func loadSynchronously(from target: URL) throws -> LoadedProject {
        try SoundroomEngineControl.ProjectPersistence.loadSynchronously(from: target, media: media)
    }
    static func validate(session: Session, assets: [UUID: AudioSamples]) throws {
        try SoundroomEngineControl.ProjectPersistence.validate(session: session, assets: assets)
    }
}
