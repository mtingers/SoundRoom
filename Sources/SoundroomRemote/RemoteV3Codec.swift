import CoreFoundation
import Foundation
import SoundroomCore

/// Browser/portable protocol boundary. Does not authenticate or execute commands.
/// Feed decoded requests into the same authoritative dispatcher as native v2.
public enum RemoteV3Codec {
    public static let version = 3
    public static let maximumSafeInteger: Double = 9_007_199_254_740_991

    public static func encodeMessage(_ message: RemoteWireMessage, devices: [RemoteDevice] = []) throws -> Data {
        let kind: String
        let fields: [String: Any]
        if case .request = message {
            // Requests already encode their own fields below. Encoding the
            // wrapper first duplicates every upload chunk and dense edit.
            kind = "request"
            fields = [:]
        } else {
            let legacy = try object(JSONEncoder().encode(message))
            guard let tag = legacy.keys.first, let value = legacy[tag] as? [String: Any] else {
                throw RemoteError.invalidFrame
            }
            kind = tag
            fields = value
        }
        var wire: [String: Any] = ["version": version, "kind": kind]
        switch message {
        case .request(let request):
            wire = try object(JSONEncoder().encode(request))
            wire["version"] = version
            wire["kind"] = kind
            wire["expectedTrackRevisions"] = Dictionary(
                uniqueKeysWithValues: request.expectedTrackRevisions.map { ($0.key.uuidString, $0.value) })
            wire["command"] = try transformEditorPayload(
                flatten(wire["command"], descriptors: RemoteV3Descriptor.commands), encoding: true)
            if case .setDevices(let input, let output) = request.command {
                var command: [String: Any] = ["type": "setDevices"]
                for (key, id) in [("inputUID", input), ("outputUID", output)] {
                    if let id {
                        guard let device = devices.first(where: { $0.id == id }) else { throw RemoteError.invalidFrame }
                        command[key] = device.uid
                    }
                }
                wire["command"] = command
            }
        case .event:
            wire["event"] = try flatten(fields["_0"], descriptors: RemoteV3Descriptor.events)
        case .reply: wire.merge(try requiredObject(fields["_0"])) { _, new in new }
        case .challenge, .hello, .welcome:
            wire.merge(try requiredObject(fields["_0"])) { _, new in new }
            wire["version"] = version
        case .failure: wire["message"] = fields["_0"]
        case .heartbeat, .heartbeatReply: wire["id"] = fields["_0"]
        }
        wire = try normalize(wire, encoding: true) as! [String: Any]
        if case .request = message, let command = wire["command"] as? [String: Any],
            let revisions = wire["expectedTrackRevisions"] as? [String: Any]
        {
            try RemoteAutomationWriteGrammar.validateRequest(
                command: command, revisions: revisions, expectedRevision: wire["expectedRevision"])
        }
        try validateTree(wire)
        let data = try JSONSerialization.data(withJSONObject: wire, options: [.sortedKeys, .fragmentsAllowed])
        guard data.count <= RemoteProtocol.maximumFrameBytes else { throw RemoteError.oversizedFrame }
        return data
    }

