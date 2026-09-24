use std::borrow::Cow;

use base64::Engine as _;

const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_BASE64_ENCODED_BYTES: usize = MAX_IMAGE_BYTES * 4 / 3 + 4;
const MAX_BASE64_WHITESPACE_BYTES: usize = MAX_BASE64_ENCODED_BYTES / 32;
pub fn is_data_image_uri(uri: &str) -> bool {
    uri.get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("data:"))
}

pub fn decode_data_image(uri: &str) -> anyhow::Result<Vec<u8>> {
    let data = uri
        .get(5..)
        .filter(|_| uri[..5].eq_ignore_ascii_case("data:"))
        .ok_or_else(|| anyhow::anyhow!("invalid image data URI"))?;
    let (metadata, payload) = data
        .split_once(',')
        .ok_or_else(|| anyhow::anyhow!("missing image data payload"))?;
    let payload = payload.split_once('#').map_or(payload, |(data, _)| data);
    let mut parameters = metadata.split(';');
    let mime = parameters.next().unwrap_or_default().trim();
    anyhow::ensure!(
        mime.get(..6)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("image/"))
            && !mime.eq_ignore_ascii_case("image/svg+xml"),
        "unsupported inline image type"
    );
    let base64 = parameters.any(|parameter| parameter.trim().eq_ignore_ascii_case("base64"));
    if base64 {
        anyhow::ensure!(
            payload.len() <= MAX_BASE64_ENCODED_BYTES + MAX_BASE64_WHITESPACE_BYTES,
            "inline image exceeds size limit"
        );
    } else {
        anyhow::ensure!(
            payload.len() <= MAX_IMAGE_BYTES * 3,
            "inline image exceeds size limit"
        );
    }
    let decoded: Cow<'_, [u8]> = percent_encoding::percent_decode_str(payload).into();
    let bytes = if base64 {
        let encoded = if decoded.iter().any(u8::is_ascii_whitespace) {
            Cow::Owned(
                decoded
                    .iter()
                    .copied()
                    .filter(|byte| !byte.is_ascii_whitespace())
                    .collect::<Vec<_>>(),
            )
        } else {
            decoded
        };
        anyhow::ensure!(
            encoded.len() <= MAX_BASE64_ENCODED_BYTES,
            "inline image exceeds size limit"
        );
        let engine = base64::engine::GeneralPurpose::new(
            &base64::alphabet::STANDARD,
            base64::engine::GeneralPurposeConfig::new()
                .with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
        );
        engine
            .decode(encoded.as_ref())
            .map_err(|_| anyhow::anyhow!("invalid inline image base64"))?
    } else {
        anyhow::ensure!(
            decoded.len() <= MAX_IMAGE_BYTES,
            "inline image exceeds size limit"
        );
        decoded.into_owned()
    };
    anyhow::ensure!(
        !bytes.is_empty() && bytes.len() <= MAX_IMAGE_BYTES,
        "invalid inline image size"
    );
    image::guess_format(&bytes).map_err(|_| anyhow::anyhow!("unsupported inline image data"))?;
    Ok(bytes)
}
