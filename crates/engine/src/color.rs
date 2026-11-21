//! The colour model: one colour intent, translated into whatever a fixture
//! can actually do.
//!
//! A show stores physical levels, so the desk needs a single place that knows
//! the three colour systems a profile can have: additive RGB (optionally with
//! a white emitter), subtractive CMY and a colour wheel. Commands speak sRGB —
//! names, hex, RGB bytes, CMY percentages, HSB or a blackbody temperature — and
//! [`assignments`] turns one intent into the levels of one mode. The sheet
//! swatch and the `Color` listing read the same system back with [`swatch`] and
//! [`describe`], so a CMY fixture is never displayed as black again.
//!
//! Nothing here is persisted. The written values are ordinary programmer
//! levels, which cues, palettes, undo and the journal already understand.

use crate::fixture_type::{Channel, ChannelFunction, Mode, SetSelector};

/// Lowest accepted blackbody temperature, in kelvin.
pub const MIN_KELVIN: u16 = 1000;
/// Highest accepted blackbody temperature, in kelvin.
pub const MAX_KELVIN: u16 = 40000;

/// An sRGB colour with components in `0..=1`.
///
/// This is the space DMX RGB levels already mean on a desk and the space the
/// GUI swatch displays. There is no colour management: the components are
/// sRGB-encoded values, compared and mixed as such.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    pub fn new(r: f64, g: f64, b: f64) -> Self {
        Self {
            r: clamp01(r),
            g: clamp01(g),
            b: clamp01(b),
        }
    }

    pub fn from_bytes(bytes: [u8; 3]) -> Self {
        Self::new(
            f64::from(bytes[0]) / 255.0,
            f64::from(bytes[1]) / 255.0,
            f64::from(bytes[2]) / 255.0,
        )
    }

    pub fn bytes(self) -> [u8; 3] {
        [
            (self.r * 255.0).round().clamp(0.0, 255.0) as u8,
            (self.g * 255.0).round().clamp(0.0, 255.0) as u8,
            (self.b * 255.0).round().clamp(0.0, 255.0) as u8,
        ]
    }

    pub fn hex(self) -> String {
        let [r, g, b] = self.bytes();
        format!("#{r:02X}{g:02X}{b:02X}")
    }

    /// The subtractive components, `0..=1`: `C = 1 - R`, and so on.
    pub fn to_cmy(self) -> (f64, f64, f64) {
        (1.0 - self.r, 1.0 - self.g, 1.0 - self.b)
    }

    /// Hue in degrees (`0..360`), saturation and brightness in `0..=1`.
    pub fn to_hsb(self) -> (f64, f64, f64) {
        let max = self.r.max(self.g).max(self.b);
        let min = self.r.min(self.g).min(self.b);
        let delta = max - min;
        let hue = if delta <= f64::EPSILON {
            0.0
        } else if (max - self.r).abs() < f64::EPSILON {
            60.0 * (((self.g - self.b) / delta) % 6.0)
        } else if (max - self.g).abs() < f64::EPSILON {
            60.0 * ((self.b - self.r) / delta + 2.0)
        } else {
            60.0 * ((self.r - self.g) / delta + 4.0)
        };
        let saturation = if max <= f64::EPSILON {
            0.0
        } else {
            delta / max
        };
        (hue.rem_euclid(360.0), saturation, max)
    }

    /// Standard HSB to RGB. `hue` wraps, `saturation` and `brightness` clamp.
    pub fn from_hsb(hue: f64, saturation: f64, brightness: f64) -> Self {
        let hue = if hue.is_finite() {
            hue.rem_euclid(360.0)
        } else {
            0.0
        };
        let saturation = clamp01(saturation);
        let brightness = clamp01(brightness);
        let chroma = brightness * saturation;
        let sector = hue / 60.0;
        let second = chroma * (1.0 - (sector % 2.0 - 1.0).abs());
        let (r, g, b) = match sector.floor() as i64 {
            0 => (chroma, second, 0.0),
            1 => (second, chroma, 0.0),
            2 => (0.0, chroma, second),
            3 => (0.0, second, chroma),
            4 => (second, 0.0, chroma),
            _ => (chroma, 0.0, second),
        };
        let floor = brightness - chroma;
        Self::new(r + floor, g + floor, b + floor)
    }

    /// Squared sRGB distance. Squared is enough for ordering and avoids a
    /// square root that would only lose precision.
    pub fn distance(self, other: Rgb) -> f64 {
        let dr = self.r - other.r;
        let dg = self.g - other.g;
        let db = self.b - other.b;
        dr * dr + dg * dg + db * db
    }
}

