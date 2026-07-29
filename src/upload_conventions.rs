use crate::erasure_coding::RedundancyLevel;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UploadRedundancyOption {
    pub(crate) level: RedundancyLevel,
    pub(crate) label: &'static str,
    pub(crate) description: &'static str,
}

impl UploadRedundancyOption {
    pub(crate) const fn value(self) -> u8 {
        self.level as u8
    }

    pub(crate) const fn is_default(self) -> bool {
        self.value() == DEFAULT_UPLOAD_REDUNDANCY_VALUE
    }
}

/// Bee's upload redundancy levels in ascending wire-value order.
pub(crate) const UPLOAD_REDUNDANCY_OPTIONS: [UploadRedundancyOption; 5] = [
    UploadRedundancyOption {
        level: RedundancyLevel::None,
        label: "None",
        description: "No parity; lowest upload and storage overhead.",
    },
    UploadRedundancyOption {
        level: RedundancyLevel::Medium,
        label: "Medium",
        description: "Bee default; balanced recovery and overhead.",
    },
    UploadRedundancyOption {
        level: RedundancyLevel::Strong,
        label: "Strong",
        description: "More parity for unreliable retrieval paths.",
    },
    UploadRedundancyOption {
        level: RedundancyLevel::Insane,
        label: "Insane",
        description: "High recovery with substantial parity overhead.",
    },
    UploadRedundancyOption {
        level: RedundancyLevel::Paranoid,
        label: "Paranoid",
        description: "Maximum recovery; highest upload and storage overhead.",
    },
];

pub(crate) const DEFAULT_UPLOAD_REDUNDANCY_VALUE: u8 = RedundancyLevel::DEFAULT_UPLOAD as u8;

/// Strict validation for callers that must report an invalid redundancy level.
pub(crate) fn validated_upload_redundancy(value: u8) -> Option<RedundancyLevel> {
    RedundancyLevel::from_u8(value)
}

/// Strict validation for JavaScript upload API arguments before integer coercion.
pub(crate) fn validated_upload_redundancy_number(value: f64) -> Option<RedundancyLevel> {
    if !value.is_finite()
        || value.fract() != 0.0
        || !(u8::MIN as f64..=u8::MAX as f64).contains(&value)
    {
        return None;
    }

    validated_upload_redundancy(value as u8)
}

/// Parse a select value, falling back to Bee's default for missing or malformed UI state.
pub(crate) fn upload_redundancy_from_select(value: Option<&str>) -> RedundancyLevel {
    value
        .and_then(|value| value.parse::<u8>().ok())
        .and_then(validated_upload_redundancy)
        .unwrap_or(RedundancyLevel::DEFAULT_UPLOAD)
}

/// Parse a JavaScript number, accepting only finite integral Bee wire values.
pub(crate) fn upload_redundancy_from_number(value: Option<f64>) -> RedundancyLevel {
    value
        .and_then(validated_upload_redundancy_number)
        .unwrap_or(RedundancyLevel::DEFAULT_UPLOAD)
}

#[cfg(test)]
mod redundancy_tests {
    use super::{
        DEFAULT_UPLOAD_REDUNDANCY_VALUE, UPLOAD_REDUNDANCY_OPTIONS, upload_redundancy_from_number,
        upload_redundancy_from_select, validated_upload_redundancy,
        validated_upload_redundancy_number,
    };
    use crate::erasure_coding::RedundancyLevel;

    const UPLOAD_INTERFACE_HTML: &str = include_str!("../static/index.html");
    const SERVICE_WORKER_JS: &str = include_str!("../static/service.js");
    const LIBRARY_RS: &str = include_str!("library.rs");
    const LIB_RS: &str = include_str!("lib.rs");

