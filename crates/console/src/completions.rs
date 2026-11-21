//! Shell completion scripts.
//!
//! Keywords come from `HELP` plus a small static list of grammar words and
//! common attributes. This is deliberately a word completer, not a second
//! parser: it never validates a line or reads a show.

use std::collections::BTreeSet;

const SHELLS: &[&str] = &["bash", "zsh", "fish"];

const EXTRA_WORDS: &[&str] = &[
    "At",
    "Back",
    "Begin",
    "Blackout",
    "Blind",
    "Channel",
    "Clear",
    "Clone",
    "Copy",
    "Cue",
    "Delete",
    "Effect",
    "End",
    "Executor",
    "Fixture",
    "Go",
    "Group",
    "Help",
    "Highlight",
    "Home",
    "Import",
    "Load",
    "Lock",
    "Macro",
    "Master",
    "Mode",
    "New",
    "Off",
    "On",
    "Palette",
    "Patch",
    "Pause",
    "Playback",
    "Preview",
    "Record",
    "Release",
    "Report",
    "Save",
    "Sequence",
    "Sheet",
    "Status",
    "Store",
    "Tap",
    "Tempo",
    "Timecode",
    "Type",
    "Undo",
    "Unlock",
    "Update",
    "World",
    "Color",
    "ColorAdd_B",
    "ColorAdd_G",
    "ColorAdd_R",
    "ColorAdd_W",
    "ColorSub_C",
    "ColorSub_M",
    "ColorSub_Y",
    "Cmy",
    "Hsb",
    "Rgb",
    "Dimmer",
    "Pan",
    "Tilt",
    "Zoom",
    "Focus",
    "Gobo",
];

pub fn script(shell: &str) -> Result<String, String> {
    if !SHELLS.iter().any(|known| known.eq_ignore_ascii_case(shell)) {
        return Err(format!(
            "`{shell}` is not a supported shell (choose bash, zsh or fish)"
        ));
    }
    let words = words();
    let list = words.join(" ");
    let script = match shell.to_ascii_lowercase().as_str() {
        "bash" => format!(
            "_openlight() {{\n    local cur\n    cur=\"${{COMP_WORDS[COMP_CWORD]}}\"\n    COMPREPLY=( $(compgen -W \"{list}\" -- \"$cur\") )\n}}\ncomplete -F _openlight openlight\n"
        ),
        "zsh" => format!(
            "#compdef openlight\n_openlight() {{\n    _arguments '1:command:({list})'\n}}\ncompdef _openlight openlight\n"
        ),
        "fish" => words
            .iter()
            .map(|word| format!("complete -c openlight -f -a '{word}'\n"))
            .collect(),
        _ => unreachable!(),
    };
    Ok(script)
}

fn words() -> Vec<String> {
    let mut words: BTreeSet<String> = EXTRA_WORDS.iter().map(|word| (*word).to_string()).collect();
    for line in engine::Engine::empty().help_text() {
        if let Some(word) = line.split_whitespace().next() {
            if word.chars().all(|character| character.is_alphanumeric()) {
                words.insert(word.to_string());
            }
        }
    }
    // Fixture and pool numbers are the other thing worth completing often.
    for number in 1..=99 {
        words.insert(number.to_string());
    }
    words.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shell_gets_a_script_and_unknown_shells_are_errors() {
        for shell in SHELLS {
            let text = script(shell).unwrap();
            assert!(text.contains("openlight"), "{shell}: {text}");
        }
        assert!(script("powershell").unwrap_err().contains("bash"));
        assert!(script("bash").unwrap().contains("Fixture"));
        assert!(script("fish").unwrap().contains("Macro"));
    }

    #[test]
    fn keywords_attributes_and_numbers_are_all_completed() {
        let words = words();
        for keyword in [
            "At", "Begin", "End", "Fixture", "Macro", "Patch", "Store", "Undo",
        ] {
            assert!(words.iter().any(|word| word == keyword), "{keyword}");
        }
        for attribute in ["ColorAdd_R", "Dimmer", "Pan", "Tilt", "Zoom"] {
            assert!(words.iter().any(|word| word == attribute), "{attribute}");
        }
        assert!(words.iter().any(|word| word == "16"));
        let bash = script("bash").unwrap();
        assert!(bash.contains("Macro") && bash.contains("ColorAdd_R") && bash.contains("16"));
    }
}
