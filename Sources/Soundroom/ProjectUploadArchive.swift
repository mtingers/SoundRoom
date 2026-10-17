import Compression
import Foundation
import SoundroomCore

enum ProjectUploadArchive {
    /// Samples at most eight large files, at three positions each. ZIP storage
    /// avoids spending seconds compressing PCM that barely shrinks; compressible
    /// audio and metadata still benefit from lossless deflate. Local transfers
    /// favor fast deflate: level 6 spends seconds for small additional savings.
    static func compressionLevel(for project: URL, isLocal: Bool) throws -> Int {
        let keys: Set<URLResourceKey> = [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey]
        guard let paths = FileManager.default.enumerator(at: project, includingPropertiesForKeys: Array(keys)) else {
            return 6
        }
        var files: [(URL, Int)] = []
        for case let url as URL in paths {
            try Task.checkCancellation()
            let values = try url.resourceValues(forKeys: keys)
            guard values.isRegularFile == true, values.isSymbolicLink != true, let size = values.fileSize,
                size >= 65_536
            else { continue }
            files.append((url, size))
            files.sort { $0.1 > $1.1 }
            if files.count > 8 { files.removeLast() }
        }
        var weightedRatio = 0.0
        var weight = 0.0
        for (url, size) in files {
            let reader = try FileHandle(forReadingFrom: url)
            defer { try? reader.close() }
            var ratios: [Double] = []
            for offset in Set([0, max(0, size / 2 - 32_768), max(0, size - 65_536)]) {
                try Task.checkCancellation()
                try reader.seek(toOffset: UInt64(offset))
                guard let sample = try reader.read(upToCount: 65_536), !sample.isEmpty else { continue }
                var output = [UInt8](repeating: 0, count: sample.count + 4096)
                let count = sample.withUnsafeBytes { source in
                    output.withUnsafeMutableBufferPointer { destination in
                        compression_encode_buffer(
                            destination.baseAddress!, destination.count,
                            source.baseAddress!.assumingMemoryBound(to: UInt8.self), source.count, nil, COMPRESSION_ZLIB
                        )
                    }
                }
                ratios.append(count == 0 ? 1 : Double(count) / Double(sample.count))
            }
            if !ratios.isEmpty {
                weightedRatio += ratios.reduce(0, +) / Double(ratios.count) * Double(size)
                weight += Double(size)
            }
        }
        guard weight > 0 else { return 6 }
        let ratio = weightedRatio / weight
        if ratio >= 0.97 { return 0 }
        return isLocal ? 1 : 6
    }
    @discardableResult static func create(_ project: URL, to destination: URL, isLocal: Bool) async throws -> Int {
        let work = Task.detached(priority: .utility) {
            let level = try compressionLevel(for: project, isLocal: isLocal)
            let process = Process()
            process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
            process.arguments = [
                "-c", "-k", "--norsrc", "--keepParent", "--zlibCompressionLevel", String(level), project.path,
                destination.path,
            ]
            process.standardOutput = FileHandle.nullDevice
            process.standardError = FileHandle.nullDevice
            try Task.checkCancellation()
            try process.run()
            process.waitUntilExit()
            try Task.checkCancellation()
            guard process.terminationStatus == 0 else {
                throw SoundroomError.message("The project could not be prepared for upload.")
            }
            return level
        }
        return try await withTaskCancellationHandler(operation: { try await work.value }, onCancel: { work.cancel() })
    }
}