/// A colour as typed on the command line.
///
/// `Cmy`, `Hsb` and `Kelvin` carry their human units: CMY and HSB saturation/
/// brightness are percentages `0..=100`, hue is degrees `0..=360`, and Kelvin
/// is an absolute temperature. [`ColorSpec::resolve`] converts them to sRGB.
#[derive(Debug, Clone, PartialEq)]
pub enum ColorSpec {
    Named(String),
    Hex(String),
    Rgb(u8, u8, u8),
    Cmy(f64, f64, f64),
    Hsb(f64, f64, f64),
    Kelvin(u16),
}

impl ColorSpec {
    /// The colour this spec means, or a message naming the bad value.
    pub fn resolve(&self) -> Result<Rgb, String> {
        match self {
            ColorSpec::Named(name) => named(name).ok_or_else(|| format!("unknown colour `{name}`")),
            ColorSpec::Hex(text) => {
                parse_hex(text).ok_or_else(|| format!("`{text}` is not a colour: use `#RRGGBB`"))
            }
            ColorSpec::Rgb(r, g, b) => Ok(Rgb::from_bytes([*r, *g, *b])),
            ColorSpec::Cmy(c, m, y) => {
                Ok(Rgb::new(1.0 - c / 100.0, 1.0 - m / 100.0, 1.0 - y / 100.0))
            }
            ColorSpec::Hsb(h, s, b) => Ok(Rgb::from_hsb(*h, s / 100.0, b / 100.0)),
            ColorSpec::Kelvin(kelvin) => {
                if !(MIN_KELVIN..=MAX_KELVIN).contains(kelvin) {
                    return Err(format!(
                        "colour temperature {kelvin}K is outside {MIN_KELVIN}..{MAX_KELVIN}K"
                    ));
                }
                Ok(from_kelvin(*kelvin))
            }
        }
    }

    /// How the value is printed in a command message.
    pub fn label(&self) -> String {
        match self {
            ColorSpec::Named(name) => name.clone(),
            ColorSpec::Hex(text) => parse_hex(text)
                .map(Rgb::hex)
                .unwrap_or_else(|| text.clone()),
            ColorSpec::Rgb(r, g, b) => format!("Rgb {r} {g} {b}"),
            ColorSpec::Cmy(c, m, y) => format!(
                "Cmy {} {} {}",
                trim_number(*c),
                trim_number(*m),
                trim_number(*y)
            ),
            ColorSpec::Hsb(h, s, b) => format!(
                "Hsb {} {} {}",
                trim_number(*h),
                trim_number(*s),
                trim_number(*b)
            ),
            ColorSpec::Kelvin(kelvin) => format!("{kelvin}K"),
        }
    }

    /// True when the label is already a hex colour, so a message need not
    /// print the hex twice.
    pub fn is_hex(&self) -> bool {
        matches!(self, ColorSpec::Hex(_))
    }
}

/// Named colours. A small fixed table, not a gel library: these are the words
/// the desk itself promises to understand.
pub const NAMED_COLOURS: &[(&str, [u8; 3])] = &[
    ("Black", [0x00, 0x00, 0x00]),
    ("Grey", [0x80, 0x80, 0x80]),
    ("White", [0xFF, 0xFF, 0xFF]),
    ("Warm White", [0xFF, 0xE0, 0xC0]),
    ("Cool White", [0xE0, 0xF0, 0xFF]),
    ("Red", [0xFF, 0x00, 0x00]),
    ("Orange", [0xFF, 0x7F, 0x00]),
    ("Amber", [0xFF, 0xBF, 0x00]),
    ("Yellow", [0xFF, 0xFF, 0x00]),
    ("Lime", [0x80, 0xFF, 0x00]),
    ("Green", [0x00, 0xFF, 0x00]),
    ("Cyan", [0x00, 0xFF, 0xFF]),
    ("Blue", [0x00, 0x00, 0xFF]),
    ("Purple", [0x80, 0x00, 0xFF]),
    ("Magenta", [0xFF, 0x00, 0xFF]),
    ("Pink", [0xFF, 0x80, 0xBF]),
    ("UV", [0x7A, 0x00, 0xFF]),
];

/// Alternate words a wheel or an operator may use for a named colour.
const COLOUR_ALIASES: &[(&str, &str)] = &[
    ("Gray", "Grey"),
    ("Open", "White"),
    ("No Colour", "White"),
    ("No Color", "White"),
    ("CTO", "Warm White"),
    ("CTB", "Cool White"),
];

/// Resolve a colour name, case-insensitively, including the aliases.
pub fn named(name: &str) -> Option<Rgb> {
    let name = name.trim();
    if let Some((_, bytes)) = NAMED_COLOURS
        .iter()
        .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
    {
        return Some(Rgb::from_bytes(*bytes));
    }
    COLOUR_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .and_then(|(_, canonical)| named(canonical))
}

