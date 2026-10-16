@testable import Soundroom
import AVFoundation
import SoundroomCore

/// Immutable private imports prevent external file edits from invalidating mapped
/// pages. Conversion writes bounded PCM blocks and uses one continuous converter.
enum StreamingAudioFile {
    static let thresholdBytes: Int64 = 16 * 1024 * 1024
    static func read(_ source: URL) throws -> AudioSamples? {
        let bytes = (try? source.resourceValues(forKeys: [.fileSizeKey]).fileSize).map(Int64.init) ?? 0
        guard bytes >= thresholdBytes else { return nil }
        if source.path.contains(".soundroom/Audio/"), let mapped = try? AudioSamples(mappedWaveFile: source) { return mapped }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent("Soundroom Mapped Imports", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let destination = directory.appendingPathComponent(UUID().uuidString).appendingPathExtension("wav")
        do {
            if ["wav", "wave"].contains(source.pathExtension.lowercased()), (try? AudioSamples(mappedWaveFile: source)) != nil {
                try FileManager.default.copyItem(at: source, to: destination)
            } else { try convert(source, to: destination) }
            return try AudioSamples(mappedWaveFile: destination, deleteOnRelease: true)
        } catch { try? FileManager.default.removeItem(at: destination); throw error }
    }
    static func convert(_ source: URL, to destination: URL) throws {
        let file = try AVAudioFile(forReading: source, commonFormat: .pcmFormatFloat32, interleaved: false)
        guard file.length > 0, file.processingFormat.sampleRate > 0,
              let format = AudioPCMFormat.make(rate: sessionRate, channels: file.processingFormat.channelCount),
              let input = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: 8192),
              let output = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 8192),
              let converter = AVAudioConverter(from: file.processingFormat, to: format) else { throw SoundroomError.message("This audio file could not be converted.") }
        var settings = format.settings; settings[AVLinearPCMIsNonInterleaved] = false
        let writer = try AVAudioFile(forWriting: destination, settings: settings, commonFormat: .pcmFormatFloat32, interleaved: false)
        converter.sampleRateConverterQuality = AVAudioQuality.max.rawValue; converter.primeMethod = .none
        converter.channelMap = (0..<Int(file.processingFormat.channelCount)).map { NSNumber(value: $0) }
        let expected = AVAudioFramePosition((Double(file.length) * sessionRate / file.processingFormat.sampleRate).rounded())
        var written: AVAudioFramePosition = 0, readFailure: Error?
        while written < expected {
            try Task.checkCancellation()
            var error: NSError?
            let status = converter.convert(to: output, error: &error) { requested, state in
                do {
                    guard file.framePosition < file.length else { state.pointee = .endOfStream; return nil }
                    try file.read(into: input, frameCount: min(input.frameCapacity, requested, AVAudioFrameCount(min(Int64(UInt32.max), file.length - file.framePosition))))
                    if input.frameLength == 0 { state.pointee = .endOfStream; return nil }
                    state.pointee = .haveData; return input
                } catch { readFailure = error; state.pointee = .endOfStream; return nil }
            }
            if let readFailure { throw readFailure }; if let error { throw error }
            guard status != .error else { throw SoundroomError.message("This audio file could not be converted.") }
            output.frameLength = min(output.frameLength, AVAudioFrameCount(min(Int64(UInt32.max), expected - written)))
            if output.frameLength > 0 { try writer.write(from: output); written += AVAudioFramePosition(output.frameLength) }
            if status == .endOfStream || output.frameLength == 0 { break }
        }
        guard written > 0 else { throw SoundroomError.message("This audio file is empty.") }
    }
    static func writeFloat(_ samples: AudioSamples, to destination: URL) throws {
        guard samples.channelCount > 0, let format = AudioPCMFormat.make(rate: sessionRate, channels: AVAudioChannelCount(samples.channelCount)),
              let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 8192) else { throw SoundroomError.message("The audio source has no channels.") }
        var settings = format.settings; settings[AVLinearPCMIsNonInterleaved] = false
        let file = try AVAudioFile(forWriting: destination, settings: settings, commonFormat: .pcmFormatFloat32, interleaved: false)
        for start in stride(from: 0, to: samples.count, by: 8192) {
            try Task.checkCancellation()
            let count = min(8192, samples.count - start); buffer.frameLength = AVAudioFrameCount(count)
            let block = samples.readFrames(start..<(start + count))
            for channel in 0..<samples.channelCount {
                for frame in 0..<count { let value = block[channel][frame]; buffer.floatChannelData![channel][frame] = value.isFinite ? value : 0 }
            }
            try file.write(from: buffer)
            if start % (8192 * 32) == 0 { samples.discardReadPages() }
        }
    }
}
