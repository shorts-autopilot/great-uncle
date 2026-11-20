//! The patch spreadsheet format.
//!
//! One column set, shared by `Patch Import` and `Report Patch`, so a round-trip
//! loses nothing. This is deliberately not a general CSV library: fields may be
//! quoted to hold a comma, and a doubled quote inside a quoted field is the only
//! escape. Anything else — an unclosed quote, a quote in the middle of a field,
//! a data row with the wrong width — is refused with the line number rather than
//! guessed at.

/// The columns of a patch CSV, in the order `Report Patch` writes them.
pub const PATCH_COLUMNS: [&str; 6] = ["number", "name", "type", "mode", "universe", "address"];

/// One fixture row from a patch spreadsheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchRow {
    pub number: u16,
    pub name: String,
    pub type_key: String,
    pub mode: String,
    pub universe: u16,
    pub address: u16,
}

/// A parsed patch plus the non-fatal things the operator should know.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedPatch {
    pub rows: Vec<PatchRow>,
    pub warnings: Vec<String>,
}

/// Parse a patch CSV. Every row is checked for shape and whole numbers here;
/// whether the type, mode, universe and address exist is the engine's job, so it
/// can report the same row numbers against the live show.
pub fn parse_patch(text: &str) -> Result<ParsedPatch, String> {
    let mut lines = text
        .lines()
        .enumerate()
        .map(|(index, line)| (index + 1, line.trim_end_matches('\r')))
        .filter(|(_, line)| !line.trim().is_empty());
    let Some((_, header_line)) = lines.next() else {
        return Err(format!(
            "patch CSV is empty; expected the header `{}`",
            PATCH_COLUMNS.join(",")
        ));
    };
    let header = parse_record(header_line).map_err(|error| format!("line 1: {error}"))?;

    let mut warnings = Vec::new();
    let mut columns: Vec<Option<usize>> = vec![None; PATCH_COLUMNS.len()];
    for (index, name) in header.iter().enumerate() {
        let name = name.trim().to_ascii_lowercase();
        let Some(position) = PATCH_COLUMNS.iter().position(|column| *column == name) else {
            warnings.push(format!("ignoring unknown column `{name}`"));
            continue;
        };
        if columns[position].replace(index).is_some() {
            return Err(format!("line 1: column `{name}` appears twice"));
        }
    }
    let missing: Vec<&str> = PATCH_COLUMNS
        .iter()
        .zip(&columns)
        .filter(|(_, index)| index.is_none())
        .map(|(name, _)| *name)
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "patch CSV is missing column{} `{}`; expected `{}`",
            if missing.len() == 1 { "" } else { "s" },
            missing.join(", "),
            PATCH_COLUMNS.join(",")
        ));
    }
    let column = |name: &str| {
        columns[PATCH_COLUMNS
            .iter()
            .position(|column| *column == name)
            .expect("required column was checked above")]
        .expect("required column was checked above")
    };

    let mut rows = Vec::new();
    for (line, record) in lines {
        let fields = parse_record(record).map_err(|error| format!("line {line}: {error}"))?;
        if fields.len() != header.len() {
            return Err(format!(
                "line {line}: expected {} columns, found {}",
                header.len(),
                fields.len()
            ));
        }
        let field = |name: &str| fields[column(name)].as_str();
        let number = whole(field("number"), line, "fixture number")?;
        let universe = whole(field("universe"), line, "universe")?;
        let address = whole(field("address"), line, "address")?;
        for (value, what) in [
            (number, "fixture number"),
            (universe, "universe"),
            (address, "address"),
        ] {
            if value == 0 {
                return Err(format!("line {line}: {what} 0 is not allowed"));
            }
        }
        let type_key = field("type").trim();
        let mode = field("mode").trim();
        if type_key.is_empty() {
            return Err(format!("line {line}: `type` is empty"));
        }
        if mode.is_empty() {
            return Err(format!("line {line}: `mode` is empty"));
        }
        rows.push(PatchRow {
            number,
            name: field("name").to_string(),
            type_key: type_key.to_string(),
            mode: mode.to_string(),
            universe,
            address,
        });
    }
    Ok(ParsedPatch { rows, warnings })
}

/// Write the patch columns, one row per fixture, in the given order.
pub fn write_patch(rows: &[PatchRow]) -> String {
    let records: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            vec![
                row.number.to_string(),
                row.name.clone(),
                row.type_key.clone(),
                row.mode.clone(),
                row.universe.to_string(),
                row.address.to_string(),
            ]
        })
        .collect();
    write_records(&PATCH_COLUMNS, &records)
}

/// Write any header and rows with the same quoting rules as the patch writer.
pub fn write_records(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut text = String::new();
    text.push_str(
        &header
            .iter()
            .map(|field| escape(field))
            .collect::<Vec<_>>()
            .join(","),
    );
    text.push('\n');
    for row in rows {
        text.push_str(
            &row.iter()
                .map(|field| escape(field))
                .collect::<Vec<_>>()
                .join(","),
        );
        text.push('\n');
    }
    text
}

