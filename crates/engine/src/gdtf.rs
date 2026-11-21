//! GDTF import.
//!
//! A `.gdtf` file is a zip archive holding `description.xml`. Only the parts the
//! engine needs are read: attributes, DMX modes, channel offsets and the channel
//! functions that describe each attribute's physical range.
//!
//! Deliberately not read (yet): wheels, models, physical descriptions and MP
//! ranges. Channel functions become channel sets and a fine DMX slot becomes
//! `Channel::fine_offset`; everything skipped is reported as an import warning
//! instead of failing the import.

use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::{Path, PathBuf};

use roxmltree::{Document, Node};

use crate::fixture_type::{AttributeDef, Channel, ChannelFunction, FixtureType, Mode};
use crate::limits::{self, LimitError, MAX_CHANNEL_FUNCTIONS, MAX_MODE_CHANNELS, MAX_SHOW_TYPES};

#[derive(Debug, Default)]
pub struct ImportReport {
    pub types: Vec<FixtureType>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
#[error("{}: {kind}", path.display())]
pub struct GdtfError {
    pub path: PathBuf,
    pub kind: GdtfErrorKind,
}

#[derive(Debug, thiserror::Error)]
pub enum GdtfErrorKind {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not a readable .gdtf archive: {0}")]
    Archive(String),
    #[error("archive has no description.xml")]
    MissingDescription,
    #[error("description.xml is not valid XML: {0}")]
    Xml(String),
    #[error("description.xml contains no <FixtureType>")]
    NoFixtureType,
    #[error("{0}")]
    Limit(#[from] LimitError),
}

/// Read a `.gdtf` archive.
pub fn import(path: &Path) -> Result<ImportReport, GdtfError> {
    let file = File::open(path).map_err(|source| GdtfError {
        path: path.to_path_buf(),
        kind: GdtfErrorKind::Io(source),
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| GdtfError {
        path: path.to_path_buf(),
        kind: GdtfErrorKind::Archive(error.to_string()),
    })?;
    let xml = description_from_archive(&mut archive).map_err(|kind| GdtfError {
        path: path.to_path_buf(),
        kind,
    })?;
    parse_description(&xml).map_err(|kind| GdtfError {
        path: path.to_path_buf(),
        kind,
    })
}

/// Read a `.gdtf` archive already held in memory. MVR embeds these archives
/// inside its own zip, so extracting to a temp directory is unnecessary.
pub fn import_bytes(bytes: &[u8]) -> Result<ImportReport, GdtfErrorKind> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|error| GdtfErrorKind::Archive(error.to_string()))?;
    let xml = description_from_archive(&mut archive)?;
    parse_description(&xml)
}

fn description_from_archive<R: Read + Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<String, GdtfErrorKind> {
    limits::check_archive(archive)?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| GdtfErrorKind::Archive(error.to_string()))?;
        let name = entry
            .name()
            .map_err(|error| GdtfErrorKind::Archive(error.to_string()))?
            .replace('\\', "/")
            .to_ascii_lowercase();
        if name == "description.xml" || name.ends_with("/description.xml") {
            let bytes = limits::read_entry(&mut entry, &name)?;
            return String::from_utf8(bytes)
                .map_err(|error| GdtfErrorKind::Xml(format!("description is not UTF-8: {error}")));
        }
    }
    Err(GdtfErrorKind::MissingDescription)
}

