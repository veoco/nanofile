use flate2::Compression;
use flate2::write::ZlibEncoder;
use std::io::Write;

pub fn compress_fs_data(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(data)?;
    encoder.finish()
}

pub fn decompress_fs_data(data: &[u8]) -> Result<Vec<u8>, std::io::Error> {
    let mut decoder = flate2::read::ZlibDecoder::new(data);
    let mut decompressed = Vec::new();
    std::io::Read::read_to_end(&mut decoder, &mut decompressed)?;
    Ok(decompressed)
}

pub fn encode_pack_fs_entries(entries: &[(String, Vec<u8>)]) -> Vec<u8> {
    let mut result = Vec::new();

    for (fs_id, data) in entries {
        result.extend_from_slice(fs_id.as_bytes());
        let size = data.len() as u32;
        result.extend_from_slice(&size.to_be_bytes());
        result.extend_from_slice(data);
    }

    result
}

pub fn decode_pack_fs_entries(data: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut entries = Vec::new();
    let mut offset = 0;

    while offset + 44 <= data.len() {
        let fs_id_bytes = &data[offset..offset + 40];
        let fs_id = String::from_utf8(fs_id_bytes.to_vec())
            .map_err(|e| format!("invalid fs_id encoding: {}", e))?;

        if fs_id.len() != 40 || !fs_id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!(
                "invalid fs_id: must be 40 hex characters, got {:?}",
                fs_id
            ));
        }

        let size_bytes = [
            data[offset + 40],
            data[offset + 41],
            data[offset + 42],
            data[offset + 43],
        ];
        let size = u32::from_be_bytes(size_bytes) as usize;

        offset += 44;

        if offset + size > data.len() {
            return Err(format!("truncated data at entry {}", entries.len()));
        }

        let entry_data = data[offset..offset + size].to_vec();
        offset += size;

        entries.push((fs_id, entry_data));
    }

    Ok(entries)
}
