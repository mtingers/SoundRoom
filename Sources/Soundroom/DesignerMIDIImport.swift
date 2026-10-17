import SoundroomCore

/// Import replaces the played sequence, including its performance controllers,
/// while keeping the instrument the user is currently designing.
enum DesignerMIDIImport {
    static func replacingSequence(in instrument: SoundDesignPattern, with imported: SoundDesignPattern)
        -> SoundDesignPattern
    {
        var next = instrument
        next.notes = imported.notes
        next.controllers = imported.controllers
        next.bpm = imported.bpm
        next.beats = imported.beats
        return next
    }
}
