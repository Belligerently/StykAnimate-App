//! Reading containers: every check happens before the value it guards is used.

use std::collections::HashMap;
use std::sync::Arc;

use styk_doc::json::{self, StringHook};
use styk_doc::{Blob, JsonError, Nums, Value};

use super::{Codec, ContainerError, HEADER, Kind, MAX_BYTES, MAX_JSON_NUMBER_BYTES, MAX_MATERIALIZED_BYTES, MAX_SKELETON_BYTES, SECTION, sha256, unpack};

/// A decoded file.
pub struct Decoded {
    /// The document, with every marker replaced by the value it stands for.
    pub document: Value,
    /// `0` for a plain JSON project, otherwise the container version.
    pub version: u8,
    /// Length of the JSON the file stores (the skeleton, for version 2).
    pub skeleton_bytes: usize,
}

/// The uncompressed, verified pieces of a container.
pub struct Parts {
    pub version: u8,
    /// The JSON text: the whole document, or the version 2 skeleton.
    pub skeleton: Vec<u8>,
    /// Version 2 sections as `(kind, bytes)`: kind 1 numeric arrays, kind 2 payloads.
    pub sections: Vec<(u8, Vec<u8>)>,
}

/// Reads a `.styk`, `.stkz` or `.stykmov` file, or a plain JSON project.
pub fn decode(bytes: &[u8], kind: Kind) -> Result<Decoded, ContainerError> {
    let parts = decode_parts(bytes, kind)?;
    let skeleton_bytes = parts.skeleton.len();
    if parts.version < 2 {
        return Ok(Decoded { document: parse_text(&parts.skeleton, None)?, version: parts.version, skeleton_bytes });
    }
    let mut markers = Markers::new(&parts.sections, parts.skeleton.len());
    let parsed = parse_text(&parts.skeleton, Some(&mut markers));
    // A refused marker surfaces as a parse failure; the container error is the cause.
    if let Some(error) = markers.error.take() {
        return Err(error);
    }
    let document = parsed?;
    // The running bound counts 32 bytes per stored number. Only a document near
    // the limit pays for the exact size.
    if markers.materialized_upper > MAX_MATERIALIZED_BYTES && json_length(&document) > MAX_MATERIALIZED_BYTES {
        return Err(ContainerError::MaterializedTooLarge);
    }
    Ok(Decoded { document, version: 2, skeleton_bytes })
}

