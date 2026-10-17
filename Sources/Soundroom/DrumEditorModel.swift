import Combine
import Foundation
import SoundroomCore

struct DrumPatternDocument: Codable, Equatable {
    var version = 1
    var patterns: [DrumPattern]
    var selected = 0
}
@MainActor final class DrumEditorModel: ObservableObject {
    @Published private(set) var document: DrumPatternDocument
    @Published private(set) var undoHistory: [DrumPatternDocument] = []
    @Published private(set) var redoHistory: [DrumPatternDocument] = []
    @Published var selectedVoice = DrumVoice.kick
    @Published var selectedStep = 0
    private var gestureStart: DrumPatternDocument?
    var pattern: DrumPattern { document.patterns[document.selected] }
    var currentStep: DrumStep? { pattern.steps.first { $0.voice == selectedVoice && $0.index == selectedStep } }
    init(pattern: DrumPattern) { document = DrumPatternDocument(patterns: [pattern]) }
    func beginGesture() { if gestureStart == nil { gestureStart = document } }
    func endGesture() {
        if let old = gestureStart, old != document {
            undoHistory.append(old)
            trim()
            redoHistory.removeAll()
        }
        gestureStart = nil
    }
    func change(_ update: (inout DrumPattern) -> Void) {
        let old = document
        update(&document.patterns[document.selected])
        guard old != document else { return }
        if gestureStart == nil {
            undoHistory.append(old)
            trim()
            redoHistory.removeAll()
        }
    }
    func selectPattern(_ index: Int) {
        endGesture()
        if document.patterns.indices.contains(index) { document.selected = index }
    }
    func newPattern(duplicate: Bool = false) {
        endGesture()
        guard document.patterns.count < 32 else { return }
        let old = document
        var next = duplicate ? pattern : DrumPattern()
        next.name = duplicate ? pattern.name + " copy" : "Pattern \(document.patterns.count + 1)"
        document.patterns.append(next)
        document.selected = document.patterns.count - 1
        undoHistory.append(old)
        trim()
        redoHistory.removeAll()
    }
    func toggle(voice: DrumVoice, index: Int) {
        selectedVoice = voice
        selectedStep = index
        change { pattern in
            if let i = pattern.steps.firstIndex(where: { $0.voice == voice && $0.index == index }) {
                pattern.steps.remove(at: i)
            } else {
                pattern.steps.append(DrumStep(voice: voice, index: index))
            }
        }
    }
    func setStep(_ update: (inout DrumStep) -> Void) {
        let voice = selectedVoice
        let index = selectedStep
        change { pattern in
            if let i = pattern.steps.firstIndex(where: { $0.voice == voice && $0.index == index }) {
                update(&pattern.steps[i])
            }
        }
    }
    func setLane(_ update: (inout DrumLane) -> Void) {
        let voice = selectedVoice
        change { pattern in
            var lane = pattern.lanes[voice.rawValue] ?? DrumLane(kit: pattern.kit)
            update(&lane)
            pattern.lanes[voice.rawValue] = lane
        }
    }
    func removeSelected() {
        let voice = selectedVoice
        let index = selectedStep
        change { $0.steps.removeAll { $0.voice == voice && $0.index == index } }
    }
    func undo() {
        endGesture()
        if let old = undoHistory.popLast() {
            redoHistory.append(document)
            document = old
        }
    }
    func redo() {
        endGesture()
        if let next = redoHistory.popLast() {
            undoHistory.append(document)
            document = next
        }
    }
    func copyPattern() throws -> Data { try JSONEncoder().encode(pattern) }
    func pastePattern(_ data: Data) throws {
        let incoming = try JSONDecoder().decode(DrumPattern.self, from: data)
        try Self.validate(incoming)
        change { $0 = incoming }
    }
    func saveDocument() throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        var saved = document
        if saved.patterns.contains(where: \.usesExtendedModels) { saved.version = 2 }
        return try encoder.encode(saved)
    }
    func loadDocument(_ data: Data) throws {
        let incoming = try JSONDecoder().decode(DrumPatternDocument.self, from: data)
        guard (1...2).contains(incoming.version), !incoming.patterns.isEmpty, incoming.patterns.count <= 32,
            incoming.patterns.indices.contains(incoming.selected)
        else { throw DrumDocumentError.invalid }
        for pattern in incoming.patterns { try Self.validate(pattern) }
        endGesture()
        undoHistory.append(document)
        trim()
        redoHistory.removeAll()
        document = incoming
    }
    static func validate(_ pattern: DrumPattern) throws {
        if let value = pattern.variationCycles, ![1, 2, 4, 8].contains(value) { throw DrumDocumentError.invalid }
        if let value = pattern.velocityHumanize, !value.isFinite || !(0...0.2).contains(value) {
            throw DrumDocumentError.invalid
        }
        for step in pattern.steps {
            if let value = step.articulation, !DrumArticulation.available(for: step.voice).contains(value) {
                throw DrumDocumentError.invalid
            }
        }
        for lane in pattern.lanes.values {
            if let value = lane.damping, !value.isFinite || !(0...1).contains(value) { throw DrumDocumentError.invalid }
        }
        guard pattern.bpm.isFinite, (30...300).contains(pattern.bpm), (1...8).contains(pattern.bars),
            [2, 4, 8].contains(pattern.stepsPerBeat),
            pattern.swing.isFinite, (0...0.7).contains(pattern.swing), pattern.humanize.isFinite,
            (0...0.025).contains(pattern.humanize),
            pattern.masterGain.isFinite, (0...1).contains(pattern.masterGain), pattern.steps.count <= 2304,
            pattern.name.count <= 120,
            pattern.steps.allSatisfy({
                $0.index >= 0 && $0.index < pattern.stepCount && $0.velocity.isFinite && (0...1).contains($0.velocity)
                    && $0.pan.isFinite && (-1...1).contains($0.pan) && $0.probability.isFinite
                    && (0...1).contains($0.probability)
            }),
            pattern.lanes.values.allSatisfy({
                $0.gain.isFinite && (0...1.5).contains($0.gain) && $0.pan.isFinite && (-1...1).contains($0.pan)
                    && $0.tuning.isFinite && (-12...12).contains($0.tuning)
            })
        else { throw DrumDocumentError.invalid }
        let identities = pattern.steps.map { "\($0.voice)-\($0.index)" }
        guard Set(identities).count == identities.count else { throw DrumDocumentError.invalid }
    }
    private func trim() { if undoHistory.count > 100 { undoHistory.removeFirst(undoHistory.count - 100) } }
}
enum DrumDocumentError: LocalizedError {
    case invalid, clipping
    var errorDescription: String? {
        self == .invalid
            ? "This drum pattern contains unsupported or invalid values."
            : "The loop exceeds 0 dBFS. Lower Level or a sound's Gain before exporting or inserting."
    }
}
