//! Text decoding for text files: UTF-8, UTF-16 LE/BE (BOM-marked or
//! heuristic-decodable). Used by the editor, the workspace search and the
//! built-in agent so a UTF-16 file never falls back to shell one-liners.

/// The encoding the file was read from (and that is re-used when the file is
/// written back), so a save preserves the original encoding rather than
/// silently changing it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum TextEncoding {
    /// UTF-8, with or without BOM (a leading BOM is stripped on read and
    /// not re-added on write).
    #[default]
    Utf8,
    /// UTF-16 little-endian; always BOM-marked on write.
    Utf16Le,
    /// UTF-16 big-endian; always BOM-marked on write.
    Utf16Be,
}

/// Detect and decode a text file. Returns the detected encoding and the text
/// with the BOM stripped. Errors when neither UTF-8 nor UTF-16 applies.
pub fn decode(bytes: &[u8]) -> Result<(TextEncoding, String), String> {
    let (encoding, text) = decode_cow(bytes)?;
    Ok((encoding, text.into_owned()))
}

/// [`decode`] without the copy for the common case: plain UTF-8 (with or
/// without a BOM) borrows the input buffer; only UTF-16 materializes a
/// new string. Callers that normalize or transform the text can work on
/// the `Cow` and pay for at most one owned result.
pub fn decode_cow<'a>(bytes: &'a [u8]) -> Result<(TextEncoding, std::borrow::Cow<'a, str>), String> {
    use std::borrow::Cow;
    if bytes.starts_with(b"\xEF\xBB\xBF") {
        let text = std::str::from_utf8(&bytes[3..])
            .map_err(|e| format!("not UTF-8: {e}"))?;
        return Ok((TextEncoding::Utf8, Cow::Borrowed(text)));
    }
    if bytes.starts_with(b"\xFF\xFE") {
        let text = decode_utf16(&bytes[2..], true)?;
        return Ok((TextEncoding::Utf16Le, Cow::Owned(text)));
    }
    if bytes.starts_with(b"\xFE\xFF") {
        let text = decode_utf16(&bytes[2..], false)?;
        return Ok((TextEncoding::Utf16Be, Cow::Owned(text)));
    }
    // UTF-8 is tried first: valid UTF-8 with no BOM wins.
    if bytes.iter().all(|&b| b != 0x00) {
        if let Ok(text) = std::str::from_utf8(bytes) {
            return Ok((TextEncoding::Utf8, Cow::Borrowed(text)));
        }
    }
    // BOM-less UTF-16: char high bytes are zero, so NULs dominate the odd
    // (LE) or even (BE) positions.
    let data = bytes;
    if data.len() >= 4 {
        let even_nul = data.iter().step_by(2).filter(|&&b| b == 0).count();
        let odd_nul = data.iter().skip(1).step_by(2).filter(|&&b| b == 0).count();
        let pairs = data.len() / 2;
        if odd_nul >= pairs / 2 && odd_nul > even_nul * 4 {
            let text = decode_utf16(data, true)?;
            // The pattern alone is weak evidence on short inputs (six
            // binary bytes can satisfy it), so the guess must also read
            // as text: control characters other than tab/newline/return
            // mean it was binary all along.
            if looks_like_text(&text) {
                return Ok((TextEncoding::Utf16Le, Cow::Owned(text)));
            }
        }
        if even_nul >= pairs / 2 && even_nul > odd_nul * 4 {
            let text = decode_utf16(data, false)?;
            if looks_like_text(&text) {
                return Ok((TextEncoding::Utf16Be, Cow::Owned(text)));
            }
        }
    }
    Err("not UTF-8 or UTF-16 text".into())
}

/// Does a decoded string read as text? Used to vet the BOM-less UTF-16
/// guess, where the byte pattern alone cannot tell text from binary.
fn looks_like_text(text: &str) -> bool {
    !text
        .chars()
        .any(|c| c.is_control() && !matches!(c, '\t' | '\n' | '\r'))
}