/// Verifies a container and returns its uncompressed pieces without building a document.
pub fn decode_parts(bytes: &[u8], kind: Kind) -> Result<Parts, ContainerError> {
    if bytes.len() > MAX_BYTES {
        return Err(ContainerError::FileTooLarge { size: bytes.len() });
    }
    if kind == Kind::Styk {
        // Projects saved before the container existed are plain JSON, possibly after a
        // byte-order mark or whitespace.
        let lead = bytes.iter().position(|b| !matches!(b, 0xef | 0xbb | 0xbf | b' ' | b'\t' | b'\r' | b'\n'));
        if lead.is_some_and(|at| bytes[at] == b'{') {
            return Ok(Parts { version: 0, skeleton: bytes.to_vec(), sections: Vec::new() });
        }
    }
    if bytes.len() < HEADER {
        return Err(ContainerError::Truncated("no room for the 56-byte header"));
    }
    if &bytes[..4] != kind.magic() {
        return Err(ContainerError::WrongMagic {
            found: [bytes[0], bytes[1], bytes[2], bytes[3]],
            expected: std::str::from_utf8(kind.magic()).unwrap(),
            kind: kind.name(),
        });
    }
    let version = bytes[4];
    let codec = Codec::from_byte(bytes[5]);
    let flags = u16::from_le_bytes([bytes[6], bytes[7]]);
    let (Some(codec), true, 0) = (codec, version == 1 || version == 2, flags) else {
        return Err(ContainerError::Unsupported("unknown version, compression or flags in the header"));
    };
    if version == 1 && codec != Codec::Zlib {
        return Err(ContainerError::Unsupported("a version 1 file must use zlib"));
    }
    let raw = u64_at(bytes, 8);
    let stored = u64_at(bytes, 16);
    let raw_limit = if version == 2 { MAX_SKELETON_BYTES } else { MAX_BYTES } as u64;
    if raw == 0 || raw > raw_limit || stored > MAX_BYTES as u64 {
        return Err(ContainerError::DeclaredTooLarge { what: "document", size: raw });
    }
    let (raw, stored) = (raw as usize, stored as usize);
    let digest = &bytes[24..56];

    if version == 1 {
        if HEADER + stored != bytes.len() {
            return Err(ContainerError::LengthMismatch("a payload length that does not match the file"));
        }
        let payload = unpack(codec, &bytes[HEADER..], raw, &|| "document".to_string())?;
        if sha256(&payload) != digest {
            return Err(ContainerError::Checksum { what: "document".to_string() });
        }
        return Ok(Parts { version: 1, skeleton: payload, sections: Vec::new() });
    }

    if bytes.len() < HEADER + 4 {
        return Err(ContainerError::Truncated("no room for the section count"));
    }
    let count = u32::from_le_bytes(bytes[HEADER..HEADER + 4].try_into().unwrap()) as usize;
    let skeleton_end = HEADER + 4 + stored;
    if count > 64 || skeleton_end + count * SECTION > bytes.len() {
        return Err(ContainerError::Truncated("no room for the skeleton and the section table"));
    }
    struct Record {
        kind: u8,
        codec: Codec,
        raw: usize,
        stored: usize,
        offset: usize,
        digest: [u8; 32],
    }
    let mut records = Vec::with_capacity(count);
    let mut position = skeleton_end + count * SECTION;
    // Upper bound of the JSON the file stands for: base64 grows payloads by a third,
    // and the app has always charged three bytes per stored byte.
    let mut expected = raw as u64;
    for index in 0..count {
        let at = skeleton_end + index * SECTION;
        let record = &bytes[at..at + SECTION];
        let section_kind = record[0];
        if section_kind != 1 && section_kind != 2 {
            return Err(ContainerError::UnknownSectionKind { index, kind: section_kind });
        }
        let reserved = record[2..8].iter().chain(&record[56..64]).any(|&b| b != 0);
        let (Some(section_codec), false) = (Codec::from_byte(record[1]), reserved) else {
            return Err(ContainerError::Unsupported("unknown compression or reserved bytes in a section record"));
        };
        let section_raw = u64_at(record, 8);
        let section_stored = u64_at(record, 16);
        if section_raw > MAX_BYTES as u64 || section_stored > (bytes.len() - position) as u64 || (section_codec == Codec::Stored && section_raw != section_stored) {
            return Err(ContainerError::SectionBounds { index, what: "its lengths exceed the file or the section limit" });
        }
        expected += section_raw * 3;
        records.push(Record {
            kind: section_kind,
            codec: section_codec,
            raw: section_raw as usize,
            stored: section_stored as usize,
            offset: position,
            digest: record[24..56].try_into().unwrap(),
        });
        position += section_stored as usize;
    }
    if position != bytes.len() {
        return Err(ContainerError::LengthMismatch("bytes after its last section"));
    }
    if expected > MAX_MATERIALIZED_BYTES as u64 {
        return Err(ContainerError::DeclaredTooLarge { what: "document with its sections", size: expected });
    }
    let skeleton = unpack(codec, &bytes[HEADER + 4..skeleton_end], raw, &|| "skeleton".to_string())?;
    if sha256(&skeleton) != digest {
        return Err(ContainerError::Checksum { what: "skeleton".to_string() });
    }
    let mut sections = Vec::with_capacity(count);
    for (index, record) in records.iter().enumerate() {
        let name = || format!("section {index}");
        let payload = unpack(record.codec, &bytes[record.offset..record.offset + record.stored], record.raw, &name)?;
        if sha256(&payload) != record.digest {
            return Err(ContainerError::Checksum { what: name() });
        }
        sections.push((record.kind, payload));
    }
    Ok(Parts { version: 2, skeleton, sections })
}

fn u64_at(bytes: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
}