/// Resolve a colour wheel set's name.
///
/// GDTF carries no colour for a channel function, only its name. The table and
/// aliases cover the common words; `CTO`/`CTB` prefixes cover filters named
/// after their temperature; a trailing number is stripped so `Red 2` reads as
/// `Red`. Anything else is deliberately not guessed.
pub fn wheel_colour(name: &str) -> Option<Rgb> {
    if let Some(rgb) = named(name) {
        return Some(rgb);
    }
    let lower = name.trim().to_ascii_lowercase();
    if lower.starts_with("cto") {
        return named("Warm White");
    }
    if lower.starts_with("ctb") {
        return named("Cool White");
    }
    let stripped = lower.trim_end_matches(|character: char| {
        character.is_ascii_digit() || character.is_whitespace()
    });
    named(stripped)
}

/// Parse exactly `#RRGGBB`.
pub fn parse_hex(text: &str) -> Option<Rgb> {
    let digits = text.trim().strip_prefix('#')?;
    if digits.len() != 6
        || !digits
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    {
        return None;
    }
    let value = u32::from_str_radix(digits, 16).ok()?;
    Some(Rgb::from_bytes([
        (value >> 16) as u8,
        ((value >> 8) & 0xFF) as u8,
        (value & 0xFF) as u8,
    ]))
}

/// Planckian-locus approximation, normalised so the brightest component is 1.
///
/// A white point should not dim the fixture: a 3200K white is a warm white at
/// full, not a dim orange.
pub fn from_kelvin(kelvin: u16) -> Rgb {
    let temperature = (f64::from(kelvin) / 100.0).clamp(10.0, 400.0);
    let channel = |value: f64| (value / 255.0).clamp(0.0, 1.0);
    let red = if temperature <= 66.0 {
        255.0
    } else {
        329.698_727_446 * (temperature - 60.0).powf(-0.133_204_759_2)
    };
    let green = if temperature <= 66.0 {
        99.470_802_586_1 * temperature.ln() - 161.119_568_166_1
    } else {
        288.122_169_528_3 * (temperature - 60.0).powf(-0.075_514_849_2)
    };
    let blue = if temperature >= 66.0 {
        255.0
    } else if temperature <= 19.0 {
        0.0
    } else {
        138.517_731_223_1 * (temperature - 10.0).ln() - 305.044_792_730_7
    };
    let rgb = Rgb::new(channel(red), channel(green), channel(blue));
    let brightest = rgb.r.max(rgb.g).max(rgb.b);
    if brightest > 0.0 {
        Rgb::new(rgb.r / brightest, rgb.g / brightest, rgb.b / brightest)
    } else {
        rgb
    }
}

/// How a mode makes colour.
enum System<'a> {
    Additive {
        red: &'a Channel,
        green: &'a Channel,
        blue: &'a Channel,
        white: Option<&'a Channel>,
    },
    Subtractive {
        cyan: &'a Channel,
        magenta: &'a Channel,
        yellow: &'a Channel,
    },
    Wheel {
        channel: &'a Channel,
    },
}

/// The levels one command writes for one mode.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Canonical attribute → physical level.
    pub assignments: Vec<(String, f64)>,
    /// Set index and name when the mode is a wheel.
    pub wheel: Option<(usize, String)>,
}

/// Resolve the levels that show `spec` on `mode`.
///
/// Additive is preferred over subtractive is impossible (a mode has one or the
/// other); additive is preferred over a wheel because a continuous system can
/// hit the colour exactly. A partial system is rejected rather than guessed.
pub fn assignments(mode: &Mode, spec: &ColorSpec) -> Result<Plan, String> {
    let rgb = spec.resolve()?;
    match system(mode)? {
        System::Additive {
            red,
            green,
            blue,
            white,
        } => {
            let extracted = white.map_or(0.0, |_| rgb.r.min(rgb.g).min(rgb.b));
            let mut assignments = vec![
                (red.attribute.clone(), rgb.r - extracted),
                (green.attribute.clone(), rgb.g - extracted),
                (blue.attribute.clone(), rgb.b - extracted),
            ];
            if let Some(white) = white {
                assignments.push((white.attribute.clone(), extracted));
            }
            Ok(Plan {
                assignments,
                wheel: None,
            })
        }
        System::Subtractive {
            cyan,
            magenta,
            yellow,
        } => {
            let (c, m, y) = rgb.to_cmy();
            Ok(Plan {
                assignments: vec![
                    (cyan.attribute.clone(), c),
                    (magenta.attribute.clone(), m),
                    (yellow.attribute.clone(), y),
                ],
                wheel: None,
            })
        }
        System::Wheel { channel } => {
            let (index, name) = wheel_choice(channel, spec, rgb)?;
            let level = channel
                .level_for_set(SetSelector::Index(index))
                .ok_or_else(|| {
                    format!("`{}` set {index} has no physical range", channel.attribute)
                })?;
            Ok(Plan {
                assignments: vec![(channel.attribute.clone(), level)],
                wheel: Some((index, name)),
            })
        }
    }
}

