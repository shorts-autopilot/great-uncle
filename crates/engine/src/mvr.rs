//! MVR import: a zip archive carrying a scene description, GDTF profiles and
//! patch addresses.
//!
//! Only the patch and profiles are used. Geometry, layers as groups, scene
//! objects and positions are deliberately ignored; a position warning is
//! counted once because the show has no 3D model to read it.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use roxmltree::{Document, Node};

use crate::fixture_type::FixtureType;
use crate::gdtf;
use crate::patch::PatchRange;
use crate::show::{Fixture, Show, Universe, MAX_UNIVERSE_NUMBER, MAX_UNIVERSE_SIZE};

const MVR_SCENE: &str = "GeneralSceneDescription.xml";

#[derive(Debug, thiserror::Error)]
#[error("{}: {kind}", path.display())]
pub struct MvrError {
    pub path: PathBuf,
    pub kind: MvrErrorKind,
}

#[derive(Debug, thiserror::Error)]
pub enum MvrErrorKind {
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("not a readable MVR archive: {0}")]
    Archive(String),
    #[error("archive has no {MVR_SCENE}")]
    MissingScene,
    #[error("{MVR_SCENE} is not valid XML: {0}")]
    Xml(String),
}

#[derive(Debug, Clone, Default)]
pub struct ImportReport {
    pub fixtures: usize,
    pub types: usize,
    pub archives: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone)]
struct Entry {
    name: String,
    type_key: String,
    mode: String,
    universe: u16,
    address: u16,
}

#[derive(Debug)]
pub struct Import {
    types: Vec<FixtureType>,
    entries: Vec<Entry>,
    archives: usize,
    warnings: Vec<String>,
}

/// Read the archive and resolve its GDTF profiles. This does not touch a show;
/// [`apply`] merges the result so the caller can make the whole import one
/// mutation.
pub fn read(path: &Path) -> Result<Import, MvrError> {
    let file = File::open(path).map_err(|source| MvrError {
        path: path.to_path_buf(),
        kind: MvrErrorKind::Io(source),
    })?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| MvrError {
        path: path.to_path_buf(),
        kind: MvrErrorKind::Archive(error.to_string()),
    })?;

    let mut scene = None;
    let mut profiles = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).map_err(|error| MvrError {
            path: path.to_path_buf(),
            kind: MvrErrorKind::Archive(error.to_string()),
        })?;
        let name = normalize(entry.name());
        if name == normalize(MVR_SCENE) {
            let mut text = String::new();
            entry.read_to_string(&mut text).map_err(|source| MvrError {
                path: path.to_path_buf(),
                kind: MvrErrorKind::Io(source),
            })?;
            scene = Some(text);
        } else if name.ends_with(".gdtf") {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).map_err(|source| MvrError {
                path: path.to_path_buf(),
                kind: MvrErrorKind::Io(source),
            })?;
            profiles.insert(name, bytes);
        }
    }

    let scene = scene.ok_or_else(|| MvrError {
        path: path.to_path_buf(),
        kind: MvrErrorKind::MissingScene,
    })?;
    let document = Document::parse(&scene).map_err(|error| MvrError {
        path: path.to_path_buf(),
        kind: MvrErrorKind::Xml(error.to_string()),
    })?;

    let archives = profiles.len();
    let (types, entries, warnings) = parse_scene(&document, &profiles);
    Ok(Import {
        types,
        entries,
        archives,
        warnings,
    })
}

impl Show {
    /// Import an MVR archive in one operation. The caller should still wrap
    /// this in [`crate::engine::Engine::mutate`] when it owns a desk, so the
    /// whole show is validated and recorded as one undo step.
    pub fn import_mvr(&mut self, path: &Path) -> Result<ImportReport, MvrError> {
        let import = read(path)?;
        Ok(apply(self, import))
    }
}

