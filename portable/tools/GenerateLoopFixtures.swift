import Foundation

/// Pure offline reference renders: does not construct/open audio hardware.
@main struct GenerateLoopFixtures {
 static func main() throws {
  let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
  let names = ["envelopes", "modern-echo", "chorus", "dither", "room-transition", "warmth-transition", "limiter-track", "limiter-master", "resonance-track", "resonance-master", "latency-chain", "mixed-latencies", "pitch-edits", "pitch-automation", "pitch-master", "pitch-chain"]
  var cases: [[String: Any]] = []
  for name in names {
   let folder = root.appendingPathComponent(name + ".soundroom", isDirectory: true)
   let session = try JSONDecoder().decode(Session.self, from: Data(contentsOf: folder.appendingPathComponent("session.json")))
   var assets: [UUID: AudioSamples] = [:]
   for track in session.tracks {
    for clip in track.clips where assets[clip.assetID] == nil {
     guard let decoded = WaveFile.decodeFloat(try Data(contentsOf: folder.appendingPathComponent("Audio/\(clip.assetID.uuidString).wav"))) else { fatalError("Invalid existing fixture media") }
     assets[clip.assetID] = decoded.samples
    }
   }
   for (label, range) in [("long", 5000..<11000), ("short",5990..<6320)] {
    let count = range.count * 4 + 137
    let mixer = RealtimeMixer(session: session, assets: assets, startFrame: range.lowerBound, loopRange: range)
    mixer.reserveCapacity(frameCount: 8192)
    let lp = UnsafeMutablePointer<Float>.allocate(capacity:8192)
    let rp = UnsafeMutablePointer<Float>.allocate(capacity:8192)
    defer { lp.deallocate(); rp.deallocate() }
    var left = [Float](); var right = [Float](); left.reserveCapacity(count); right.reserveCapacity(count)
    while left.count < count {
     if mixer.position == range.upperBound { mixer.rewindForLoop(to: range.lowerBound) }
     let block = min(257, count-left.count, range.upperBound-mixer.position)
     mixer.render(intoLeft:lp, intoRight:rp, frameCount:block)
     left.append(contentsOf: UnsafeBufferPointer(start:lp,count:block)); right.append(contentsOf: UnsafeBufferPointer(start:rp,count:block))
    }
    let output = "loop-\(name)-\(label)-swift.wav"
    try WaveFile.encode(AudioSamples(channels:[left,right]),float:true).write(to:root.appendingPathComponent(output))
    cases.append(["project":name,"start":range.lowerBound,"end":range.upperBound,"frames":count,"reference":output])
   }
  }
  try JSONSerialization.data(withJSONObject:cases, options:[.prettyPrinted,.sortedKeys]).write(to:root.appendingPathComponent("loop-cases.json"))
  print("Generated \(cases.count) actual Swift loop renders without opening hardware")
 }
}