/// The rendered colour of a mode, reading each attribute through `level`.
///
/// `None` means the mode has no colour system, or its wheel is between sets.
pub fn swatch(mode: &Mode, level: impl Fn(&str) -> f64) -> Option<Rgb> {
    match system(mode).ok()? {
        System::Additive {
            red,
            green,
            blue,
            white,
        } => {
            let white = white.map_or(0.0, |channel| level(&channel.attribute));
            Some(Rgb::new(
                level(&red.attribute) + white,
                level(&green.attribute) + white,
                level(&blue.attribute) + white,
            ))
        }
        System::Subtractive {
            cyan,
            magenta,
            yellow,
        } => Some(Rgb::new(
            1.0 - level(&cyan.attribute),
            1.0 - level(&magenta.attribute),
            1.0 - level(&yellow.attribute),
        )),
        System::Wheel { channel } => {
            let (_, function) = current_set(channel, level(&channel.attribute))?;
            wheel_colour(&function.name)
        }
    }
}

/// One line describing a mode's current colour: its system, its components and
/// the resulting hex. `Err` explains why the mode has no colour system.
pub fn describe(mode: &Mode, level: impl Fn(&str) -> f64) -> Result<String, String> {
    match system(mode)? {
        System::Additive {
            red,
            green,
            blue,
            white,
        } => {
            let mut parts = vec![
                format!("R {}%", percent(level(&red.attribute))),
                format!("G {}%", percent(level(&green.attribute))),
                format!("B {}%", percent(level(&blue.attribute))),
            ];
            if let Some(white) = white {
                parts.push(format!("W {}%", percent(level(&white.attribute))));
            }
            let rgb = swatch(mode, level).unwrap_or_default();
            Ok(format!("additive  {}  {}", parts.join(" "), rgb.hex()))
        }
        System::Subtractive {
            cyan,
            magenta,
            yellow,
        } => {
            let parts = [
                format!("C {}%", percent(level(&cyan.attribute))),
                format!("M {}%", percent(level(&magenta.attribute))),
                format!("Y {}%", percent(level(&yellow.attribute))),
            ];
            let rgb = swatch(mode, level).unwrap_or_default();
            Ok(format!("subtractive  {}  {}", parts.join(" "), rgb.hex()))
        }
        System::Wheel { channel } => match current_set(channel, level(&channel.attribute)) {
            Some((index, function)) => match wheel_colour(&function.name) {
                Some(rgb) => Ok(format!(
                    "wheel  set {index} \"{}\"  {}",
                    function.name,
                    rgb.hex()
                )),
                None => Ok(format!("wheel  set {index} \"{}\"", function.name)),
            },
            None => Ok("wheel  no set".to_string()),
        },
    }
}

fn system(mode: &Mode) -> Result<System<'_>, String> {
    if let (Some(red), Some(green), Some(blue)) = (
        mode.channel_for("ColorAdd_R"),
        mode.channel_for("ColorAdd_G"),
        mode.channel_for("ColorAdd_B"),
    ) {
        return Ok(System::Additive {
            red,
            green,
            blue,
            white: mode.channel_for("ColorAdd_W"),
        });
    }
    if let (Some(cyan), Some(magenta), Some(yellow)) = (
        mode.channel_for("ColorSub_C"),
        mode.channel_for("ColorSub_M"),
        mode.channel_for("ColorSub_Y"),
    ) {
        return Ok(System::Subtractive {
            cyan,
            magenta,
            yellow,
        });
    }
    if let Some(channel) = mode
        .channels
        .iter()
        .filter(|channel| is_wheel(channel) && !channel.functions.is_empty())
        .min_by_key(|channel| channel.offset)
    {
        return Ok(System::Wheel { channel });
    }
    let mut attributes: Vec<&str> = mode
        .channels
        .iter()
        .map(|channel| channel.attribute.as_str())
        .collect();
    attributes.sort_unstable();
    attributes.dedup();
    Err(format!(
        "no additive (ColorAdd_R/G/B), subtractive (ColorSub_C/M/Y) or ColorWheel channel (attributes: {})",
        if attributes.is_empty() {
            "none".to_string()
        } else {
            attributes.join(", ")
        }
    ))
}