/// Merge a read archive into a show. A fixture whose name already exists is
/// updated in place; the rest are appended at the next free numbers in document
/// order. Every accepted fixture is checked before it is inserted, so one bad
/// address does not discard the rest of the rig.
pub fn apply(show: &mut Show, import: Import) -> ImportReport {
    let mut warnings = import.warnings;
    let type_count = import.types.len();
    for fixture_type in import.types {
        let key = fixture_type.key();
        match show
            .fixture_types
            .iter()
            .position(|existing| existing.key().eq_ignore_ascii_case(&key))
        {
            Some(index) => show.fixture_types[index] = fixture_type,
            None => show.fixture_types.push(fixture_type),
        }
    }

    let mut numbers: BTreeSet<u16> = show.fixtures.iter().map(|fixture| fixture.number).collect();
    let mut names: BTreeSet<String> = show
        .fixtures
        .iter()
        .map(|fixture| fixture.name.trim().to_ascii_lowercase())
        .collect();
    let mut ranges: Vec<(u16, PatchRange)> = show
        .fixtures
        .iter()
        .filter_map(|fixture| {
            let (_, mode) = show.mode_for(fixture).ok()?;
            let range = PatchRange::new(fixture.universe, fixture.address, mode.footprint())?;
            Some((fixture.number, range))
        })
        .collect();
    let mut next = 1u16;
    let mut imported = 0usize;
    let mut imported_names = BTreeSet::new();

    for entry in import.entries {
        let Some(fixture_type) = show.find_type(&entry.type_key).cloned() else {
            warnings.push(format!(
                "fixture `{}`: type `{}` was not imported",
                entry.name, entry.type_key
            ));
            continue;
        };
        let Some(mode) = fixture_type.mode(&entry.mode).cloned() else {
            warnings.push(format!(
                "fixture `{}`: mode `{}` is not in `{}`",
                entry.name,
                entry.mode,
                fixture_type.key()
            ));
            continue;
        };
        if entry.universe == 0 || entry.universe > MAX_UNIVERSE_NUMBER {
            warnings.push(format!(
                "fixture `{}` skipped: universe {} is outside 1..={MAX_UNIVERSE_NUMBER}",
                entry.name, entry.universe
            ));
            continue;
        }
        let Some(range) = PatchRange::new(entry.universe, entry.address, mode.footprint()) else {
            warnings.push(format!(
                "fixture `{}` skipped: invalid address {}.{:03}",
                entry.name, entry.universe, entry.address
            ));
            continue;
        };
        if !show
            .universes
            .iter()
            .any(|universe| universe.number == entry.universe)
        {
            show.universes.push(Universe {
                number: entry.universe,
                label: format!("Universe {}", entry.universe),
                size: MAX_UNIVERSE_SIZE,
            });
            show.universes.sort_by_key(|universe| universe.number);
        }
        let Some(universe) = show.universe(entry.universe) else {
            continue;
        };
        if !range.fits(universe.size) {
            warnings.push(format!(
                "fixture `{}` skipped: {} channels at {}.{:03} overrun universe {} ({} slots)",
                entry.name,
                range.footprint(),
                entry.universe,
                entry.address,
                entry.universe,
                universe.size
            ));
            continue;
        }
        let base = if entry.name.trim().is_empty() {
            fixture_type.name.clone()
        } else {
            entry.name.trim().to_string()
        };
        let entry_name = base.to_ascii_lowercase();
        let replacement = (!entry_name.is_empty() && !imported_names.contains(&entry_name))
            .then(|| {
                show.fixtures
                    .iter()
                    .position(|fixture| fixture.name.eq_ignore_ascii_case(&base))
            })
            .flatten();
        let replacement_number = replacement.map(|index| show.fixtures[index].number);
        if let Some((other, _)) = ranges.iter().find(|(number, other)| {
            Some(*number) != replacement_number
                && other.universe == range.universe
                && other.overlaps(&range)
        }) {
            warnings.push(format!(
                "fixture `{}` skipped: address {}.{:03} overlaps fixture {}",
                entry.name, entry.universe, entry.address, other
            ));
            continue;
        }

        if let Some(index) = replacement {
            let number = show.fixtures[index].number;
            ranges.retain(|(range_number, _)| *range_number != number);
            if let Some(old_name) = show.fixtures.get(index).map(|fixture| fixture.name.clone()) {
                names.remove(&old_name.trim().to_ascii_lowercase());
            }
            names.insert(entry_name.clone());
            show.fixtures[index].name = base;
            show.fixtures[index].type_key = fixture_type.key();
            show.fixtures[index].mode = mode.name;
            show.fixtures[index].universe = entry.universe;
            show.fixtures[index].address = entry.address;
            ranges.push((number, range));
            imported_names.insert(entry_name);
            imported += 1;
            continue;
        }

        while numbers.contains(&next) {
            next = next.saturating_add(1);
        }
        let number = next;
        numbers.insert(number);
        next = next.saturating_add(1);
        let name = unique_name(&base, &mut names);
        show.fixtures.push(Fixture {
            number,
            name,
            type_key: fixture_type.key(),
            mode: mode.name,
            universe: entry.universe,
            address: entry.address,
            home: Default::default(),
        });
        ranges.push((number, range));
        imported_names.insert(entry_name);
        imported += 1;
    }

    show.fixtures.sort_by_key(|fixture| fixture.number);
    ImportReport {
        fixtures: imported,
        types: type_count,
        archives: import.archives,
        warnings,
    }
}

