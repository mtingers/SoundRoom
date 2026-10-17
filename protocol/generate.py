#!/usr/bin/env python3
"""Deterministic contract extraction. --check fails when generated artifacts drift."""

import argparse
import json
import pathlib
import re

from generate_models import add_artifacts
from generate_rust import add_rust_artifacts
from model_contract import body_at, enum_declarations, extract, logical_lines, split_top

ROOT = pathlib.Path(__file__).resolve().parent.parent
source = (ROOT / "Sources/SoundroomRemote/RemoteProtocol.swift").read_text()
aliases = {
    "editTracks": ["edit"],
    "editor": ["intent"],
    "snapshot": ["snapshot"],
    "delta": ["delta"],
    "transport": ["transport"],
    "effectMeters": ["metersByEffect"],
    "devices": ["devices"],
    "presence": ["peers"],
    "midiSources": ["sources"],
    "status": ["message"],
    "waveform": ["asset"],
    "analysis": ["analysis"],
    "projects": ["projects"],
}


def cases(name):
    declaration = re.search(r"public enum " + re.escape(name) + r":[^{]*\{", source)
    if declaration is None:
        raise ValueError("Missing protocol enum " + name)
    block = body_at(source, declaration.end())
    out = {}
    for line in logical_lines(block):
        match = re.match(r"\s*case (\w+)(?:\((.*)\))?$", line)
        if not match:
            continue
        tag, args = match.groups()
        fields = []
        if args:
            for n, arg in enumerate(split_top(args)):
                if re.match(r"^\w+: ", arg):
                    key, typ = arg.split(": ", 1)
                else:
                    key, typ = aliases.get(tag, ["value"])[n], arg
                typ = typ.split(" = ", 1)[0]
                fields.append(
                    {
                        "name": key,
                        "legacy": key if re.match(r"^\w+: ", arg) else "_" + str(n),
                        "type": typ.rstrip("?"),
                        "optional": typ.endswith("?"),
                    }
                )
        if name == "RemoteCommand" and tag == "setDevices":
            for f in fields:
                f.update(name=f["name"].replace("ID", "UID"), type="String")
        out[tag] = fields
    return out


commands, events = cases("RemoteCommand"), cases("RemoteEvent")
# Operation names belong to the wire contract, independently of a host backend.
operations = json.loads((ROOT / "protocol/editor_operations.json").read_text())
if (
    not isinstance(operations, list)
    or not operations
    or any(not isinstance(op, str) or not re.fullmatch(r"[a-zA-Z][\w.]+", op) for op in operations)
    or operations != sorted(set(operations))
):
    raise ValueError("editor_operations.json must contain sorted, unique operation names")
# Grammar descriptors cover all persisted/wire records, including immutable lets.
model_paths = sorted((ROOT / "Sources").glob("SoundroomCore/*.swift")) + sorted(
    (ROOT / "Sources").glob("SoundroomRemote/*.swift")
)
record_fields, raw_enums = extract(model_paths)
for record in record_fields.values():
    for field in record["fields"]:
        if field["type"] == "UInt64" and field["name"] in ("seed", "organicSeed"):
            field.update(type="UInt64String", nativeType="UInt64")
record_fields.setdefault(
    "RemoteWebClientRecord",
    {
        "source": str(ROOT / "Sources/SoundroomRemote/RemoteWebPairing.swift"),
        "fields": [
            {"name": "id", "type": "UUID", "optional": False, "required": True},
            {"name": "name", "type": "String", "optional": False, "required": True},
            {"name": "role", "type": "RemoteRole", "optional": False, "required": True},
        ],
    },
)
for name, fields in {
    "RemoteBrowserCredential": [
        ("version", "Int"),
        ("serverID", "UUID"),
        ("credentialID", "UUID"),
        ("clientSecret", "Data"),
        ("role", "RemoteRole"),
    ],
    "RemoteWebPairingGrant": [
        ("code", "String"),
        ("name", "String"),
        ("role", "RemoteRole"),
        ("expiresInSeconds", "Int"),
    ],
}.items():
    record_fields.setdefault(
        name,
        {
            "source": str(ROOT / "Sources/SoundroomRemote/RemoteWebPairing.swift"),
            "fields": [
                {"name": field, "type": kind, "optional": False, "required": True}
                for field, kind in fields
            ],
        },
    )
