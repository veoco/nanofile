//! EXIF parsing for the sandboxed image worker.
//!
//! The parser is a pure function of the image bytes and runs inside the child, so
//! it lives with the image job rather than with the service that downloads the
//! file and orchestrates the gate.

use std::io::Cursor;

use base::error::AppError;
/// Parse EXIF bytes into a JSON object of known tags. Runs inside the sandbox
/// child, which is why it is reachable from there.
pub(crate) fn extract_exif(content: &[u8]) -> Result<serde_json::Value, AppError> {
    let mut cursor = Cursor::new(content);
    let exif = match exif::Reader::new().read_from_container(&mut cursor) {
        Ok(e) => e,
        Err(_) => return Ok(serde_json::Value::Null),
    };

    let mut fields = serde_json::Map::new();

    // GPS latitude with reference — skip if all-zero (invalid).
    if has_valid_gps(&exif, exif::Tag::GPSLatitude) {
        insert_gps_field(
            &exif,
            &mut fields,
            exif::Tag::GPSLatitude,
            exif::Tag::GPSLatitudeRef,
            "GPSLatitude",
        );
    }
    if has_valid_gps(&exif, exif::Tag::GPSLongitude) {
        insert_gps_field(
            &exif,
            &mut fields,
            exif::Tag::GPSLongitude,
            exif::Tag::GPSLongitudeRef,
            "GPSLongitude",
        );
    }

    for tag in [
        exif::Tag::Make,
        exif::Tag::Model,
        exif::Tag::DateTimeOriginal,
        exif::Tag::ExposureTime,
        exif::Tag::FNumber,
        exif::Tag::ISOSpeed,
        exif::Tag::FocalLength,
        exif::Tag::Flash,
        exif::Tag::Software,
        exif::Tag::PixelXDimension,
        exif::Tag::PixelYDimension,
        exif::Tag::Orientation,
    ] {
        if let Some(field) = exif.get_field(tag, exif::In::PRIMARY) {
            let tag_name = format!("{tag}");
            let value = match tag {
                // Bitmask/enum fields — raw Short so frontend maps to labels.
                t if t == exif::Tag::Orientation || t == exif::Tag::Flash => {
                    if let exif::Value::Short(vals) = &field.value {
                        vals.first().map(|v| v.to_string()).unwrap_or_default()
                    } else {
                        field.display_value().to_string()
                    }
                }
                // Ascii fields may have multiple values (Software can carry
                // empty trailing entries); join non-empty ones with ", ".
                t if t == exif::Tag::Software => {
                    if let exif::Value::Ascii(vals) = &field.value {
                        vals.iter()
                            .filter(|v| !v.is_empty())
                            .filter_map(|v| String::from_utf8(v.clone()).ok())
                            .collect::<Vec<_>>()
                            .join(", ")
                    } else {
                        field.display_value().to_string()
                    }
                }
                _ => field.display_value().to_string(),
            };
            fields.insert(tag_name, serde_json::Value::String(value));
        }
    }

    Ok(serde_json::Value::Object(fields))
}

/// Check whether a GPS coordinate field has at least one non-zero rational
/// component (avoids displaying a "NaN° NaN′ NaN″" coordinate for invalid data).
fn has_valid_gps(exif: &exif::Exif, coord_tag: exif::Tag) -> bool {
    if let Some(field) = exif.get_field(coord_tag, exif::In::PRIMARY)
        && let exif::Value::Rational(vals) = &field.value
    {
        return vals.iter().any(|r| r.num != 0 && r.denom != 0);
    }
    false
}

fn insert_gps_field(
    exif: &exif::Exif,
    fields: &mut serde_json::Map<String, serde_json::Value>,
    coord_tag: exif::Tag,
    ref_tag: exif::Tag,
    key: &str,
) {
    if let (Some(coord), Some(coord_ref)) = (
        exif.get_field(coord_tag, exif::In::PRIMARY),
        exif.get_field(ref_tag, exif::In::PRIMARY),
    ) {
        fields.insert(
            key.to_string(),
            serde_json::Value::String(format_gps_dms(coord, coord_ref)),
        );
    }
}

/// Format a GPS coordinate as `31° 25′ 46.5816″ N` (symbols instead of the
/// exif crate's "deg/min/sec" words); the hemisphere letter is kept as-is.
fn format_gps_dms(coord: &exif::Field, coord_ref: &exif::Field) -> String {
    let dms = match &coord.value {
        exif::Value::Rational(vals) if vals.len() >= 3 => {
            let comp = |r: &exif::Rational| {
                let v = r.to_f64();
                if v.is_nan() { 0.0 } else { v }
            };
            format!(
                "{}° {}′ {}″",
                comp(&vals[0]),
                comp(&vals[1]),
                comp(&vals[2])
            )
        }
        _ => coord.display_value().to_string(),
    };
    format!("{dms} {}", coord_ref.display_value())
}