fn parse_scene(
    document: &Document,
    profiles: &BTreeMap<String, Vec<u8>>,
) -> (Vec<FixtureType>, Vec<Entry>, Vec<String>) {
    let root = document.root_element();
    let mut warnings = Vec::new();
    let mut fixtures = Vec::new();
    if let Some(layers) = root.descendants().find(|node| is(*node, "Layers")) {
        collect_fixtures(layers, &mut fixtures);
    }

    let mut types: BTreeMap<String, FixtureType> = BTreeMap::new();
    let mut entries = Vec::new();
    let mut positions = 0usize;
    for fixture in fixtures {
        let name = text(fixture, "Name").unwrap_or_default();
        let spec = text(fixture, "GDTFSpec").unwrap_or_default();
        let requested_mode = text(fixture, "GDTFMode").unwrap_or_default();
        if text(fixture, "Matrix").is_some() {
            positions += 1;
        }
        let Some(address) = text(fixture, "Address").and_then(|raw| parse_address(&raw)) else {
            warnings.push(format!(
                "fixture `{}` skipped: address is missing or invalid",
                fallback_name(&name)
            ));
            continue;
        };
        if spec.trim().is_empty() {
            warnings.push(format!(
                "fixture `{}` skipped: no GDTFSpec",
                fallback_name(&name)
            ));
            continue;
        }
        let Some(bytes) = profile_bytes(profiles, &spec) else {
            warnings.push(format!(
                "fixture `{}` skipped: GDTF `{}` is not in the archive",
                fallback_name(&name),
                spec
            ));
            continue;
        };
        let profile = match gdtf::import_bytes(bytes) {
            Ok(report) => report.types.into_iter().next(),
            Err(error) => {
                warnings.push(format!(
                    "fixture `{}` skipped: GDTF `{}` is unreadable: {error}",
                    fallback_name(&name),
                    spec
                ));
                None
            }
        };
        let Some(profile) = profile else {
            warnings.push(format!(
                "fixture `{}` skipped: GDTF `{}` has no FixtureType",
                fallback_name(&name),
                spec
            ));
            continue;
        };
        let type_key = profile.key();
        types
            .entry(type_key.to_ascii_lowercase())
            .or_insert_with(|| profile.clone());
        let mode = if requested_mode.trim().is_empty() {
            profile.default_mode().map(|mode| mode.name.clone())
        } else {
            profile.mode(&requested_mode).map(|mode| mode.name.clone())
        };
        let Some(mode) = mode else {
            warnings.push(format!(
                "fixture `{}` skipped: `{}` has no mode `{}`",
                fallback_name(&name),
                type_key,
                requested_mode
            ));
            continue;
        };
        entries.push(Entry {
            name,
            type_key,
            mode,
            universe: address.0,
            address: address.1,
        });
    }
    if positions > 0 {
        warnings.push(format!(
            "{positions} position{} ignored: no 3D model",
            if positions == 1 { "" } else { "s" }
        ));
    }
    (types.into_values().collect(), entries, warnings)
}

fn collect_fixtures<'a, 'input>(node: Node<'a, 'input>, out: &mut Vec<Node<'a, 'input>>) {
    for child in node.children().filter(|child| child.is_element()) {
        if is(child, "Fixture") {
            out.push(child);
        } else if is(child, "Layer") || is(child, "ChildList") {
            collect_fixtures(child, out);
        }
    }
}

fn is(node: Node, name: &str) -> bool {
    node.is_element() && node.tag_name().name() == name
}

