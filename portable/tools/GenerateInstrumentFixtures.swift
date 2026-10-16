import Foundation
struct DrumFixture: Codable { var kit: DrumKit; var voice: DrumVoice; var velocity: Double; var robin: Int; var articulation: DrumArticulation; var damping: Double }
struct MIDIContentReference: Codable { var parts: [MIDIPartReference]; var bpm: Double; var beats: Double
 init(_ content: MIDIFileContent) {parts = content.parts.map { MIDIPartReference(id: $0.id, name: $0.name, notes: $0.notes, controllers: $0.controllers) }; bpm = content.bpm; beats = content.beats}
}
struct MIDIPartReference: Codable { var id: Int; var name: String; var notes: [SoundDesignNote]; var controllers: [SoundDesignController] }
@main struct GenerateInstrumentFixtures {
 static func main() throws {
  let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
  try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
  let encoder = JSONEncoder(); encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
  var fixtures: [DrumFixture] = []
  for (k, kit) in DrumKit.allCases.enumerated() {
   for (v, voice) in DrumVoice.allCases.enumerated() {
    let articulation = DrumArticulation.available(for: voice)[(k + v) % DrumArticulation.available(for: voice).count]
    let fixture = DrumFixture(kit: kit, voice: voice, velocity: [0.27, 0.61, 0.93][(k + v) % 3], robin: k * 3 + v, articulation: articulation, damping: 0.15 + Double((k + v) % 7) * 0.12)
    fixtures.append(fixture)
    let audio = DrumSynthesis.render(kit: kit, voice: voice, velocity: fixture.velocity, roundRobin: fixture.robin, articulation: articulation, damping: fixture.damping)
    try WaveFile.encode(audio, float: true).write(to: root.appendingPathComponent("drum-" + kit.rawValue + "-" + voice.rawValue + "-swift.wav"))
   }
  }
  try encoder.encode(fixtures).write(to: root.appendingPathComponent("drum-voices.json"))
  for (name, kit) in [("legacy", DrumKit.acoustic), ("electronic", .electronic), ("extended", .jazzMaple)] {
   var pattern = DrumPattern(); pattern.apply(.breakbeat); pattern.kit = kit; pattern.bpm = 285; pattern.name = name
   if name != "legacy" {
    pattern.swing = 0.32; pattern.humanize = 0.009; pattern.velocityHumanize = 0.12; pattern.variationCycles = 2
    pattern.steps.append(DrumStep(voice: .ride, index: 0, velocity: 0.63, pan: 0.42, probability: 0.72))
    pattern.steps.append(DrumStep(voice: .openHat, index: 3, velocity: 0.53, pan: -0.4, probability: 0.83))
    pattern.steps.append(DrumStep(voice: .crash, index: 0, velocity: 0.37, probability: 0.65))
    var lane = DrumLane(kit: name == "extended" ? .vintageDry : .electronic); lane.damping = 0.73; lane.tuning = -2.3; lane.gain = 0.74; lane.pan = 0.1
    pattern.lanes["snare"] = lane
    pattern.steps[3].articulation = .rimshot
   }
   try encoder.encode(pattern).write(to: root.appendingPathComponent("drum-pattern-" + name + ".json"))
   try WaveFile.encode(pattern.render(repeats: 2, includeTail: name == "extended"), float: true).write(to: root.appendingPathComponent("drum-pattern-" + name + "-swift.wav"))
  }
  var source = SoundDesignPattern(notes: [SoundDesignNote(pitch: 48, velocity: 0.73, startBeat: 0, durationBeats: 0.43), SoundDesignNote(pitch: 67, velocity: 0.54, startBeat: 0.64, durationBeats: 0.71), SoundDesignNote(pitch: 72, velocity: 0.93, startBeat: 1.03, durationBeats: 0.4)], bpm: 287, beats: 2)
  source.notes[0].articulation = .accent; source.notes[1].articulation = .staccato; source.notes[2].articulation = .tenuto
  source.controllers = [.init(kind: .expression, beat: 0, value: 0.87), .init(kind: .pitchBend, beat: 0.2, value: 0.61), .init(kind: .modulation, beat: 0.37, value: 0.68), .init(kind: .sustain, beat: 0.32, value: 1), .init(kind: .expression, beat: 1.2, value: 0.73), .init(kind: .sustain, beat: 1.71, value: 0)]
  source.organic = OrganicSound(drift: 0.34, timbre: 0.46, breath: 0.61, seed: 18446744073709551613)
  var designs: [(String, SoundDesignPattern)] = []
  for preset in SoundDesignPreset.allCases {
   let patch = preset.applying(to: source)
   designs.append(("preset-" + String(SoundDesignPreset.allCases.firstIndex(of: preset)!), patch))
  }
  for waveform in SoundDesignWaveform.allCases {
   var patch = source; patch.waveform = waveform; patch.secondWaveform = .pulse; patch.oscillatorMix = 0.37; patch.detune = -18.2; patch.pulseWidth = 0.23; patch.subLevel = 0.37; patch.noiseLevel = 0.18; patch.drive = 2.3; patch.tone = 0.47; patch.attack = 0.003; patch.decay = 0.17; patch.sustain = 0.63; patch.release = 0.21
   designs.append(("wave-" + waveform.rawValue, patch))
  }
  for organic in OrganicPatch.allCases {designs.append(("organic-" + String(OrganicPatch.allCases.firstIndex(of: organic)!), organic.applying(to: source)))}
  var layered = source; layered.baseGain = 0.2; layered.fineTune = -8
  layered.layers = [SoundDesignPreset.feltUpright, .acousticGuitar, .uprightBass, .vibraphone, .woodenFlute, .soloCello].enumerated().map { i, preset in
   var l = SoundDesignLayer(preset: preset); l.gain = 0.12; l.pan = Double(i) / 3 - 0.8
   l.delay = Double(i) * 0.07; l.fineTune = Double(i) * 3 - 8; l.lowestPitch = 48; l.highestPitch = 69
   l.decayOverride = 0.19; l.sustainOverride = 0.6; return l
  }
  designs.append(("six-layers", layered))
  for (name, patch) in designs {
   try encoder.encode(patch).write(to: root.appendingPathComponent("designer-" + name + ".json"))
   let midi = MIDIFile.encode(patch)
   try midi.write(to: root.appendingPathComponent("designer-" + name + ".mid"))
   try encoder.encode(MIDIContentReference(try MIDIFile.inspect(midi))).write(to: root.appendingPathComponent("midi-" + name + "-swift.json"))
   try WaveFile.encode(patch.render(), float: true).write(to: root.appendingPathComponent("designer-" + name + "-swift.wav"))
   if name == "organic-1" {try WaveFile.encode(patch.renderLoop(), float: true).write(to: root.appendingPathComponent("designer-" + name + "-loop-swift.wav"))}
  }
  for instrument in [MusicalInstrument.woodenFlute, .soloCello, .vibraphone] {
   let p = instrument.applying(to: SoundDesignPattern(notes: [.init(pitch: 57, velocity: 0.8, startBeat: 0, durationBeats: 2)], beats: 2))
   let name = "sustained-" + instrument.rawValue
   try encoder.encode(p).write(to: root.appendingPathComponent("designer-" + name + ".json"))
   try WaveFile.encode(p.render(), float: true).write(to: root.appendingPathComponent("designer-" + name + "-swift.wav"))
  }
  for instrument in MusicalInstrument.allCases {
   var patch = instrument.applying(to: source); patch.character = 0.37; patch.resonance = 0.62
   var voice = SoundDesignVoice(pitch: 61, velocity: 0.83, pattern: patch, beat: 0.25)
   voice.controller(.expression, value: 0.78, immediate: true)
   var audio = [Float](repeating: 0, count: 28000)
   for i in audio.indices {
    if i == 1400 {voice.controller(.pitchBend, value: 0.72)}
    if i == 2400 {voice.controller(.modulation, value: 0.64)}
    if i == 4300 {voice.noteOff()}
    audio[i] = voice.renderNext()
   }
   try encoder.encode(patch).write(to: root.appendingPathComponent("voice-" + instrument.rawValue + ".json"))
   try WaveFile.encode(AudioSamples(channels: [audio]), float: true).write(to: root.appendingPathComponent("voice-" + instrument.rawValue + "-swift.wav"))
  }

  let conductor: [UInt8] = [0, 0xFF, 0x51, 3, 10, 0x2C, 0x2A, 0x87, 0x40, 0xFF, 0x2F, 0]
  let piano: [UInt8] = [0, 0xFF, 3, 5] + Array("Piano".utf8) + [0, 0xC0, 0, 0, 0xB0, 11, 64, 0, 0x90, 60, 100, 0, 64, 90, 0x83, 0x60, 60, 0, 0, 64, 0, 0, 0xFF, 0x2F, 0]
  let bass: [UInt8] = [0, 0xFF, 3, 4] + Array("Bass".utf8) + [0, 0xC1, 32, 0, 0xB1, 11, 127, 0x83, 0x60, 0x91, 36, 80, 0x83, 0x60, 0x81, 36, 0, 0, 0xFF, 0x2F, 0]
  var file = Array("MThd".utf8) + [UInt8](arrayLiteral: 0,0,0,6,0,1,0,3,1,0xE0)
  for track in [conductor,piano,bass] {file += Array("MTrk".utf8) + [UInt8((track.count >> 24) & 255), UInt8((track.count >> 16) & 255), UInt8((track.count >> 8) & 255), UInt8(track.count & 255)] + track}
  try Data(file).write(to: root.appendingPathComponent("midi-format1.mid"))
  try encoder.encode(MIDIContentReference(try MIDIFile.inspect(Data(file)))).write(to: root.appendingPathComponent("midi-format1-swift.json"))

  let bank = SoundDesignInstrument(maximumVoices: 3)
  let bankPatch = SoundDesignPreset.brightLead.applying(to: source)
  var bankLeft = [Float](repeating: 0, count: 24000), bankRight = bankLeft
  for i in bankLeft.indices {
   if [0, 700, 1700, 2700].contains(i) {
    let index = [0, 700, 1700, 2700].firstIndex(of: i)!
    bank.addVoice(SoundDesignVoice(pitch: 55 + index * 4, velocity: 0.54 + Double(index) * 0.12, pattern: bankPatch, keyPitch: 60 + index, gain: 0.82, pan: -0.7 + Double(index) * 0.4, stereo: true, beat: Double(index) * 0.25))
   }
   if i == 1000 { bank.controller(.expression, value: 0.73) }
   if i == 2300 { bank.controller(.sustain, value: 1) }
   if i == 3300 { bank.noteOff(pitch: 61) }
   if i == 4300 { bank.controller(.pitchBend, value: 0.67) }
   if i == 5300 { bank.controller(.sustain, value: 0) }
   if i == 6700 { bank.allNotesOff() }
   bankLeft.withUnsafeMutableBufferPointer { l in bankRight.withUnsafeMutableBufferPointer { r in bank.renderStereo(left: l.baseAddress! + i, right: r.baseAddress! + i, count: 1) } }
  }
  try encoder.encode(bankPatch).write(to: root.appendingPathComponent("voice-bank.json"))
  try WaveFile.encode(AudioSamples(channels: [bankLeft, bankRight]), float: true).write(to: root.appendingPathComponent("voice-bank-swift.wav"))

 }
}
