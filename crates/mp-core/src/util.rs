//! 基础工具：系统随机数、十六进制、Base64、恢复密钥用的 Base32、时间戳。

use crate::error::{CoreError, Result};
use std::time::{SystemTime, UNIX_EPOCH};

/// 取 N 个系统随机字节。
pub fn random_array<const N: usize>() -> Result<[u8; N]> {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes)?;
    Ok(bytes)
}

/// 32 位十六进制随机标识。
pub fn random_id() -> Result<String> {
    Ok(hex_encode(&random_array::<16>()?))
}

/// 随机的块标识。
pub fn random_block_id() -> Result<u64> {
    Ok(u64::from_le_bytes(random_array::<8>()?))
}

/// Unix 时间戳，毫秒。
pub fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|passed| passed.as_millis() as u64)
        .unwrap_or(0)
}

/// 文件名必须是单一的文件名组件，在 Windows 上也可以使用。
pub fn valid_file_name(name: &str) -> bool {
    if name.is_empty() || name.ends_with([' ', '.']) || name.chars().any(|ch| ch.is_control() || r#"\/:*?"<>|"#.contains(ch)) {
        return false;
    }
    !is_windows_device_name(name)
}

pub fn is_windows_device_name(name: &str) -> bool {
    let device = name.split('.').next().unwrap_or(name).trim_end().to_ascii_uppercase();
    if matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$") {
        return true;
    }
    device.strip_prefix("COM").or_else(|| device.strip_prefix("LPT"))
        .is_some_and(|suffix| matches!(suffix, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"))
}

pub fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 15) as usize] as char);
    }
    out
}

