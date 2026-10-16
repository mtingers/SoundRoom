import Foundation
import SoundroomCore

// Server-stamped live Write grammar. Definition is not a host capability.
// Values contain no client time; saved points use the actual control lookup
// frame, while outputFrame names the submitted timeline clock separately.

public enum RemoteAutomationWriteTargetKind: String, Codable, CaseIterable, Sendable {
    case volume, pan, effectAmount, effectParameter
}

public enum RemoteAutomationWriteMode: String, Codable, CaseIterable, Sendable {
    case touch
}

public enum RemoteAutomationWriteValueDisposition: String, Codable, CaseIterable, Sendable {
    case applied, superseded
}

public enum RemoteAutomationWritePhase: String, Codable, CaseIterable, Sendable {
    case writing, finalizing, committed, cancelled, conflict, expired, disconnected, failed
}

public enum RemoteAutomationWriteErrorDomain: String, Codable, CaseIterable, Sendable {
    case physical, decibels, logarithmic
}

public struct RemoteAutomationWriteTarget: Codable, Equatable, Sendable {
    public var kind: RemoteAutomationWriteTargetKind
    public var effectID: UUID?
    public var parameterID: String?
    public init(kind: RemoteAutomationWriteTargetKind, effectID: UUID? = nil, parameterID: String? = nil) {
        self.kind = kind
        self.effectID = effectID
        self.parameterID = parameterID
    }
}

public struct RemoteAutomationWriteBeginRequest: Codable, Equatable, Sendable {
    public var gestureID: UUID
    public var trackID: UUID
    public var target: RemoteAutomationWriteTarget
    public var mode: RemoteAutomationWriteMode
    public init(gestureID: UUID, trackID: UUID, target: RemoteAutomationWriteTarget, mode: RemoteAutomationWriteMode) {
        self.gestureID = gestureID
        self.trackID = trackID
        self.target = target
        self.mode = mode
    }
}

public struct RemoteAutomationWriteValueRequest: Codable, Equatable, Sendable {
    public var gestureID: UUID
    public var sequence: Int
    public var value: Float
    public init(gestureID: UUID, sequence: Int, value: Float) {
        self.gestureID = gestureID
        self.sequence = sequence
        self.value = value
    }
}

public struct RemoteAutomationWriteEndRequest: Codable, Equatable, Sendable {
    public var gestureID: UUID
    public var finalSequence: Int
    public var cancel: Bool
    public init(gestureID: UUID, finalSequence: Int, cancel: Bool) {
        self.gestureID = gestureID
        self.finalSequence = finalSequence
        self.cancel = cancel
    }
}

public struct RemoteAutomationWriteDraftRequest: Codable, Equatable, Sendable {
    public var draftID: UUID?
    public var gestureID: UUID?
    public init(draftID: UUID? = nil, gestureID: UUID? = nil) {
        self.draftID = draftID
        self.gestureID = gestureID
    }
}

public struct RemoteAutomationWriteMapping: Codable, Equatable, Sendable {
    public var minimum: Float
    public var maximum: Float
    public var unit: String
    public var logarithmic: Bool
    public var integer: Bool
    public var interpolationScale: AutomationValueScale
    public var errorDomain: RemoteAutomationWriteErrorDomain
    public var errorTolerance: Float
    public init(minimum: Float, maximum: Float, unit: String, logarithmic: Bool, integer: Bool, interpolationScale: AutomationValueScale, errorDomain: RemoteAutomationWriteErrorDomain, errorTolerance: Float) {
        self.minimum = minimum
        self.maximum = maximum
        self.unit = unit
        self.logarithmic = logarithmic
        self.integer = integer
        self.interpolationScale = interpolationScale
        self.errorDomain = errorDomain
        self.errorTolerance = errorTolerance
    }
}