/// Parse the contents of `description.xml`.
pub fn parse_description(xml: &str) -> Result<ImportReport, GdtfErrorKind> {
    let document = Document::parse(xml).map_err(|error| GdtfErrorKind::Xml(error.to_string()))?;
    let root = document.root_element();

    let nodes: Vec<Node> = if root.has_tag_name("FixtureType") {
        vec![root]
    } else {
        root.children()
            .filter(|node| node.has_tag_name("FixtureType"))
            .collect()
    };

    let mut types = Vec::new();
    let mut warnings = Vec::new();
    for node in nodes {
        if types.len() >= MAX_SHOW_TYPES {
            return Err(LimitError::FixtureTypes {
                found: types.len() + 1,
            }
            .into());
        }
        types.push(parse_fixture_type(node, &mut warnings)?);
    }
    if types.is_empty() {
        return Err(GdtfErrorKind::NoFixtureType);
    }
    for fixture_type in &types {
        if fixture_type.modes.is_empty() {
            warnings.push(format!("{}: no DMX modes", fixture_type.key()));
        }
    }
    Ok(ImportReport { types, warnings })
}

/// Parse `description.xml` bytes without first opening an archive.
pub fn parse_description_bytes(xml: &[u8]) -> Result<ImportReport, GdtfErrorKind> {
    let xml = std::str::from_utf8(xml)
        .map_err(|error| GdtfErrorKind::Xml(format!("description is not UTF-8: {error}")))?;
    parse_description(xml)
}

fn parse_fixture_type(
    node: Node,
    warnings: &mut Vec<String>,
) -> Result<FixtureType, GdtfErrorKind> {
    let manufacturer = attribute(node, "Manufacturer").unwrap_or_default();
    let name = attribute(node, "Name")
        .or_else(|| attribute(node, "LongName"))
        .unwrap_or_else(|| "Unnamed".to_string());
    let description = attribute(node, "Description")
        .or_else(|| attribute(node, "LongName"))
        .unwrap_or_default();

    let mut attributes = Vec::new();
    if let Some(collect) = child(node, "Attributes") {
        for attribute_node in collect.children().filter(|n| n.has_tag_name("Attribute")) {
            let name = attribute(attribute_node, "Name").unwrap_or_default();
            if name.is_empty() {
                continue;
            }
            attributes.push(AttributeDef {
                pretty: attribute(attribute_node, "Pretty").unwrap_or_else(|| name.clone()),
                feature: attribute(attribute_node, "Feature").unwrap_or_default(),
                unit: attribute(attribute_node, "PhysicalUnit").unwrap_or_default(),
                name,
            });
        }
    }

    let mut modes = Vec::new();
    if let Some(collect) = child(node, "DMXModes") {
        for mode_node in collect.children().filter(|n| n.has_tag_name("DMXMode")) {
            modes.push(parse_mode(mode_node, &manufacturer, &name, warnings)?);
        }
    }

    Ok(FixtureType {
        manufacturer,
        name,
        description,
        attributes,
        modes,
        home: Default::default(),
    })
}