    #[test]
    fn dropdown_options_match_bee_wire_values_and_labels() {
        let rendered = UPLOAD_REDUNDANCY_OPTIONS
            .iter()
            .map(|option| {
                (
                    option.value(),
                    option.label,
                    option.description,
                    option.is_default(),
                )
            })
            .collect::<Vec<_>>();

        assert_eq!(
            rendered,
            vec![
                (
                    0,
                    "None",
                    "No parity; lowest upload and storage overhead.",
                    false
                ),
                (
                    1,
                    "Medium",
                    "Bee default; balanced recovery and overhead.",
                    true
                ),
                (
                    2,
                    "Strong",
                    "More parity for unreliable retrieval paths.",
                    false
                ),
                (
                    3,
                    "Insane",
                    "High recovery with substantial parity overhead.",
                    false
                ),
                (
                    4,
                    "Paranoid",
                    "Maximum recovery; highest upload and storage overhead.",
                    false
                ),
            ]
        );
        assert_eq!(DEFAULT_UPLOAD_REDUNDANCY_VALUE, 1);
    }

    #[test]
    fn strict_validation_accepts_only_bee_levels() {
        for option in UPLOAD_REDUNDANCY_OPTIONS {
            assert_eq!(
                validated_upload_redundancy(option.value()),
                Some(option.level)
            );
        }
        assert_eq!(validated_upload_redundancy(5), None);
        assert_eq!(validated_upload_redundancy(u8::MAX), None);
    }

    #[test]
    fn select_values_fall_back_to_medium() {
        assert_eq!(
            upload_redundancy_from_select(Some("0")),
            RedundancyLevel::None
        );
        assert_eq!(
            upload_redundancy_from_select(Some("4")),
            RedundancyLevel::Paranoid
        );
        for malformed in [None, Some(""), Some("-1"), Some("5"), Some("1.0")] {
            assert_eq!(
                upload_redundancy_from_select(malformed),
                RedundancyLevel::Medium
            );
        }
    }

    #[test]
    fn javascript_numbers_must_be_finite_integral_bee_values() {
        assert_eq!(
            validated_upload_redundancy_number(2.0),
            Some(RedundancyLevel::Strong)
        );
        assert_eq!(
            upload_redundancy_from_number(Some(2.0)),
            RedundancyLevel::Strong
        );
        for malformed in [
            None,
            Some(-1.0),
            Some(-255.0),
            Some(1.5),
            Some(5.0),
            Some(257.0),
            Some(f64::NAN),
            Some(f64::INFINITY),
        ] {
            if let Some(value) = malformed {
                assert_eq!(validated_upload_redundancy_number(value), None);
            }
            assert_eq!(
                upload_redundancy_from_number(malformed),
                RedundancyLevel::Medium
            );
        }
    }

