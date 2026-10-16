use crate::{
    models::{MODEL_ENUMS, MODEL_FIELDS},
    Error,
};
use serde_json::Value;
pub const SAFE_INTEGER: f64 = 9_007_199_254_740_991.;
pub const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_SNAPSHOT_BYTES: usize = 64 * 1024 * 1024;
pub fn uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, b)| {
            if [8, 13, 18, 23].contains(&index) {
                b == b'-'
            } else {
                b.is_ascii_hexdigit()
            }
        })
}
pub fn validate_model(value: &Value, kind: &str) -> Result<(), Error> {
    validate(value, kind, 0)
}
fn validate(value: &Value, kind: &str, depth: usize) -> Result<(), Error> {
    let fail = || Error::Protocol(format!("Invalid {kind} model"));
    if depth > 64 {
        return Err(fail());
    }
    match kind {
        "Bool" => {
            if !value.is_boolean() {
                return Err(fail());
            }
        }
        "String" => {
            if value.as_str().filter(|s| s.len() <= 65536).is_none() {
                return Err(fail());
            }
        }
        "UUID" => {
            if value.as_str().filter(|s| uuid(s)).is_none() {
                return Err(fail());
            }
        }
        "Data" => {
            use base64::Engine;
            if value
                .as_str()
                .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
                .is_none()
            {
                return Err(fail());
            }
        }
        "UInt64String" => {
            if value
                .as_str()
                .filter(|s| {
                    s.parse::<u64>()
                        .map(|v| v.to_string() == *s)
                        .unwrap_or(false)
                })
                .is_none()
            {
                return Err(fail());
            }
        }
        "Int" | "UInt32" | "UInt64" | "Float" | "Double" | "Date" => {
            let n = value.as_f64().ok_or_else(fail)?;
            if !n.is_finite()
                || n.abs() > SAFE_INTEGER
                || ((kind == "Int" || kind == "UInt32" || kind == "UInt64") && n.trunc() != n)
                || ((kind == "UInt32" || kind == "UInt64") && n < 0.)
                || (kind == "UInt32" && n > u32::MAX as f64)
            {
                return Err(fail());
            }
        }
        "Range<Int>" => {
            let pair = value.as_array().filter(|a| a.len() == 2).ok_or_else(fail)?;
            for item in pair {
                validate(item, "Int", depth + 1)?;
            }
            if pair[0].as_i64().ok_or_else(fail)? > pair[1].as_i64().ok_or_else(fail)? {
                return Err(fail());
            }
        }
        _ if kind.starts_with('[') => {
            let inner = &kind[1..kind.len() - 1];
            let mut nesting = 0;
            let mut colon = None;
            for (i, c) in inner.char_indices() {
                match c {
                    '[' | '<' => nesting += 1,
                    ']' | '>' => nesting -= 1,
                    ':' if nesting == 0 => {
                        colon = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            if let Some(i) = colon {
                let object = value
                    .as_object()
                    .filter(|v| v.len() <= 1024)
                    .ok_or_else(fail)?;
                let key = &inner[..i].trim();
                let item_kind = inner[i + 1..].trim();
                let mut ids = std::collections::BTreeSet::new();
                for (name, item) in object {
                    if name.len() > 256 {
                        return Err(fail());
                    }
                    validate(&Value::String(name.clone()), key, depth + 1)?;
                    if *key == "UUID" && !ids.insert(name.to_ascii_uppercase()) {
                        return Err(fail());
                    }
                    validate(item, item_kind, depth + 1)?;
                }
            } else {
                let array = value
                    .as_array()
                    .filter(|v| v.len() <= 100000)
                    .ok_or_else(fail)?;
                for item in array {
                    validate(item, inner, depth + 1)?;
                }
            }
        }
        "RemoteCommand" => {
            tagged(value, crate::models::COMMAND_FIELDS, depth)?;
            validate_command_constraints(value)?;
        }
        _ => {
            if let Some((_, cases)) = MODEL_ENUMS.iter().find(|(name, _)| *name == kind) {
                if value.as_str().filter(|s| cases.contains(s)).is_none() {
                    return Err(fail());
                }
            } else if let Some((_, fields)) = MODEL_FIELDS.iter().find(|(name, _)| *name == kind) {
                let object = value.as_object().ok_or_else(fail)?;
                if object
                    .keys()
                    .any(|key| !fields.iter().any(|f| f.name == key))
                {
                    return Err(fail());
                }
                for field in *fields {
                    match object.get(field.name) {
                        Some(Value::Null) if field.optional => {}
                        Some(item) => validate(item, field.kind, depth + 1)?,
                        None if !field.required => {}
                        _ => return Err(fail()),
                    }
                }
                if ["Track", "RemoteTrackControls"].contains(&kind)
                    && (object
                        .get("gain")
                        .and_then(Value::as_f64)
                        .is_some_and(|gain| !(0. ..=4.).contains(&gain))
                        || object
                            .get("pan")
                            .and_then(Value::as_f64)
                            .is_some_and(|pan| !(-1. ..=1.).contains(&pan)))
                {
                    return Err(fail());
                }
                if kind == "RemoteTrackControlPatch"
                    && (["gain", "pan", "muted", "solo", "armed"]
                        .iter()
                        .all(|name| object.get(*name).map_or(true, Value::is_null))
                        || object
                            .get("gain")
                            .and_then(Value::as_f64)
                            .is_some_and(|gain| !(0. ..=4.).contains(&gain))
                        || object
                            .get("pan")
                            .and_then(Value::as_f64)
                            .is_some_and(|pan| !(-1. ..=1.).contains(&pan)))
                {
                    return Err(fail());
                }
                if kind == "RemoteSessionDelta" {
                    if let Some(patches) = object.get("controlPatches").and_then(Value::as_array) {
                        let mut ids = std::collections::BTreeSet::new();
                        if patches.len() > 1024
                            || patches.iter().any(|patch| {
                                !ids.insert(
                                    patch["trackID"]
                                        .as_str()
                                        .unwrap_or_default()
                                        .to_ascii_uppercase(),
                                )
                            })
                        {
                            return Err(fail());
                        }
                    }
                }
                if kind.starts_with("RemoteAutomationWrite") {
                    crate::write_validation::model(value, kind)?;
                }
                if kind == "RemoteEditorIntent" {
                    crate::write_validation::intent(value)?;
                    let op = object
                        .get("operation")
                        .and_then(Value::as_str)
                        .ok_or_else(fail)?;
                    if !crate::contract::EDITOR_OPERATIONS.contains(&op) {
                        return Err(fail());
                    }
                    if let Some(payload) = object.get("payload").and_then(Value::as_str) {
                        let payload_kind = match op {
                            "designer.preview"
                            | "designer.configure"
                            | "designer.noteOn"
                            | "designer.midiConnect"
                            | "designer.commit" => Some("SoundDesignPattern"),
                            "drums.preview" | "drums.commit" | "drums.export" => {
                                Some("DrumPattern")
                            }
                            "drums.saveKit" => Some("DrumKitDesign"),
                            "effects.preview" | "effects.apply" | "pitch.manualApply" => {
                                Some("[String: Float]")
                            }
                            "meters.subscribe" | "project.export" => Some("[UUID]"),
                            "master.applyProposal" => Some("[RealtimeEffect]"),
                            "client.features" => Some("[String]"),
                            _ => None,
                        };
                        if let Some(payload_kind) = payload_kind {
                            use base64::Engine;
                            let bytes = base64::engine::general_purpose::STANDARD
                                .decode(payload)
                                .map_err(|_| fail())?;
                            let model: Value = serde_json::from_slice(&bytes)?;
                            validate(&model, payload_kind, depth + 1)?;
                        }
                    }
                }
                if kind == "RemoteRequest" {
                    let intent = &value["command"]["intent"];
                    if crate::write_validation::payload_type(
                        intent["operation"].as_str().unwrap_or_default(),
                    )
                    .is_some()
                    {
                        use base64::Engine;
                        let payload: Value = serde_json::from_slice(
                            &base64::engine::general_purpose::STANDARD
                                .decode(intent["payload"].as_str().ok_or_else(fail)?)
                                .map_err(|_| fail())?,
                        )?;
                        let versions = value["expectedTrackRevisions"].as_object();
                        let keys = versions
                            .map(|v| v.keys().collect::<Vec<_>>())
                            .unwrap_or_default();
                        if object.get("expectedRevision").is_some_and(|v| !v.is_null())
                            || (if intent["operation"] == "automation.writeBegin" {
                                keys.len() != 1
                                    || !keys[0].eq_ignore_ascii_case(
                                        payload["trackID"].as_str().unwrap_or_default(),
                                    )
                            } else {
                                !keys.is_empty()
                            })
                        {
                            return Err(fail());
                        }
                    }

                    for key in ["expectedRevision"] {
                        if let Some(value) = object.get(key).filter(|v| !v.is_null()) {
                            if value.as_i64().filter(|v| *v >= 0).is_none() {
                                return Err(fail());
                            }
                        }
                    }
                    if let Some(map) = object
                        .get("expectedTrackRevisions")
                        .and_then(Value::as_object)
                    {
                        if map
                            .values()
                            .any(|v| v.as_i64().filter(|v| *v >= 0).is_none())
                        {
                            return Err(fail());
                        }
                    }
                }
                if kind == "RemoteEditorContext" {
                    for key in ["cursorFrame", "rangeStart", "rangeEnd"] {
                        if let Some(value) = object.get(key).filter(|v| !v.is_null()) {
                            if value.as_i64().filter(|v| *v >= 0).is_none() {
                                return Err(fail());
                            }
                        }
                    }
                }
                if kind == "Session"
                    && object
                        .get("version")
                        .is_some_and(|v| v.as_u64().filter(|v| (1..=11).contains(v)).is_none())
                {
                    return Err(fail());
                }
                if kind == "MusicalTiming"
                    && (object
                        .get("tempo")
                        .and_then(Value::as_f64)
                        .filter(|v| (20. ..=300.).contains(v))
                        .is_none()
                        || object
                            .get("beatsPerBar")
                            .and_then(Value::as_u64)
                            .filter(|v| (1..=16).contains(v))
                            .is_none()
                        || object
                            .get("beatUnit")
                            .and_then(Value::as_u64)
                            .filter(|v| [2, 4, 8, 16].contains(v))
                            .is_none())
                {
                    return Err(fail());
                }
            } else {
                return Err(fail());
            }
        }
    }
    Ok(())
}
pub fn decode_model<T: serde::de::DeserializeOwned>(bytes: &[u8], kind: &str) -> Result<T, Error> {
    if bytes.is_empty() || bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(Error::Protocol("Model exceeds its byte limit".into()));
    }
    let value: Value = serde_json::from_slice(bytes)?;
    validate_model(&value, kind)?;
    Ok(serde_json::from_value(value)?)
}

pub fn validate_command(value: &Value) -> Result<(), Error> {
    tagged(value, crate::models::COMMAND_FIELDS, 0)?;
    validate_command_constraints(value)
}
pub fn validate_event(value: &Value) -> Result<(), Error> {
    tagged(value, crate::models::EVENT_FIELDS, 0)
}
fn tagged(
    value: &Value,
    descriptors: &[(&str, &[crate::models::ModelField])],
    depth: usize,
) -> Result<(), Error> {
    let object = value
        .as_object()
        .ok_or_else(|| Error::Protocol("Tagged message requires an object".into()))?;
    let tag = object
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::Protocol("Missing message type".into()))?;
    let (_, fields) = descriptors
        .iter()
        .find(|(name, _)| *name == tag)
        .ok_or_else(|| Error::Protocol("Unknown message type".into()))?;
    if object
        .keys()
        .any(|key| key != "type" && !fields.iter().any(|f| f.name == key))
    {
        return Err(Error::Protocol("Unknown tagged message field".into()));
    }
    for field in *fields {
        match object.get(field.name) {
            Some(Value::Null) if field.optional => {}
            Some(value) => validate(value, field.kind, depth + 1)?,
            None if !field.required => {}
            _ => {
                return Err(Error::Protocol(
                    "Required tagged message field is missing".into(),
                ))
            }
        }
    }
    Ok(())
}
fn validate_command_constraints(value: &Value) -> Result<(), Error> {
    let object = value.as_object().unwrap();
    let tag = object["type"].as_str().unwrap();
    for key in ["frame", "start", "end", "offset", "bytes", "channel"] {
        if let Some(value) = object.get(key).filter(|v| !v.is_null()) {
            if value.as_i64().filter(|v| *v >= 0).is_none() {
                return Err(Error::Protocol("Negative command position/count".into()));
            }
        }
    }
    if tag == "beginUpload"
        && object["bytes"]
            .as_i64()
            .filter(|v| (1..=2_147_483_648).contains(v))
            .is_none()
    {
        return Err(Error::Protocol("Upload exceeds the supported size".into()));
    }
    if tag == "uploadChunk" {
        use base64::Engine;
        if base64::engine::general_purpose::STANDARD
            .decode(object["data"].as_str().unwrap())
            .map_err(|_| Error::Protocol("Invalid upload bytes".into()))?
            .len()
            > 262144
        {
            return Err(Error::Protocol("Upload chunk is too large".into()));
        }
    }
    Ok(())
}