fn text(node: Node, name: &str) -> Option<String> {
    let value = node.attribute(name).map(str::to_string).or_else(|| {
        node.children()
            .find(|child| is(*child, name))
            .and_then(|child| child.text())
            .map(str::to_string)
    })?;
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn fallback_name(name: &str) -> String {
    if name.trim().is_empty() {
        "unnamed".into()
    } else {
        name.trim().to_string()
    }
}

fn parse_address(raw: &str) -> Option<(u16, u16)> {
    let (universe, address) = raw.trim().split_once('.')?;
    let universe = universe.trim().parse::<u16>().ok()?;
    let address = address.trim().parse::<u16>().ok()?;
    (universe > 0 && universe <= MAX_UNIVERSE_NUMBER && address > 0).then_some((universe, address))
}

fn profile_bytes<'a>(profiles: &'a BTreeMap<String, Vec<u8>>, spec: &str) -> Option<&'a Vec<u8>> {
    let spec = normalize(spec);
    profiles.get(&spec).or_else(|| {
        let base = spec.rsplit('/').next().unwrap_or(&spec);
        profiles.iter().find_map(|(name, bytes)| {
            let candidate = name.rsplit('/').next().unwrap_or(name);
            (candidate == base).then_some(bytes)
        })
    })
}

fn normalize(name: &str) -> String {
    name.replace('\\', "/")
        .trim_start_matches("./")
        .to_ascii_lowercase()
}