fn is_wheel(channel: &Channel) -> bool {
    channel
        .attribute
        .get(..10)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("colorwheel"))
}

fn wheel_choice(channel: &Channel, spec: &ColorSpec, rgb: Rgb) -> Result<(usize, String), String> {
    // An operator who names a set gets that set, even if another one is nearer.
    if let ColorSpec::Named(name) = spec {
        if let Some((index, _)) = channel
            .sets()
            .find(|(_, function)| function.name.eq_ignore_ascii_case(name.trim()))
        {
            return Ok((index, name.trim().to_string()));
        }
    }
    let mut best: Option<(usize, String, f64)> = None;
    for (index, function) in channel.sets() {
        let Some(colour) = wheel_colour(&function.name) else {
            continue;
        };
        let distance = rgb.distance(colour);
        if best
            .as_ref()
            .is_none_or(|(_, _, previous)| distance < *previous)
        {
            best = Some((index, function.name.clone(), distance));
        }
    }
    best.map(|(index, name, _)| (index, name)).ok_or_else(|| {
        let names: Vec<&str> = channel
            .sets()
            .map(|(_, function)| function.name.as_str())
            .collect();
        format!(
            "`{}` has no set named as a colour (sets: {})",
            channel.attribute,
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        )
    })
}

/// The set whose DMX band contains the current level.
fn current_set(channel: &Channel, level: f64) -> Option<(usize, &ChannelFunction)> {
    let coarse = channel.raw_from_level(level);
    channel.sets().find(|(_, function)| {
        coarse >= function.dmx_from.min(function.dmx_to)
            && coarse <= function.dmx_from.max(function.dmx_to)
    })
}

fn percent(level: f64) -> i64 {
    (level.clamp(0.0, 1.0) * 100.0).round() as i64
}

fn clamp01(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Print `60.0` as `60` and `42.5` as `42.5`, so labels stay readable.
fn trim_number(value: f64) -> String {
    if (value - value.round()).abs() < 1e-9 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value}")
    }
}

impl Default for Rgb {
    fn default() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_bytes(rgb: Rgb, expected: [u8; 3]) {
        assert_eq!(rgb.bytes(), expected, "{}", rgb.hex());
    }

    fn channel(offset: u16, attribute: &str) -> Channel {
        Channel {
            offset,
            fine_offset: None,
            attribute: attribute.into(),
            default_raw: 0,
            functions: Vec::new(),
        }
    }

    fn function(name: &str, dmx_from: u8, dmx_to: u8, from: f64, to: f64) -> ChannelFunction {
        ChannelFunction {
            name: name.into(),
            dmx_from,
            dmx_to,
            physical_from: from,
            physical_to: to,
        }
    }

    fn additive_mode() -> Mode {
        Mode {
            name: "rgb".into(),
            channels: vec![
                channel(1, "Dimmer"),
                channel(2, "ColorAdd_R"),
                channel(3, "ColorAdd_G"),
                channel(4, "ColorAdd_B"),
            ],
        }
    }

    fn rgbw_mode() -> Mode {
        let mut mode = additive_mode();
        mode.channels.push(channel(5, "ColorAdd_W"));
        mode
    }

    fn subtractive_mode() -> Mode {
        Mode {
            name: "cmy".into(),
            channels: vec![
                channel(1, "Dimmer"),
                channel(2, "ColorSub_C"),
                channel(3, "ColorSub_M"),
                channel(4, "ColorSub_Y"),
            ],
        }
    }

    fn wheel_mode() -> Mode {
        Mode {
            name: "wheel".into(),
            channels: vec![
                channel(1, "Dimmer"),
                Channel {
                    offset: 2,
                    fine_offset: None,
                    attribute: "ColorWheel1".into(),
                    default_raw: 0,
                    functions: vec![
                        function("Open", 0, 31, 0.0, 20.0),
                        function("Red", 32, 63, 20.0, 40.0),
                        function("Green", 64, 95, 40.0, 60.0),
                        function("Blue", 96, 127, 60.0, 80.0),
                        function("CTO 3200K", 128, 159, 80.0, 100.0),
                    ],
                },
            ],
        }
    }

    fn level_of(attribute: &str) -> f64 {
        match attribute {
            "ColorAdd_R" | "ColorSub_C" => 0.0,
            "ColorAdd_G" | "ColorSub_M" => 1.0,
            "ColorAdd_B" | "ColorSub_Y" => 1.0,
            _ => 0.0,
        }
    }

