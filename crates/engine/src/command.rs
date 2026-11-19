//! The command language shared by every input surface.
//!
//! Grammar (keywords are case-insensitive):
//!
//! ```text
//! Fixture 1 Thru 5                        select fixtures
//! Fixture 1 Thru 5 At 50                  set Dimmer (0 to 100)
//! Fixture 1 At ColorAdd_R 40              set any attribute of the mode
//! Fixture 1 At Red Full                   pretty attribute names work too
//! At Palette Color 3                      apply a palette to the selection
//! Full | Off                              apply to the current selection
//! Clear | Clear Values | Clear All
//! Patch Fixture 1 At 1.001 Type "X" [Mode "4ch"]
//! Unpatch Fixture 1 Thru 3
//! Import Gdtf "file.gdtf"
//! New [Show] ["name"]
//! Store Palette <Feature> <n> ["name"]    capture the programmer
//! Delete Palette <Feature> <n> | Palette
//! Store Cue 1 ["name"] [Fade 2] [Delay 0.5] [Mib [On|Off]] [Sequence 2]
//! Delete Cue 1 [Sequence 2] | Sequence [2]
//! Assign Executor 1 Sequence 1
//! Executor 1 At 75 | Full | Off           executor master fader
//! Go | Back | Pause | Release [Executor n | Sequence n]
//! Group 1 | Group 1 At 50 | Group 1 Full | Group 1 Off
//! Store Group 1 ["name"] | Delete Group 1 | Group
//! World 1 | World All                          activate / deactivate a world
//! Store World 1 ["name"] | Delete World 1 | World
//! Filter 1 | Filter All                       activate / deactivate a filter
//! Store Filter 1 ["name"] Dimmer Color        names must be quoted here
//! Delete Filter 1 | Filter
//! Master Grand At 75 | Master Rate At 200 | Master Group 1 At 50 | Master
//! Effect 1 On [Fixture 1 Thru 4] | Effect 1 Off | Effect Off | Effect
//! Store Effect 1 ["name"] [Attribute Dimmer] [Waveform Sine]
//!              [Rate 1] [Size 50] [Spread 0]
//! Delete Effect 1 | Effect
//! Undo | Redo
//! Store Timecode 1 ["name"] [Fps 25] | Delete Timecode 1 | Timecode [1]
//! Timecode 1 Add 00:00:10:00 Sequence 1 Cue 2
//! Timecode 1 Remove 00:00:10:00 | Timecode 1 At 00:00:10:00
//! Timecode 1 Start | Pause | Stop
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

/// Where a level comes from: a literal, or a palette in a feature pool.
#[derive(Debug, Clone, PartialEq)]
pub enum LevelSource {
    Level(Level),
    Palette { feature: String, number: u16 },
}