models = {
    name: {
        "source": str(pathlib.Path(record["source"]).relative_to(ROOT)),
        "fields": record["fields"],
    }
    for name, record in record_fields.items()
}
manifest = {
    "version": 3,
    "commands": commands,
    "events": events,
    "editorOperations": operations,
    "models": models,
    "enums": raw_enums,
    "limits": {
        "safeInteger": 9007199254740991,
        "maxFrameBytes": 8388608,
        "maxQueueBytes": 33554432,
        "maxUploadChunkBytes": 262144,
        "maxUploadBytes": 2147483648,
        "maxClients": 16,
        "maxPendingRequests": 64,
        "snapshotPageBytes": 524288,
    },
}


def field_schema(f):
    t = f["type"]
    if t == "UUID":
        s = {"type": "string", "format": "uuid"}
    elif t == "String":
        s = {"type": "string", "maxLength": 65536}
    elif t == "Bool":
        s = {"type": "boolean"}
    elif t in ("Int", "UInt32"):
        s = {
            "type": "integer",
            "minimum": 0,
            "maximum": 4294967295 if t == "UInt32" else 9007199254740991,
        }
    elif t in ("Double", "Float"):
        s = {"type": "number"}
    elif t == "SoundDesignControllerKind":
        s = {"type": "string", "enum": ["sustain", "modulation", "expression", "pitchBend"]}
    elif t == "Data":
        s = {"type": "string", "contentEncoding": "base64"}
    elif t.startswith("[") and ":" not in t:
        s = {"type": "array", "items": field_schema({"type": t[1:-1]})}
    elif t in models:
        s = {"$ref": "#/$defs/" + t}
    elif t in raw_enums:
        s = {"type": "string", "enum": raw_enums[t]}
    else:
        s = {"type": "object", "description": t}
    if f.get("optional"):
        s = {"anyOf": [s, {"type": "null"}]}
    return s


schema = {
    "$schema": "https://json-schema.org/draft/2020-12/schema",
    "$id": "https://soundroom.local/protocol/v3/request.schema.json",
    "type": "object",
    "additionalProperties": False,
    "required": ["version", "kind", "id", "clientID", "command"],
    "properties": {
        "version": {"const": 3},
        "kind": {"const": "request"},
        "id": {"type": "string", "format": "uuid"},
        "clientID": {"type": "string", "format": "uuid"},
        "expectedRevision": {"type": "integer", "minimum": 0, "maximum": 9007199254740991},
        "expectedTrackRevisions": {
            "type": "object",
            "propertyNames": {"format": "uuid"},
            "additionalProperties": {"type": "integer", "minimum": 0, "maximum": 9007199254740991},
        },
        "command": {"oneOf": []},
    },
}
for tag, fields in commands.items():
    schema["properties"]["command"]["oneOf"].append(
        {
            "type": "object",
            "additionalProperties": False,
            "properties": {"type": {"const": tag}, **{f["name"]: field_schema(f) for f in fields}},
            "required": ["type"] + [f["name"] for f in fields if not f["optional"]],
        }
    )
ts = [
    "// Generated by protocol/generate.py; do not edit.",
    "import type * as models from './models';",
    "export type JSONValue = null | boolean | number | string | JSONValue[] | {[key:string]:JSONValue};",
]


def ts_type(t):
    if t in ("Int", "UInt32", "Float", "Double"):
        return "number"
    if t in ("UUID", "String", "Data"):
        return "string"
    if t == "Bool":
        return "boolean"
    if t == "SoundDesignControllerKind":
        return '"sustain"|"modulation"|"expression"|"pitchBend"'
    if t.startswith("[") and ":" not in t:
        return ts_type(t[1:-1]) + "[]"
    if t in models:
        return "models." + t
    if t in raw_enums:
        return "|".join(json.dumps(value) for value in raw_enums[t])
    return "{[key:string]:JSONValue}"


