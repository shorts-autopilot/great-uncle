//! Property tests for the parsers that read input from outside the project.
//!
//! The command line, the show file, the desk file and the timecode parser all
//! consume text an operator or another program produced. None of them may
//! panic or loop on any input; either the input is rejected with an error, or
//! it produces a value that passes the same validation a file load uses.
//!
//! The fuzz targets in `fuzz/` explore the same surfaces with coverage
//! guidance. These properties run on every `cargo test`, so a regression is
//! caught without a nightly toolchain.

use engine::command::{lex, parse};
use engine::desk::DeskFile;
use engine::timecode;
use engine::Show;
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, ..ProptestConfig::default() })]

    #[test]
    fn command_lines_never_panic(line in "\\PC{0,120}") {
        if let Ok(tokens) = lex(&line) {
            let _ = parse(&tokens);
        }
    }

    #[test]
    fn show_documents_never_panic(text in "\\PC{0,600}") {
        if let Ok(show) = text.parse::<Show>() {
            prop_assert!(
                show.validate().is_empty(),
                "a parsed show must validate: {:?}",
                show.validate()
            );
        }
    }

    #[test]
    fn desk_documents_never_panic(text in "\\PC{0,600}") {
        if let Ok(desk) = serde_json::from_str::<DeskFile>(&text) {
            let _ = serde_json::to_string(&desk);
        }
    }

    #[test]
    fn timecode_parse_is_total(text in "\\PC{0,80}") {
        let _ = timecode::parse(&text, 25);
    }

    #[test]
    fn timecode_round_trips(
        hours in 0u8..24,
        minutes in 0u8..60,
        seconds in 0u8..60,
        frames in 0u8..30,
        fps in 24u8..31,
    ) {
        prop_assume!(u16::from(frames) < u16::from(fps));
        let total = (u64::from(hours) * 3600 + u64::from(minutes) * 60 + u64::from(seconds))
            * u64::from(fps)
            + u64::from(frames);
        let text = timecode::format(total, fps);
        prop_assert_eq!(timecode::parse(&text, fps), Ok(total));
    }
}