fn whole(value: &str, line: usize, what: &str) -> Result<u16, String> {
    let value = value.trim();
    let number: u32 = value
        .parse()
        .map_err(|_| format!("line {line}: `{value}` is not a whole {what}"))?;
    if number > u32::from(u16::MAX) {
        return Err(format!("line {line}: {what} {number} is out of range"));
    }
    Ok(number as u16)
}

fn escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// Split one record into fields, honouring quoted fields and doubled quotes.
fn parse_record(record: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = record.chars().collect();
    let mut index = 0;
    let mut fields = Vec::new();
    loop {
        while index < chars.len() && chars[index].is_whitespace() {
            index += 1;
        }
        if index < chars.len() && chars[index] == '"' {
            index += 1;
            let mut field = String::new();
            loop {
                if index >= chars.len() {
                    return Err("a quoted field is not closed".into());
                }
                match chars[index] {
                    '"' if chars.get(index + 1) == Some(&'"') => {
                        field.push('"');
                        index += 2;
                    }
                    '"' => {
                        index += 1;
                        break;
                    }
                    character => {
                        field.push(character);
                        index += 1;
                    }
                }
            }
            while index < chars.len() && chars[index].is_whitespace() {
                index += 1;
            }
            fields.push(field);
            if index == chars.len() {
                break;
            }
            if chars[index] != ',' {
                return Err(format!(
                    "unexpected `{}` after a quoted field",
                    chars[index]
                ));
            }
            index += 1;
            if index == chars.len() {
                fields.push(String::new());
                break;
            }
        } else {
            let start = index;
            while index < chars.len() && chars[index] != ',' {
                if chars[index] == '"' {
                    return Err("a quote must start a field".into());
                }
                index += 1;
            }
            fields.push(
                chars[start..index]
                    .iter()
                    .collect::<String>()
                    .trim()
                    .to_string(),
            );
            if index == chars.len() {
                break;
            }
            index += 1;
            if index == chars.len() {
                fields.push(String::new());
                break;
            }
        }
    }
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(number: u16, name: &str) -> PatchRow {
        PatchRow {
            number,
            name: name.into(),
            type_key: "OpenLight Demo Wash".into(),
            mode: "4ch".into(),
            universe: 1,
            address: number * 4 + 1,
        }
    }

    #[test]
    fn a_patch_round_trips_through_the_writer() {
        let rows = vec![row(1, "Demo Wash"), row(2, "Wash, warm")];
        let text = write_patch(&rows);
        assert!(text.starts_with("number,name,type,mode,universe,address\n"));
        let parsed = parse_patch(&text).unwrap();
        assert_eq!(parsed.rows, rows);
        assert!(parsed.warnings.is_empty());
    }

    #[test]
    fn a_quoted_comma_is_the_one_field_feature() {
        let parsed =
            parse_patch("number,name,type,mode,universe,address\n1,\"Wash, warm\",Type,4ch,1,1\n")
                .unwrap();
        assert_eq!(parsed.rows[0].name, "Wash, warm");
    }

    #[test]
    fn an_unknown_column_is_ignored_with_a_warning() {
        let parsed =
            parse_patch("number,name,type,mode,universe,address,note\n1,A,T,4ch,1,1,hello\n")
                .unwrap();
        assert_eq!(parsed.rows.len(), 1);
        assert_eq!(parsed.warnings, vec!["ignoring unknown column `note`"]);
    }

    #[test]
    fn a_missing_header_names_the_expected_columns() {
        let error = parse_patch("number,name\n1,A\n").unwrap_err();
        assert!(error.contains("missing columns"), "{error}");
        assert!(error.contains("type"), "{error}");
        assert!(error.contains("universe"), "{error}");
    }

    #[test]
    fn a_bad_value_reports_its_line_number() {
        let error =
            parse_patch("number,name,type,mode,universe,address\n1,A,T,4ch,1,not-a-number\n")
                .unwrap_err();
        assert!(error.starts_with("line 2:"), "{error}");
        assert!(error.contains("address"), "{error}");
    }

    #[test]
    fn malformed_quotes_and_wide_rows_are_refused() {
        assert!(
            parse_patch("number,name,type,mode,universe,address\n1,\"A,T,4ch,1,1\n")
                .unwrap_err()
                .contains("not closed")
        );
        assert!(
            parse_patch("number,name,type,mode,universe,address\n1,A,T,4ch,1,1,extra\n")
                .unwrap_err()
                .contains("expected 6 columns")
        );
        assert!(
            parse_patch("number,name,type,mode,universe,address\n1,A\"B,T,4ch,1,1\n")
                .unwrap_err()
                .contains("quote must start")
        );
    }

    #[test]
    fn zero_numbers_are_refused_before_the_patch_is_touched() {
        let error =
            parse_patch("number,name,type,mode,universe,address\n1,A,T,4ch,0,1\n").unwrap_err();
        assert!(error.contains("universe 0"), "{error}");
    }

    #[test]
    fn a_header_with_no_rows_is_a_valid_empty_patch() {
        let parsed = parse_patch("Number,Name,Type,Mode,Universe,Address\n").unwrap();
        assert!(parsed.rows.is_empty());
    }
}
