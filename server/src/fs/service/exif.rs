use std::io::Cursor;
use std::sync::Arc;

use sea_orm::DatabaseConnection;

use crate::error::AppError;
use crate::repo::download::Downloader;

pub struct ExifService {
    db: Arc<DatabaseConnection>,
    block_store: crate::storage::DynBlockStorage,
}

impl ExifService {
    pub fn new(db: Arc<DatabaseConnection>, block_store: crate::storage::DynBlockStorage) -> Self {
        Self { db, block_store }
    }

    fn db(&self) -> &DatabaseConnection {
        self.db.as_ref()
    }

    /// Extract EXIF metadata from an image file.
    ///
    /// Returns a JSON object with EXIF fields that were found, or `null` if
    /// the file contains no EXIF data.
    pub async fn get_exif(&self, repo_id: &str, path: &str) -> Result<serde_json::Value, AppError> {
        let content = Downloader::download_file(self.db(), repo_id, path, &self.block_store, None)
            .await
            .map_err(|_| AppError::NotFound("file not found".into()))?;

        let exif_data = tokio::task::spawn_blocking(move || Self::extract_exif(&content))
            .await
            .map_err(|e| AppError::Internal(format!("EXIF parsing panicked: {e}")))??;

        Ok(exif_data)
    }

    /// Parse EXIF bytes and return a JSON object with known tags.
    fn extract_exif(content: &[u8]) -> Result<serde_json::Value, AppError> {
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
        // GPS longitude with reference.
        if has_valid_gps(&exif, exif::Tag::GPSLongitude) {
            insert_gps_field(
                &exif,
                &mut fields,
                exif::Tag::GPSLongitude,
                exif::Tag::GPSLongitudeRef,
                "GPSLongitude",
            );
        }

        // Standard EXIF tags.
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
                    // Ascii fields that may have multiple values (e.g.
                    // Software can contain empty trailing entries).
                    // Join all non-empty values with ", " — no extra quotes.
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
                    // All other tags — use the default display formatter.
                    _ => field.display_value().to_string(),
                };
                fields.insert(tag_name, serde_json::Value::String(value));
            }
        }

        Ok(serde_json::Value::Object(fields))
    }
}

/// Check whether a GPS coordinate field has at least one non-zero rational
/// component (avoids displaying "NaN deg NaN min NaN sec" for invalid data).
fn has_valid_gps(exif: &exif::Exif, coord_tag: exif::Tag) -> bool {
    if let Some(field) = exif.get_field(coord_tag, exif::In::PRIMARY) {
        if let exif::Value::Rational(vals) = &field.value {
            return vals.iter().any(|r| r.num != 0 && r.denom != 0);
        }
    }
    false
}

/// Helper: extract a GPS coordinate field combined with its reference.
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
            serde_json::Value::String(format!(
                "{} {}",
                coord.display_value(),
                coord_ref.display_value()
            )),
        );
    }
}