fn parse_mode(
    node: Node,
    manufacturer: &str,
    fixture: &str,
    warnings: &mut Vec<String>,
) -> Result<Mode, GdtfErrorKind> {
    let name = attribute(node, "Name").unwrap_or_else(|| "Mode".to_string());
    let context = format!("{manufacturer} {fixture} / {name}");
    let mut channels = Vec::new();

    if let Some(collect) = child(node, "DMXChannels") {
        for channel_node in collect.children().filter(|n| n.has_tag_name("DMXChannel")) {
            let dmx_break: u16 = attribute(channel_node, "DMXBreak")
                .and_then(|value| value.parse().ok())
                .unwrap_or(1);
            if dmx_break != 1 {
                warnings.push(format!(
                    "{context}: channel on DMX break {dmx_break} skipped (only break 1 is output)"
                ));
                continue;
            }

            let offset = attribute(channel_node, "Offset")
                .map(|value| parse_address(&value))
                .unwrap_or(1);
            if offset == 0 {
                warnings.push(format!("{context}: channel with offset 0 skipped"));
                continue;
            }
            let default_raw = attribute(channel_node, "Default")
                .map(|value| parse_dmx_value(&value).1)
                .unwrap_or(0);

            let logical: Vec<Node> = channel_node
                .children()
                .filter(|n| n.has_tag_name("LogicalChannel"))
                .collect();
            let found_attribute = logical
                .first()
                .and_then(|node| attribute(*node, "Attribute"))
                .or_else(|| attribute(channel_node, "Attribute"));
            let Some(attribute_name) = found_attribute else {
                warnings.push(format!(
                    "{context}: channel at offset {offset} has no attribute, skipped"
                ));
                continue;
            };
            if logical.len() > 1 {
                let ignored: Vec<String> = logical
                    .iter()
                    .skip(1)
                    .filter_map(|node| attribute(*node, "Attribute"))
                    .collect();
                warnings.push(format!(
                    "{context}: mutually exclusive logical channels on offset {offset}; \
                     using `{attribute_name}` and ignoring {}",
                    ignored.join(", ")
                ));
            }

            let (functions, fine_delta) = logical
                .first()
                .map(|node| parse_functions(*node, &attribute_name, &context, warnings))
                .transpose()?
                .unwrap_or((Vec::new(), None));
            let fine_offset = match fine_delta {
                Some(delta) => match offset.checked_add(delta) {
                    Some(slot) => Some(slot),
                    None => {
                        warnings.push(format!(
                            "{context}: fine offset after channel {offset} overflows; imported as 8-bit"
                        ));
                        None
                    }
                },
                None => None,
            };

            channels.push(Channel {
                offset,
                fine_offset,
                attribute: attribute_name,
                default_raw,
                functions,
            });
        }
    }

    if channels.len() > MAX_MODE_CHANNELS {
        return Err(LimitError::ModeChannels {
            mode: name,
            found: channels.len(),
        }
        .into());
    }
    channels.sort_by_key(|channel| channel.offset);
    Ok(Mode { name, channels })
}

/// Parse a logical channel's functions.
///
/// Returns the functions and the fine channel's offset relative to the coarse
/// channel, when the profile gives one. `DMXFrom="1/0"` means the low byte is
/// one slot past this channel. A profile whose functions disagree about the
/// fine offset is imported as 8-bit with a warning, because there is no safe
/// way to guess which byte moves.
fn parse_functions(
    logical: Node,
    attribute_name: &str,
    context: &str,
    warnings: &mut Vec<String>,
) -> Result<(Vec<ChannelFunction>, Option<u16>), GdtfErrorKind> {
    struct Raw {
        name: String,
        from: (u8, u8),
        to: Option<(u8, u8)>,
        physical_from: f64,
        physical_to: f64,
    }

    let mut raw_functions = Vec::new();
    for node in logical
        .children()
        .filter(|n| n.has_tag_name("ChannelFunction"))
    {
        let name = attribute(node, "Name").unwrap_or_else(|| attribute_name.to_string());
        let from = attribute(node, "DMXFrom")
            .map(|value| parse_dmx_value(&value))
            .unwrap_or((0, 1));
        let to = attribute(node, "DMXTo").map(|value| parse_dmx_value(&value));
        let physical_from = attribute(node, "PhysicalFrom")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(0.0);
        let physical_to = attribute(node, "PhysicalTo")
            .and_then(|value| value.parse::<f64>().ok())
            .unwrap_or(1.0);
        raw_functions.push(Raw {
            name,
            from,
            to,
            physical_from,
            physical_to,
        });
    }

    if raw_functions.len() > MAX_CHANNEL_FUNCTIONS {
        return Err(LimitError::ChannelFunctions {
            attribute: attribute_name.to_string(),
            found: raw_functions.len(),
        }
        .into());
    }
    raw_functions.sort_by_key(|function| (function.from.0, function.from.1));

    let mut fine_delta = raw_functions.first().map_or(0, |raw| raw.from.0);
    let consistent = raw_functions.iter().all(|raw| {
        let to_byte = raw.to.map_or(raw.from.0, |(byte, _)| byte);
        raw.from.0 == fine_delta && to_byte == fine_delta
    });
    if !consistent {
        warnings.push(format!(
            "{context}: 16-bit channel `{attribute_name}` has inconsistent fine offsets; imported as 8-bit"
        ));
        fine_delta = 0;
    }

    let mut functions = Vec::new();
    for (index, raw) in raw_functions.iter().enumerate() {
        let dmx_from = raw.from.1;
        let dmx_to = match raw.to {
            Some((_, value)) => value,
            None => raw_functions
                .get(index + 1)
                .map(|next| next.from.1.saturating_sub(1))
                .unwrap_or(255),
        };
        if dmx_to < dmx_from {
            warnings.push(format!(
                "{context}: function `{}` has DMX range {dmx_from}..{dmx_to}, ignored",
                raw.name
            ));
            continue;
        }
        functions.push(ChannelFunction {
            name: raw.name.clone(),
            dmx_from,
            dmx_to,
            physical_from: raw.physical_from,
            physical_to: raw.physical_to,
        });
    }

    if functions.is_empty() {
        functions.push(ChannelFunction {
            name: attribute_name.to_string(),
            ..ChannelFunction::default()
        });
    }
    Ok((
        functions,
        (fine_delta != 0).then_some(u16::from(fine_delta)),
    ))
}

