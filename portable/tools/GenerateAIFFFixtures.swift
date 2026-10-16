// Pure Apple AudioFile file APIs. No AudioHardware, AudioUnit, AVAudioEngine or
// device enumeration/opening. Native container interoperability references only.
import Foundation
import AudioToolbox
let root = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
try FileManager.default.createDirectory(at: root, withIntermediateDirectories: false)
func checked(_ status: OSStatus) throws {
    if status != noErr { throw NSError(domain:"SoundroomAIFFFixture",code:Int(status)) }
}
let samples: [Float] = [-1, -0.75, -0.125, 0, 0.125, 0.5, 0.999, 0.25]
var references: [[String:Any]] = []
for (name,bits,isFloat,little,rate,channels) in [
    ("pcm16",16,false,false,48000.0,2),
    ("pcm24",24,false,false,44100.0,2),
    ("pcm32",32,false,false,96000.0,4),
    ("float32",32,true,false,48000.0,2),
    ("float64",64,true,false,192000.0,2),
    ("little16",16,false,true,48000.0,2),
    ("fractional",16,false,false,22254.54,1)
] {
    var format = AudioStreamBasicDescription(mSampleRate:rate,mFormatID:kAudioFormatLinearPCM,
        mFormatFlags:kAudioFormatFlagIsPacked | (isFloat ? kAudioFormatFlagIsFloat : kAudioFormatFlagIsSignedInteger) | (little ? 0 : kAudioFormatFlagIsBigEndian),
        mBytesPerPacket:UInt32(bits/8*channels),mFramesPerPacket:1,mBytesPerFrame:UInt32(bits/8*channels),
        mChannelsPerFrame:UInt32(channels),mBitsPerChannel:UInt32(bits),mReserved:0)
    let url=root.appendingPathComponent(name+".aiff")
    var bytes:[UInt8] = []
    var expected:[[Float]] = Array(repeating:[],count:channels)
    for frame in 0..<samples.count {
        for channel in 0..<channels {
            let sample = samples[frame] * (channel % 2 == 0 ? 1 : -0.5)
            let word: UInt64
            let value: Float
            if isFloat {
                word = bits == 32 ? UInt64(sample.bitPattern) : Double(sample).bitPattern
                value = sample
            } else {
                let scale = pow(2.0,Double(bits-1))
                let integer = Int64((Double(sample)*scale).rounded())
                word = UInt64(bitPattern:integer)
                value = Float(Double(integer)/scale)
            }
            let order = little ? Array(0..<bits/8) : Array((0..<bits/8).reversed())
            for byte in order { bytes.append(UInt8(truncatingIfNeeded:word >> (byte*8))) }
            expected[channel].append(value)
        }
    }
    if name == "fractional" || little {
        // Apple's current AudioFile create rejects sowt and the legacy-rate case.
        // This fixture follows the primary AIFF specification directly instead.
        var data = Data()
        func word<T: FixedWidthInteger>(_ value: T) {
            var big = value.bigEndian
            withUnsafeBytes(of: &big) { data.append(contentsOf: $0) }
        }
        data.append(contentsOf: "FORM".utf8); word(UInt32((little ? 78 : 46)+bytes.count))
        data.append(contentsOf: (little ? "AIFCFVER" : "AIFF").utf8)
        if little { word(UInt32(4)); word(UInt32(0xa2805140)) }
        data.append(contentsOf: "COMM".utf8); word(UInt32(little ? 38 : 18))
        word(UInt16(channels)); word(UInt32(samples.count)); word(UInt16(bits))
        let encoded = rate.bitPattern
        let exponent = Int((encoded >> 52) & 0x7ff) - 1023
        let significand = ((encoded & 0x000fffffffffffff) | 0x0010000000000000) << 11
        word(UInt16(exponent + 16383)); word(significand)
        if little {
            data.append(contentsOf: "sowt".utf8); data.append(14)
            data.append(contentsOf: "not compressed".utf8); data.append(0)
        }
        data.append(contentsOf: "SSND".utf8); word(UInt32(bytes.count+8))
        word(UInt32(0)); word(UInt32(0)); data.append(contentsOf: bytes)
        try data.write(to:url,options:.withoutOverwriting)
    } else {
        var file: AudioFileID?
        try checked(AudioFileCreateWithURL(url as CFURL,isFloat || little ? kAudioFileAIFCType : kAudioFileAIFFType,&format,[],&file))
        var packets=UInt32(samples.count)
        try bytes.withUnsafeBytes { try checked(AudioFileWritePackets(file!,false,UInt32(bytes.count),nil,0,&packets,$0.baseAddress!)) }
        try checked(AudioFileClose(file!))
    }
    var readFile: AudioFileID?
    var nativeRead = false
    if AudioFileOpenURL(url as CFURL,.readPermission,0,&readFile) == noErr {
        var size = UInt32(MemoryLayout<AudioStreamBasicDescription>.size)
        var description = AudioStreamBasicDescription()
        var recovered = Array(repeating:UInt8(0),count:bytes.count)
        var count = UInt32(bytes.count)
        let metadata = AudioFileGetProperty(readFile!,kAudioFilePropertyDataFormat,&size,&description)
        let status = recovered.withUnsafeMutableBytes { AudioFileReadBytes(readFile!,false,0,&count,$0.baseAddress!) }
        nativeRead = metadata == noErr && status == noErr && recovered == bytes && description.mSampleRate == rate
        try checked(AudioFileClose(readFile!))
    }
    references.append(["name":name,"bits":bits,"float":isFloat,"little":little,"sampleRate":rate,"channels":channels,"samples":expected,"nativeCreated":name != "fractional" && !little,"nativeReadVerified":nativeRead])
}
let json=try JSONSerialization.data(withJSONObject:references,options:[.prettyPrinted,.sortedKeys])
try json.write(to:root.appendingPathComponent("native-reference.json"),options:.withoutOverwriting)
print("Generated five actual Apple AudioFile references and two explicit specification references; no device calls")