ts.append(
    "export type RemoteEvent =\n"
    + " |\n".join(
        " {type:"
        + json.dumps(tag)
        + ";"
        + "".join(
            f"{f['name']}{'?' if f['optional'] else ''}:{ts_type(f['type'])};" for f in fields
        )
        + "}"
        for tag, fields in events.items()
    )
    + ";"
)
ts.append(
    "export type RemoteCommand =\n"
    + " |\n".join(
        " {type:"
        + json.dumps(tag)
        + ";"
        + "".join(
            f"{f['name']}{'?' if f['optional'] else ''}:{ts_type(f['type'])};" for f in fields
        )
        + "}"
        for tag, fields in commands.items()
    )
    + ";"
)
ts.extend(
    [
        "export const commandFields = " + json.dumps(commands, indent=2) + " as const;",
        "export const eventFields = " + json.dumps(events, indent=2) + " as const;",
        "export const editorOperations = " + json.dumps(operations) + " as const;",
        "export const limits = " + json.dumps(manifest["limits"]) + " as const;",
    ]
)
ts.append(
    'export interface RemoteRequest {version:3;kind:"request";id:string;clientID:string;expectedRevision?:number;expectedTrackRevisions?:Record<string,number>;command:RemoteCommand}'
)
rust = [
    "// Generated operation metadata; host validates and dispatches typed commands.",
    "pub const PROTOCOL_VERSION: u32 = 3;",
    "pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;",
    "pub const COMMANDS: &[&str] = &[" + ",".join(json.dumps(t) for t in commands) + "];",
    "pub const EDITOR_OPERATIONS: &[&str] = &["
    + ",".join(json.dumps(t) for t in operations)
    + "];",
]
swift = [
    "// Generated by protocol/generate.py; do not edit.",
    "import Foundation",
    "enum RemoteV3Descriptor {",
    "    static let commands: [String: [(name: String, legacy: String, type: String, optional: Bool)]] = [",
]
for tag, fields in commands.items():
    swift.append(
        "        "
        + json.dumps(tag)
        + ": ["
        + ", ".join(
            "("
            + ", ".join(
                [
                    json.dumps(f["name"]),
                    json.dumps(f["legacy"]),
                    json.dumps(f["type"]),
                    "true" if f["optional"] else "false",
                ]
            )
            + ")"
            for f in fields
        )
        + "],"
    )
swift += [
    "    ]",
    "    static let events: [String: [(name: String, legacy: String, type: String, optional: Bool)]] = [",
]
for tag, fields in events.items():
    swift.append(
        "        "
        + json.dumps(tag)
        + ": ["
        + ", ".join(
            "("
            + ", ".join(
                [
                    json.dumps(f["name"]),
                    json.dumps(f["legacy"]),
                    json.dumps(f["type"]),
                    "true" if f["optional"] else "false",
                ]
            )
            + ")"
            for f in fields
        )
        + "],"
    )
swift += [
    "    ]",
    "    static let editorOperations: Set<String> = ["
    + ", ".join(json.dumps(o) for o in operations)
    + "]",
    "}",
]
artifacts = {
    "protocol/generated/manifest.json": json.dumps(manifest, indent=2) + "\n",
    "protocol/generated/request.schema.json": json.dumps(schema, indent=2) + "\n",
    "protocol/generated/contract.ts": "\n".join(ts) + "\n",
    "protocol/generated/contract.rs": "\n".join(rust) + "\n",
    "Sources/SoundroomRemote/RemoteV3Descriptor.swift": "\n".join(swift) + "\n",
}


def envelope(kind, properties, required=None):
    return {
        "type": "object",
        "additionalProperties": False,
        "properties": {"version": {"const": 3}, "kind": {"const": kind}, **properties},
        "required": ["version", "kind"] + (list(properties) if required is None else required),
    }


uuid_schema = {"type": "string", "format": "uuid"}
text_schema = {"type": "string"}
revision_schema = {"type": "integer", "minimum": 0, "maximum": 9007199254740991}
base64_schema = {"type": "string", "contentEncoding": "base64"}
role_schema = {"type": "string", "enum": ["viewer", "editor", "operator"]}
event_schemas = []
for tag, fields in events.items():
    event_schemas.append(
        {
            "type": "object",
            "additionalProperties": False,
            "properties": {"type": {"const": tag}, **{f["name"]: field_schema(f) for f in fields}},
            "required": ["type"] + [f["name"] for f in fields if not f["optional"]],
        }
    )