    public static func decodeMessage(_ data: Data, devices: [RemoteDevice] = []) throws -> RemoteWireMessage {
        guard !data.isEmpty, data.count <= RemoteProtocol.maximumFrameBytes else { throw RemoteError.oversizedFrame }
        var wire = try object(data)
        try validateTree(wire)
        guard let v = wire["version"] as? NSNumber, CFGetTypeID(v) != CFBooleanGetTypeID(),
            v.doubleValue == Double(version),
            let kind = wire["kind"] as? String
        else { throw RemoteError.protocolMismatch }
        wire.removeValue(forKey: "version")
        wire.removeValue(forKey: "kind")
        var legacy: [String: Any]
        switch kind {
        case "request":
            try keys(
                wire, allowed: ["id", "clientID", "expectedRevision", "expectedTrackRevisions", "command"],
                required: ["id", "clientID", "command"])
            try uuid(wire["id"])
            try uuid(wire["clientID"])
            if let revision = wire["expectedRevision"] { try integer(revision, minimum: 0) }
            let revisions = try requiredObject(wire["expectedTrackRevisions"] ?? [:])
            for (id, value) in revisions {
                try uuid(id)
                try integer(value, minimum: 0)
            }
            let command = try requiredObject(wire["command"])
            if command["type"] as? String == "setDevices" {
                try keys(command, allowed: ["type", "inputUID", "outputUID"], required: ["type"])
                var mapped: [String: Any] = [:]
                for (wireKey, legacyKey) in [("inputUID", "inputID"), ("outputUID", "outputID")] {
                    if let raw = command[wireKey], !(raw is NSNull) {
                        guard let uid = raw as? String, let device = devices.first(where: { $0.uid == uid }) else {
                            throw RemoteError.rejected("The selected device is unavailable on this server.")
                        }
                        mapped[legacyKey] = device.id
                    }
                }
                wire["command"] = ["setDevices": mapped]
            } else {
                try validateCommand(command)
                try RemoteAutomationWriteGrammar.validateRequest(
                    command: command, revisions: revisions, expectedRevision: wire["expectedRevision"])
                wire["command"] = try expand(
                    transformEditorPayload(command, encoding: false), descriptors: RemoteV3Descriptor.commands)
            }
            wire["expectedTrackRevisions"] = revisions.flatMap { [$0.key, $0.value] }
            legacy = [kind: ["_0": wire]]
        case "event":
            try keys(wire, allowed: ["event"], required: ["event"])
            legacy = [kind: ["_0": try expand(requiredObject(wire["event"]), descriptors: RemoteV3Descriptor.events)]]
        case "reply":
            try keys(
                wire, allowed: ["requestID", "status", "revision", "message", "payload"],
                required: ["requestID", "status", "revision", "message"])
            try uuid(wire["requestID"])
            try integer(wire["revision"] ?? -1, minimum: 0)
            legacy = [kind: ["_0": wire]]
        case "challenge":
            try keys(wire, allowed: ["serverID", "nonce"], required: ["serverID", "nonce"])
            try uuid(wire["serverID"])
            wire["version"] = RemoteProtocol.version
            legacy = [kind: ["_0": wire]]
        case "hello":
            try keys(
                wire, allowed: ["serverID", "peer", "credentialID", "proof"],
                required: ["serverID", "peer", "credentialID", "proof"])
            try uuid(wire["serverID"])
            try uuid(wire["credentialID"])
            let peer = try requiredObject(wire["peer"])
            try keys(peer, allowed: ["id", "name", "role"], required: ["id", "name", "role"])
            try uuid(peer["id"])
            wire["version"] = RemoteProtocol.version
            legacy = [kind: ["_0": wire]]
        case "welcome":
            try keys(wire, allowed: ["serverID", "name", "role"], required: ["serverID", "name", "role"])
            try uuid(wire["serverID"])
            wire["version"] = RemoteProtocol.version
            legacy = [kind: ["_0": wire]]
        case "failure":
            try keys(wire, allowed: ["message"], required: ["message"])
            legacy = [kind: ["_0": wire["message"]!]]
        case "heartbeat", "heartbeatReply":
            try keys(wire, allowed: ["id"], required: ["id"])
            try uuid(wire["id"])
            legacy = [kind: ["_0": wire["id"]!]]
        default: throw RemoteError.invalidFrame
        }
        legacy = try normalize(legacy, encoding: false) as! [String: Any]
        return try JSONDecoder().decode(RemoteWireMessage.self, from: JSONSerialization.data(withJSONObject: legacy))
    }

