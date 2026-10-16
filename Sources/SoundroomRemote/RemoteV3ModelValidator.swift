import Foundation
import CoreFoundation

/// Strict v3 model grammar shared with generated schemas and TypeScript models.
/// This is wire/model validation. The authoritative host still owns permissions,
/// references, resource revisions, device availability and project/audio invariants.
public struct RemoteV3ModelValidationError: Error, LocalizedError, Equatable, Sendable {
    public let path: String
    public let expectedType: String
    public let reason: String
    public var errorDescription: String? { "Invalid control model at \(path) (\(expectedType)): \(reason)" }
}

public enum RemoteV3ModelValidator {
    private static let recordKeys = RemoteV3Models.records.mapValues { Set($0.map(\.name)) }
    public static func validateJSON(_ data: Data, type: String) throws {
        guard !data.isEmpty, data.count <= RemoteSnapshotPages.maximumSnapshotBytes else { throw RemoteError.oversizedFrame }
        try validate(JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed]), type: type)
    }
    static func validate(_ value: Any, type: String, depth: Int = 0, path: String? = nil) throws {
        let path = path ?? type
        func invalid(_ reason: String = "Value does not match the wire grammar.") -> RemoteV3ModelValidationError {
            .init(path: path, expectedType: type, reason: reason)
        }
        guard depth <= 64 else { throw invalid() }
        func numeric(integer: Bool = false, minimum: Double = -RemoteV3Codec.maximumSafeInteger, maximum: Double = RemoteV3Codec.maximumSafeInteger) throws {
            guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite,
                  (minimum...maximum).contains(number.doubleValue), !integer || number.doubleValue.rounded() == number.doubleValue else { throw invalid() }
        }
        switch type {
        case "Int": try numeric(integer: true)
        case "UInt32": try numeric(integer: true, minimum: 0, maximum: Double(UInt32.max))
        case "UInt64": try numeric(integer: true, minimum: 0)
        case "UInt64String": guard let text = value as? String, UInt64(text) != nil, String(UInt64(text)!) == text else { throw invalid() }
        case "Float", "Double", "Date": try numeric()
        case "Bool": guard let number = value as? NSNumber, CFGetTypeID(number) == CFBooleanGetTypeID() else { throw invalid() }
        case "String": guard let text = value as? String, text.utf8.count <= 65536 else { throw invalid() }
        case "UUID": guard let text = value as? String, text.count == 36, UUID(uuidString: text) != nil else { throw invalid() }
        case "Data": guard let text = value as? String, Data(base64Encoded: text) != nil else { throw invalid() }
        case "Range<Int>":
            guard let pair = value as? [Any], pair.count == 2 else { throw invalid() }
            for item in pair { try validate(item, type: "Int", depth: depth + 1, path: path + "[]") }
            guard (pair[0] as! NSNumber).doubleValue <= (pair[1] as! NSNumber).doubleValue else { throw invalid() }
        default:
            if type.hasPrefix("[") {
                let inner = String(type.dropFirst().dropLast())
                var nesting = 0; var delimiter: String.Index?
                for index in inner.indices {
                    let c = inner[index]
                    if c == "[" || c == "<" { nesting += 1 }
                    else if c == "]" || c == ">" { nesting -= 1 }
                    else if c == ":" && nesting == 0 { delimiter = index; break }
                }
                if let delimiter {
                    guard let object = value as? [String: Any], object.count <= 1024 else { throw invalid() }
                    let keyType = String(inner[..<delimiter]).trimmingCharacters(in: .whitespaces)
                    let valueType = String(inner[inner.index(after: delimiter)...]).trimmingCharacters(in: .whitespaces)
                    var identities = Set<UUID>()
                    for (key, item) in object {
                        guard key.utf8.count <= 256 else { throw invalid() }
                        try validate(key, type: keyType, depth: depth + 1, path: path + ".<key>")
                        if keyType == "UUID", let id = UUID(uuidString: key), !identities.insert(id).inserted { throw invalid() }
                        try validate(item, type: valueType, depth: depth + 1, path: path + ".<value>")
                    }
                } else {
                    guard let array = value as? [Any], array.count <= 100000 else { throw invalid() }
                    for (index, item) in array.enumerated() { try validate(item, type: inner, depth: depth + 1, path: path + "[\(index)]") }
                }
            } else if let cases = RemoteV3Models.enums[type] {
                guard let text = value as? String, cases.contains(text) else { throw invalid("Expected one of: " + cases.sorted().joined(separator: ", ")) }
            } else if let fields = RemoteV3Models.records[type] {
                guard let object = value as? [String: Any], object.count <= fields.count, object.keys.allSatisfy({ recordKeys[type]!.contains($0) }) else { throw invalid() }
                for field in fields {
                    if let item = object[field.name], !(item is NSNull) { try validate(item, type: field.type, depth: depth + 1, path: path + "." + field.name) }
                    else if field.required || (object[field.name] is NSNull && !field.optional) { throw RemoteV3ModelValidationError(path: path + "." + field.name, expectedType: field.type, reason: "Required value is missing or null.") }
                }
                if type == "Track" || type == "RemoteTrackControls" {
                    guard object["gain"] as? NSNumber == nil || (0...4).contains((object["gain"] as! NSNumber).doubleValue),
                          object["pan"] as? NSNumber == nil || (-1...1).contains((object["pan"] as! NSNumber).doubleValue) else { throw invalid() }
                }
                if type == "RemoteTrackControlPatch" {
                    guard ["gain", "pan", "muted", "solo", "armed"].contains(where: { object[$0] != nil && !(object[$0] is NSNull) }),
                          object["gain"] as? NSNumber == nil || (0...4).contains((object["gain"] as! NSNumber).doubleValue),
                          object["pan"] as? NSNumber == nil || (-1...1).contains((object["pan"] as! NSNumber).doubleValue) else { throw invalid() }
                }
                if type == "RemoteSessionDelta", let patches = object["controlPatches"] as? [[String: Any]] {
                    let ids = patches.compactMap { ($0["trackID"] as? String).flatMap(UUID.init(uuidString:)) }
                    guard patches.count <= 1024, Set(ids).count == patches.count else { throw invalid() }
                }
                if type.hasPrefix("RemoteAutomationWrite") { try RemoteAutomationWriteGrammar.validate(object, type: type) }
                if type == "RemoteEditorIntent" { _ = try RemoteAutomationWriteGrammar.validateIntent(object) }
                if type == "RemoteRequest", let command = object["command"] as? [String: Any] { try RemoteAutomationWriteGrammar.validateRequest(command: command, revisions: object["expectedTrackRevisions"] as? [String: Any] ?? [:], expectedRevision: object["expectedRevision"]) }
                if type == "Session", let version = object["version"] as? NSNumber, !(1...11).contains(version.intValue) { throw RemoteError.protocolMismatch }
                if type == "MusicalTiming" {
                    guard let tempo = object["tempo"] as? NSNumber, (20...300).contains(tempo.doubleValue),
                          let beats = object["beatsPerBar"] as? NSNumber, (1...16).contains(beats.intValue),
                          let unit = object["beatUnit"] as? NSNumber, [2, 4, 8, 16].contains(unit.intValue) else { throw invalid() }
                }
            } else { throw invalid() }
        }
    }
}