/// Which playback a `Go`, `Back`, `Pause` or `Release` addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackRef {
    Sequence(u16),
    Executor(u16),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Select(Vec<u16>),
    SetLevel {
        fixtures: Option<Vec<u16>>,
        attribute: Option<String>,
        source: LevelSource,
    },
    Off {
        fixtures: Option<Vec<u16>>,
    },
    Clear {
        all: bool,
        values: bool,
    },
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
    New {
        name: Option<String>,
    },
    StorePalette {
        feature: String,
        number: u16,
        name: Option<String>,
    },
    DeletePalette {
        feature: String,
        number: u16,
    },
    ListPalettes,
    StoreCue {
        sequence: u16,
        number: f64,
        name: Option<String>,
        fade: Option<f64>,
        delay: Option<f64>,
        mib: Option<bool>,
    },
    DeleteCue {
        sequence: u16,
        number: f64,
    },
    ListSequences {
        only: Option<u16>,
    },
    AssignExecutor {
        executor: u16,
        sequence: u16,
    },
    SetExecutorLevel {
        executor: u16,
        level: Level,
    },
    ListExecutors {
        only: Option<u16>,
    },
    SelectGroup(u16),
    SetLevelGroup {
        group: u16,
        attribute: Option<String>,
        source: LevelSource,
    },
    OffGroup {
        group: u16,
    },
    StoreGroup {
        number: u16,
        name: Option<String>,
    },
    DeleteGroup {
        number: u16,
    },
    ListGroups,
    StoreWorld {
        number: u16,
        name: Option<String>,
    },
    DeleteWorld {
        number: u16,
    },
    /// `None` deactivates the active world.
    ActivateWorld {
        number: Option<u16>,
    },
    ListWorlds,
    StoreFilter {
        number: u16,
        name: Option<String>,
        features: Vec<String>,
    },
    DeleteFilter {
        number: u16,
    },
    ActivateFilter {
        number: Option<u16>,
    },
    ListFilters,
    SetGrandMaster {
        level: Level,
    },
    SetRateMaster {
        level: Level,
    },
    SetGroupMaster {
        group: u16,
        level: Level,
    },
    ListMasters,
    StoreEffect {
        number: u16,
        name: Option<String>,
        attribute: Option<String>,
        waveform: Option<String>,
        rate: Option<f64>,
        size: Option<f64>,
        spread: Option<f64>,
    },
    DeleteEffect {
        number: u16,
    },
    ListEffects {
        only: Option<u16>,
    },
    EffectOn {
        effect: u16,
        fixtures: Option<Vec<u16>>,
    },
    EffectOff {
        effect: Option<u16>,
    },
    StoreTimecode {
        number: u16,
        name: Option<String>,
        fps: Option<u8>,
    },
    DeleteTimecode {
        number: u16,
    },
    AddTimecodeEvent {
        timecode: u16,
        at: String,
        sequence: u16,
        cue: f64,
    },
    RemoveTimecodeEvent {
        timecode: u16,
        at: String,
    },
    StartTimecode {
        timecode: u16,
    },
    PauseTimecode {
        timecode: u16,
    },
    StopTimecode {
        timecode: u16,
    },
    LocateTimecode {
        timecode: u16,
        at: String,
    },
    ListTimecodes {
        only: Option<u16>,
    },
    Undo,
    Redo,
    Go {
        target: Option<PlaybackRef>,
    },
    Back {
        target: Option<PlaybackRef>,
    },
    Pause {
        target: Option<PlaybackRef>,
    },
    Release {
        target: Option<PlaybackRef>,
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
                // `1.001` is a patch address and `00:00:10:00` is a timecode;
                // both are one token that the parser validates.
                while index < chars.len()
                    && (chars[index].is_ascii_digit() || chars[index] == '.' || chars[index] == ':')
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

/// Keywords that may never be read as an object name.
const RESERVED: &[&str] = &[
    "fade",
    "delay",
    "sequence",
    "in",
    "at",
    "full",
    "off",
    "palette",
    "executor",
    "fixture",
    "type",
    "mode",
    "thru",
    "values",
    "all",
    "show",
    "group",
    "world",
    "filter",
    "master",
    "grand",
    "rate",
    "effect",
    "attribute",
    "waveform",
    "size",
    "spread",
    "on",
    "mib",
    "timecode",
    "fps",
    "add",
    "remove",
    "start",
    "stop",
    "undo",
    "redo",
];

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
            let (attribute, source) = self.level_source()?;
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: None,
                attribute,
                source,
            });
        }
        if word.eq_ignore_ascii_case("full") {
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: None,
                attribute: None,
                source: LevelSource::Level(Level::Full),
            });
        }
        if word.eq_ignore_ascii_case("off") {
            let fixtures = self.optional_fixture_list()?;
            self.expect_end()?;
            return Ok(Command::Off { fixtures });
        }
        if word.eq_ignore_ascii_case("clear") {
            let mut all = false;
            let mut values = false;
            if self.eat_word("all") {
                all = true;
                values = true;
            } else if self.eat_word("values") {
                values = true;
            }
            self.expect_end()?;
            return Ok(Command::Clear { all, values });
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
        if word.eq_ignore_ascii_case("new") {
            self.eat_word("show");
            let name = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::New { name });
        }
        if word.eq_ignore_ascii_case("store") {
            return self.store();
        }
        if word.eq_ignore_ascii_case("delete") {
            return self.delete();
        }
        if word.eq_ignore_ascii_case("assign") {
            return self.assign();
        }
        if word.eq_ignore_ascii_case("go") {
            let target = self.playback_ref()?;
            self.expect_end()?;
            return Ok(Command::Go { target });
        }
        if word.eq_ignore_ascii_case("back") {
            let target = self.playback_ref()?;
            self.expect_end()?;
            return Ok(Command::Back { target });
        }
        if word.eq_ignore_ascii_case("pause") {
            let target = self.playback_ref()?;
            self.expect_end()?;
            return Ok(Command::Pause { target });
        }
        if word.eq_ignore_ascii_case("release") {
            let target = self.playback_ref()?;
            self.expect_end()?;
            return Ok(Command::Release { target });
        }
        if word.eq_ignore_ascii_case("palette") || word.eq_ignore_ascii_case("palettes") {
            self.expect_end()?;
            return Ok(Command::ListPalettes);
        }
        if word.eq_ignore_ascii_case("sequence") || word.eq_ignore_ascii_case("sequences") {
            let only = if matches!(self.peek(), Some(Token::Number(_))) {
                Some(self.whole_number("sequence number")?)
            } else {
                None
            };
            self.expect_end()?;
            return Ok(Command::ListSequences { only });
        }
        if word.eq_ignore_ascii_case("executor") || word.eq_ignore_ascii_case("executors") {
            return self.executor();
        }
        if word.eq_ignore_ascii_case("group") || word.eq_ignore_ascii_case("groups") {
            return self.group_command();
        }
        if word.eq_ignore_ascii_case("world") || word.eq_ignore_ascii_case("worlds") {
            return self.world_command();
        }
        if word.eq_ignore_ascii_case("filter") || word.eq_ignore_ascii_case("filters") {
            return self.filter_command();
        }
        if word.eq_ignore_ascii_case("master") || word.eq_ignore_ascii_case("masters") {
            return self.master_command();
        }
        if word.eq_ignore_ascii_case("effect") || word.eq_ignore_ascii_case("effects") {
            return self.effect_command();
        }
        if word.eq_ignore_ascii_case("timecode") || word.eq_ignore_ascii_case("timecodes") {
            return self.timecode_command();
        }
        if word.eq_ignore_ascii_case("undo") {
            self.expect_end()?;
            return Ok(Command::Undo);
        }
        if word.eq_ignore_ascii_case("redo") {
            self.expect_end()?;
            return Ok(Command::Redo);
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
            let (attribute, source) = self.level_source()?;
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: Some(fixtures),
                attribute,
                source,
            });
        }
        if self.eat_word("full") {
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: Some(fixtures),
                attribute: None,
                source: LevelSource::Level(Level::Full),
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

    /// A positive whole number such as a palette, sequence or executor number.
    fn whole_number(&mut self, what: &str) -> Result<u16, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) => {
                if raw.contains('.') {
                    return Err(CommandError::new(format!(
                        "{what} `{raw}` must be a whole number"
                    )));
                }
                let number: u32 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a {what}")))?;
                if number == 0 || number > u32::from(u16::MAX) {
                    return Err(CommandError::new(format!(
                        "{what} {number} is out of range"
                    )));
                }
                Ok(number as u16)
            }
            Some(other) => Err(CommandError::new(format!("expected a {what}, got {other}"))),
            None => Err(CommandError::new(format!("expected a {what}"))),
        }
    }

    fn cue_number(&mut self) -> Result<f64, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) => {
                let number: f64 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a cue number")))?;
                if !number.is_finite() || number < 0.0 {
                    return Err(CommandError::new(format!(
                        "cue number {raw} is not a finite positive number"
                    )));
                }
                Ok(number)
            }
            Some(other) => Err(CommandError::new(format!(
                "expected a cue number, got {other}"
            ))),
            None => Err(CommandError::new(
                "expected a cue number, e.g. `Store Cue 1`",
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
                "use a percentage from 0 to 100, `Full`, or `Palette <Feature> <n>`",
            )),
            None => Err(CommandError::with_hint(
                "expected a level",
                "use a percentage from 0 to 100, `Full`, or `Palette <Feature> <n>`",
            )),
        }
    }

    /// `[<Attribute>] <level>` or `[<Attribute>] Palette <Feature> <n>`.
    fn level_source(&mut self) -> Result<(Option<String>, LevelSource), CommandError> {
        if self.eat_word("palette") {
            let feature = self.name_token("palette feature")?;
            let number = self.whole_number("palette number")?;
            return Ok((None, LevelSource::Palette { feature, number }));
        }
        let attribute = self.optional_attribute();
        if self.eat_word("palette") {
            let feature = self.name_token("palette feature")?;
            let number = self.whole_number("palette number")?;
            return Ok((attribute, LevelSource::Palette { feature, number }));
        }
        let level = self.level()?;
        Ok((attribute, LevelSource::Level(level)))
    }

    fn optional_attribute(&mut self) -> Option<String> {
        match self.peek() {
            Some(Token::Word(word))
                if !word.eq_ignore_ascii_case("full")
                    && !word.eq_ignore_ascii_case("palette")
                    && !word.eq_ignore_ascii_case("off") =>
            {
                let word = word.clone();
                self.position += 1;
                Some(word)
            }
            _ => None,
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

    fn group_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListGroups);
        }
        let number = self.whole_number("group number")?;
        if self.at_end() {
            return Ok(Command::SelectGroup(number));
        }
        if self.eat_word("at") {
            let (attribute, source) = self.level_source()?;
            self.expect_end()?;
            return Ok(Command::SetLevelGroup {
                group: number,
                attribute,
                source,
            });
        }
        if self.eat_word("full") {
            self.expect_end()?;
            return Ok(Command::SetLevelGroup {
                group: number,
                attribute: None,
                source: LevelSource::Level(Level::Full),
            });
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::OffGroup { group: number });
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "after a group use `At <level>`, `Full` or `Off`",
        ))
    }

    fn world_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListWorlds);
        }
        if self.eat_word("all") || self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::ActivateWorld { number: None });
        }
        let number = self.whole_number("world number")?;
        self.expect_end()?;
        Ok(Command::ActivateWorld {
            number: Some(number),
        })
    }

    fn filter_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListFilters);
        }
        if self.eat_word("all") || self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::ActivateFilter { number: None });
        }
        let number = self.whole_number("filter number")?;
        self.expect_end()?;
        Ok(Command::ActivateFilter {
            number: Some(number),
        })
    }

    fn master_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListMasters);
        }
        if self.eat_word("grand") {
            let level = self.master_level()?;
            return Ok(Command::SetGrandMaster { level });
        }
        if self.eat_word("rate") {
            let level = self.master_level()?;
            return Ok(Command::SetRateMaster { level });
        }
        if self.eat_word("group") {
            let group = self.whole_number("group number")?;
            let level = self.master_level()?;
            return Ok(Command::SetGroupMaster { group, level });
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "use `Master Grand`, `Master Rate` or `Master Group <n>`",
        ))
    }

    fn master_level(&mut self) -> Result<Level, CommandError> {
        if self.eat_word("at") {
            let level = self.level()?;
            self.expect_end()?;
            return Ok(level);
        }
        if self.eat_word("full") {
            self.expect_end()?;
            return Ok(Level::Full);
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Level::Percent(0.0));
        }
        Err(CommandError::with_hint(
            "expected a master level",
            "use `At <level>`, `Full` or `Off`",
        ))
    }

    fn effect_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListEffects { only: None });
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::EffectOff { effect: None });
        }
        let number = self.whole_number("effect number")?;
        if self.at_end() {
            return Ok(Command::ListEffects { only: Some(number) });
        }
        if self.eat_word("on") {
            let fixtures = self.optional_fixture_list()?;
            self.expect_end()?;
            return Ok(Command::EffectOn {
                effect: number,
                fixtures,
            });
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::EffectOff {
                effect: Some(number),
            });
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "use `Effect <n> On`, `Effect <n> Off` or `Effect <n>`",
        ))
    }

    fn timecode_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListTimecodes { only: None });
        }
        let number = self.whole_number("timecode number")?;
        if self.at_end() {
            return Ok(Command::ListTimecodes { only: Some(number) });
        }
        if self.eat_word("add") {
            let at = self.timecode_token()?;
            if !self.eat_word("sequence") {
                return Err(CommandError::with_hint(
                    "`Timecode <n> Add` needs a sequence",
                    "`Timecode 1 Add 00:00:10:00 Sequence 1 Cue 2`",
                ));
            }
            let sequence = self.whole_number("sequence number")?;
            if !self.eat_word("cue") {
                return Err(CommandError::with_hint(
                    "`Timecode <n> Add` needs a cue",
                    "`Timecode 1 Add 00:00:10:00 Sequence 1 Cue 2`",
                ));
            }
            let cue = self.cue_number()?;
            self.expect_end()?;
            return Ok(Command::AddTimecodeEvent {
                timecode: number,
                at,
                sequence,
                cue,
            });
        }
        if self.eat_word("remove") {
            let at = self.timecode_token()?;
            self.expect_end()?;
            return Ok(Command::RemoveTimecodeEvent {
                timecode: number,
                at,
            });
        }
        if self.eat_word("start") {
            self.expect_end()?;
            return Ok(Command::StartTimecode { timecode: number });
        }
        if self.eat_word("pause") {
            self.expect_end()?;
            return Ok(Command::PauseTimecode { timecode: number });
        }
        if self.eat_word("stop") {
            self.expect_end()?;
            return Ok(Command::StopTimecode { timecode: number });
        }
        if self.eat_word("at") {
            let at = self.timecode_token()?;
            self.expect_end()?;
            return Ok(Command::LocateTimecode {
                timecode: number,
                at,
            });
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "use `Timecode 1 Add|Remove <timecode>`, `Start`, `Pause`, `Stop` or `At <timecode>`",
        ))
    }

    fn timecode_token(&mut self) -> Result<String, CommandError> {
        match self.bump() {
            Some(Token::Number(text)) | Some(Token::Word(text)) => Ok(text),
            Some(other) => Err(CommandError::with_hint(
                format!("expected a timecode, found {other}"),
                "write positions as `HH:MM:SS:FF`, e.g. `00:01:30:00`",
            )),
            None => Err(CommandError::with_hint(
                "expected a timecode",
                "write positions as `HH:MM:SS:FF`, e.g. `00:01:30:00`",
            )),
        }
    }

    fn store(&mut self) -> Result<Command, CommandError> {
        if self.eat_word("timecode") {
            let number = self.whole_number("timecode number")?;
            let name = self.optional_name()?;
            let mut fps = None;
            if self.eat_word("fps") {
                let value = self.whole_number("frame rate")?;
                fps = Some(u8::try_from(value).map_err(|_| {
                    CommandError::new(format!("frame rate {value} is outside 1..=60"))
                })?);
            }
            self.expect_end()?;
            return Ok(Command::StoreTimecode { number, name, fps });
        }
        if self.eat_word("group") {
            let number = self.whole_number("group number")?;
            let name = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::StoreGroup { number, name });
        }
        if self.eat_word("world") {
            let number = self.whole_number("world number")?;
            let name = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::StoreWorld { number, name });
        }
        if self.eat_word("filter") {
            let number = self.whole_number("filter number")?;
            // A filter name must be quoted so bare pool names are unambiguous.
            let name = self.quoted_name();
            let mut features = Vec::new();
            while let Some(Token::Word(word)) = self.peek().cloned() {
                self.position += 1;
                features.push(word);
            }
            self.expect_end()?;
            if features.is_empty() {
                return Err(CommandError::with_hint(
                    format!("filter {number} needs at least one feature"),
                    "name the pools it allows, e.g. `Store Filter 1 Dimmer Color`",
                ));
            }
            return Ok(Command::StoreFilter {
                number,
                name,
                features,
            });
        }
        if self.eat_word("effect") {
            let number = self.whole_number("effect number")?;
            let name = self.optional_name()?;
            let mut attribute = None;
            let mut waveform = None;
            let mut rate = None;
            let mut size = None;
            let mut spread = None;
            loop {
                if self.eat_word("attribute") {
                    attribute = Some(self.name_token("attribute")?);
                } else if self.eat_word("waveform") {
                    waveform = Some(self.name_token("waveform")?);
                } else if self.eat_word("rate") {
                    rate = Some(self.number_value("rate")?);
                } else if self.eat_word("size") {
                    size = Some(self.number_value("size")?);
                } else if self.eat_word("spread") {
                    spread = Some(self.number_value("spread")?);
                } else {
                    break;
                }
            }
            self.expect_end()?;
            return Ok(Command::StoreEffect {
                number,
                name,
                attribute,
                waveform,
                rate,
                size,
                spread,
            });
        }
        if self.eat_word("palette") {
            let feature = self.name_token("palette feature")?;
            let number = self.whole_number("palette number")?;
            let name = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::StorePalette {
                feature,
                number,
                name,
            });
        }
        if self.eat_word("cue") {
            let number = self.cue_number()?;
            let mut name = self.optional_name()?;
            let mut fade = None;
            let mut delay = None;
            let mut mib = None;
            let mut sequence = 1;
            loop {
                if self.eat_word("fade") {
                    fade = Some(self.seconds("fade")?);
                } else if self.eat_word("delay") {
                    delay = Some(self.seconds("delay")?);
                } else if self.eat_word("mib") {
                    if self.eat_word("off") {
                        mib = Some(false);
                    } else {
                        self.eat_word("on");
                        mib = Some(true);
                    }
                } else if self.eat_word("sequence") {
                    sequence = self.whole_number("sequence number")?;
                } else if self.eat_word("in") {
                    self.eat_word("sequence");
                    sequence = self.whole_number("sequence number")?;
                } else if let Some(candidate) = self.optional_name()? {
                    if name.is_some() {
                        return Err(CommandError::with_hint(
                            format!("unexpected name `{candidate}`"),
                            "quote the cue name once",
                        ));
                    }
                    name = Some(candidate);
                } else {
                    break;
                }
            }
            self.expect_end()?;
            return Ok(Command::StoreCue {
                sequence,
                number,
                name,
                fade,
                delay,
                mib,
            });
        }
        Err(CommandError::with_hint(
            "`Store` needs an object",
            "`Store Palette <Feature> <n>` or `Store Cue <n>`",
        ))
    }

    fn delete(&mut self) -> Result<Command, CommandError> {
        if self.eat_word("group") {
            let number = self.whole_number("group number")?;
            self.expect_end()?;
            return Ok(Command::DeleteGroup { number });
        }
        if self.eat_word("world") {
            let number = self.whole_number("world number")?;
            self.expect_end()?;
            return Ok(Command::DeleteWorld { number });
        }
        if self.eat_word("filter") {
            let number = self.whole_number("filter number")?;
            self.expect_end()?;
            return Ok(Command::DeleteFilter { number });
        }
        if self.eat_word("effect") {
            let number = self.whole_number("effect number")?;
            self.expect_end()?;
            return Ok(Command::DeleteEffect { number });
        }
        if self.eat_word("timecode") {
            let number = self.whole_number("timecode number")?;
            self.expect_end()?;
            return Ok(Command::DeleteTimecode { number });
        }
        if self.eat_word("palette") {
            let feature = self.name_token("palette feature")?;
            let number = self.whole_number("palette number")?;
            self.expect_end()?;
            return Ok(Command::DeletePalette { feature, number });
        }
        if self.eat_word("cue") {
            let number = self.cue_number()?;
            let mut sequence = 1;
            if self.eat_word("sequence") {
                sequence = self.whole_number("sequence number")?;
            } else if self.eat_word("in") {
                self.eat_word("sequence");
                sequence = self.whole_number("sequence number")?;
            }
            self.expect_end()?;
            return Ok(Command::DeleteCue { sequence, number });
        }
        Err(CommandError::with_hint(
            "`Delete` needs an object",
            "`Delete Palette <Feature> <n>` or `Delete Cue <n>`",
        ))
    }

    fn assign(&mut self) -> Result<Command, CommandError> {
        if !self.eat_word("executor") {
            return Err(CommandError::with_hint(
                "`Assign` needs an executor",
                "`Assign Executor 1 Sequence 1`",
            ));
        }
        let executor = self.whole_number("executor number")?;
        self.eat_word("to");
        if !self.eat_word("sequence") {
            return Err(CommandError::with_hint(
                "`Assign` needs an object",
                "`Assign Executor 1 Sequence 1`",
            ));
        }
        let sequence = self.whole_number("sequence number")?;
        self.expect_end()?;
        Ok(Command::AssignExecutor { executor, sequence })
    }

    fn executor(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListExecutors { only: None });
        }
        let number = self.whole_number("executor number")?;
        if self.at_end() {
            return Ok(Command::ListExecutors { only: Some(number) });
        }
        if self.eat_word("at") {
            let level = self.level()?;
            self.expect_end()?;
            return Ok(Command::SetExecutorLevel {
                executor: number,
                level,
            });
        }
        if self.eat_word("full") {
            self.expect_end()?;
            return Ok(Command::SetExecutorLevel {
                executor: number,
                level: Level::Full,
            });
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::SetExecutorLevel {
                executor: number,
                level: Level::Percent(0.0),
            });
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "use `Executor <n> At <level>` or `Executor <n>` to inspect it",
        ))
    }

    fn playback_ref(&mut self) -> Result<Option<PlaybackRef>, CommandError> {
        if self.eat_word("executor") {
            return Ok(Some(PlaybackRef::Executor(
                self.whole_number("executor number")?,
            )));
        }
        if self.eat_word("sequence") {
            return Ok(Some(PlaybackRef::Sequence(
                self.whole_number("sequence number")?,
            )));
        }
        Ok(None)
    }

    fn optional_name(&mut self) -> Result<Option<String>, CommandError> {
        match self.peek().cloned() {
            Some(Token::Quoted(name)) => {
                self.position += 1;
                Ok(Some(name))
            }
            Some(Token::Word(word))
                if !RESERVED
                    .iter()
                    .any(|keyword| word.eq_ignore_ascii_case(keyword)) =>
            {
                self.position += 1;
                Ok(Some(word))
            }
            _ => Ok(None),
        }
    }

    /// A quoted object name. Filters use this so bare words remain features.
    fn quoted_name(&mut self) -> Option<String> {
        match self.peek().cloned() {
            Some(Token::Quoted(name)) => {
                self.position += 1;
                Some(name)
            }
            _ => None,
        }
    }

    fn number_value(&mut self, what: &str) -> Result<f64, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) | Some(Token::Word(raw)) => {
                let value: f64 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a {what}")))?;
                if !value.is_finite() || value < 0.0 {
                    return Err(CommandError::new(format!(
                        "{what} {raw} is not a positive number"
                    )));
                }
                Ok(value)
            }
            Some(other) => Err(CommandError::new(format!("expected a {what}, got {other}"))),
            None => Err(CommandError::new(format!("expected a {what}"))),
        }
    }

    fn seconds(&mut self, what: &str) -> Result<f64, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) | Some(Token::Word(raw)) => {
                let value: f64 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a {what} time")))?;
                if !value.is_finite() || value < 0.0 {
                    return Err(CommandError::new(format!(
                        "{what} {raw} is not a positive number of seconds"
                    )));
                }
                Ok(value)
            }
            Some(other) => Err(CommandError::new(format!(
                "expected a {what} time in seconds, got {other}"
            ))),
            None => Err(CommandError::new(format!(
                "expected a {what} time in seconds"
            ))),
        }
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
                attribute: None,
                source: LevelSource::Level(Level::Percent(50.0))
            }
        );
        assert_eq!(
            command("1 + 3 At Full"),
            Command::SetLevel {
                fixtures: Some(vec![1, 3]),
                attribute: None,
                source: LevelSource::Level(Level::Full)
            }
        );
        assert_eq!(
            command("Full"),
            Command::SetLevel {
                fixtures: None,
                attribute: None,
                source: LevelSource::Level(Level::Full)
            }
        );
        assert_eq!(
            command("At 0"),
            Command::SetLevel {
                fixtures: None,
                attribute: None,
                source: LevelSource::Level(Level::Percent(0.0))
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
    fn parses_attributes_and_palettes() {
        assert_eq!(
            command("Fixture 1 At ColorAdd_R 40"),
            Command::SetLevel {
                fixtures: Some(vec![1]),
                attribute: Some("ColorAdd_R".into()),
                source: LevelSource::Level(Level::Percent(40.0))
            }
        );
        assert_eq!(
            command("At Dimmer Full"),
            Command::SetLevel {
                fixtures: None,
                attribute: Some("Dimmer".into()),
                source: LevelSource::Level(Level::Full)
            }
        );
        assert_eq!(
            command("Fixture 1 Thru 2 At Palette Color 3"),
            Command::SetLevel {
                fixtures: Some(vec![1, 2]),
                attribute: None,
                source: LevelSource::Palette {
                    feature: "Color".into(),
                    number: 3
                }
            }
        );
        assert_eq!(
            command("At Red 25"),
            Command::SetLevel {
                fixtures: None,
                attribute: Some("Red".into()),
                source: LevelSource::Level(Level::Percent(25.0))
            }
        );
    }

    #[test]
    fn parses_store_and_delete_objects() {
        assert_eq!(
            command(r#"Store Palette Color 2 "Deep Blue""#),
            Command::StorePalette {
                feature: "Color".into(),
                number: 2,
                name: Some("Deep Blue".into()),
            }
        );
        assert_eq!(
            command("Store Cue 1.5 Fade 2 Delay 0.5 Sequence 3"),
            Command::StoreCue {
                sequence: 3,
                number: 1.5,
                name: None,
                fade: Some(2.0),
                delay: Some(0.5),
                mib: None,
            }
        );
        assert_eq!(
            command(r#"Store Cue 2 "Look" In Sequence 2"#),
            Command::StoreCue {
                sequence: 2,
                number: 2.0,
                name: Some("Look".into()),
                fade: None,
                delay: None,
                mib: None,
            }
        );
        assert_eq!(
            command("Store Cue 3 Mib Fade 1"),
            Command::StoreCue {
                sequence: 1,
                number: 3.0,
                name: None,
                fade: Some(1.0),
                delay: None,
                mib: Some(true),
            }
        );
        assert_eq!(
            command("Store Cue 3 Mib Off"),
            Command::StoreCue {
                sequence: 1,
                number: 3.0,
                name: None,
                fade: None,
                delay: None,
                mib: Some(false),
            }
        );
        assert_eq!(
            command("Delete Cue 4 Sequence 2"),
            Command::DeleteCue {
                sequence: 2,
                number: 4.0
            }
        );
        assert_eq!(
            command("Delete Palette Beam 1"),
            Command::DeletePalette {
                feature: "Beam".into(),
                number: 1
            }
        );
    }

    #[test]
    fn parses_executors_and_playback_commands() {
        assert_eq!(
            command("Assign Executor 2 Sequence 1"),
            Command::AssignExecutor {
                executor: 2,
                sequence: 1
            }
        );
        assert_eq!(
            command("Executor 2 At 75"),
            Command::SetExecutorLevel {
                executor: 2,
                level: Level::Percent(75.0)
            }
        );
        assert_eq!(
            command("Executor 2 Full"),
            Command::SetExecutorLevel {
                executor: 2,
                level: Level::Full
            }
        );
        assert_eq!(command("Executor"), Command::ListExecutors { only: None });
        assert_eq!(
            command("Go Executor 2"),
            Command::Go {
                target: Some(PlaybackRef::Executor(2))
            }
        );
        assert_eq!(
            command("Go Sequence 1"),
            Command::Go {
                target: Some(PlaybackRef::Sequence(1))
            }
        );
        assert_eq!(command("Go"), Command::Go { target: None });
        assert_eq!(command("Back"), Command::Back { target: None });
        assert_eq!(command("Pause"), Command::Pause { target: None });
        assert_eq!(command("Release"), Command::Release { target: None });
        assert_eq!(
            command("Clear All"),
            Command::Clear {
                all: true,
                values: true
            }
        );
        assert_eq!(
            command("Clear Values"),
            Command::Clear {
                all: false,
                values: true
            }
        );
        assert_eq!(
            command("New Show \"Gig\""),
            Command::New {
                name: Some("Gig".into())
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
        assert!(error("Store Cue 1 Fade -1").message.contains("positive"));
        assert!(error("Store Palette Color 0")
            .message
            .contains("out of range"));
        assert!(error("Go Executor").message.contains("expected"));
    }

    #[test]
    fn at_without_selection_is_valid_at_parse_time() {
        assert_eq!(
            command("At 25"),
            Command::SetLevel {
                fixtures: None,
                attribute: None,
                source: LevelSource::Level(Level::Percent(25.0))
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

    #[test]
    fn parses_group_commands() {
        assert_eq!(command("Group"), Command::ListGroups);
        assert_eq!(command("Group 2"), Command::SelectGroup(2));
        assert_eq!(
            command("Group 2 At 40"),
            Command::SetLevelGroup {
                group: 2,
                attribute: None,
                source: LevelSource::Level(Level::Percent(40.0))
            }
        );
        assert_eq!(
            command("Group 2 At Palette Color 1"),
            Command::SetLevelGroup {
                group: 2,
                attribute: None,
                source: LevelSource::Palette {
                    feature: "Color".into(),
                    number: 1
                }
            }
        );
        assert_eq!(command("Group 2 Off"), Command::OffGroup { group: 2 });
        assert_eq!(
            command(r#"Store Group 3 "Front""#),
            Command::StoreGroup {
                number: 3,
                name: Some("Front".into())
            }
        );
        assert_eq!(
            command("Delete Group 3"),
            Command::DeleteGroup { number: 3 }
        );
    }

    #[test]
    fn parses_worlds_and_filters() {
        assert_eq!(command("World"), Command::ListWorlds);
        assert_eq!(
            command("World 2"),
            Command::ActivateWorld { number: Some(2) }
        );
        assert_eq!(
            command("World All"),
            Command::ActivateWorld { number: None }
        );
        assert_eq!(
            command("Store World 2 Front"),
            Command::StoreWorld {
                number: 2,
                name: Some("Front".into())
            }
        );
        assert_eq!(
            command("Delete World 2"),
            Command::DeleteWorld { number: 2 }
        );

        assert_eq!(command("Filter"), Command::ListFilters);
        assert_eq!(
            command("Filter 3"),
            Command::ActivateFilter { number: Some(3) }
        );
        assert_eq!(
            command("Store Filter 3 \"Intensity\" Dimmer Color"),
            Command::StoreFilter {
                number: 3,
                name: Some("Intensity".into()),
                features: vec!["Dimmer".into(), "Color".into()]
            }
        );
        assert_eq!(
            command("Store Filter 3 Dimmer"),
            Command::StoreFilter {
                number: 3,
                name: None,
                features: vec!["Dimmer".into()]
            }
        );
        assert_eq!(
            command("Delete Filter 3"),
            Command::DeleteFilter { number: 3 }
        );
        assert!(error("Store Filter 3")
            .message
            .contains("at least one feature"));
    }

    #[test]
    fn parses_timecode_and_history_commands() {
        assert_eq!(command("Timecode"), Command::ListTimecodes { only: None });
        assert_eq!(
            command("Timecode 2"),
            Command::ListTimecodes { only: Some(2) }
        );
        assert_eq!(
            command(r#"Store Timecode 2 "Song" Fps 30"#),
            Command::StoreTimecode {
                number: 2,
                name: Some("Song".into()),
                fps: Some(30),
            }
        );
        assert_eq!(
            command("Timecode 2 Add 00:01:30:12 Sequence 3 Cue 4.5"),
            Command::AddTimecodeEvent {
                timecode: 2,
                at: "00:01:30:12".into(),
                sequence: 3,
                cue: 4.5,
            }
        );
        assert_eq!(
            command("Timecode 2 Remove 00:01:30:12"),
            Command::RemoveTimecodeEvent {
                timecode: 2,
                at: "00:01:30:12".into(),
            }
        );
        assert_eq!(
            command("Timecode 2 At 00:00:05:00"),
            Command::LocateTimecode {
                timecode: 2,
                at: "00:00:05:00".into(),
            }
        );
        assert_eq!(
            command("Timecode 2 Start"),
            Command::StartTimecode { timecode: 2 }
        );
        assert_eq!(
            command("Timecode 2 Pause"),
            Command::PauseTimecode { timecode: 2 }
        );
        assert_eq!(
            command("Timecode 2 Stop"),
            Command::StopTimecode { timecode: 2 }
        );
        assert_eq!(
            command("Delete Timecode 2"),
            Command::DeleteTimecode { number: 2 }
        );
        assert_eq!(command("Undo"), Command::Undo);
        assert_eq!(command("Redo"), Command::Redo);
        assert!(error("Timecode 1 Add 00:00:00:00 Sequence 1")
            .message
            .contains("needs a cue"));
    }

    #[test]
    fn parses_master_commands() {
        assert_eq!(command("Master"), Command::ListMasters);
        assert_eq!(
            command("Master Grand At 75"),
            Command::SetGrandMaster {
                level: Level::Percent(75.0)
            }
        );
        assert_eq!(
            command("Master Grand Off"),
            Command::SetGrandMaster {
                level: Level::Percent(0.0)
            }
        );
        assert_eq!(
            command("Master Rate At 200"),
            Command::SetRateMaster {
                level: Level::Percent(200.0)
            }
        );
        assert_eq!(
            command("Master Group 4 Full"),
            Command::SetGroupMaster {
                group: 4,
                level: Level::Full
            }
        );
    }

    #[test]
    fn parses_effect_commands() {
        assert_eq!(command("Effect"), Command::ListEffects { only: None });
        assert_eq!(command("Effect 2"), Command::ListEffects { only: Some(2) });
        assert_eq!(
            command("Effect 2 On"),
            Command::EffectOn {
                effect: 2,
                fixtures: None
            }
        );
        assert_eq!(
            command("Effect 2 On Fixture 1 Thru 3"),
            Command::EffectOn {
                effect: 2,
                fixtures: Some(vec![1, 2, 3])
            }
        );
        assert_eq!(
            command("Effect 2 Off"),
            Command::EffectOff { effect: Some(2) }
        );
        assert_eq!(command("Effect Off"), Command::EffectOff { effect: None });

        assert_eq!(
            command("Store Effect 2 Pulse Attribute ColorAdd_R Waveform Square Rate 2 Size 25 Spread 90"),
            Command::StoreEffect {
                number: 2,
                name: Some("Pulse".into()),
                attribute: Some("ColorAdd_R".into()),
                waveform: Some("Square".into()),
                rate: Some(2.0),
                size: Some(25.0),
                spread: Some(90.0)
            }
        );
        assert_eq!(
            command(r#"Store Effect 2 "Default""#),
            Command::StoreEffect {
                number: 2,
                name: Some("Default".into()),
                attribute: None,
                waveform: None,
                rate: None,
                size: None,
                spread: None
            }
        );
        assert_eq!(
            command("Delete Effect 2"),
            Command::DeleteEffect { number: 2 }
        );
    }
}