    public static func encodeCapabilities(_ capabilities: RemoteCapabilities) throws -> Data {
        let object: [String: Any] = [
            "version": 3, "kind": "capabilities", "capabilities": try self.object(JSONEncoder().encode(capabilities)),
        ]
        let wire = try normalize(object, encoding: true)
        try validateTree(wire)
        let data = try JSONSerialization.data(withJSONObject: wire, options: [.sortedKeys])
        guard data.count <= RemoteProtocol.maximumFrameBytes else { throw RemoteError.oversizedFrame }
        return data
    }
    public static func decodeCapabilities(_ data: Data) throws -> RemoteCapabilities {
        guard data.count <= RemoteProtocol.maximumFrameBytes else { throw RemoteError.oversizedFrame }
        let wire = try object(data)
        try validateTree(wire)
        try keys(wire, allowed: ["version", "kind", "capabilities"], required: ["version", "kind", "capabilities"])
        guard let v = wire["version"] as? NSNumber, CFGetTypeID(v) != CFBooleanGetTypeID(), v.doubleValue == 3,
            wire["kind"] as? String == "capabilities"
        else { throw RemoteError.protocolMismatch }
        try RemoteV3ModelValidator.validate(requiredObject(wire["capabilities"]), type: "RemoteCapabilities")
        let result = try JSONDecoder().decode(
            RemoteCapabilities.self,
            from: JSONSerialization.data(
                withJSONObject: normalize(requiredObject(wire["capabilities"]), encoding: false)))
        guard result.sampleRate.isFinite, (1...768000).contains(result.sampleRate), result.maxFrameBytes > 0,
            result.maxUploadChunkBytes > 0, result.maxUploadBytes >= 0, result.maxTimelineFrame >= 0
        else { throw RemoteError.invalidFrame }
        return result
    }

    public static func encodeSnapshotDocument(_ snapshot: RemoteSnapshot) throws -> Data {
        let wire = try normalize(object(JSONEncoder().encode(snapshot)), encoding: true)
        try validateTree(wire)
        return try JSONSerialization.data(withJSONObject: wire, options: [.sortedKeys])
    }
    public static func decodeSnapshotDocument(_ data: Data) throws -> RemoteSnapshot {
        guard data.count <= RemoteSnapshotPages.maximumSnapshotBytes else { throw RemoteError.oversizedFrame }
        let wire = try object(data)
        try validateTree(wire)
        try RemoteV3ModelValidator.validate(wire, type: "RemoteSnapshot")
        return try JSONDecoder().decode(
            RemoteSnapshot.self, from: JSONSerialization.data(withJSONObject: normalize(wire, encoding: false)))
    }