public struct RemoteAutomationWriteLimits: Codable, Equatable, Sendable {
    public var maximumLeases: Int
    public var maximumCaptureSamples: Int
    public var maximumDurationFrames: Int
    public var maximumWrittenPoints: Int
    public var maximumLanePoints: Int
    public var maximumDraftBytes: Int
    public var leaseTimeoutMilliseconds: Int
    public var rampFrames: Int
    public init(maximumLeases: Int, maximumCaptureSamples: Int, maximumDurationFrames: Int, maximumWrittenPoints: Int, maximumLanePoints: Int, maximumDraftBytes: Int, leaseTimeoutMilliseconds: Int, rampFrames: Int) {
        self.maximumLeases = maximumLeases
        self.maximumCaptureSamples = maximumCaptureSamples
        self.maximumDurationFrames = maximumDurationFrames
        self.maximumWrittenPoints = maximumWrittenPoints
        self.maximumLanePoints = maximumLanePoints
        self.maximumDraftBytes = maximumDraftBytes
        self.leaseTimeoutMilliseconds = leaseTimeoutMilliseconds
        self.rampFrames = rampFrames
    }
}

public struct RemoteAutomationWriteStamp: Codable, Equatable, Sendable {
    public var transportEpoch: Int
    public var outputFrame: Int
    public var controlFrame: Int
    public var playbackClock: Int
    public var latencyOffsetFrames: Int
    public init(transportEpoch: Int, outputFrame: Int, controlFrame: Int, playbackClock: Int, latencyOffsetFrames: Int) {
        self.transportEpoch = transportEpoch
        self.outputFrame = outputFrame
        self.controlFrame = controlFrame
        self.playbackClock = playbackClock
        self.latencyOffsetFrames = latencyOffsetFrames
    }
}

public struct RemoteAutomationWriteBeginReply: Codable, Equatable, Sendable {
    public var gestureID: UUID
    public var trackID: UUID
    public var target: RemoteAutomationWriteTarget
    public var trackRevision: Int
    public var initialValue: Float
    public var mapping: RemoteAutomationWriteMapping
    public var limits: RemoteAutomationWriteLimits
    public var stamp: RemoteAutomationWriteStamp
    public init(gestureID: UUID, trackID: UUID, target: RemoteAutomationWriteTarget, trackRevision: Int, initialValue: Float, mapping: RemoteAutomationWriteMapping, limits: RemoteAutomationWriteLimits, stamp: RemoteAutomationWriteStamp) {
        self.gestureID = gestureID
        self.trackID = trackID
        self.target = target
        self.trackRevision = trackRevision
        self.initialValue = initialValue
        self.mapping = mapping
        self.limits = limits
        self.stamp = stamp
    }
}

public struct RemoteAutomationWriteValueReply: Codable, Equatable, Sendable {
    public var gestureID: UUID
    public var sequence: Int
    public var appliedSequence: Int
    public var disposition: RemoteAutomationWriteValueDisposition
    public var targetValue: Float
    public var rampStartValue: Float
    public var rampFrames: Int
    public var stamp: RemoteAutomationWriteStamp
    public init(gestureID: UUID, sequence: Int, appliedSequence: Int, disposition: RemoteAutomationWriteValueDisposition, targetValue: Float, rampStartValue: Float, rampFrames: Int, stamp: RemoteAutomationWriteStamp) {
        self.gestureID = gestureID
        self.sequence = sequence
        self.appliedSequence = appliedSequence
        self.disposition = disposition
        self.targetValue = targetValue
        self.rampStartValue = rampStartValue
        self.rampFrames = rampFrames
        self.stamp = stamp
    }
}

public struct RemoteAutomationWriteLeaseUpdate: Codable, Equatable, Sendable {
    public var gestureID: UUID
    public var phase: RemoteAutomationWritePhase
    public var reason: String
    public var revision: Int?
    public var lastAppliedSequence: Int?
    public var stamp: RemoteAutomationWriteStamp?
    public var draftID: UUID?
    public init(gestureID: UUID, phase: RemoteAutomationWritePhase, reason: String, revision: Int? = nil, lastAppliedSequence: Int? = nil, stamp: RemoteAutomationWriteStamp? = nil, draftID: UUID? = nil) {
        self.gestureID = gestureID
        self.phase = phase
        self.reason = reason
        self.revision = revision
        self.lastAppliedSequence = lastAppliedSequence
        self.stamp = stamp
        self.draftID = draftID
    }
}