fn decode_utf16(bytes: &[u8], little_endian: bool) -> Result<String, String> {
    let mut units = Vec::with_capacity(bytes.len() / 2 + 1);
    for pair in bytes.chunks_exact(2) {
        let u = if little_endian {
            pair[0] as u16 | (pair[1] as u16) << 8
        } else {
            (pair[0] as u16) << 8 | pair[1] as u16
        };
        units.push(u);
    }
    // Trailing byte (odd length) is dropped: it cannot be a valid code unit.
    let text = String::from_utf16_lossy(&units);
    Ok(text)
}

/// Encode `text` in the file's original encoding (BOMs written for UTF-16,
/// none for UTF-8).
pub fn encode(encoding: TextEncoding, text: &str) -> Vec<u8> {
    match encoding {
        TextEncoding::Utf8 => text.as_bytes().to_vec(),
        TextEncoding::Utf16Le => {
            let mut out = vec![0xFF, 0xFE];
            for u in text.encode_utf16() {
                out.push(u as u8);
                out.push((u >> 8) as u8);
            }
            out
        }
        TextEncoding::Utf16Be => {
            let mut out = vec![0xFE, 0xFF];
            for u in text.encode_utf16() {
                out.push((u >> 8) as u8);
                out.push(u as u8);
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_bom_tagged_utf16() {
        let mut le = vec![0xFF, 0xFE];
        for u in [0x41, 0x42] {
            le.push(u as u8);
            le.push((u >> 8) as u8);
        }
        let (enc, text) = decode(&le).unwrap();
        assert_eq!(enc, TextEncoding::Utf16Le);
        assert_eq!(text, "AB");

        let mut be = vec![0xFE, 0xFF];
        for u in [0x41, 0x42] {
            be.push((u >> 8) as u8);
            be.push(u as u8);
        }
        let (enc, text) = decode(&be).unwrap();
        assert_eq!(enc, TextEncoding::Utf16Be);
        assert_eq!(text, "AB");
    }

    #[test]
    fn strips_utf8_bom() {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend_from_slice("hi".as_bytes());
        let (enc, text) = decode(&bytes).unwrap();
        assert_eq!(enc, TextEncoding::Utf8);
        assert_eq!(text, "hi");
    }

    #[test]
    fn plain_utf8_is_unchanged() {
        let (enc, text) = decode("hello".as_bytes()).unwrap();
        assert_eq!(enc, TextEncoding::Utf8);
        assert_eq!(text, "hello");
    }

    #[test]
    fn detects_bomless_utf16_le_by_null_pattern() {
        // "ab" as UTF-16 LE: 61 00 62 00
        let bytes: Vec<u8> = vec![0x61, 0x00, 0x62, 0x00, 0x63, 0x00, 0x64, 0x00];
        let (enc, text) = decode(&bytes).unwrap();
        assert_eq!(enc, TextEncoding::Utf16Le);
        assert_eq!(text, "abcd");
    }

    #[test]
    fn rejects_binary_and_undecidable() {
        assert!(decode(b"\x00\x01\x02\x03\x04\x05").is_err());
        assert!(decode(&[0xFF, 0xFE, 0xFF, 0xFE, 0xFF, 0xFE]).is_ok()); // BOM-stripped, decodes
    }

    #[test]
    fn encode_roundtrip_le_and_be() {
        for enc in [TextEncoding::Utf16Le, TextEncoding::Utf16Be] {
            let bytes = encode(enc, "héllo");
            let (e2, text) = decode(&bytes).unwrap();
            assert_eq!(e2, enc);
            assert_eq!(text, "héllo");
        }
        let bytes = encode(TextEncoding::Utf8, "héllo");
        let (e2, text) = decode(&bytes).unwrap();
        assert_eq!(e2, TextEncoding::Utf8);
        assert_eq!(text, "héllo");
    }
}