    private static func object(_ data: Data) throws -> [String: Any] {
        try requiredObject(JSONSerialization.jsonObject(with: data))
    }
    private static func requiredObject(_ value: Any?) throws -> [String: Any] {
        guard let value = value as? [String: Any] else { throw RemoteError.invalidFrame }
        return value
    }
    private static func keys(_ object: [String: Any], allowed: Set<String>, required: Set<String>) throws {
        guard Set(object.keys).isSubset(of: allowed), required.isSubset(of: Set(object.keys)) else {
            throw RemoteError.invalidFrame
        }
    }
    private static func uuid(_ value: Any?) throws {
        guard let text = value as? String, text.count == 36, UUID(uuidString: text) != nil else {
            throw RemoteError.invalidFrame
        }
    }
    private static func integer(
        _ value: Any, minimum: Double = -maximumSafeInteger, maximum: Double = maximumSafeInteger
    ) throws {
        guard let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID(), number.doubleValue.isFinite,
            number.doubleValue.rounded() == number.doubleValue, (minimum...maximum).contains(number.doubleValue)
        else { throw RemoteError.invalidFrame }
    }
    private static func validateTree(_ value: Any, depth: Int = 0) throws {
        guard depth <= 64 else { throw RemoteError.invalidFrame }
        if let object = value as? [String: Any] {
            for (key, child) in object {
                guard key.utf8.count <= 256 else { throw RemoteError.invalidFrame }
                try validateTree(child, depth: depth + 1)
            }
        } else if let array = value as? [Any] {
            for child in array { try validateTree(child, depth: depth + 1) }
        } else if let number = value as? NSNumber, CFGetTypeID(number) != CFBooleanGetTypeID() {
            guard number.doubleValue.isFinite, abs(number.doubleValue) <= maximumSafeInteger else {
                throw RemoteError.invalidFrame
            }
        }
    }
    private static func flatten(
        _ raw: Any?, descriptors: [String: [(name: String, legacy: String, type: String, optional: Bool)]]
    ) throws -> [String: Any] {
        let legacy = try requiredObject(raw)
        guard legacy.count == 1, let tag = legacy.keys.first, let descriptor = descriptors[tag] else {
            throw RemoteError.invalidFrame
        }
        let values = try requiredObject(legacy[tag])
        var result: [String: Any] = ["type": tag]
        for field in descriptor { if let value = values[field.legacy] { result[field.name] = value } }
        return result
    }
    private static func expand(
        _ values: [String: Any], descriptors: [String: [(name: String, legacy: String, type: String, optional: Bool)]]
    ) throws -> [String: Any] {
        guard let tag = values["type"] as? String, let descriptor = descriptors[tag] else {
            throw RemoteError.invalidFrame
        }
        try keys(
            values, allowed: Set(descriptor.map(\.name) + ["type"]),
            required: Set(descriptor.filter { !$0.optional }.map(\.name) + ["type"]))
        var result: [String: Any] = [:]
        for field in descriptor {
            if let value = values[field.name], !(value is NSNull) {
                try validate(value, type: field.type)
                if field.type.hasPrefix("[UUID:") {
                    let object = try requiredObject(value)
                    for id in object.keys { try uuid(id) }
                    result[field.legacy] = object.sorted { $0.key < $1.key }.flatMap { [$0.key, $0.value] }
                } else {
                    result[field.legacy] = value
                }
            } else if !field.optional {
                throw RemoteError.invalidFrame
            }
        }
        return [tag: result]
    }
    private static func validate(_ value: Any, type: String) throws {
        try RemoteV3ModelValidator.validate(value, type: type)
    }
    private static func validateCommand(_ command: [String: Any]) throws {
        guard let tag = command["type"] as? String else { throw RemoteError.invalidFrame }
        if tag == "editor" {
            let intent = try requiredObject(command["intent"])
            try keys(
                intent, allowed: ["operation", "context", "integers", "numbers", "strings", "flags", "payload"],
                required: ["operation", "context", "integers", "numbers", "strings", "flags"])
            guard let operation = intent["operation"] as? String,
                RemoteV3Descriptor.editorOperations.contains(operation)
            else { throw RemoteError.rejected("Unsupported server operation.") }
            _ = try RemoteAutomationWriteGrammar.validateIntent(intent)
            let context = try requiredObject(intent["context"])
            try keys(
                context,
                allowed: [
                    "trackID", "clipID", "clipIDs", "cursorFrame", "rangeStart", "rangeEnd", "effectID",
                    "automationTarget",
                ], required: ["clipIDs", "cursorFrame"])
            for key in ["cursorFrame", "rangeStart", "rangeEnd"] {
                if let value = context[key], !(value is NSNull) { try integer(value, minimum: 0) }
            }
            for key in ["trackID", "clipID", "effectID"] {
                if let value = context[key], !(value is NSNull) { try uuid(value) }
            }
            for (_, value) in try requiredObject(intent["integers"]) { try integer(value) }
            if let payload = intent["payload"], !(payload is NSNull) {
                try validate(payload, type: "Data")
                let payloadTypes: [String: String] = [
                    "designer.preview": "SoundDesignPattern", "designer.configure": "SoundDesignPattern",
                    "designer.noteOn": "SoundDesignPattern", "designer.midiConnect": "SoundDesignPattern",
                    "designer.commit": "SoundDesignPattern",
                    "drums.preview": "DrumPattern", "drums.commit": "DrumPattern", "drums.export": "DrumPattern",
                    "drums.saveKit": "DrumKitDesign",
                    "effects.preview": "[String: Float]", "effects.apply": "[String: Float]",
                    "pitch.manualApply": "[String: Float]", "meters.subscribe": "[UUID]", "project.export": "[UUID]",
                    "master.applyProposal": "[RealtimeEffect]", "client.features": "[String]",
                ]
                if let type = payloadTypes[operation], let text = payload as? String,
                    let data = Data(base64Encoded: text)
                {
                    try RemoteV3ModelValidator.validateJSON(data, type: type)
                }
            }
        }
        for key in ["frame", "start", "end", "offset", "bytes", "channel"] {
            if let value = command[key], !(value is NSNull) { try integer(value, minimum: 0) }
        }
        if tag == "uploadChunk", let text = command["data"] as? String {
            guard let data = Data(base64Encoded: text), data.count <= RemoteProtocol.maximumUploadChunkBytes else {
                throw RemoteError.invalidFrame
            }
        }
        if tag == "beginUpload" { try integer(command["bytes"] ?? -1, minimum: 1, maximum: 2_147_483_648) }
    }
    private static func transformEditorPayload(_ command: [String: Any], encoding: Bool) throws -> [String: Any] {
        guard command["type"] as? String == "editor", var intent = command["intent"] as? [String: Any],
            let text = intent["payload"] as? String, let data = Data(base64Encoded: text),
            let json = try? JSONSerialization.jsonObject(with: data, options: [.fragmentsAllowed])
        else { return command }
        let normalized = try normalize(json, encoding: encoding)
        try validateTree(encoding ? normalized : json)
        intent["payload"] = try JSONSerialization.data(
            withJSONObject: normalized, options: [.sortedKeys, .fragmentsAllowed]
        ).base64EncodedString()
        var result = command
        result["intent"] = intent
        return result
    }
    /// Swift UUID dictionaries use alternating arrays; v3 uses UUID-keyed objects.
    /// UInt64 synthesis seeds are decimal strings, independent of JS number precision.
    private static func normalize(_ value: Any, encoding: Bool, key: String? = nil) throws -> Any {
        if key == "seed" || key == "organicSeed" {
            if value is NSNull { return value }
            if encoding, let n = value as? NSNumber { return n.stringValue }
            if !encoding, let text = value as? String, let n = UInt64(text) { return NSNumber(value: n) }
            throw RemoteError.invalidFrame
        }
        if encoding, key == "trackRevisions" || key == "metersByEffect", let array = value as? [Any] {
            guard array.count % 2 == 0 else { throw RemoteError.invalidFrame }
            var object: [String: Any] = [:]
            for index in stride(from: 0, to: array.count, by: 2) {
                try uuid(array[index])
                object[array[index] as! String] = try normalize(array[index + 1], encoding: true)
            }
            return object
        }
        if !encoding, let object = value as? [String: Any], key == "trackRevisions" || (key == "_0" && !object.isEmpty),
            object.keys.allSatisfy({ UUID(uuidString: $0) != nil })
        {
            return try object.sorted { $0.key < $1.key }.flatMap { [$0.key, try normalize($0.value, encoding: false)] }
        }
        if let object = value as? [String: Any] {
            return try object.mapValuesWithKeys { k, v in try normalize(v, encoding: encoding, key: k) }
        }
        if let array = value as? [Any] { return try array.map { try normalize($0, encoding: encoding) } }
        return value
    }
}
private extension Dictionary where Key == String, Value == Any {
    func mapValuesWithKeys(_ transform: (String, Any) throws -> Any) rethrows -> [String: Any] {
        var result: [String: Any] = [:]
        for (key, value) in self { result[key] = try transform(key, value) }
        return result
    }
}