    #[test]
    fn named_lookup_is_case_insensitive_and_resolves_aliases() {
        assert_bytes(named("red").unwrap(), [255, 0, 0]);
        assert_bytes(named("RED").unwrap(), [255, 0, 0]);
        assert_bytes(named("Open").unwrap(), [255, 255, 255]);
        assert_bytes(named("no color").unwrap(), [255, 255, 255]);
        assert_bytes(named("CTO").unwrap(), named("Warm White").unwrap().bytes());
        assert!(named("Chartreuse").is_none());
        assert_bytes(named("gray").unwrap(), [128, 128, 128]);
        assert_eq!(NAMED_COLOURS.len(), 17);
    }

    #[test]
    fn hex_accepts_six_digits_and_rejects_anything_else() {
        let rgb = parse_hex("#FF8000").unwrap();
        assert_eq!(rgb.bytes(), [255, 128, 0]);
        assert_eq!(parse_hex("#ff8000").unwrap().bytes(), [255, 128, 0]);
        assert_eq!(parse_hex("#000000").unwrap().bytes(), [0, 0, 0]);
        assert!(parse_hex("#F80").is_none());
        assert!(parse_hex("FF8000").is_none());
        assert!(parse_hex("#GG8000").is_none());
        assert!(parse_hex("#FF80000").is_none());
        assert_eq!(parse_hex("#FF8000").unwrap().hex(), "#FF8000");
    }

    #[test]
    fn hsb_round_trips_primaries_and_greys() {
        assert_eq!(Rgb::from_hsb(0.0, 1.0, 1.0).to_hsb(), (0.0, 1.0, 1.0));
        assert_bytes(Rgb::from_hsb(0.0, 1.0, 1.0), [255, 0, 0]);
        assert_bytes(Rgb::from_hsb(60.0, 1.0, 1.0), [255, 255, 0]);
        assert_bytes(Rgb::from_hsb(120.0, 1.0, 1.0), [0, 255, 0]);
        assert_bytes(Rgb::from_hsb(240.0, 1.0, 1.0), [0, 0, 255]);
        assert_bytes(Rgb::from_hsb(360.0, 1.0, 1.0), [255, 0, 0]);
        let grey = Rgb::from_hsb(90.0, 0.0, 0.5);
        assert_bytes(grey, [128, 128, 128]);
        let (_, saturation, _) = grey.to_hsb();
        assert!(saturation.abs() < 1e-9);
        assert_bytes(Rgb::from_hsb(30.0, 1.0, 0.5), [128, 64, 0]);
    }

    #[test]
    fn cmy_inverts_rgb() {
        let red = Rgb::from_bytes([255, 0, 0]);
        assert_eq!(red.to_cmy(), (0.0, 1.0, 1.0));
        let (c, m, y) = Rgb::from_bytes([51, 102, 204]).to_cmy();
        assert_bytes(Rgb::new(1.0 - c, 1.0 - m, 1.0 - y), [51, 102, 204]);
    }

    #[test]
    fn kelvin_gets_warmer_as_it_gets_lower() {
        let warm = from_kelvin(3200);
        let cool = from_kelvin(9000);
        assert!(warm.r > warm.g && warm.g > warm.b, "{}", warm.hex());
        assert!(cool.b > cool.r, "{}", cool.hex());
        assert!(from_kelvin(2000).g < warm.g, "2000K is warmer than 3200K");
        let daylight = from_kelvin(6500);
        assert!(daylight.r.min(daylight.g).min(daylight.b) > 0.9);
        assert_eq!(from_kelvin(3200).r, 1.0, "the white point is not dimmed");
        let error = ColorSpec::Kelvin(900).resolve().unwrap_err();
        assert!(error.contains("900K"), "{error}");
        assert!(ColorSpec::Kelvin(6500).resolve().is_ok());
    }

    #[test]
    fn additive_assigns_three_levels() {
        let plan = assignments(&additive_mode(), &ColorSpec::Named("Red".into())).unwrap();
        assert_eq!(
            plan.assignments,
            vec![
                ("ColorAdd_R".into(), 1.0),
                ("ColorAdd_G".into(), 0.0),
                ("ColorAdd_B".into(), 0.0),
            ]
        );
        assert!(plan.wheel.is_none());

        let orange = ColorSpec::Hex("#FF8000".into());
        let plan = assignments(&additive_mode(), &orange).unwrap();
        assert!((plan.assignments[0].1 - 1.0).abs() < 1e-9);
        assert!((plan.assignments[1].1 - 128.0 / 255.0).abs() < 1e-9);
        assert_eq!(plan.assignments[2].1, 0.0);
    }

