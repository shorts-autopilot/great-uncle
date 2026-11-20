//! OSC address mapping. Every mapped address becomes ordinary command text,
//! so the remote inherits the engine's validation, lock, journal and undo.

use crate::osc::{OscMessage, OscValue};

#[derive(Debug, Clone, PartialEq)]
pub enum RemoteCommand {
    /// Run this line through `Engine::execute`.
    Line(String),
    /// Feed an external OSC timecode position.
    Timecode(TimecodeValue),
}

#[derive(Debug, Clone, PartialEq)]
pub enum TimecodeValue {
    /// Seconds into the active show.
    Seconds(f64),
    /// `HH:MM:SS:FF` at the active show's frame rate.
    Text(String),
}

/// Map one decoded message. Unknown addresses, wrong argument types and values
/// outside the command grammar return `None`; OSC has no error channel.
pub fn command(message: &OscMessage) -> Option<RemoteCommand> {
    let address = normalize(&message.address)?;
    let parts: Vec<&str> = address.strip_prefix('/')?.split('/').collect();
    match parts.as_slice() {
        ["openlight", "executor", number, action] => {
            let number = number.parse::<u16>().ok().filter(|number| *number > 0)?;
            match *action {
                "go" => Some(RemoteCommand::Line(format!("Go Executor {number}"))),
                "back" => Some(RemoteCommand::Line(format!("Back Executor {number}"))),
                "fader" => Some(RemoteCommand::Line(format!(
                    "Executor {number} At {}",
                    percent(message)?
                ))),
                "flash" => {
                    let on = boolean(message)?;
                    Some(RemoteCommand::Line(format!(
                        "Executor {number} Flash{}",
                        if on { "" } else { " Off" }
                    )))
                }
                _ => None,
            }
        }
        ["openlight", "sequence", number, "go"] => {
            let number = number.parse::<u16>().ok().filter(|number| *number > 0)?;
            Some(RemoteCommand::Line(format!("Go Sequence {number}")))
        }
        ["openlight", "fixture", number, "at"] => {
            let number = number.parse::<u16>().ok().filter(|number| *number > 0)?;
            Some(RemoteCommand::Line(format!(
                "Fixture {number} At {}",
                percent(message)?
            )))
        }
        ["openlight", "group", number, "at"] => {
            let number = number.parse::<u16>().ok().filter(|number| *number > 0)?;
            Some(RemoteCommand::Line(format!(
                "Group {number} At {}",
                percent(message)?
            )))
        }
        ["openlight", "grand"] => Some(RemoteCommand::Line(format!(
            "Master Grand At {}",
            percent(message)?
        ))),
        ["openlight", "blackout"] => Some(RemoteCommand::Line(if boolean(message)? {
            "Blackout".to_string()
        } else {
            "Blackout Off".to_string()
        })),
        ["openlight", "command"] => match message.values.as_slice() {
            [OscValue::String(line)] => Some(RemoteCommand::Line(line.clone())),
            _ => None,
        },
        ["openlight", "timecode"] => match message.values.as_slice() {
            [OscValue::Float(seconds)] if seconds.is_finite() && *seconds >= 0.0 => Some(
                RemoteCommand::Timecode(TimecodeValue::Seconds(f64::from(*seconds))),
            ),
            [OscValue::String(text)] => {
                Some(RemoteCommand::Timecode(TimecodeValue::Text(text.clone())))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Accept both the documented `/openlight/...` scheme and the legacy example
/// `/osc/openlight/...` prefix. A trailing slash is intentionally not stripped:
/// it does not match the schema.
fn normalize(address: &str) -> Option<String> {
    if address.starts_with("/openlight/") {
        return Some(address.to_string());
    }
    address
        .strip_prefix("/osc/openlight/")
        .map(|rest| format!("/openlight/{rest}"))
}

fn percent(message: &OscMessage) -> Option<i64> {
    let [OscValue::Float(value)] = message.values.as_slice() else {
        return None;
    };
    if !value.is_finite() {
        return None;
    }
    Some((f64::from(*value).clamp(0.0, 1.0) * 100.0).round() as i64)
}

fn boolean(message: &OscMessage) -> Option<bool> {
    match message.values.as_slice() {
        [OscValue::Int(0)] => Some(false),
        [OscValue::Int(1)] => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(address: &str, value: OscValue) -> OscMessage {
        OscMessage {
            address: address.into(),
            values: vec![value],
        }
    }

    fn line(address: &str, value: OscValue) -> Option<String> {
        match command(&message(address, value))? {
            RemoteCommand::Line(line) => Some(line),
            RemoteCommand::Timecode(_) => None,
        }
    }

    #[test]
    fn every_documented_address_maps_to_the_exact_command() {
        assert_eq!(
            line("/openlight/executor/1/go", OscValue::Int(0)).as_deref(),
            Some("Go Executor 1")
        );
        assert_eq!(
            line("/openlight/executor/1/back", OscValue::Int(0)).as_deref(),
            Some("Back Executor 1")
        );
        assert_eq!(
            line("/openlight/executor/1/fader", OscValue::Float(0.75)).as_deref(),
            Some("Executor 1 At 75")
        );
        assert_eq!(
            line("/openlight/executor/1/flash", OscValue::Int(1)).as_deref(),
            Some("Executor 1 Flash")
        );
        assert_eq!(
            line("/openlight/executor/1/flash", OscValue::Int(0)).as_deref(),
            Some("Executor 1 Flash Off")
        );
        assert_eq!(
            line("/openlight/sequence/2/go", OscValue::Int(0)).as_deref(),
            Some("Go Sequence 2")
        );
        assert_eq!(
            line("/openlight/fixture/5/at", OscValue::Float(0.5)).as_deref(),
            Some("Fixture 5 At 50")
        );
        assert_eq!(
            line("/openlight/group/3/at", OscValue::Float(1.0)).as_deref(),
            Some("Group 3 At 100")
        );
        assert_eq!(
            line("/openlight/grand", OscValue::Float(0.25)).as_deref(),
            Some("Master Grand At 25")
        );
        assert_eq!(
            line("/openlight/blackout", OscValue::Int(1)).as_deref(),
            Some("Blackout")
        );
        assert_eq!(
            line("/openlight/blackout", OscValue::Int(0)).as_deref(),
            Some("Blackout Off")
        );
        assert_eq!(
            line("/openlight/command", OscValue::String("Cue 2".into())).as_deref(),
            Some("Cue 2")
        );
    }

    #[test]
    fn floats_clamp_and_round_like_the_command_line() {
        assert_eq!(
            line("/openlight/grand", OscValue::Float(-0.5)).as_deref(),
            Some("Master Grand At 0")
        );
        assert_eq!(
            line("/openlight/grand", OscValue::Float(2.0)).as_deref(),
            Some("Master Grand At 100")
        );
        assert_eq!(
            line("/openlight/grand", OscValue::Float(0.749_999_9)).as_deref(),
            Some("Master Grand At 75")
        );
        assert_eq!(line("/openlight/grand", OscValue::Float(f32::NAN)), None);
    }

    #[test]
    fn booleans_only_accept_zero_and_one() {
        assert_eq!(line("/openlight/blackout", OscValue::Int(2)), None);
        assert_eq!(line("/openlight/blackout", OscValue::Float(1.0)), None);
    }

    #[test]
    fn unknown_addresses_and_trailing_slashes_are_unmatched() {
        assert_eq!(
            line("/openlight/executor/1/fader/", OscValue::Float(0.5)),
            None
        );
        assert_eq!(line("/openlight/nope", OscValue::Int(1)), None);
        assert_eq!(line("/other/grand", OscValue::Float(0.5)), None);
        assert_eq!(line("/openlight/executor/0/go", OscValue::Int(0)), None);
    }

    #[test]
    fn the_legacy_osc_prefix_is_accepted() {
        assert_eq!(
            line("/osc/openlight/grand", OscValue::Float(0.5)).as_deref(),
            Some("Master Grand At 50")
        );
    }

    #[test]
    fn timecode_accepts_seconds_or_text() {
        assert_eq!(
            command(&message("/openlight/timecode", OscValue::Float(1.5))),
            Some(RemoteCommand::Timecode(TimecodeValue::Seconds(1.5)))
        );
        assert_eq!(
            command(&message(
                "/openlight/timecode",
                OscValue::String("00:00:02:00".into())
            )),
            Some(RemoteCommand::Timecode(TimecodeValue::Text(
                "00:00:02:00".into()
            )))
        );
    }
}
