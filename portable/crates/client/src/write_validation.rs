use crate::Error;
use serde_json::Value;
#[path = "../../../../protocol/generated/write_ranges.rs"]
mod ranges;
pub(crate) fn payload_type(operation: &str) -> Option<&'static str> {
    match operation {
        "automation.writeBegin" => Some("RemoteAutomationWriteBeginRequest"),
        "automation.writeValue" => Some("RemoteAutomationWriteValueRequest"),
        "automation.writeEnd" => Some("RemoteAutomationWriteEndRequest"),
        "automation.writeDraft" => Some("RemoteAutomationWriteDraftRequest"),
        _ => None,
    }
}
pub(crate) fn model(value: &Value, kind: &str) -> Result<(), Error> {
    let fail = || Error::Protocol(format!("Invalid {kind} Write domain"));
    let present = |key: &str| value.get(key).is_some_and(|v| !v.is_null());
    let n = |key: &str| value[key].as_f64().unwrap_or(f64::NAN);
    if let Some((_, fields)) = ranges::WRITE_RANGES.iter().find(|(name, _)| *name == kind) {
        for (key, minimum, maximum) in *fields {
            if present(key) && !(*minimum..=*maximum).contains(&n(key)) {
                return Err(fail());
            }
        }
    }
    let invalid = match kind {
        "RemoteAutomationWriteTarget" => {
            let effect = present("effectID");
            let parameter = present("parameterID");
            let shape = match value["kind"].as_str() {
                Some("volume" | "pan") => effect || parameter,
                Some("effectAmount") => !effect || parameter,
                _ => !effect || !parameter,
            };
            shape
                || value["parameterID"].as_str().is_some_and(|s| {
                    s.is_empty()
                        || s.len() > 128
                        || s.chars().any(|c| (c as u32) < 32 || c as u32 == 127)
                })
        }
        "RemoteAutomationWriteDraftRequest" => present("draftID") == present("gestureID"),
        "RemoteAutomationWriteStamp" => {
            let sum = n("outputFrame") + n("latencyOffsetFrames");
            sum > crate::validation::SAFE_INTEGER || sum != n("controlFrame")
        }
        "RemoteAutomationWriteMapping" => n("minimum") >= n("maximum"),
        "RemoteAutomationWriteLimits" => n("maximumWrittenPoints") > n("maximumLanePoints"),
        "RemoteAutomationWriteValueReply" => {
            if value["disposition"] == "applied" {
                n("appliedSequence") != n("sequence")
            } else {
                n("appliedSequence") <= n("sequence")
            }
        }
        "RemoteAutomationWriteLeaseUpdate" => {
            value["reason"].as_str().bound_or_missing(|s| s.len() > 512)
                || value["phase"] == "committed" && !present("revision")
        }
        "RemoteAutomationWriteDraftReply" => {
            let artifact = &value["artifact"];
            value["reason"].as_str().bound_or_missing(|s| s.len() > 512)
                || !present("firstStamp")
                || !present("lastStamp")
                || artifact["bytes"]
                    .as_u64()
                    .bound_or_missing(|n| n == 0 || n > 33554432)
                || artifact["sha256"].as_str().bound_or_missing(|s| {
                    s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit())
                })
        }
        "RemoteAutomationWriteCapturedRamp" => (value["restoring"] == true) == present("sequence"),
        "RemoteAutomationWriteCapturedRelease" => {
            present("lastControlFrame") != present("lastValue")
                || (present("lastControlFrame")
                    && value["stamp"]["controlFrame"].as_f64() != Some(n("lastControlFrame") + 1.))
        }
        "RemoteAutomationWriteDraftContent" => {
            let original = value["originalPoints"].as_array().ok_or_else(fail)?;
            let ramps = value["capturedRamps"].as_array().ok_or_else(fail)?;
            if original.len() > 100000
                || ramps.is_empty()
                || ramps.len() > 65536
                || ramps[0]["restoring"] != false
            {
                return Err(fail());
            }
            let first = &ramps[0]["stamp"];
            let mut previous_frame = -1.;
            let mut previous_sequence = 0.;
            for (index, ramp) in ramps.iter().enumerate() {
                let stamp = &ramp["stamp"];
                let frame = stamp["controlFrame"].as_f64().unwrap_or(f64::NAN);
                if frame <= previous_frame
                    || stamp["transportEpoch"] != first["transportEpoch"]
                    || stamp["latencyOffsetFrames"] != first["latencyOffsetFrames"]
                    || (ramp["restoring"] == true && index != ramps.len() - 1)
                {
                    return Err(fail());
                }
                if let Some(sequence) = ramp["sequence"].as_f64() {
                    if sequence <= previous_sequence {
                        return Err(fail());
                    }
                    previous_sequence = sequence;
                }
                previous_frame = frame;
            }
            let terminal = &value["terminal"];
            let stamp = &terminal["stamp"];
            value["reason"].as_str().bound_or_missing(|s| s.len() > 512)
                || original.iter().any(|p| {
                    p["frame"]
                        .as_f64()
                        .bound_or_missing(|f| !(0. ..=4147200000.).contains(&f))
                        || p["value"]
                            .as_f64()
                            .bound_or_missing(|v| !(0. ..=1.).contains(&v))
                })
                || (!terminal.is_null()
                    && (stamp["transportEpoch"] != first["transportEpoch"]
                        || stamp["latencyOffsetFrames"] != first["latencyOffsetFrames"]
                        || terminal["lastControlFrame"]
                            .as_f64()
                            .is_some_and(|f| f < previous_frame)))
        }
        _ => false,
    };
    if invalid {
        return Err(fail());
    }
    Ok(())
}
trait OptionBound<T> {
    fn bound_or_missing(self, p: impl FnOnce(T) -> bool) -> bool;
}
impl<T> OptionBound<T> for Option<T> {
    fn bound_or_missing(self, p: impl FnOnce(T) -> bool) -> bool {
        self.map_or(true, p)
    }
}
pub(crate) fn intent(value: &Value) -> Result<Option<&'static str>, Error> {
    let Some(kind) = payload_type(value["operation"].as_str().unwrap_or_default()) else {
        return Ok(None);
    };
    let fail =
        || Error::Protocol("Write requires only its typed payload, without a client cursor".into());
    let context = value["context"].as_object().ok_or_else(fail)?;
    if context.iter().any(|(key, v)| match key.as_str() {
        "clipIDs" => v.as_array().bound_or_missing(|a| !a.is_empty()),
        "cursorFrame" => v.as_i64() != Some(0),
        _ => !v.is_null(),
    }) || ["integers", "numbers", "strings", "flags"]
        .iter()
        .any(|key| value[key].as_object().bound_or_missing(|o| !o.is_empty()))
    {
        return Err(fail());
    }
    use base64::Engine;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(value["payload"].as_str().ok_or_else(fail)?)
        .map_err(|_| fail())?;
    if bytes.len() > 4096 {
        return Err(fail());
    }
    crate::validation::validate_model(&serde_json::from_slice(&bytes)?, kind)?;
    Ok(Some(kind))
}