    #[test]
    fn rgbw_extracts_the_white_component() {
        let plan = assignments(&rgbw_mode(), &ColorSpec::Named("White".into())).unwrap();
        assert_eq!(
            plan.assignments,
            vec![
                ("ColorAdd_R".into(), 0.0),
                ("ColorAdd_G".into(), 0.0),
                ("ColorAdd_B".into(), 0.0),
                ("ColorAdd_W".into(), 1.0),
            ]
        );

        let plan = assignments(&rgbw_mode(), &ColorSpec::Named("Red".into())).unwrap();
        assert_eq!(
            plan.assignments.last(),
            Some(&("ColorAdd_W".into(), 0.0)),
            "a saturated colour clears the white emitter"
        );

        let plan = assignments(&rgbw_mode(), &ColorSpec::Rgb(255, 255, 128)).unwrap();
        let white = plan.assignments[3].1;
        assert!((white - 128.0 / 255.0).abs() < 1e-9, "W = min(r, g, b)");
        assert!((plan.assignments[0].1 - (1.0 - white)).abs() < 1e-9);
        assert_eq!(plan.assignments[2].1, 0.0);
    }

    #[test]
    fn subtractive_inverts_the_colour() {
        let plan = assignments(&subtractive_mode(), &ColorSpec::Named("Red".into())).unwrap();
        assert_eq!(
            plan.assignments,
            vec![
                ("ColorSub_C".into(), 0.0),
                ("ColorSub_M".into(), 1.0),
                ("ColorSub_Y".into(), 1.0),
            ]
        );

        let plan = assignments(&subtractive_mode(), &ColorSpec::Named("White".into())).unwrap();
        assert!(plan.assignments.iter().all(|(_, level)| level.abs() < 1e-9));
    }

    #[test]
    fn additive_wins_when_a_mode_has_both() {
        let mut mode = additive_mode();
        mode.channels.push(Channel {
            offset: 5,
            fine_offset: None,
            attribute: "ColorWheel1".into(),
            default_raw: 0,
            functions: vec![function("Red", 0, 255, 0.0, 100.0)],
        });
        let plan = assignments(&mode, &ColorSpec::Named("Green".into())).unwrap();
        assert!(plan.wheel.is_none());
        assert_eq!(plan.assignments.len(), 3);
    }

    #[test]
    fn wheel_prefers_an_exact_name_and_otherwise_the_nearest() {
        let mode = wheel_mode();
        let plan = assignments(&mode, &ColorSpec::Named("Red".into())).unwrap();
        assert_eq!(plan.wheel, Some((2, "Red".into())));
        assert_eq!(plan.assignments, vec![("ColorWheel1".into(), 0.3)]);

        let plan = assignments(&mode, &ColorSpec::Rgb(0, 0, 255)).unwrap();
        assert_eq!(plan.wheel, Some((4, "Blue".into())));

        let plan = assignments(&mode, &ColorSpec::Hsb(120.0, 100.0, 100.0)).unwrap();
        assert_eq!(plan.wheel, Some((3, "Green".into())));

        let plan = assignments(&mode, &ColorSpec::Named("CTO".into())).unwrap();
        assert_eq!(
            plan.wheel,
            Some((5, "CTO 3200K".into())),
            "warm white is nearest to the CTO set"
        );

        // Exact name wins over an equally close set: `Open` and `White` both
        // resolve to white, but the operator asked for `Open`.
        let mut duplicate = wheel_mode();
        duplicate.channels[1].functions = vec![
            function("White", 0, 127, 0.0, 50.0),
            function("Open", 128, 255, 50.0, 100.0),
        ];
        let plan = assignments(&duplicate, &ColorSpec::Named("Open".into())).unwrap();
        assert_eq!(plan.wheel, Some((2, "Open".into())));
    }

    #[test]
    fn wheel_ties_pick_the_lowest_index() {
        let mode = Mode {
            name: "wheel".into(),
            channels: vec![
                channel(1, "Dimmer"),
                Channel {
                    offset: 2,
                    fine_offset: None,
                    attribute: "ColorWheel1".into(),
                    default_raw: 0,
                    functions: vec![
                        function("Red", 0, 127, 0.0, 100.0),
                        function("Green", 128, 255, 100.0, 200.0),
                    ],
                },
            ],
        };
        let plan = assignments(&mode, &ColorSpec::Rgb(255, 255, 0)).unwrap();
        assert_eq!(plan.wheel, Some((1, "Red".into())));
    }

    #[test]
    fn wheel_without_named_sets_lists_them() {
        let mode = Mode {
            name: "wheel".into(),
            channels: vec![
                channel(1, "Dimmer"),
                Channel {
                    offset: 2,
                    fine_offset: None,
                    attribute: "ColorWheel1".into(),
                    default_raw: 0,
                    functions: vec![function("Gobo 1", 0, 255, 0.0, 100.0)],
                },
            ],
        };
        let error = assignments(&mode, &ColorSpec::Named("Red".into())).unwrap_err();
        assert!(error.contains("no set named as a colour"), "{error}");
        assert!(error.contains("Gobo 1"), "{error}");
    }