fn unique_name(base: &str, names: &mut BTreeSet<String>) -> String {
    let base = if base.trim().is_empty() {
        "Fixture"
    } else {
        base.trim()
    };
    if names.insert(base.to_ascii_lowercase()) {
        return base.to_string();
    }
    for suffix in 2u16.. {
        let candidate = format!("{base} {suffix}");
        if names.insert(candidate.to_ascii_lowercase()) {
            return candidate;
        }
    }
    unreachable!()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    const GDTF: &str = r#"<GDTF><FixtureType Name="MVR Wash" Manufacturer="OpenLight">
      <Attributes>
        <Attribute Name="Dimmer" Pretty="Dimmer" Feature="Dimmer"/>
        <Attribute Name="ColorAdd_R" Pretty="Red" Feature="Color"/>
      </Attributes>
      <DMXModes><DMXMode Name="4ch"><DMXChannels>
        <DMXChannel Offset="1"><LogicalChannel Attribute="Dimmer">
          <ChannelFunction Name="Dimmer" DMXFrom="0/0" DMXTo="0/255"/>
        </LogicalChannel></DMXChannel>
        <DMXChannel Offset="2"><LogicalChannel Attribute="ColorAdd_R">
          <ChannelFunction Name="Red" DMXFrom="0/0" DMXTo="0/255"/>
        </LogicalChannel></DMXChannel>
      </DMXChannels></DMXMode></DMXModes>
    </FixtureType></GDTF>"#;

    fn gdtf_zip() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("description.xml", options).unwrap();
        writer.write_all(GDTF.as_bytes()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn mvr_zip(scene: &str, include_profile: bool) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file(MVR_SCENE, options).unwrap();
        writer.write_all(scene.as_bytes()).unwrap();
        if include_profile {
            writer
                .start_file("GDTF/OpenLight/MVR Wash.gdtf", options)
                .unwrap();
            writer.write_all(&gdtf_zip()).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    fn profile_only_zip() -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        writer
            .start_file("GDTF/OpenLight/MVR Wash.gdtf", options)
            .unwrap();
        writer.write_all(&gdtf_zip()).unwrap();
        writer.finish().unwrap().into_inner()
    }

    fn fixture_xml(name: &str, address: &str, spec: &str) -> String {
        format!(
            r#"<Fixture Name="{name}" Address="{address}" GDTFSpec="{spec}" GDTFMode="4ch">
                 <Matrix>0,0,0</Matrix>
               </Fixture>"#
        )
    }

    fn scene(fixtures: &str) -> String {
        format!(
            r#"<GeneralSceneDescription><Scene><Layers><Layer><ChildList>
               {fixtures}
             </ChildList></Layer></Layers></Scene></GeneralSceneDescription>"#
        )
    }

    fn write_mvr(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "openlight-mvr-{}-{name}-{:?}.mvr",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn a_scene_imports_types_patch_and_one_position_warning() {
        let fixtures = format!(
            "{}{}",
            fixture_xml("Wash 1", "1.001", "GDTF/OpenLight/MVR Wash.gdtf"),
            fixture_xml("Wash 2", "1.010", "MVR Wash.gdtf")
        );
        let path = write_mvr("well-formed", &mvr_zip(&scene(&fixtures), true));
        let mut show = Show::new();
        let report = show.import_mvr(&path).unwrap();
        assert_eq!(report.fixtures, 2);
        assert_eq!(report.types, 1);
        assert_eq!(report.archives, 1);
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("positions ignored")));
        assert_eq!(show.fixture_types.len(), 1);
        assert_eq!(show.fixtures.len(), 2);
        assert_eq!(show.fixtures[0].number, 1);
        assert_eq!(show.fixtures[1].number, 2);
        assert_eq!(show.fixtures[0].universe, 1);
        assert_eq!(show.fixtures[0].address, 1);
        assert_eq!(show.fixtures[1].address, 10);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_missing_profile_skips_one_fixture_and_keeps_the_rest() {
        let fixtures = format!(
            "{}{}",
            fixture_xml("Known", "1.001", "MVR Wash.gdtf"),
            fixture_xml("Unknown", "1.010", "Missing.gdtf")
        );
        let path = write_mvr("missing", &mvr_zip(&scene(&fixtures), true));
        let mut show = Show::new();
        let report = show.import_mvr(&path).unwrap();
        assert_eq!(report.fixtures, 1);
        assert_eq!(show.fixtures.len(), 1);
        assert_eq!(show.fixtures[0].name, "Known");
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("Unknown") && warning.contains("Missing")));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_out_of_range_address_skips_the_named_fixture() {
        let fixtures = format!(
            "{}{}",
            fixture_xml("Bad", "1.999", "MVR Wash.gdtf"),
            fixture_xml("Good", "1.001", "MVR Wash.gdtf")
        );
        let path = write_mvr("address", &mvr_zip(&scene(&fixtures), true));
        let mut show = Show::new();
        let report = show.import_mvr(&path).unwrap();
        assert_eq!(report.fixtures, 1);
        assert_eq!(show.fixtures[0].name, "Good");
        assert!(report
            .warnings
            .iter()
            .any(|warning| warning.contains("Bad") && warning.contains("999")));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_scene_is_accepted_with_a_namespace_and_no_scene_wrapper() {
        let xml = format!(
            r#"<GeneralSceneDescription xmlns="http://www.gdtf.eu/mvr">
                 <Layers><Layer><ChildList>{}</ChildList></Layer></Layers>
               </GeneralSceneDescription>"#,
            fixture_xml("Wash", "1.001", "MVR Wash.gdtf")
        );
        let path = write_mvr("namespace", &mvr_zip(&xml, true));
        let mut show = Show::new();
        let report = show.import_mvr(&path).unwrap();
        assert_eq!(report.fixtures, 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_broken_archive_is_an_error_that_names_the_expected_entry() {
        let path = write_mvr("broken", b"this is not a zip");
        let error = read(&path).unwrap_err().to_string();
        assert!(error.contains("MVR"), "{error}");
        let _ = std::fs::remove_file(path);

        let path = write_mvr("no-scene", &profile_only_zip());
        let error = read(&path).unwrap_err().to_string();
        assert!(error.contains(MVR_SCENE), "{error}");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn importing_twice_replaces_named_fixtures_and_reuses_the_type() {
        let fixtures = fixture_xml("Wash", "1.001", "MVR Wash.gdtf");
        let path = write_mvr("twice", &mvr_zip(&scene(&fixtures), true));
        let mut show = Show::new();
        assert_eq!(show.import_mvr(&path).unwrap().fixtures, 1);
        assert_eq!(show.import_mvr(&path).unwrap().fixtures, 1);
        assert_eq!(show.fixture_types.len(), 1);
        assert_eq!(show.fixtures.len(), 1);
        assert_eq!(show.fixtures[0].number, 1);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_engine_command_dispatches_mvr() {
        let fixtures = fixture_xml("Wash", "1.001", "MVR Wash.gdtf");
        let path = write_mvr("command", &mvr_zip(&scene(&fixtures), true));
        let mut engine = crate::engine::Engine::empty();
        let outcome = engine
            .execute(&format!("Import Mvr \"{}\"", path.display()))
            .unwrap();
        assert!(outcome.lines[0].contains("1 fixture,"), "{outcome:?}");
        assert_eq!(engine.show.fixtures.len(), 1);
        let _ = std::fs::remove_file(path);
    }
}