peer_schema = {
    "type": "object",
    "additionalProperties": False,
    "required": ["id", "name", "role"],
    "properties": {"id": uuid_schema, "name": text_schema, "role": role_schema},
}
credential_schema = {
    "type": "object",
    "additionalProperties": False,
    "required": ["version", "serverID", "credentialID", "clientSecret", "role"],
    "properties": {
        "version": {"const": 3},
        "serverID": uuid_schema,
        "credentialID": uuid_schema,
        "clientSecret": base64_schema,
        "role": role_schema,
    },
}
messages = [
    envelope(
        "capabilities",
        {
            "capabilities": {
                "type": "object",
                "description": "RemoteCapabilities host facts and factory metadata",
            }
        },
    ),
    schema,
    envelope("event", {"event": {"oneOf": event_schemas}}),
    envelope(
        "reply",
        {
            "requestID": uuid_schema,
            "status": {"enum": ["accepted", "rejected", "conflict", "busy"]},
            "revision": revision_schema,
            "message": text_schema,
            "payload": base64_schema,
        },
        ["requestID", "status", "revision", "message"],
    ),
    envelope("challenge", {"serverID": uuid_schema, "nonce": base64_schema}),
    envelope(
        "hello",
        {
            "serverID": uuid_schema,
            "peer": peer_schema,
            "credentialID": uuid_schema,
            "proof": text_schema,
        },
    ),
    envelope("welcome", {"serverID": uuid_schema, "name": text_schema, "role": role_schema}),
    envelope("pair", {"code": text_schema, "name": text_schema}),
    envelope("paired", {"credential": credential_schema}),
    envelope("failure", {"message": text_schema}),
]
for kind in ["heartbeat", "heartbeatReply"]:
    messages.append(envelope(kind, {"id": uuid_schema}))
for kind in ["snapshotBegin", "snapshotEnd"]:
    messages.append(
        envelope(
            kind,
            {
                "transferID": uuid_schema,
                "revision": revision_schema,
                "totalBytes": {"type": "integer", "minimum": 1, "maximum": 67108864},
                "sha256": {"type": "string", "pattern": "^[0-9a-fA-F]{64}$"},
            },
        )
    )
messages.append(
    envelope(
        "snapshotPage",
        {
            "transferID": uuid_schema,
            "revision": revision_schema,
            "offset": revision_schema,
            "data": base64_schema,
        },
    )
)
artifacts["protocol/generated/messages.schema.json"] = (
    json.dumps(
        {
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$id": "https://soundroom.local/protocol/v3/messages.schema.json",
            "oneOf": messages,
        },
        indent=2,
    )
    + "\n"
)
fixture = {
    "version": 3,
    "kind": "request",
    "id": "12345678-1234-4234-8234-123456789abc",
    "clientID": "12345678-1234-4234-8234-123456789abc",
    "expectedRevision": 7,
    "expectedTrackRevisions": {"12345678-1234-4234-8234-123456789abc": 4},
    "command": {"type": "seek", "frame": 48000},
}
artifacts["protocol/fixtures/request-seek.v3.json"] = json.dumps(fixture, indent=2) + "\n"
artifacts["protocol/fixtures/hello.v3.json"] = (
    json.dumps(
        {
            "version": 3,
            "kind": "hello",
            "serverID": fixture["id"],
            "peer": {"id": fixture["id"], "name": "Browser studio", "role": "viewer"},
            "credentialID": fixture["id"],
            "proof": "fixture-proof-not-a-valid-credential",
        },
        indent=2,
    )
    + "\n"
)

add_artifacts(artifacts, models, raw_enums, schema, commands, ts, enum_declarations(model_paths))

add_rust_artifacts(artifacts, models, raw_enums, commands, events, enum_declarations(model_paths))

check = argparse.ArgumentParser()
check.add_argument("--check", action="store_true")
args = check.parse_args()
for relative, content in artifacts.items():
    path = ROOT / relative
    if args.check:
        if not path.exists() or path.read_text() != content:
            raise SystemExit("Generated contract drift: " + relative)
    else:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
print(
    f"{len(commands)} commands, {len(events)} events, {len(operations)} editor operations, {len(models)} records"
)