/// Parses the JSON text. Bytes that are not UTF-8 are replaced, as the browser's
/// text decoder replaced them before parsing.
fn parse_text(text: &[u8], markers: Option<&mut Markers<'_>>) -> Result<Value, ContainerError> {
    let lossy;
    let text = match std::str::from_utf8(text) {
        Ok(_) => text,
        Err(_) => {
            lossy = String::from_utf8_lossy(text).into_owned();
            lossy.as_bytes()
        }
    };
    Ok(match markers {
        Some(markers) => json::parse_with(text, markers)?,
        None => json::parse(text)?,
    })
}

/// Length of `JSON.stringify(value)`.
fn json_length(value: &Value) -> usize {
    // Rare (documents near 1 GiB of JSON): building the text is acceptable.
    let mut out = Vec::new();
    json::write(value, &mut out);
    out.len()
}

/// Turns marker strings into the values they stand for while the tree is built.
struct Markers<'a> {
    sections: &'a [(u8, Vec<u8>)],
    /// One array per distinct marker text: equal markers are the same array.
    arrays: HashMap<String, Value>,
    /// One copy of each payload range, however many markers name it.
    payloads: HashMap<(usize, usize, usize), Arc<[u8]>>,
    /// Bytes the JavaScript reader would allocate: sections, 48 per restored number,
    /// base64 text per payload reference.
    allocation: usize,
    /// Upper bound of the expanded document's JSON length.
    materialized_upper: usize,
    /// The container error behind a refused string (the JSON layer only carries its own).
    error: Option<ContainerError>,
}

impl<'a> Markers<'a> {
    fn new(sections: &'a [(u8, Vec<u8>)], skeleton_bytes: usize) -> Self {
        Markers {
            sections,
            arrays: HashMap::new(),
            payloads: HashMap::new(),
            allocation: sections.iter().map(|(_, bytes)| bytes.len()).sum(),
            materialized_upper: skeleton_bytes,
            error: None,
        }
    }

    fn fail(&mut self, error: ContainerError) -> Option<Result<Value, JsonError>> {
        self.error.get_or_insert(error);
        // The parser stops at the first error; the stored container error is the one reported.
        Some(Err(JsonError::Syntax { offset: 0, message: "invalid section marker" }))
    }

    fn admit_allocation(&mut self, bytes: usize) -> Result<(), ContainerError> {
        if bytes > MAX_MATERIALIZED_BYTES.saturating_sub(self.allocation) {
            return Err(ContainerError::MaterializedTooLarge);
        }
        self.allocation += bytes;
        Ok(())
    }

    fn array(&mut self, text: &str, fields: &[&str]) -> Result<Value, ContainerError> {
        let invalid = || ContainerError::InvalidArraySection { marker: text.to_string() };
        let type_name = fields[0];
        let width = match type_name {
            "u8" => 1,
            "u16" => 2,
            "u32" | "i32" | "f32" => 4,
            _ => 8,
        };
        let [section, offset, count] = [safe_integer(fields[1]), safe_integer(fields[2]), safe_integer(fields[3])];
        let (Some(section), Some(offset), Some(count)) = (section, offset, count) else { return Err(invalid()) };
        let bytes = match self.sections.get(section) {
            Some((1, bytes)) => bytes.as_slice(),
            _ => return Err(invalid()),
        };
        if offset % width != 0 || offset > bytes.len() || count > (bytes.len() - offset) / width {
            return Err(invalid());
        }
        self.admit_allocation(count.saturating_mul(48))?;
        let data = &bytes[offset..offset + count * width];
        let non_finite = || ContainerError::NonFinite { marker: text.to_string() };
        let numbers = match type_name {
            "u8" => Nums::F32(data.iter().map(|&b| b as f32).collect()),
            "u16" => Nums::F32(data.as_chunks::<2>().0.iter().map(|&c| u16::from_le_bytes(c) as f32).collect()),
            "u32" => Nums::new(&data.as_chunks::<4>().0.iter().map(|&c| u32::from_le_bytes(c) as f64).collect::<Vec<_>>()),
            "i32" => Nums::new(&data.as_chunks::<4>().0.iter().map(|&c| i32::from_le_bytes(c) as f64).collect::<Vec<_>>()),
            "f32" => {
                let values: Box<[f32]> = data.as_chunks::<4>().0.iter().map(|&c| f32::from_le_bytes(c)).collect();
                if values.iter().any(|v| !v.is_finite()) {
                    return Err(non_finite());
                }
                Nums::F32(values)
            }
            _ => {
                let values: Vec<f64> = data.as_chunks::<8>().0.iter().map(|&c| f64::from_le_bytes(c)).collect();
                if values.iter().any(|v| !v.is_finite()) {
                    return Err(non_finite());
                }
                Nums::new(&values)
            }
        };
        Ok(if count == 0 { Value::Arr(Arc::new(Vec::new())) } else { Value::Nums(Arc::new(numbers)) })
    }

