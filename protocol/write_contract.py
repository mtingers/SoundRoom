"""Shared numeric domains for callback-stamped Write DTOs; no host capability."""

RANGES = {
    "RemoteAutomationWriteValueRequest": {"sequence": (1, 9007199254740991), "value": (0, 1)},
    "RemoteAutomationWriteEndRequest": {"finalSequence": (0, 9007199254740991)},
    "RemoteAutomationWriteBeginReply": {
        "trackRevision": (0, 9007199254740991),
        "initialValue": (0, 1),
    },
    "RemoteAutomationWriteMapping": {"errorTolerance": (0.000001, 1)},
    "RemoteAutomationWriteLimits": {
        "maximumLeases": (1, 16),
        "maximumCaptureSamples": (1, 65536),
        "maximumDurationFrames": (1, 28800000),
        "maximumWrittenPoints": (1, 4096),
        "maximumLanePoints": (1, 100000),
        "maximumDraftBytes": (1, 33554432),
        "leaseTimeoutMilliseconds": (500, 10000),
        "rampFrames": (16, 480),
    },
    "RemoteAutomationWriteStamp": {
        "transportEpoch": (1, 9007199254740991),
        "outputFrame": (0, 9007199254740991),
        "controlFrame": (0, 9007199254740991),
        "playbackClock": (0, 9007199254740991),
        "latencyOffsetFrames": (0, 9007199254740991),
    },
    "RemoteAutomationWriteValueReply": {
        "sequence": (1, 9007199254740991),
        "appliedSequence": (1, 9007199254740991),
        "targetValue": (0, 1),
        "rampStartValue": (0, 1),
        "rampFrames": (16, 480),
    },
    "RemoteAutomationWriteLeaseUpdate": {
        "revision": (0, 9007199254740991),
        "lastAppliedSequence": (0, 9007199254740991),
    },
    "RemoteAutomationWriteDraftReply": {
        "appliedValueCount": (1, 65536),
        "expiresInSeconds": (1, 3600),
    },
    "RemoteAutomationWriteCapturedRamp": {
        "sequence": (1, 9007199254740991),
        "startValue": (0, 1),
        "targetValue": (0, 1),
        "rampFrames": (16, 480),
    },
    "RemoteAutomationWriteCapturedRelease": {
        "lastControlFrame": (0, 9007199254740991),
        "lastValue": (0, 1),
    },
    "RemoteAutomationWriteDraftContent": {
        "version": (1, 1),
        "trackRevision": (0, 9007199254740991),
        "baseline": (0, 1),
    },
}


