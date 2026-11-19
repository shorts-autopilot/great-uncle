//! The command language shared by every input surface.
//!
//! Phase 1 grammar:
//!
//! ```text
//! Fixture 1 Thru 5 [At 50 | Full | Off]    selection and intensity
//! 1 Thru 5 At 50                           fixture keyword is optional
//! At 50 | Full | Off                       apply to the current selection
//! Clear [All]                              clear the selection
//! Patch Fixture 1 At 1.001 Type "X" [Mode "4ch"]
//! Unpatch Fixture 1 Thru 3
//! Import Gdtf "file.gdtf"
//! Sheet | Status | Save "show.json" | Load "show.json" | Help
//! ```
//!
//! `universe.address` uses a dot: `1.001` is universe 1, address 1.

use std::fmt;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Word(String),
    Number(String),
    Quoted(String),
    Plus,
    Thru,
}

impl fmt::Display for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Word(word) => write!(formatter, "`{word}`"),
            Token::Number(number) => write!(formatter, "`{number}`"),
            Token::Quoted(text) => write!(formatter, "`\"{text}\"`"),
            Token::Plus => write!(formatter, "`+`"),
            Token::Thru => write!(formatter, "`Thru`"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Level {
    /// 0..100.
    Percent(f64),
    Full,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Select(Vec<u16>),
    SetLevel {
        fixtures: Option<Vec<u16>>,
        level: Level,
    },
    Off {
        fixtures: Option<Vec<u16>>,
    },
    Clear,
    Patch {
        fixture: u16,
        universe: u16,
        address: u16,
        type_key: Option<String>,
        mode: Option<String>,
    },
    Unpatch {
        fixtures: Vec<u16>,
    },
    ImportGdtf {
        path: String,
    },
    Sheet,
    Status,
    Save {
        path: String,
    },
    Load {
        path: String,
    },
    Help,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    pub message: String,
    pub hint: Option<String>,
}

impl CommandError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: Some(hint.into()),
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(formatter, " ({hint})")?;
        }
        Ok(())
    }
}

impl std::error::Error for CommandError {}

/// Split a line into `;` separated statements, ignoring separators in quotes.
pub fn split_statements(line: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    for (index, character) in line.char_indices() {
        match quote {
            Some(active) if character == active => quote = None,
            Some(_) => {}
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character == ';' => {
                statements.push(line[start..index].trim());
                start = index + character.len_utf8();
            }
            None => {}
        }
    }
    statements.push(line[start..].trim());
    statements
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect()
}

pub fn lex(line: &str) -> Result<Vec<Token>, CommandError> {
    let chars: Vec<char> = line.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];
        if character.is_whitespace() {
            index += 1;
            continue;
        }
        match character {
            '+' => {
                tokens.push(Token::Plus);
                index += 1;
            }
            '"' | '\'' => {
                let quote = character;
                index += 1;
                let mut text = String::new();
                let mut closed = false;
                while index < chars.len() {
                    let current = chars[index];
                    if current == '\\' && index + 1 < chars.len() {
                        text.push(chars[index + 1]);
                        index += 2;
                        continue;
                    }
                    if current == quote {
                        closed = true;
                        index += 1;
                        break;
                    }
                    text.push(current);
                    index += 1;
                }
                if !closed {
                    return Err(CommandError::with_hint(
                        "unterminated quoted string",
                        "close the string with the same quote character",
                    ));
                }
                tokens.push(Token::Quoted(text));
            }
            digit if digit.is_ascii_digit() => {
                let start = index;
                while index < chars.len() && (chars[index].is_ascii_digit() || chars[index] == '.')
                {
                    index += 1;
                }
                // `4ch` is a name, `50%` is a number followed by a suffix.
                if index < chars.len() && (chars[index].is_alphabetic() || chars[index] == '_') {
                    while index < chars.len() && is_word_char(chars[index]) {
                        index += 1;
                    }
                    tokens.push(Token::Word(chars[start..index].iter().collect()));
                } else {
                    tokens.push(Token::Number(chars[start..index].iter().collect()));
                }
            }
            _ => {
                let start = index;
                while index < chars.len() && is_word_char(chars[index]) {
                    index += 1;
                }
                if index == start {
                    return Err(CommandError::new(format!(
                        "unexpected character `{character}`"
                    )));
                }
                let word: String = chars[start..index].iter().collect();
                if word.eq_ignore_ascii_case("thru") {
                    tokens.push(Token::Thru);
                } else {
                    tokens.push(Token::Word(word));
                }
            }
        }
    }

    Ok(tokens)
}

