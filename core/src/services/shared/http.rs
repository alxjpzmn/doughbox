use reqwest::Response;

#[derive(Debug)]
pub(crate) struct ResponseTooLarge;

impl std::fmt::Display for ResponseTooLarge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTP response exceeds size limit")
    }
}

impl std::error::Error for ResponseTooLarge {}

/// Check both the advertised length and the actual streamed bytes before extending
/// the buffer: chunked responses must obey the same bound as fixed-length ones.
pub(crate) async fn limited_body(mut response: Response, limit: usize) -> anyhow::Result<Vec<u8>> {
    let length = response.content_length();
    if length.is_some_and(|length| length > limit as u64) {
        return Err(ResponseTooLarge.into());
    }
    let mut bytes = Vec::with_capacity(length.unwrap_or(0) as usize);
    while let Some(chunk) = response.chunk().await.map_err(|error| error.without_url())? {
        if chunk.len() > limit - bytes.len() {
            return Err(ResponseTooLarge.into());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
