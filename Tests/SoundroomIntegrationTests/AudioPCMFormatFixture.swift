@testable import Soundroom
import AVFoundation

enum AudioPCMFormat {
    /// Channels beyond stereo require an explicit layout. Device channels are
    /// discrete microphone inputs rather than surround-speaker positions.
    static func make(rate: Double, channels: AVAudioChannelCount) -> AVAudioFormat? {
        guard rate.isFinite, rate > 0, channels > 0, channels <= 65535 else { return nil }
        if channels <= 2 { return AVAudioFormat(standardFormatWithSampleRate: rate, channels: channels) }
        guard let layout = AVAudioChannelLayout(layoutTag: kAudioChannelLayoutTag_DiscreteInOrder | channels) else { return nil }
        return AVAudioFormat(standardFormatWithSampleRate: rate, channelLayout: layout)
    }
}