fn is_word_char(character: char) -> bool {
    !character.is_whitespace() && !matches!(character, '+' | '"' | '\'' | ';')
}

pub fn parse(tokens: &[Token]) -> Result<Command, CommandError> {
    let mut parser = Parser {
        tokens,
        position: 0,
    };
    parser.command()
}

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn bump(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).cloned();
        if token.is_some() {
            self.position += 1;
        }
        token
    }

    fn at_end(&self) -> bool {
        self.position >= self.tokens.len()
    }

    fn eat_word(&mut self, keyword: &str) -> bool {
        if matches!(self.peek(), Some(Token::Word(word)) if word.eq_ignore_ascii_case(keyword)) {
            self.position += 1;
            return true;
        }
        false
    }

    fn expect_end(&self) -> Result<(), CommandError> {
        match self.peek() {
            Some(token) => Err(CommandError::with_hint(
                format!("unexpected {token}"),
                "type `Help` for the command list",
            )),
            None => Ok(()),
        }
    }

    fn command(&mut self) -> Result<Command, CommandError> {
        let Some(token) = self.peek().cloned() else {
            return Err(CommandError::new("empty command"));
        };
        match token {
            Token::Word(word) => self.word_command(&word),
            Token::Number(_) => {
                let fixtures = self.fixture_list()?;
                self.selection_or_action(fixtures)
            }
            other => Err(CommandError::with_hint(
                format!("unexpected {other}"),
                "a command starts with a keyword or a fixture number",
            )),
        }
    }

    fn word_command(&mut self, word: &str) -> Result<Command, CommandError> {
        self.position += 1;
        if word.eq_ignore_ascii_case("fixture") {
            let fixtures = self.fixture_list()?;
            return self.selection_or_action(fixtures);
        }
        if word.eq_ignore_ascii_case("at") {
            let level = self.level()?;
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: None,
                level,
            });
        }
        if word.eq_ignore_ascii_case("full") {
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: None,
                level: Level::Full,
            });
        }
        if word.eq_ignore_ascii_case("off") {
            let fixtures = self.optional_fixture_list()?;
            self.expect_end()?;
            return Ok(Command::Off { fixtures });
        }
        if word.eq_ignore_ascii_case("clear") {
            self.eat_word("all");
            self.expect_end()?;
            return Ok(Command::Clear);
        }
        if word.eq_ignore_ascii_case("patch") {
            return self.patch();
        }
        if word.eq_ignore_ascii_case("unpatch") {
            self.eat_word("fixture");
            let fixtures = self.fixture_list()?;
            self.expect_end()?;
            return Ok(Command::Unpatch { fixtures });
        }
        if word.eq_ignore_ascii_case("import") {
            self.eat_word("gdtf");
            let path = self.path_token()?;
            self.expect_end()?;
            return Ok(Command::ImportGdtf { path });
        }
        if word.eq_ignore_ascii_case("sheet") || word.eq_ignore_ascii_case("fixtures") {
            self.expect_end()?;
            return Ok(Command::Sheet);
        }
        if word.eq_ignore_ascii_case("status") {
            self.expect_end()?;
            return Ok(Command::Status);
        }
        if word.eq_ignore_ascii_case("output") {
            self.eat_word("status");
            self.expect_end()?;
            return Ok(Command::Status);
        }
        if word.eq_ignore_ascii_case("save") {
            let path = self.path_token()?;
            self.expect_end()?;
            return Ok(Command::Save { path });
        }
        if word.eq_ignore_ascii_case("load") {
            let path = self.path_token()?;
            self.expect_end()?;
            return Ok(Command::Load { path });
        }
        if word.eq_ignore_ascii_case("help") || word == "?" {
            self.expect_end()?;
            return Ok(Command::Help);
        }
        Err(CommandError::with_hint(
            format!("unknown command `{word}`"),
            "type `Help` for the command list",
        ))
    }

    fn selection_or_action(&mut self, fixtures: Vec<u16>) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::Select(fixtures));
        }
        if self.eat_word("at") {
            let level = self.level()?;
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: Some(fixtures),
                level,
            });
        }
        if self.eat_word("full") {
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: Some(fixtures),
                level: Level::Full,
            });
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::Off {
                fixtures: Some(fixtures),
            });
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "after a fixture list use `At <level>`, `Full` or `Off`",
        ))
    }

    fn optional_fixture_list(&mut self) -> Result<Option<Vec<u16>>, CommandError> {
        self.eat_word("fixture");
        if matches!(self.peek(), Some(Token::Number(_))) {
            Ok(Some(self.fixture_list()?))
        } else {
            Ok(None)
        }
    }

    fn fixture_list(&mut self) -> Result<Vec<u16>, CommandError> {
        let mut fixtures = Vec::new();
        loop {
            let first = self.fixture_number()?;
            if matches!(self.peek(), Some(Token::Thru)) {
                self.position += 1;
                let last = self.fixture_number()?;
                let (low, high) = (first.min(last), first.max(last));
                if u32::from(high) - u32::from(low) > 4096 {
                    return Err(CommandError::new(format!(
                        "range {first} Thru {last} is too large"
                    )));
                }
                fixtures.extend(low..=high);
            } else {
                fixtures.push(first);
            }
            if matches!(self.peek(), Some(Token::Plus)) {
                self.position += 1;
                continue;
            }
            break;
        }
        fixtures.sort_unstable();
        fixtures.dedup();
        Ok(fixtures)
    }

    fn fixture_number(&mut self) -> Result<u16, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) => {
                let number: u32 = raw.parse().map_err(|_| {
                    CommandError::new(format!("`{raw}` is not a whole fixture number"))
                })?;
                if number == 0 || number > u32::from(u16::MAX) {
                    return Err(CommandError::new(format!(
                        "fixture number {number} is out of range"
                    )));
                }
                Ok(number as u16)
            }
            Some(other) => Err(CommandError::new(format!(
                "expected a fixture number, got {other}"
            ))),
            None => Err(CommandError::new(
                "expected a fixture number, e.g. `Fixture 1 Thru 4`",
            )),
        }
    }

    fn level(&mut self) -> Result<Level, CommandError> {
        if self.eat_word("full") {
            return Ok(Level::Full);
        }
        match self.bump() {
            Some(Token::Number(raw)) => {
                self.eat_word("percent");
                if matches!(self.peek(), Some(Token::Word(word)) if word == "%") {
                    self.position += 1;
                }
                let percent: f64 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a level")))?;
                Ok(Level::Percent(percent))
            }
            Some(other) => Err(CommandError::with_hint(
                format!("expected a level, got {other}"),
                "use a percentage from 0 to 100, or `Full`",
            )),
            None => Err(CommandError::with_hint(
                "expected a level",
                "use a percentage from 0 to 100, or `Full`",
            )),
        }
    }

    fn patch(&mut self) -> Result<Command, CommandError> {
        self.eat_word("fixture");
        let fixture = self.fixture_number()?;
        self.eat_word("at");
        let (universe, address) = self.patch_address()?;

        let mut type_key = None;
        let mut mode = None;
        loop {
            if self.eat_word("type") {
                type_key = Some(self.name_token("fixture type")?);
            } else if self.eat_word("mode") {
                mode = Some(self.name_token("mode")?);
            } else if self.eat_word("at") {
                // `At` may be repeated by habit; the address is already read.
            } else {
                break;
            }
        }
        self.expect_end()?;
        Ok(Command::Patch {
            fixture,
            universe,
            address,
            type_key,
            mode,
        })
    }

    fn patch_address(&mut self) -> Result<(u16, u16), CommandError> {
        let Some(Token::Number(raw)) = self.bump() else {
            return Err(CommandError::with_hint(
                "expected a patch address",
                "use `universe.address`, e.g. `1.001`",
            ));
        };
        let Some((universe, address)) = raw.split_once('.') else {
            return Err(CommandError::with_hint(
                format!("patch address `{raw}` needs a universe and an address"),
                "use `universe.address`, e.g. `1.001`",
            ));
        };
        let universe: u16 = universe
            .parse()
            .map_err(|_| CommandError::new(format!("`{universe}` is not a universe number")))?;
        let address: u16 = address
            .parse()
            .map_err(|_| CommandError::new(format!("`{address}` is not a DMX address")))?;
        if address == 0 || address > 512 {
            return Err(CommandError::new(format!(
                "DMX address {address} is outside 1..=512"
            )));
        }
        Ok((universe, address))
    }

    fn path_token(&mut self) -> Result<String, CommandError> {
        match self.bump() {
            Some(Token::Quoted(path)) | Some(Token::Word(path)) | Some(Token::Number(path)) => {
                Ok(path)
            }
            Some(other) => Err(CommandError::with_hint(
                format!("expected a path, got {other}"),
                "quote paths that contain spaces",
            )),
            None => Err(CommandError::new("expected a path")),
        }
    }

    fn name_token(&mut self, what: &str) -> Result<String, CommandError> {
        match self.bump() {
            Some(Token::Quoted(name)) | Some(Token::Word(name)) => Ok(name),
            Some(other) => Err(CommandError::with_hint(
                format!("expected a {what}, got {other}"),
                "quote names that contain spaces",
            )),
            None => Err(CommandError::new(format!("expected a {what}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(line: &str) -> Command {
        parse(&lex(line).unwrap()).unwrap()
    }

    fn error(line: &str) -> CommandError {
        match lex(line).and_then(|tokens| parse(&tokens)) {
            Ok(command) => panic!("`{line}` parsed as {command:?}"),
            Err(error) => error,
        }
    }

    #[test]
    fn lexer_separates_numbers_words_quotes_and_operators() {
        let tokens =
            lex(r#"Patch Fixture 1 At 1.001 Type "OpenLight Demo Wash" + 4ch 50%"#).unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Word("Patch".into()),
                Token::Word("Fixture".into()),
                Token::Number("1".into()),
                Token::Word("At".into()),
                Token::Number("1.001".into()),
                Token::Word("Type".into()),
                Token::Quoted("OpenLight Demo Wash".into()),
                Token::Plus,
                Token::Word("4ch".into()),
                Token::Number("50".into()),
                Token::Word("%".into()),
            ]
        );
    }

    #[test]
    fn lexer_reports_unterminated_quotes() {
        assert!(lex("\"unfinished").is_err());
    }

    #[test]
    fn statements_split_on_semicolons_outside_quotes() {
        assert_eq!(
            split_statements("Fixture 1 At 50; Save \"a;b.json\""),
            vec!["Fixture 1 At 50", "Save \"a;b.json\""]
        );
        assert!(split_statements("  ;; ").is_empty());
    }

    #[test]
    fn parses_selection_and_levels() {
        assert_eq!(
            command("Fixture 1 Thru 5"),
            Command::Select(vec![1, 2, 3, 4, 5])
        );
        assert_eq!(
            command("Fixture 1 Thru 5 At 50"),
            Command::SetLevel {
                fixtures: Some(vec![1, 2, 3, 4, 5]),
                level: Level::Percent(50.0)
            }
        );
        assert_eq!(
            command("1 + 3 At Full"),
            Command::SetLevel {
                fixtures: Some(vec![1, 3]),
                level: Level::Full
            }
        );
        assert_eq!(
            command("Full"),
            Command::SetLevel {
                fixtures: None,
                level: Level::Full
            }
        );
        assert_eq!(
            command("At 0"),
            Command::SetLevel {
                fixtures: None,
                level: Level::Percent(0.0)
            }
        );
        assert_eq!(command("Off"), Command::Off { fixtures: None });
        assert_eq!(
            command("Off Fixture 3"),
            Command::Off {
                fixtures: Some(vec![3])
            }
        );
    }

    #[test]
    fn ranges_are_order_independent_and_deduplicated() {
        assert_eq!(command("5 Thru 1"), Command::Select(vec![1, 2, 3, 4, 5]));
        assert_eq!(command("2 + 2"), Command::Select(vec![2]));
    }

    #[test]
    fn parses_patch_with_type_and_mode() {
        assert_eq!(
            command(r#"Patch Fixture 7 At 2.101 Type "OpenLight Demo Wash" Mode "4ch""#),
            Command::Patch {
                fixture: 7,
                universe: 2,
                address: 101,
                type_key: Some("OpenLight Demo Wash".into()),
                mode: Some("4ch".into()),
            }
        );
    }

    #[test]
    fn parse_errors_have_actionable_hints() {
        let failure = error("Patch Fixture 1 At 1");
        assert!(
            failure
                .hint
                .as_deref()
                .unwrap()
                .contains("universe.address"),
            "{failure}"
        );
        let failure = error("Patch Fixture 1 At 1.1000");
        assert!(failure.message.contains("outside 1..=512"), "{failure}");
        let failure = error("Nonsense");
        assert!(failure.hint.unwrap().contains("Help"));
        let failure = error("Thru 4");
        assert!(failure.hint.unwrap().contains("starts with"));
    }

    #[test]
    fn at_without_selection_is_valid_at_parse_time() {
        assert_eq!(
            command("At 25"),
            Command::SetLevel {
                fixtures: None,
                level: Level::Percent(25.0)
            }
        );
    }

    #[test]
    fn parses_paths() {
        assert_eq!(
            command(r#"Import Gdtf "examples/gdtf/a b.gdtf""#),
            Command::ImportGdtf {
                path: "examples/gdtf/a b.gdtf".into()
            }
        );
        assert_eq!(
            command("Save target/show.json"),
            Command::Save {
                path: "target/show.json".into()
            }
        );
    }
}