public struct RemoteAutomationWriteDraftReply: Codable, Equatable, Sendable {
    public var draftID: UUID
    public var gestureID: UUID
    public var trackID: UUID
    public var target: RemoteAutomationWriteTarget
    public var reason: String
    public var appliedValueCount: Int
    public var firstStamp: RemoteAutomationWriteStamp?
    public var lastStamp: RemoteAutomationWriteStamp?
    public var mapping: RemoteAutomationWriteMapping
    public var expiresInSeconds: Int
    public var artifact: RemoteArtifact
    public init(draftID: UUID, gestureID: UUID, trackID: UUID, target: RemoteAutomationWriteTarget, reason: String, appliedValueCount: Int, firstStamp: RemoteAutomationWriteStamp? = nil, lastStamp: RemoteAutomationWriteStamp? = nil, mapping: RemoteAutomationWriteMapping, expiresInSeconds: Int, artifact: RemoteArtifact) {
        self.draftID = draftID
        self.gestureID = gestureID
        self.trackID = trackID
        self.target = target
        self.reason = reason
        self.appliedValueCount = appliedValueCount
        self.firstStamp = firstStamp
        self.lastStamp = lastStamp
        self.mapping = mapping
        self.expiresInSeconds = expiresInSeconds
        self.artifact = artifact
    }
}

/// A started ramp, not proof that its planned endpoint was ever sampled.
public struct RemoteAutomationWriteCapturedRamp: Codable, Equatable, Sendable {
    public var sequence: Int?
    public var restoring: Bool
    public var stamp: RemoteAutomationWriteStamp
    public var startValue: Float
    public var targetValue: Float
    public var rampFrames: Int
    public init(sequence: Int? = nil, restoring: Bool, stamp: RemoteAutomationWriteStamp, startValue: Float, targetValue: Float, rampFrames: Int) {
        self.sequence = sequence; self.restoring = restoring; self.stamp = stamp
        self.startValue = startValue; self.targetValue = targetValue; self.rampFrames = rampFrames
    }
}
public struct RemoteAutomationWriteCapturedRelease: Codable, Equatable, Sendable {
    public var stamp: RemoteAutomationWriteStamp
    public var lastControlFrame: Int?
    public var lastValue: Float?
    public init(stamp: RemoteAutomationWriteStamp, lastControlFrame: Int? = nil, lastValue: Float? = nil) {
        self.stamp = stamp; self.lastControlFrame = lastControlFrame; self.lastValue = lastValue
    }
}
/// Verified artifact JSON. An incomplete terminal is review data, never an
/// automatically applicable authored curve or a promise of audible endpoints.
public struct RemoteAutomationWriteDraftContent: Codable, Equatable, Sendable {
    public var version: Int
    public var gestureID: UUID
    public var trackID: UUID
    public var target: RemoteAutomationWriteTarget
    public var trackRevision: Int
    public var baseline: Float
    public var mapping: RemoteAutomationWriteMapping
    public var reason: String
    public var originalPoints: [AutomationPoint]
    public var capturedRamps: [RemoteAutomationWriteCapturedRamp]
    public var terminal: RemoteAutomationWriteCapturedRelease?
    public init(version: Int = 1, gestureID: UUID, trackID: UUID, target: RemoteAutomationWriteTarget, trackRevision: Int, baseline: Float, mapping: RemoteAutomationWriteMapping, reason: String, originalPoints: [AutomationPoint], capturedRamps: [RemoteAutomationWriteCapturedRamp], terminal: RemoteAutomationWriteCapturedRelease? = nil) {
        self.version = version; self.gestureID = gestureID; self.trackID = trackID; self.target = target
        self.trackRevision = trackRevision; self.baseline = baseline; self.mapping = mapping; self.reason = reason
        self.originalPoints = originalPoints; self.capturedRamps = capturedRamps; self.terminal = terminal
    }
}