    fn payload(&mut self, text: &str, fields: &[&str], prefix: &str) -> Result<Value, ContainerError> {
        let invalid = || ContainerError::InvalidPayloadSection { marker: text.to_string() };
        let [section, offset, count] = [safe_integer(fields[0]), safe_integer(fields[1]), safe_integer(fields[2])];
        let (Some(section), Some(offset), Some(count)) = (section, offset, count) else { return Err(invalid()) };
        let bytes = match self.sections.get(section) {
            Some((2, bytes)) => bytes.as_slice(),
            _ => return Err(invalid()),
        };
        if offset > bytes.len() || count > bytes.len() - offset {
            return Err(invalid());
        }
        let encoded = count.div_ceil(3) * 4;
        self.admit_allocation(encoded)?;
        self.materialized_upper = self.materialized_upper.saturating_add(encoded);
        let data = self.payloads.entry((section, offset, count)).or_insert_with(|| Arc::from(&bytes[offset..offset + count])).clone();
        Ok(Value::Blob(Arc::new(Blob { prefix: Arc::from(prefix), bytes: data })))
    }
}

/// A run of ASCII digits as a JavaScript safe integer.
fn safe_integer(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u64>().ok().filter(|&n| n < (1 << 53)).map(|n| n as usize)
}

fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

impl StringHook for Markers<'_> {
    fn string(&mut self, text: &str) -> Option<Result<Value, JsonError>> {
        // Every marker starts with "S2" and a colon at index 3.
        if text.len() < 4 || !text.starts_with("S2") || text.as_bytes()[3] != b':' {
            return None;
        }
        let rest = &text[4..];
        match text.as_bytes()[2] {
            b'B' => {
                let fields: Vec<&str> = rest.split(':').collect();
                let well_formed = fields.len() == 4 && matches!(fields[0], "u8" | "u16" | "u32" | "i32" | "f32" | "f64") && fields[1..].iter().all(|f| digits(f));
                if !well_formed {
                    return self.fail(ContainerError::MalformedMarker { marker: text.to_string() });
                }
                if let Some(shared) = self.arrays.get(text) {
                    let count = shared.array_len().unwrap_or(0);
                    self.materialized_upper = self.materialized_upper.saturating_add(count * (MAX_JSON_NUMBER_BYTES + 1) + 2);
                    return Some(Ok(shared.clone()));
                }
                match self.array(text, &fields) {
                    Ok(value) => {
                        let count = value.array_len().unwrap_or(0);
                        self.materialized_upper = self.materialized_upper.saturating_add(count * (MAX_JSON_NUMBER_BYTES + 1) + 2);
                        self.arrays.insert(text.to_string(), value.clone());
                        Some(Ok(value))
                    }
                    Err(error) => self.fail(error),
                }
            }
            b'S' => {
                let fields: Vec<&str> = rest.split(':').collect();
                if fields.len() != 3 || !fields.iter().all(|f| digits(f)) {
                    return self.fail(ContainerError::MalformedMarker { marker: text.to_string() });
                }
                Some(self.payload(text, &fields, "").or_else(|error| self.fail(error).unwrap()))
            }
            b'D' => {
                let mut fields = rest.splitn(4, ':');
                let (Some(a), Some(b), Some(c), Some(prefix)) = (fields.next(), fields.next(), fields.next(), fields.next()) else {
                    return self.fail(ContainerError::MalformedMarker { marker: text.to_string() });
                };
                if ![a, b, c].iter().all(|f| digits(f)) {
                    return self.fail(ContainerError::MalformedMarker { marker: text.to_string() });
                }
                Some(self.payload(text, &[a, b, c], prefix).or_else(|error| self.fail(error).unwrap()))
            }
            b'E' => Some(Ok(Value::str(rest))),
            _ => None,
        }
    }
}
