// Copyright (c) 2026 OptionLab LLC. All rights reserved.
//! Content-preserving macOS arm64 re-signing evidence. This verifies bytes only;
//! the native owner separately verifies the Developer ID signature/certificate.
use crate::Result;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentDerivation {
    pub schema: String,
    pub unsigned_sha256: String,
    pub signed_sha256: String,
    pub content_sha256: String,
}
fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn u32_at(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or("macho_truncated")?
            .try_into()
            .unwrap(),
    ))
}
fn u64_at(bytes: &[u8], offset: usize) -> Result<u64> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or("macho_truncated")?
            .try_into()
            .unwrap(),
    ))
}
/// Normalize only signature length and the two __LINKEDIT sizes that codesign
/// adjusts. Signature offset, every other header/load command and code byte stay
/// bound. Existing LC_CODE_SIGNATURE is required; unsupported transformations fail.
pub fn content_sha256(bytes: &[u8]) -> Result<String> {
    if bytes.len() > 512 * 1024 * 1024
        || u32_at(bytes, 0)? != 0xfeedfacf
        || u32_at(bytes, 4)? != 0x0100000c
        || ![2, 6, 8].contains(&u32_at(bytes, 12)?)
    {
        return Err("macho_arm64_image_required".into());
    }
    let count = u32_at(bytes, 16)? as usize;
    let cmd_bytes = u32_at(bytes, 20)? as usize;
    if count == 0 || count > 4096 || cmd_bytes > 1024 * 1024 {
        return Err("macho_load_bound".into());
    }
    let end = 32usize.checked_add(cmd_bytes).ok_or("macho_load_bound")?;
    if end > bytes.len() {
        return Err("macho_truncated".into());
    }
    let mut cursor = 32;
    let mut sig = None;
    let mut link = None;
    for _ in 0..count {
        let command = u32_at(bytes, cursor)?;
        let size = u32_at(bytes, cursor + 4)? as usize;
        if size < 8 || !size.is_multiple_of(8) || cursor.checked_add(size).is_none_or(|v| v > end) {
            return Err("macho_command_invalid".into());
        }
        if command == 0x1d {
            if size != 16 || sig.is_some() {
                return Err("macho_signature_command_invalid".into());
            }
            sig = Some((
                cursor,
                u32_at(bytes, cursor + 8)? as usize,
                u32_at(bytes, cursor + 12)? as usize,
            ));
        }
        if command == 0x19 && bytes.get(cursor + 8..cursor + 24) == Some(b"__LINKEDIT\0\0\0\0\0\0")
        {
            if size != 72 || link.is_some() {
                return Err("macho_linkedit_invalid".into());
            }
            let fileoff = u64_at(bytes, cursor + 40)?;
            let filesize = u64_at(bytes, cursor + 48)?;
            if fileoff.checked_add(filesize) != Some(bytes.len() as u64) {
                return Err("macho_linkedit_extent_invalid".into());
            }
            link = Some(cursor);
        }
        cursor += size;
    }
    if cursor != end {
        return Err("macho_command_count_invalid".into());
    }
    let (command, offset, size) = sig.ok_or("macho_existing_signature_required")?;
    let link = link.ok_or("macho_linkedit_required")?;
    if offset < end
        || size < 12
        || offset.checked_add(size) != Some(bytes.len())
        || u64_at(bytes, link + 40)? > offset as u64
    {
        return Err("macho_signature_extent_invalid".into());
    }
    // SuperBlob magic is big endian. Native verification owns its cryptography.
    if bytes.get(offset..offset + 4) != Some(&[0xfa, 0xde, 0x0c, 0xc0]) {
        return Err("macho_signature_blob_invalid".into());
    }
    let mut content = bytes[..offset].to_vec();
    content[command + 12..command + 16].fill(0);
    content[link + 32..link + 40].fill(0);
    content[link + 48..link + 56].fill(0);
    Ok(sha(&content))
}
pub fn verify(unsigned: &[u8], signed: &[u8]) -> Result<ContentDerivation> {
    let content = content_sha256(unsigned)?;
    if content != content_sha256(signed)? || unsigned == signed {
        return Err("macho_signing_changed_content_or_no_derivation".into());
    }
    Ok(ContentDerivation {
        schema: "tradeassembly.macho-signature-derivation/v1".into(),
        unsigned_sha256: sha(unsigned),
        signed_sha256: sha(signed),
        content_sha256: content,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn image(signature: usize, marker: u8) -> Vec<u8> {
        let offset = 160;
        let mut b = vec![0u8; offset + signature];
        for (o, n) in [
            (0, 0xfeedfacfu32),
            (4, 0x0100000c),
            (12, 2),
            (16, 2),
            (20, 88),
            (32, 0x19),
            (36, 72),
            (104, 0x1d),
            (108, 16),
            (112, offset as u32),
            (116, signature as u32),
        ] {
            b[o..o + 4].copy_from_slice(&n.to_le_bytes());
        }
        b[40..56].copy_from_slice(b"__LINKEDIT\0\0\0\0\0\0");
        let length = b.len();
        b[64..72].copy_from_slice(&(length as u64).to_le_bytes());
        b[72..80].copy_from_slice(&120u64.to_le_bytes());
        b[80..88].copy_from_slice(&((length - 120) as u64).to_le_bytes());
        b[120..160].fill(42);
        b[offset..offset + 4].copy_from_slice(&[0xfa, 0xde, 0x0c, 0xc0]);
        b[offset + 4..].fill(marker);
        b
    }
    #[test]
    fn signature_only_growth_preserves_code_identity() {
        let a = image(32, 1);
        let b = image(64, 2);
        let r = verify(&a, &b).unwrap();
        assert_ne!(r.unsigned_sha256, r.signed_sha256);
        assert_eq!(r.content_sha256, content_sha256(&a).unwrap());
    }
    #[test]
    fn changed_code_header_offset_and_ambiguous_commands_fail() {
        let a = image(32, 1);
        for offset in [4, 12, 112, 128] {
            let mut b = image(64, 2);
            b[offset] ^= 1;
            assert!(verify(&a, &b).is_err());
        }
        assert!(verify(&a, &a).is_err());
        let mut b = image(64, 2);
        b.push(0);
        assert!(verify(&a, &b).is_err());
    }
    #[test]
    fn malformed_and_unsigned_images_fail() {
        for b in [vec![], vec![0; 128], image(4, 1)] {
            assert!(content_sha256(&b).is_err());
        }
        let mut b = image(32, 1);
        b[104..108].copy_from_slice(&0u32.to_le_bytes());
        assert!(content_sha256(&b).is_err());
    }
}