def add_artifacts(artifacts, defs):
    import json

    for name, fields in RANGES.items():
        for field, (minimum, maximum) in fields.items():
            target = defs[name]["properties"][field]
            if "anyOf" in target:
                target = target["anyOf"][0]
            target.update(minimum=minimum, maximum=maximum)
    defs["RemoteAutomationWriteTarget"]["allOf"] = [
        {
            "if": {"properties": {"kind": {"enum": ["volume", "pan"]}}},
            "then": {"properties": {"effectID": {"type": "null"}, "parameterID": {"type": "null"}}},
        },
        {
            "if": {"properties": {"kind": {"const": "effectAmount"}}},
            "then": {
                "required": ["effectID"],
                "properties": {
                    "effectID": {"type": "string", "format": "uuid"},
                    "parameterID": {"type": "null"},
                },
            },
        },
        {
            "if": {"properties": {"kind": {"const": "effectParameter"}}},
            "then": {
                "required": ["effectID", "parameterID"],
                "properties": {
                    "effectID": {"type": "string", "format": "uuid"},
                    "parameterID": {"type": "string", "minLength": 1, "maxLength": 128},
                },
            },
        },
    ]
    defs["RemoteAutomationWriteDraftRequest"]["oneOf"] = [
        {
            "required": [one],
            "properties": {one: {"type": "string", "format": "uuid"}, other: {"type": "null"}},
        }
        for one, other in [("draftID", "gestureID"), ("gestureID", "draftID")]
    ]
    for name in (
        "RemoteAutomationWriteLeaseUpdate",
        "RemoteAutomationWriteDraftReply",
        "RemoteAutomationWriteDraftContent",
    ):
        defs[name]["properties"]["reason"].update(maxLength=512)
    defs["RemoteAutomationWriteDraftContent"]["properties"]["capturedRamps"].update(
        minItems=1, maxItems=65536
    )
    defs["RemoteAutomationWriteDraftContent"]["properties"]["originalPoints"].update(
        maxItems=100000
    )
    defs["RemoteAutomationWriteDraftContent"]["properties"]["originalPoints"]["items"] = {
        "allOf": [
            {"$ref": "#/$defs/AutomationPoint"},
            {
                "properties": {
                    "frame": {"minimum": 0, "maximum": 4147200000},
                    "value": {"minimum": 0, "maximum": 1},
                }
            },
        ]
    }
    defs["RemoteAutomationWriteCapturedRamp"]["allOf"] = [
        {
            "if": {"properties": {"restoring": {"const": True}}},
            "then": {"properties": {"sequence": {"type": "null"}}},
            "else": {
                "required": ["sequence"],
                "properties": {
                    "sequence": {"type": "integer", "minimum": 1, "maximum": 9007199254740991}
                },
            },
        }
    ]
    defs["RemoteAutomationWriteCapturedRelease"]["oneOf"] = [
        {
            "required": ["lastControlFrame", "lastValue"],
            "properties": {
                "lastControlFrame": {"type": "integer", "minimum": 0, "maximum": 9007199254740991},
                "lastValue": {"type": "number", "minimum": 0, "maximum": 1},
            },
        },
        {"properties": {"lastControlFrame": {"type": "null"}, "lastValue": {"type": "null"}}},
    ]
    for name, description in {
        "RemoteAutomationWriteStamp": "controlFrame must equal outputFrame + latencyOffsetFrames within safe integer range; these are structural clocks, not an audible-onset promise.",
        "RemoteAutomationWriteMapping": "minimum < maximum; error tolerance is measured in the declared audible domain.",
        "RemoteAutomationWriteLimits": "maximumWrittenPoints <= maximumLanePoints.",
        "RemoteAutomationWriteValueReply": "applied disposition requires appliedSequence==sequence; superseded requires appliedSequence>sequence.",
        "RemoteAutomationWriteLeaseUpdate": "committed phase requires a nonnull authored revision.",
        "RemoteAutomationWriteDraftReply": "Nonempty captures require firstStamp and lastStamp; verified artifact bytes <=32MiB and SHA-256 hex digest.",
        "RemoteAutomationWriteCapturedRamp": "sequence is present exactly when restoring is false. Planned ramp endpoint does not claim every sample was applied.",
        "RemoteAutomationWriteCapturedRelease": "lastControlFrame/lastValue are both present or both absent; if present stamp.controlFrame is exactly lastControlFrame+1.",
        "RemoteAutomationWriteDraftContent": "Version1 bounded review data: one transport epoch/offset, strictly increasing captured source frames/sequences, restoration only last. Optional terminal absence means incomplete, never automatic reapply.",
    }.items():
        defs[name]["description"] = description
    artifacts["protocol/generated/write-ranges.ts"] = (
        "// Generated by protocol/generate.py; do not edit.\nexport const writeRanges = "
        + json.dumps(RANGES, indent=2)
        + " as const;\n"
    )
    rust_header = """// Generated by protocol/generate.py; do not edit.
pub type FieldRange = (&'static str, f64, f64);
pub type ModelRanges = (&'static str, &'static [FieldRange]);
pub const WRITE_RANGES: &[ModelRanges] = &[
"""
    artifacts["protocol/generated/write_ranges.rs"] = (
        rust_header
        + "".join(
            "    ("
            + json.dumps(name)
            + ", &["
            + ",".join(
                "("
                + json.dumps(field)
                + ", "
                + str(float(bounds[0]))
                + ", "
                + str(float(bounds[1]))
                + ")"
                for field, bounds in fields.items()
            )
            + "]),\n"
            for name, fields in RANGES.items()
        )
        + "];\n"
    )
    artifacts["Sources/SoundroomRemote/RemoteAutomationWriteRanges.swift"] = (
        "// Generated by protocol/generate.py; do not edit.\nimport Foundation\nenum RemoteAutomationWriteRanges {\n    static let fields: [String: [(String, Double, Double)]] = [\n"
        + "".join(
            "        "
            + json.dumps(name)
            + ": ["
            + ", ".join(
                "("
                + json.dumps(field)
                + ", "
                + str(float(bounds[0]))
                + ", "
                + str(float(bounds[1]))
                + ")"
                for field, bounds in fields.items()
            )
            + "],\n"
            for name, fields in RANGES.items()
        )
        + "    ]\n}\n"
    )