/// 解析十六进制串；遇到非法字符返回 None。
pub fn hex_decode(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    fn value(ch: char) -> Option<u8> {
        match ch {
            '0'..='9' => Some(ch as u8 - b'0'),
            'a'..='f' => Some(ch as u8 - b'a' + 10),
            'A'..='F' => Some(ch as u8 - b'A' + 10),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(text.len() / 2);
    let mut chars = text.chars();
    while let Some(high) = chars.next() {
        let low = chars.next()?;
        out.push((value(high)? << 4) | value(low)?);
    }
    Some(out)
}

const BASE64_ALPHABET: &[u8; 64] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let first = chunk[0] as u32;
        let second = *chunk.get(1).unwrap_or(&0) as u32;
        let third = *chunk.get(2).unwrap_or(&0) as u32;
        let packed = (first << 16) | (second << 8) | third;
        out.push(BASE64_ALPHABET[(packed >> 18) as usize & 63] as char);
        out.push(BASE64_ALPHABET[(packed >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            BASE64_ALPHABET[(packed >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            BASE64_ALPHABET[packed as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// 严格解码：只接受标准字母表，填充符只能出现在末尾。
pub fn base64_decode(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut quad = [0u8; 4];
    let mut count = 0usize;
    let mut padding = 0usize;
    for ch in text.chars() {
        let value = match ch {
            'A'..='Z' => ch as u8 - b'A',
            'a'..='z' => ch as u8 - b'a' + 26,
            '0'..='9' => ch as u8 - b'0' + 52,
            '+' => 62,
            '/' => 63,
            '=' => {
                padding += 1;
                0
            }
            _ => return Err(CoreError::BadFormat("Base64".into())),
        };
        // 填充符之后不能再有别的字符。
        if padding > 0 && ch != '=' {
            return Err(CoreError::BadFormat("Base64".into()));
        }
        quad[count] = value;
        count += 1;
        if count == 4 {
            let packed = ((quad[0] as u32) << 18)
                | ((quad[1] as u32) << 12)
                | ((quad[2] as u32) << 6)
                | quad[3] as u32;
            out.push((packed >> 16) as u8);
            out.push((packed >> 8) as u8);
            out.push(packed as u8);
            count = 0;
            // 不在这里结束：后面若还跟着字符，循环开头的检查会报错。
        }
    }
    if count != 0 || padding > 2 {
        return Err(CoreError::BadFormat("Base64".into()));
    }
    if (padding == 1 && quad[2] & 3 != 0) || (padding == 2 && quad[1] & 15 != 0) {
        return Err(CoreError::BadFormat("Base64".into()));
    }
    out.truncate(out.len() - padding);
    Ok(out)
}

const BASE32_ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// Base32 编码，不带填充符。
pub fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let mut buffer: u64 = 0;
    let mut bits = 0u32;
    for &byte in data {
        buffer = (buffer << 8) | byte as u64;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(BASE32_ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(BASE32_ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// Base32 解码，不带填充符；忽略大小写、连字符和空白。
pub fn base32_decode(text: &str) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buffer: u64 = 0;
    let mut bits = 0u32;
    for ch in text.chars() {
        if ch == '-' || ch.is_whitespace() {
            continue;
        }
        // 取值必须用转成大写之后的那个字符，否则小写输入会算出错值。
        let upper = ch.to_ascii_uppercase();
        let value = match upper {
            'A'..='Z' => (upper as u8 - b'A') as u64,
            '2'..='7' => (upper as u8 - b'2') as u64 + 26,
            _ => return Err(CoreError::BadFormat("Base32".into())),
        };
        buffer = (buffer << 5) | value;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push(((buffer >> bits) & 0xFF) as u8);
        }
    }
    // 末尾剩余的位必须为零，否则不是合法编码。
    if bits >= 5 || (bits > 0 && (buffer & ((1 << bits) - 1)) != 0) {
        return Err(CoreError::BadFormat("Base32".into()));
    }
    Ok(out)
}

/// 恢复密钥的字节数。128 位随机量足够，同时手抄不至于太长。
pub const RECOVERY_KEY_LEN: usize = 16;

/// 把恢复密钥排成每 5 个字符一组、用连字符分隔的形式。
pub fn format_recovery_key(bytes: &[u8; RECOVERY_KEY_LEN]) -> String {
    let text = base32_encode(bytes);
    text.as_bytes()
        .chunks(5)
        .map(|group| std::str::from_utf8(group).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("-")
}

/// 解析用户输入的恢复密钥。
pub fn parse_recovery_key(text: &str) -> Result<[u8; RECOVERY_KEY_LEN]> {
    let bytes = base32_decode(text)?;
    if bytes.len() != RECOVERY_KEY_LEN {
        return Err(CoreError::Invalid("恢复密钥的长度不对".into()));
    }
    let mut key = [0u8; RECOVERY_KEY_LEN];
    key.copy_from_slice(&bytes);
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip_all_lengths() {
        for len in 0usize..70 {
            let data: Vec<u8> = (0..len).map(|i| (i * 37 + len * 11) as u8).collect();
            let text = base64_encode(&data);
            assert_eq!(text.len(), len.div_ceil(3) * 4);
            assert_eq!(base64_decode(&text).unwrap(), data);
        }
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
    }

    #[test]
    fn base64_rejects_bad_input() {
        assert!(base64_decode("Zm9vYg").is_err());
        assert!(base64_decode("Zm9vYg=").is_err());
        assert!(base64_decode("Zm=vYg==").is_err());
        assert!(base64_decode("Zm9vYg==Zm9v").is_err());
        assert!(base64_decode("Zm9vYg===").is_err());
        assert!(base64_decode("Zm9v\nYg==").is_err());
        assert!(base64_decode("Zh==").is_err());
        assert!(base64_decode("Zm9=").is_err());
    }

    #[test]
    fn recovery_key_roundtrip() {
        let key = [0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let text = format_recovery_key(&key);
        assert_eq!(text.len(), 26 + 5);
        assert_eq!(text, "AAISE-M2EKV-THPCE-ZVK54-ZXPO7-4");
        assert_eq!(parse_recovery_key(&text).unwrap(), key);
        assert_eq!(parse_recovery_key(&text.to_lowercase()).unwrap(), key);
        assert_eq!(parse_recovery_key(&format!(" {text}\n")).unwrap(), key);
        assert!(parse_recovery_key("ABC").is_err());
    }

    #[test]
    fn hex_roundtrip() {
        let id = random_id().unwrap();
        assert_eq!(id.len(), 32);
        assert_eq!(hex_decode(&id).unwrap().len(), 16);
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
    }
}
