import Foundation

/// Shared domains only. Host roles, lease ownership and callback proof are separate.
enum RemoteAutomationWriteGrammar {
    static var defaultOperations: [String] {
        RemoteV3Descriptor.editorOperations.filter { !$0.hasPrefix("automation.write") }.sorted()
    }
    static let payloadTypes = [
        "automation.writeBegin": "RemoteAutomationWriteBeginRequest",
        "automation.writeValue": "RemoteAutomationWriteValueRequest",
        "automation.writeEnd": "RemoteAutomationWriteEndRequest",
        "automation.writeDraft": "RemoteAutomationWriteDraftRequest",
    ]
    static func validate(_ object: [String: Any], type: String) throws {
        func fail() -> RemoteV3ModelValidationError {
            .init(path: type, expectedType: type, reason: "Invalid server-stamped Write domain.")
        }
        func number(_ key: String) -> Double { (object[key] as? NSNumber)?.doubleValue ?? .nan }
        func present(_ key: String) -> Bool { object[key] != nil && !(object[key] is NSNull) }
        for (field, minimum, maximum) in RemoteAutomationWriteRanges.fields[type] ?? [] where present(field) {
            guard (minimum...maximum).contains(number(field)) else { throw fail() }
        }
        switch type {
        case "RemoteAutomationWriteTarget":
            let effect = present("effectID")
            let parameter = present("parameterID")
            let kind = object["kind"] as? String
            guard
                kind == "volume" || kind == "pan"
                    ? !effect && !parameter : kind == "effectAmount" ? effect && !parameter : effect && parameter
            else { throw fail() }
            if let key = object["parameterID"] as? String {
                guard !key.isEmpty, key.utf8.count <= 128,
                    !key.unicodeScalars.contains(where: { $0.value < 32 || $0.value == 127 })
                else { throw fail() }
            }
        case "RemoteAutomationWriteDraftRequest": guard present("draftID") != present("gestureID") else { throw fail() }
        case "RemoteAutomationWriteStamp":
            let sum = number("outputFrame") + number("latencyOffsetFrames")
            guard sum <= RemoteV3Codec.maximumSafeInteger, sum == number("controlFrame") else { throw fail() }
        case "RemoteAutomationWriteMapping": guard number("minimum") < number("maximum") else { throw fail() }
        case "RemoteAutomationWriteLimits":
            guard number("maximumWrittenPoints") <= number("maximumLanePoints") else { throw fail() }
        case "RemoteAutomationWriteValueReply":
            guard
                object["disposition"] as? String == "applied"
                    ? number("appliedSequence") == number("sequence") : number("appliedSequence") > number("sequence")
            else { throw fail() }
        case "RemoteAutomationWriteLeaseUpdate":
            guard (object["reason"] as? String)?.utf8.count ?? Int.max <= 512,
                object["phase"] as? String != "committed" || present("revision")
            else { throw fail() }
        case "RemoteAutomationWriteDraftReply":
            guard (object["reason"] as? String)?.utf8.count ?? Int.max <= 512, present("firstStamp"),
                present("lastStamp"),
                let artifact = object["artifact"] as? [String: Any], let bytes = artifact["bytes"] as? NSNumber,
                (1...33554432).contains(bytes.intValue),
                let hash = artifact["sha256"] as? String, hash.utf8.count == 64,
                hash.utf8.allSatisfy({ (48...57).contains($0) || (65...70).contains($0) || (97...102).contains($0) })
            else { throw fail() }
        case "RemoteAutomationWriteCapturedRamp":
            guard (object["restoring"] as? Bool == true) != present("sequence") else { throw fail() }
        case "RemoteAutomationWriteCapturedRelease":
            guard present("lastControlFrame") == present("lastValue") else { throw fail() }
            if present("lastControlFrame") {
                guard let stamp = object["stamp"] as? [String: Any],
                    (stamp["controlFrame"] as? NSNumber)?.doubleValue == number("lastControlFrame") + 1
                else { throw fail() }
            }
        case "RemoteAutomationWriteDraftContent":
            guard (object["reason"] as? String)?.utf8.count ?? Int.max <= 512,
                let original = object["originalPoints"] as? [[String: Any]], original.count <= 100000,
                original.allSatisfy({ p in
                    guard let frame = p["frame"] as? NSNumber, let value = p["value"] as? NSNumber else { return false }
                    return (0...4147200000).contains(frame.doubleValue) && (0...1).contains(value.doubleValue)
                }),
                let ramps = object["capturedRamps"] as? [[String: Any]], !ramps.isEmpty, ramps.count <= 65536,
                ramps[0]["restoring"] as? Bool == false
            else { throw fail() }
            var previousFrame: Double = -1
            var previousSequence: Double = 0
            let firstStamp = ramps[0]["stamp"] as! [String: Any]
            let epoch = (firstStamp["transportEpoch"] as! NSNumber).doubleValue
            let offset = (firstStamp["latencyOffsetFrames"] as! NSNumber).doubleValue
            for (index, ramp) in ramps.enumerated() {
                let stamp = ramp["stamp"] as! [String: Any]
                let frame = (stamp["controlFrame"] as! NSNumber).doubleValue
                guard frame > previousFrame, (stamp["transportEpoch"] as! NSNumber).doubleValue == epoch,
                    (stamp["latencyOffsetFrames"] as! NSNumber).doubleValue == offset,
                    ramp["restoring"] as? Bool != true || index == ramps.count - 1
                else { throw fail() }
                if let sequence = ramp["sequence"] as? NSNumber {
                    guard sequence.doubleValue > previousSequence else { throw fail() }
                    previousSequence = sequence.doubleValue
                }
                previousFrame = frame
            }
            if let terminal = object["terminal"] as? [String: Any] {
                let stamp = terminal["stamp"] as! [String: Any]
                guard (stamp["transportEpoch"] as! NSNumber).doubleValue == epoch,
                    (stamp["latencyOffsetFrames"] as! NSNumber).doubleValue == offset,
                    (terminal["lastControlFrame"] as? NSNumber).map({ $0.doubleValue >= previousFrame }) ?? true
                else { throw fail() }
            }
        default: break
        }
    }
    static func validateRequest(command: [String: Any], revisions: [String: Any], expectedRevision: Any?) throws {
        guard command["type"] as? String == "editor", let intent = command["intent"] as? [String: Any],
            let operation = intent["operation"] as? String, payloadTypes[operation] != nil
        else { return }
        _ = try validateIntent(intent)
        guard expectedRevision == nil || expectedRevision is NSNull,
            let text = intent["payload"] as? String, let data = Data(base64Encoded: text),
            let payload = try JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { throw RemoteError.invalidFrame }
        if operation == "automation.writeBegin" {
            guard revisions.count == 1, let track = payload["trackID"] as? String,
                revisions.keys.first?.caseInsensitiveCompare(track) == .orderedSame
            else { throw RemoteError.invalidFrame }
        } else {
            guard revisions.isEmpty else { throw RemoteError.invalidFrame }
        }
    }
    static func validateIntent(_ object: [String: Any]) throws -> String? {
        guard let operation = object["operation"] as? String, let type = payloadTypes[operation] else { return nil }
        guard let context = object["context"] as? [String: Any],
            context.allSatisfy({ key, value in
                if key == "clipIDs" { return (value as? [Any])?.isEmpty == true }
                if key == "cursorFrame" { return (value as? NSNumber)?.intValue == 0 }
                return value is NSNull
            }),
            ["integers", "numbers", "strings", "flags"].allSatisfy({ (object[$0] as? [String: Any])?.isEmpty == true }),
            let encoded = object["payload"] as? String, let data = Data(base64Encoded: encoded), data.count <= 4096
        else { throw RemoteError.invalidFrame }
        try RemoteV3ModelValidator.validateJSON(data, type: type)
        return type
    }
}
