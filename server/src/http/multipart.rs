//! Bounded readers for multipart fields.
//!
//! Both multipart libraries in use expose their field as a
//! `Stream<Item = Result<Bytes, E>>`, so the size policy lives here instead of in
//! two verbatim readers: a declared `Content-Length` over the limit is rejected up
//! front, and a lying header is caught while reading.

use base::error::AppError;

/// Reject a multipart field from its declared `Content-Length` before buffering.
pub(crate) fn check_field_length(
    headers: &axum::http::HeaderMap,
    limit: usize,
) -> Result<(), AppError> {
    if let Some(len) = headers
        .get(axum::http::header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<usize>().ok())
        && len > limit
    {
        return Err(AppError::ContentTooLarge);
    }
    Ok(())
}

/// Buffer a chunk stream under `limit`.
///
/// Both multipart libraries in use expose their field as a
/// `Stream<Item = Result<Bytes, E>>`, so the size policy lives here instead of in
/// two verbatim readers: a declared `Content-Length` over the limit is rejected up
/// front, and a lying header is caught while reading. `1 MiB` is allocated up
/// front (`limit.min(1<<20)`) and the rest grows only as data arrives.
pub(crate) async fn read_stream_limited<S, E>(stream: S, limit: usize) -> Result<Vec<u8>, AppError>
where
    S: futures::Stream<Item = Result<bytes::Bytes, E>>,
    E: std::fmt::Display,
{
    use futures::StreamExt;

    let mut stream = std::pin::pin!(stream);
    let mut out = Vec::with_capacity(limit.min(1 << 20));
    while let Some(chunk) = stream
        .next()
        .await
        .transpose()
        .map_err(|e| AppError::Internal(format!("multipart field read error: {e}")))?
    {
        if out.len() + chunk.len() > limit {
            return Err(AppError::ContentTooLarge);
        }
        out.extend_from_slice(&chunk);
    }
    Ok(out)
}

/// Stream a multipart field into memory under `limit`: reject an oversized part from
/// its Content-Length before buffering and cap the bytes read, so a lying header
/// cannot force a large buffer.
pub async fn read_multipart_field_limited(
    field: &mut axum::extract::multipart::Field<'_>,
    limit: usize,
) -> Result<Vec<u8>, AppError> {
    check_field_length(field.headers(), limit)?;
    read_stream_limited(&mut *field, limit).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_multipart_length_over_the_cap_is_rejected() {
        let mut headers = axum::http::HeaderMap::new();
        headers.insert(axum::http::header::CONTENT_LENGTH, "10".parse().unwrap());
        assert!(matches!(
            check_field_length(&headers, 9).unwrap_err(),
            AppError::ContentTooLarge
        ));
        assert!(check_field_length(&headers, 10).is_ok());
        assert!(check_field_length(&axum::http::HeaderMap::new(), 0).is_ok());
    }

    #[tokio::test]
    async fn a_streamed_field_is_capped_and_chunked_in_order() {
        let chunks = futures::stream::iter(vec![
            Ok::<_, std::io::Error>(bytes::Bytes::from_static(b"abc")),
            Ok(bytes::Bytes::from_static(b"def")),
        ]);
        assert_eq!(read_stream_limited(chunks, 6).await.unwrap(), b"abcdef");

        let over = futures::stream::iter(vec![Ok::<_, std::io::Error>(bytes::Bytes::from_static(
            b"abc",
        ))]);
        assert!(matches!(
            read_stream_limited(over, 2).await.unwrap_err(),
            AppError::ContentTooLarge
        ));

        let failed = futures::stream::iter(vec![Err::<bytes::Bytes, _>(std::io::Error::other(
            "read failed",
        ))]);
        assert!(matches!(
            read_stream_limited(failed, 10).await.unwrap_err(),
            AppError::Internal(_)
        ));
    }
}