fn attribute(node: Node, name: &str) -> Option<String> {
    node.attribute(name)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children().find(|child| child.has_tag_name(name))
}

/// GDTF `DMXValue` is `byte/value`; plain numbers are a byte 0 value.
fn parse_dmx_value(raw: &str) -> (u8, u8) {
    match raw.split_once('/') {
        Some((byte, value)) => (
            byte.trim().parse().unwrap_or(0),
            value.trim().parse().unwrap_or(0),
        ),
        None => (0, raw.trim().parse().unwrap_or(0)),
    }
}

/// GDTF `DMXAddress` may be `universe/address`; channel offsets are relative.
fn parse_address(raw: &str) -> u16 {
    raw.rsplit('/')
        .next()
        .unwrap_or(raw)
        .trim()
        .parse()
        .unwrap_or(1)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    pub(crate) const DEMO_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<GDTF DataVersion="1.2">
  <FixtureType Name="Demo Wash" ShortName="Demo" Manufacturer="OpenLight"
               Description="Synthetic four channel wash for tests and demos.">
    <Attributes>
      <Attribute Name="Dimmer" Pretty="Dimmer" Feature="Dimmer" PhysicalUnit="Percent"/>
      <Attribute Name="ColorAdd_R" Pretty="Red" Feature="Color" PhysicalUnit="Percent"/>
      <Attribute Name="ColorAdd_G" Pretty="Green" Feature="Color" PhysicalUnit="Percent"/>
      <Attribute Name="ColorAdd_B" Pretty="Blue" Feature="Color" PhysicalUnit="Percent"/>
    </Attributes>
    <DMXModes>
      <DMXMode Name="4ch" Geometry="Body">
        <DMXChannels>
          <DMXChannel DMXBreak="1" Offset="1" Default="0/0" Geometry="Body">
            <LogicalChannel Attribute="Dimmer">
              <ChannelFunction Name="Dimmer" Attribute="Dimmer"
                               DMXFrom="0/0" DMXTo="0/255"
                               PhysicalFrom="0" PhysicalTo="100"/>
            </LogicalChannel>
          </DMXChannel>
          <DMXChannel DMXBreak="1" Offset="2" Default="0/0" Geometry="Body">
            <LogicalChannel Attribute="ColorAdd_R">
              <ChannelFunction Name="Red" Attribute="ColorAdd_R"
                               DMXFrom="0/0" DMXTo="0/255"
                               PhysicalFrom="0" PhysicalTo="100"/>
            </LogicalChannel>
          </DMXChannel>
          <DMXChannel DMXBreak="1" Offset="3" Default="0/0" Geometry="Body">
            <LogicalChannel Attribute="ColorAdd_G">
              <ChannelFunction Name="Green" Attribute="ColorAdd_G"
                               DMXFrom="0/0" DMXTo="0/255"
                               PhysicalFrom="0" PhysicalTo="100"/>
            </LogicalChannel>
          </DMXChannel>
          <DMXChannel DMXBreak="1" Offset="4" Default="0/0" Geometry="Body">
            <LogicalChannel Attribute="ColorAdd_B">
              <ChannelFunction Name="Blue" Attribute="ColorAdd_B"
                               DMXFrom="0/0" DMXTo="0/255"
                               PhysicalFrom="0" PhysicalTo="100"/>
            </LogicalChannel>
          </DMXChannel>
        </DMXChannels>
      </DMXMode>
    </DMXModes>
  </FixtureType>
</GDTF>
"#;

    #[test]
    fn parses_fixture_type_mode_and_channels() {
        let report = parse_description(DEMO_XML).unwrap();
        assert_eq!(report.types.len(), 1);
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);

        let fixture_type = &report.types[0];
        assert_eq!(fixture_type.key(), "OpenLight Demo Wash");
        assert_eq!(fixture_type.attributes.len(), 4);
        assert_eq!(fixture_type.modes.len(), 1);

        let mode = &fixture_type.modes[0];
        assert_eq!(mode.name, "4ch");
        assert_eq!(mode.footprint(), 4);
        assert_eq!(
            mode.channels
                .iter()
                .map(|channel| channel.attribute.as_str())
                .collect::<Vec<_>>(),
            vec!["Dimmer", "ColorAdd_R", "ColorAdd_G", "ColorAdd_B"]
        );
        let dimmer = mode.channel_for("Dimmer").unwrap();
        assert_eq!(dimmer.raw_from_level(1.0), 255);
        assert_eq!(dimmer.raw_from_level(0.0), 0);
        assert_eq!(fixture_type.pretty_attribute("ColorAdd_R"), "Red");
    }

    #[test]
    fn dmx_to_defaults_to_next_function_start_minus_one() {
        let xml = r#"<GDTF><FixtureType Name="Two" Manufacturer="OL">
          <DMXModes><DMXMode Name="M"><DMXChannels>
            <DMXChannel Offset="1"><LogicalChannel Attribute="Dimmer">
              <ChannelFunction Name="Off" DMXFrom="0/0" PhysicalFrom="0" PhysicalTo="1"/>
              <ChannelFunction Name="On" DMXFrom="0/128" PhysicalFrom="1" PhysicalTo="2"/>
            </LogicalChannel></DMXChannel>
          </DMXChannels></DMXMode></DMXModes>
        </FixtureType></GDTF>"#;
        let report = parse_description(xml).unwrap();
        let functions = &report.types[0].modes[0].channels[0].functions;
        assert_eq!(functions.len(), 2);
        assert_eq!((functions[0].dmx_from, functions[0].dmx_to), (0, 127));
        assert_eq!((functions[1].dmx_from, functions[1].dmx_to), (128, 255));
    }

    #[test]
    fn missing_dmx_to_on_last_function_is_255() {
        let xml = r#"<GDTF><FixtureType Name="One" Manufacturer="OL">
          <DMXModes><DMXMode Name="M"><DMXChannels>
            <DMXChannel Offset="1"><LogicalChannel Attribute="Dimmer">
              <ChannelFunction DMXFrom="0/0"/>
            </LogicalChannel></DMXChannel>
          </DMXChannels></DMXMode></DMXModes>
        </FixtureType></GDTF>"#;
        let report = parse_description(xml).unwrap();
        let function = &report.types[0].modes[0].channels[0].functions[0];
        assert_eq!((function.dmx_from, function.dmx_to), (0, 255));
    }

    #[test]
    fn channel_without_functions_still_becomes_linear() {
        let xml = r#"<GDTF><FixtureType Name="One" Manufacturer="OL">
          <DMXModes><DMXMode Name="M"><DMXChannels>
            <DMXChannel Offset="1"><LogicalChannel Attribute="Dimmer"/></DMXChannel>
          </DMXChannels></DMXMode></DMXModes>
        </FixtureType></GDTF>"#;
        let report = parse_description(xml).unwrap();
        let channel = &report.types[0].modes[0].channels[0];
        assert_eq!(channel.functions.len(), 1);
        assert_eq!(channel.raw_from_level(1.0), 255);
    }

    #[test]
    fn non_break_1_channels_are_skipped_with_a_warning() {
        let xml = r#"<GDTF><FixtureType Name="One" Manufacturer="OL">
          <DMXModes><DMXMode Name="M"><DMXChannels>
            <DMXChannel Offset="1" DMXBreak="2"><LogicalChannel Attribute="Dimmer"/></DMXChannel>
            <DMXChannel Offset="2"><LogicalChannel Attribute="ColorAdd_R"/></DMXChannel>
          </DMXChannels></DMXMode></DMXModes>
        </FixtureType></GDTF>"#;
        let report = parse_description(xml).unwrap();
        assert_eq!(report.types[0].modes[0].channels.len(), 1);
        assert!(report.warnings.iter().any(|w| w.contains("break 2")));
    }

    #[test]
    fn sixteen_bit_channels_pair_their_fine_slot() {
        let xml = r#"<GDTF><FixtureType Name="Wide" Manufacturer="OL">
          <DMXModes><DMXMode Name="M"><DMXChannels>
            <DMXChannel Offset="3"><LogicalChannel Attribute="Pan">
              <ChannelFunction DMXFrom="1/0" DMXTo="1/255"/>
            </LogicalChannel></DMXChannel>
          </DMXChannels></DMXMode></DMXModes>
        </FixtureType></GDTF>"#;
        let report = parse_description(xml).unwrap();
        assert!(report.warnings.is_empty(), "{:?}", report.warnings);
        let mode = &report.types[0].modes[0];
        let channel = &mode.channels[0];
        assert_eq!(channel.offset, 3);
        assert_eq!(channel.fine_offset, Some(4));
        assert_eq!(mode.footprint(), 4, "the fine slot is inside the footprint");
    }

    #[test]
    fn a_disagreeing_fine_offset_warns_and_stays_eight_bit() {
        let xml = r#"<GDTF><FixtureType Name="Wide" Manufacturer="OL">
          <DMXModes><DMXMode Name="M"><DMXChannels>
            <DMXChannel Offset="1"><LogicalChannel Attribute="Pan">
              <ChannelFunction DMXFrom="1/0" DMXTo="0/255"/>
            </LogicalChannel></DMXChannel>
          </DMXChannels></DMXMode></DMXModes>
        </FixtureType></GDTF>"#;
        let report = parse_description(xml).unwrap();
        assert!(
            report
                .warnings
                .iter()
                .any(|warning| warning.contains("inconsistent fine offsets")),
            "{:?}",
            report.warnings
        );
        assert_eq!(report.types[0].modes[0].channels[0].fine_offset, None);
    }

    #[test]
    fn parses_a_description_from_bytes_for_archive_less_imports() {
        let report = parse_description_bytes(DEMO_XML.as_bytes()).unwrap();
        assert_eq!(report.types[0].key(), "OpenLight Demo Wash");
        assert!(matches!(
            parse_description_bytes(b"<not xml"),
            Err(GdtfErrorKind::Xml(_))
        ));
    }

    #[test]
    fn missing_fixture_type_is_an_error() {
        assert!(matches!(
            parse_description("<GDTF></GDTF>"),
            Err(GdtfErrorKind::NoFixtureType)
        ));
        assert!(matches!(
            parse_description("<not xml"),
            Err(GdtfErrorKind::Xml(_))
        ));
    }

    #[test]
    fn imports_from_a_real_zip_archive() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("description.xml", options).unwrap();
        writer.write_all(DEMO_XML.as_bytes()).unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let path =
            std::env::temp_dir().join(format!("openlight-test-{}-demo.gdtf", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        let report = import(&path).unwrap();
        assert_eq!(report.types[0].key(), "OpenLight Demo Wash");
        let _ = std::fs::remove_file(&path);
    }

    fn archive_with(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    #[test]
    fn archives_over_the_entry_limit_are_refused() {
        let names: Vec<String> = (0..=limits::MAX_ARCHIVE_ENTRIES)
            .map(|index| format!("entry-{index}"))
            .collect();
        let entries: Vec<(&str, &[u8])> = names
            .iter()
            .map(|name| (name.as_str(), b"x".as_slice()))
            .collect();
        let error = import_bytes(&archive_with(&entries)).unwrap_err();
        assert!(
            matches!(
                error,
                GdtfErrorKind::Limit(LimitError::ArchiveEntries { .. })
            ),
            "{error}"
        );
    }

    #[test]
    fn archive_paths_cannot_escape() {
        let error = import_bytes(&archive_with(&[("../description.xml", b"<GDTF/>")])).unwrap_err();
        assert!(
            matches!(error, GdtfErrorKind::Limit(LimitError::UnsafePath { .. })),
            "{error}"
        );
    }

    #[test]
    fn a_mode_over_512_channels_is_refused() {
        let channels: String = (1..=limits::MAX_MODE_CHANNELS + 1)
            .map(|offset| {
                format!(
                    "<DMXChannel Offset=\"{offset}\"><LogicalChannel Attribute=\"A{offset}\"/></DMXChannel>"
                )
            })
            .collect();
        let xml = format!(
            "<GDTF><FixtureType Name=\"Big\" Manufacturer=\"OL\"><DMXModes><DMXMode Name=\"M\">\
             <DMXChannels>{channels}</DMXChannels></DMXMode></DMXModes></FixtureType></GDTF>"
        );
        let error = parse_description(&xml).unwrap_err();
        assert!(
            matches!(
                error,
                GdtfErrorKind::Limit(LimitError::ModeChannels { found: 513, .. })
            ),
            "{error}"
        );
    }

    #[test]
    fn a_channel_over_4096_functions_is_refused() {
        let functions: String = (0..=limits::MAX_CHANNEL_FUNCTIONS)
            .map(|index| format!("<ChannelFunction Name=\"F{index}\" DMXFrom=\"0/0\"/>"))
            .collect();
        let xml = format!(
            "<GDTF><FixtureType Name=\"Many\" Manufacturer=\"OL\"><DMXModes><DMXMode Name=\"M\">\
             <DMXChannels><DMXChannel Offset=\"1\"><LogicalChannel Attribute=\"Dimmer\">\
             {functions}</LogicalChannel></DMXChannel></DMXChannels></DMXMode></DMXModes></FixtureType></GDTF>"
        );
        let error = parse_description(&xml).unwrap_err();
        assert!(
            matches!(
                error,
                GdtfErrorKind::Limit(LimitError::ChannelFunctions { found: 4097, .. })
            ),
            "{error}"
        );
    }

    #[test]
    fn archive_without_description_is_an_error() {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("thumbnail.png", options).unwrap();
        writer.write_all(b"not an image").unwrap();
        let bytes = writer.finish().unwrap().into_inner();

        let path =
            std::env::temp_dir().join(format!("openlight-test-{}-empty.gdtf", std::process::id()));
        std::fs::write(&path, bytes).unwrap();
        assert!(matches!(
            import(&path),
            Err(GdtfError {
                kind: GdtfErrorKind::MissingDescription,
                ..
            })
        ));
        let _ = std::fs::remove_file(&path);
    }
}