    #[test]
    fn built_in_and_npm_rendered_interface_have_the_medium_default_dropdown() {
        let select_start = UPLOAD_INTERFACE_HTML
            .find(r#"<select id="uploadRedundancyLevel""#)
            .expect("upload redundancy selector");
        let select_end = UPLOAD_INTERFACE_HTML[select_start..]
            .find("</select>")
            .map(|offset| select_start + offset)
            .expect("upload redundancy selector end");
        let select = &UPLOAD_INTERFACE_HTML[select_start..select_end];

        let mut cursor = 0;
        for option in UPLOAD_REDUNDANCY_OPTIONS {
            let marker = format!(r#"<option value="{}""#, option.value());
            let position = select[cursor..]
                .find(&marker)
                .map(|offset| cursor + offset)
                .unwrap_or_else(|| panic!("missing {} option", option.label));
            assert!(position >= cursor, "dropdown order changed");
            cursor = position + marker.len();
        }
        assert!(select.contains(r#"<option value="1" selected>Medium — recommended</option>"#));
        assert!(UPLOAD_INTERFACE_HTML.contains("Higher levels improve loss recovery"));
    }

    #[test]
    fn explicit_wasm_upload_levels_are_validated_before_integer_coercion() {
        for source in [LIBRARY_RS, LIB_RS] {
            assert!(source.contains(
                r##"#[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")] redundancy_level: f64"##
            ));
            assert!(!source.contains(
                r##"#[wasm_bindgen(unchecked_param_type = "UploadRedundancyLevel")] redundancy_level: u8"##
            ));
        }
        assert!(LIB_RS.contains("validated_upload_redundancy_number(redundancy_level)"));
        assert!(LIBRARY_RS.contains("validated_upload_redundancy_number(redundancy_level)"));
    }

    #[test]
    fn service_worker_redundancy_header_uses_strict_base_ten_parsing() {
        for marker in [
            "function parseUploadRedundancyHeader(value)",
            "value === null || value === \"\"",
            r#"/^[0-9]+$/.test(value)"#,
            "Number.isSafeInteger(level) && level <= 4",
            "parsedRedundancy === null",
        ] {
            assert!(SERVICE_WORKER_JS.contains(marker), "missing {marker}");
        }
    }
}

pub(crate) type ResourceEntry = (Vec<u8>, String, String);

fn encoded_field_len(len: usize) -> Option<usize> {
    8usize.checked_add(len)
}

fn push_field(output: &mut Vec<u8>, bytes: &[u8]) -> Option<()> {
    let len = u64::try_from(bytes.len()).ok()?;
    output.extend_from_slice(&len.to_le_bytes());
    output.extend_from_slice(bytes);
    Some(())
}

/// Encode a browser resource bundle without per-entry concatenation buffers.
pub(crate) fn encode_resource_bundle(
    resources: Vec<ResourceEntry>,
    index: String,
) -> Option<Vec<u8>> {
    let mut encoded_len = encoded_field_len(index.len())?;
    for (data, media_type, name) in &resources {
        encoded_len = encoded_len
            .checked_add(encoded_field_len(media_type.len())?)?
            .checked_add(encoded_field_len(name.len())?)?
            .checked_add(encoded_field_len(data.len())?)?;
    }

    let mut output = Vec::new();
    output.try_reserve_exact(encoded_len).ok()?;
    push_field(&mut output, index.as_bytes())?;
    for (data, media_type, name) in resources {
        push_field(&mut output, media_type.as_bytes())?;
        push_field(&mut output, name.as_bytes())?;
        push_field(&mut output, &data)?;
    }
    debug_assert_eq!(output.len(), encoded_len);
    Some(output)
}

fn read_len(input: &[u8], cursor: &mut usize) -> Option<usize> {
    let end = cursor.checked_add(8)?;
    let bytes: [u8; 8] = input.get(*cursor..end)?.try_into().ok()?;
    *cursor = end;

    usize::try_from(u64::from_le_bytes(bytes)).ok()
}

fn read_bytes<'a>(input: &'a [u8], cursor: &mut usize, len: usize) -> Option<&'a [u8]> {
    let end = cursor.checked_add(len)?;
    let bytes = input.get(*cursor..end)?;
    *cursor = end;
    Some(bytes)
}

fn read_string(input: &[u8], cursor: &mut usize) -> Option<String> {
    let len = read_len(input, cursor)?;
    let bytes = read_bytes(input, cursor, len)?;

    // Keep the historical codec behavior for invalid text while making malformed
    // framing an all-or-nothing parse failure.
    Some(String::from_utf8(bytes.to_vec()).unwrap_or_default())
}

/// Decode the compact resource bundle exchanged with the browser UI.
///
/// Every cursor movement is checked so untrusted message bytes cannot panic the
/// WASM module or wrap a length on 32-bit targets.
pub(crate) fn decode_resource_bundle(input: &[u8]) -> Option<(Vec<ResourceEntry>, String)> {
    let mut cursor = 0;
    let index = read_string(input, &mut cursor)?;
    let mut resources = Vec::new();

    while cursor < input.len() {
        let media_type = read_string(input, &mut cursor)?;
        let name = read_string(input, &mut cursor)?;
        let data_len = read_len(input, &mut cursor)?;
        let data = read_bytes(input, &mut cursor, data_len)?.to_vec();
        resources.push((data, media_type, name));
    }

    Some((resources, index))
}

pub(crate) const FILE_UPLOAD_READ_WINDOW_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct FileSlicePlan {
    size: u64,
    next: u64,
}

impl FileSlicePlan {
    pub(crate) fn new(size: u64) -> Self {
        Self { size, next: 0 }
    }
}

impl Iterator for FileSlicePlan {
    type Item = (u64, u64);

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.size {
            return None;
        }

        let start = self.next;
        let end = start
            .saturating_add(FILE_UPLOAD_READ_WINDOW_BYTES)
            .min(self.size);
        self.next = end;
        Some((start, end))
    }
}