    #[test]
    fn partial_and_missing_systems_are_rejected_with_the_attributes() {
        let partial = Mode {
            name: "half rgb".into(),
            channels: vec![
                channel(1, "Dimmer"),
                channel(2, "ColorAdd_R"),
                channel(3, "ColorAdd_G"),
            ],
        };
        let error = assignments(&partial, &ColorSpec::Named("Red".into())).unwrap_err();
        assert!(error.contains("ColorAdd_R/G/B"), "{error}");
        assert!(error.contains("ColorAdd_R"), "{error}");

        let plain = Mode {
            name: "dimmer".into(),
            channels: vec![channel(1, "Dimmer")],
        };
        let error = assignments(&plain, &ColorSpec::Named("Red".into())).unwrap_err();
        assert!(error.contains("Dimmer"), "{error}");
        assert!(describe(&plain, |_| 0.0).is_err());
        assert!(swatch(&plain, |_| 0.0).is_none());
    }

    #[test]
    fn swatch_reads_every_system() {
        let additive = additive_mode();
        let rgb = swatch(&additive, level_of).unwrap();
        assert_bytes(rgb, [0, 255, 255]);

        let subtractive = subtractive_mode();
        let rgb = swatch(&subtractive, level_of).unwrap();
        assert_bytes(rgb, [255, 0, 0]);

        let wheel = wheel_mode();
        let red_level = wheel.channels[1]
            .level_for_set(SetSelector::Name("Red"))
            .unwrap();
        assert_bytes(swatch(&wheel, |_| red_level).unwrap(), [255, 0, 0]);

        let rgbw = rgbw_mode();
        assert_bytes(
            swatch(
                &rgbw,
                |attribute| {
                    if attribute == "ColorAdd_W" {
                        1.0
                    } else {
                        0.0
                    }
                },
            )
            .unwrap(),
            [255, 255, 255],
        );
    }

    #[test]
    fn describe_prints_components_and_the_wheel_set() {
        let additive = additive_mode();
        let line = describe(&additive, |_| 1.0).unwrap();
        assert_eq!(line, "additive  R 100% G 100% B 100%  #FFFFFF");

        let wheel = wheel_mode();
        let red_level = wheel.channels[1]
            .level_for_set(SetSelector::Name("Red"))
            .unwrap();
        let line = describe(&wheel, |_| red_level).unwrap();
        assert_eq!(line, "wheel  set 2 \"Red\"  #FF0000");

        let line = describe(&wheel, |_| 0.0).unwrap();
        assert_eq!(line, "wheel  set 1 \"Open\"  #FFFFFF");

        let subtractive = subtractive_mode();
        let line = describe(&subtractive, |_| 0.0).unwrap();
        assert_eq!(line, "subtractive  C 0% M 0% Y 0%  #FFFFFF");
    }

    #[test]
    fn spec_labels_are_stable() {
        assert_eq!(ColorSpec::Named("Red".into()).label(), "Red");
        assert_eq!(ColorSpec::Hex("#ff8000".into()).label(), "#FF8000");
        assert!(ColorSpec::Hex("#ff8000".into()).is_hex());
        assert_eq!(ColorSpec::Rgb(255, 128, 0).label(), "Rgb 255 128 0");
        assert_eq!(ColorSpec::Cmy(0.0, 60.0, 100.0).label(), "Cmy 0 60 100");
        assert_eq!(ColorSpec::Hsb(30.0, 100.0, 50.5).label(), "Hsb 30 100 50.5");
        assert_eq!(ColorSpec::Kelvin(3200).label(), "3200K");
        assert_eq!(
            ColorSpec::Cmy(0.0, 60.0, 100.0).resolve().unwrap().bytes(),
            [255, 102, 0]
        );
        assert_eq!(
            ColorSpec::Hsb(30.0, 100.0, 50.0).resolve().unwrap().bytes(),
            [128, 64, 0]
        );
    }

    #[test]
    fn additive_uv_is_left_alone() {
        let mut mode = additive_mode();
        mode.channels.push(channel(5, "ColorAdd_UV"));
        let plan = assignments(&mode, &ColorSpec::Named("Red".into())).unwrap();
        assert!(
            !plan
                .assignments
                .iter()
                .any(|(attribute, _)| attribute == "ColorAdd_UV"),
            "UV is not part of the visible colour model"
        );
    }
}
