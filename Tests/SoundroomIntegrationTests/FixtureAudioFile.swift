import AVFoundation
import SoundroomCore
@testable import Soundroom

// File-only fixture codec; this type cannot open audio hardware.
enum FixtureAudioFile {
    static func read(_ url: URL) throws -> AudioSamples {
        if let streamed = try StreamingAudioFile.read(url) { return streamed }
        if ["wav", "wave"].contains(url.pathExtension.lowercased()),
           let decoded = WaveFile.decodeFloat(try Data(contentsOf: url, options: .mappedIfSafe)) {
            guard decoded.samples.count > 0 else { throw SoundroomError.message("This audio file is empty.") }
            return try convertedSamples(decoded.samples, from: decoded.rate)
        }
        let file = try AVAudioFile(forReading: url, commonFormat: .pcmFormatFloat32, interleaved: false)
        guard file.length > 0, file.length <= Int64(UInt32.max),
              let buffer = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: 8192) else {
            throw SoundroomError.message("This file is empty or too large to load.")
        }
        var channels = (0..<Int(buffer.format.channelCount)).map { _ in [Float]() }
        for channel in channels.indices { channels[channel].reserveCapacity(Int(file.length)) }
        while file.framePosition < file.length {
            try Task.checkCancellation()
            let before = file.framePosition
            try file.read(into: buffer, frameCount: min(buffer.frameCapacity, AVAudioFrameCount(file.length - before)))
            guard buffer.frameLength > 0, file.framePosition > before, let ptr = buffer.floatChannelData else {
                throw SoundroomError.message("The audio file ended before all its samples could be read.")
            }
            for channel in channels.indices { channels[channel].append(contentsOf: UnsafeBufferPointer(start: ptr[channel], count: Int(buffer.frameLength))) }
        }
        return try convertedSamples(AudioSamples(channels: channels), from: file.processingFormat.sampleRate)
    }
    /// Native anti-aliasing sample-rate conversion for imported audio and takes.
    /// The 48 kHz path retains float WAV values and frame counts exactly.
    static func convertedSamples(_ audio: AudioSamples, from rate: Double) throws -> AudioSamples {
        guard rate != sessionRate else { return audio }
        guard rate > 0, !audio.channels.isEmpty, audio.count > 0,
              audio.channels.allSatisfy({ $0.count == audio.count }), audio.count <= Int(UInt32.max),
              let inputFormat = AudioPCMFormat.make(rate: rate, channels: AVAudioChannelCount(audio.channels.count)),
              let outputFormat = AudioPCMFormat.make(rate: sessionRate, channels: AVAudioChannelCount(audio.channels.count)),
              let input = AVAudioPCMBuffer(pcmFormat: inputFormat, frameCapacity: AVAudioFrameCount(audio.count)),
              let converter = AVAudioConverter(from: inputFormat, to: outputFormat) else {
            throw SoundroomError.message("The audio sample rate could not be converted.")
        }
        let expectedValue = (Double(audio.count) * sessionRate / rate).rounded()
        guard expectedValue.isFinite, expectedValue > 0, expectedValue < Double(UInt32.max) - 256 else {
            throw SoundroomError.message("This audio file is too large to convert.")
        }
        let expected = Int(expectedValue)
        guard expected > 0, expected < Int(UInt32.max) - 256,
              let output = AVAudioPCMBuffer(pcmFormat: outputFormat, frameCapacity: AVAudioFrameCount(expected + 256)) else {
            throw SoundroomError.message("This audio file is too large to convert.")
        }
        input.frameLength = AVAudioFrameCount(audio.count)
        for channel in audio.channels.indices {
            audio.channels[channel].withUnsafeBufferPointer { input.floatChannelData![channel].update(from: $0.baseAddress!, count: audio.count) }
        }
        converter.sampleRateConverterQuality = AVAudioQuality.max.rawValue
        converter.channelMap = audio.channels.indices.map { NSNumber(value: $0) }
        converter.primeMethod = .none
        var supplied = false, error: NSError?
        let status = converter.convert(to: output, error: &error) { _, result in
            if supplied { result.pointee = .endOfStream; return nil }
            supplied = true; result.pointee = .haveData; return input
        }
        if let error { throw error }
        guard status != .error, output.frameLength > 0, let data = output.floatChannelData else {
            throw SoundroomError.message("The audio sample rate could not be converted.")
        }
        let length = min(expected, Int(output.frameLength))
        return AudioSamples(channels: audio.channels.indices.map { Array(UnsafeBufferPointer(start: data[$0], count: length)) })
    }
    static func write(_ audio: AudioSamples, to url: URL, format: String) throws {
        if format == "wav" { try WaveFile.encode(audio).write(to: url, options: .atomic); return }
        guard audio.channelCount > 0 else { throw SoundroomError.message("The audio source has no channels.") }
        let pcmFormat = AVAudioFormat(standardFormatWithSampleRate: sessionRate, channels: 2)!
        let settings: [String: Any] = format == "m4a" ? [AVFormatIDKey: kAudioFormatMPEG4AAC, AVSampleRateKey: sessionRate, AVNumberOfChannelsKey: 2, AVEncoderBitRateKey: 256_000] : [AVFormatIDKey: kAudioFormatLinearPCM, AVSampleRateKey: sessionRate, AVNumberOfChannelsKey: 2, AVLinearPCMBitDepthKey: 24, AVLinearPCMIsBigEndianKey: true, AVLinearPCMIsFloatKey: false]
        let file = try AVAudioFile(forWriting: url, settings: settings, commonFormat: .pcmFormatFloat32, interleaved: false)
        let buffer = AVAudioPCMBuffer(pcmFormat: pcmFormat, frameCapacity: 8192)!
        var offset = 0
        while offset < audio.count {
            let count = min(8192, audio.count - offset)
            buffer.frameLength = AVAudioFrameCount(count)
            let block = audio.readFrames(offset..<(offset + count), channels: [0, min(1, audio.channelCount - 1)])
            for c in 0..<2 { block[c].withUnsafeBufferPointer { buffer.floatChannelData![c].update(from: $0.baseAddress!, count: count) } }
            try file.write(from: buffer); offset += count
        }
    }
}
