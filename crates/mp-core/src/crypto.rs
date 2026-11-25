//! 加密原语。全部使用公开标准：Argon2id 做密钥派生，AES-256-GCM 做认证加密，SHA-256 做子密钥派生。
//!
//! 设计要点（细则见仓库 `docs/format.md`）：
//! - 库里有一个随机主密钥，所有内容都用它派生出的子密钥加密。
//! - 密码和恢复密钥各自「包装」一份主密钥，所以改密码只需要重写包装，不用重写整个库。
//! - 媒体文件按 1 MiB 分块加密，每块独立认证，可以按需解密任意一段（流式播放用）。

use crate::error::{CoreError, Result};
use crate::util::{base64_decode, base64_encode, random_array};
use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce};
use argon2::{Algorithm, Argon2, Params, Version};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroize;

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;

/// 媒体文件的分块大小。
pub const CHUNK_SIZE: usize = 1024 * 1024;

/// 库的主密钥。析构时清零。
#[derive(Clone)]
pub struct MasterKey([u8; KEY_LEN]);

impl MasterKey {
    pub fn generate() -> Result<Self> {
        Ok(Self(random_array::<KEY_LEN>()?))
    }

    pub fn from_bytes(bytes: &[u8]) -> Option<Self> {
        let mut key = [0u8; KEY_LEN];
        if bytes.len() != KEY_LEN {
            return None;
        }
        key.copy_from_slice(bytes);
        Some(Self(key))
    }

    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl Drop for MasterKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 永不把密钥打进日志。
        f.write_str("MasterKey(<已隐藏>)")
    }
}

/// Argon2id 参数。内存单位是 KiB。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KdfParams {
    pub m_cost_kib: u32,
    pub t_cost: u32,
    pub p_cost: u32,
    pub salt: String,
}

/// 默认强度：64 MiB 内存、3 轮。解锁大约零点几秒，是「启动快」和「抗暴力破解」之间的折中。
pub const DEFAULT_M_COST_KIB: u32 = 64 * 1024;
pub const DEFAULT_T_COST: u32 = 3;
pub const DEFAULT_P_COST: u32 = 1;

impl KdfParams {
    /// 生成一组带新随机盐的默认参数。
    pub fn generate() -> Result<Self> {
        Ok(Self {
            m_cost_kib: DEFAULT_M_COST_KIB,
            t_cost: DEFAULT_T_COST,
            p_cost: DEFAULT_P_COST,
            salt: base64_encode(&random_array::<16>()?),
        })
    }

    /// 从文件读进来的参数要先过这道闸，防止被篡改成让程序卡死或耗尽内存的值。
    pub fn validate(&self) -> Result<()> {
        if !(1..=16).contains(&self.p_cost) {
            return Err(CoreError::KdfParams);
        }
        let salt_ok = (16..=64).contains(&base64_decode(&self.salt)?.len());
        let memory_ok = (8 * self.p_cost.max(1)..=1_048_576).contains(&self.m_cost_kib);
        if salt_ok
            && memory_ok
            && (1..=64).contains(&self.t_cost)
            && (1..=16).contains(&self.p_cost)
        {
            Ok(())
        } else {
            Err(CoreError::KdfParams)
        }
    }

    /// 用口令派生出包装密钥。
    pub fn derive(&self, secret: &[u8]) -> Result<[u8; KEY_LEN]> {
        self.validate()?;
        let salt = base64_decode(&self.salt)?;
        let params = Params::new(self.m_cost_kib, self.t_cost, self.p_cost, Some(KEY_LEN))
            .map_err(|_| CoreError::KdfParams)?;
        let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
        let mut output = [0u8; KEY_LEN];
        argon.hash_password_into(secret, &salt, &mut output).map_err(CoreError::Kdf)?;
        Ok(output)
    }
}

/// 被包装的主密钥：随机数 + 密文（密文末尾带 16 字节认证标签）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WrappedKey {
    pub nonce: String,
    pub ciphertext: String,
}

fn wrap_aad(vault_id: &str, kind: &str) -> Vec<u8> {
    let mut aad = b"moss-pocket/wrap/v1|".to_vec();
    aad.extend_from_slice(kind.as_bytes());
    aad.push(b'|');
    aad.extend_from_slice(vault_id.as_bytes());
    aad
}

/// 用包装密钥加密主密钥。明文里附带库标识，解包时再核对一次。
pub fn wrap_master_key(
    wrapping_key: &[u8; KEY_LEN],
    vault_id: &str,
    kind: &str,
    master: &MasterKey,
) -> Result<WrappedKey> {
    let nonce = random_array::<NONCE_LEN>()?;
    let mut plaintext = Vec::with_capacity(KEY_LEN + vault_id.len());
    plaintext.extend_from_slice(master.as_bytes());
    plaintext.extend_from_slice(vault_id.as_bytes());
    let ciphertext = encrypt(wrapping_key, &nonce, &plaintext, &wrap_aad(vault_id, kind))?;
    Ok(WrappedKey {
        nonce: base64_encode(&nonce),
        ciphertext: base64_encode(&ciphertext),
    })
}

/// 解包主密钥。口令不对或数据被改过都返回 [`CoreError::Decrypt`]。
pub fn unwrap_master_key(
    wrapping_key: &[u8; KEY_LEN],
    vault_id: &str,
    kind: &str,
    wrapped: &WrappedKey,
) -> Result<MasterKey> {
    let nonce_bytes = base64_decode(&wrapped.nonce)?;
    let Ok(nonce): std::result::Result<[u8; NONCE_LEN], Vec<u8>> = nonce_bytes.try_into() else {
        return Err(CoreError::BadFormat("包装随机数长度".into()));
    };
    let ciphertext = base64_decode(&wrapped.ciphertext)?;
    let plaintext = decrypt(wrapping_key, &nonce, &ciphertext, &wrap_aad(vault_id, kind))?;
    if plaintext.len() != KEY_LEN + vault_id.len() || &plaintext[KEY_LEN..] != vault_id.as_bytes() {
        return Err(CoreError::Decrypt(aes_gcm::Error));
    }
    MasterKey::from_bytes(&plaintext[..KEY_LEN]).ok_or(CoreError::Decrypt(aes_gcm::Error))
}

/// AES-256-GCM 加密，输出密文加 16 字节标签。
pub fn encrypt(
    key: &[u8; KEY_LEN],
    random: &[u8; NONCE_LEN],
    plaintext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| CoreError::Encrypt(aes_gcm::Error))?;
    cipher
        .encrypt(&Nonce::try_from(random.as_slice()).expect("随机数长度固定为 12 字节"), Payload { msg: plaintext, aad })
        .map_err(CoreError::Encrypt)
}

/// AES-256-GCM 解密。认证失败即密钥不对或内容被改过。
pub fn decrypt(
    key: &[u8; KEY_LEN],
    random: &[u8; NONCE_LEN],
    ciphertext: &[u8],
    aad: &[u8],
) -> Result<Vec<u8>> {
    if ciphertext.len() < TAG_LEN {
        return Err(aes_gcm::Error.into());
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| aes_gcm::Error)?;
    cipher
        .decrypt(&Nonce::try_from(random.as_slice()).expect("随机数长度固定为 12 字节"), Payload { msg: ciphertext, aad })
        .map_err(CoreError::Decrypt)
}

/// 从主密钥派生某个用途的子密钥。
pub fn derive_key(master: &MasterKey, vault_id: &str, purpose: &str, name: &str) -> [u8; KEY_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(master.as_bytes());
    hasher.update(b"moss-pocket/v1|");
    hasher.update(purpose.as_bytes());
    hasher.update(b"|");
    hasher.update(vault_id.as_bytes());
    hasher.update(b"|");
    hasher.update(name.as_bytes());
    hasher.finalize().into()
}

/// 分块加密的随机数：4 字节随机前缀加 8 字节块序号，保证同一密钥下每块的随机数都不重复。
pub fn chunk_nonce(prefix: &[u8; 4], index: u64) -> [u8; NONCE_LEN] {
    let mut nonce = [0u8; NONCE_LEN];
    nonce[..4].copy_from_slice(prefix);
    nonce[4..].copy_from_slice(&index.to_le_bytes());
    nonce
}

/// 分块的附加认证数据：绑定库、媒体文件、块序号和本块明文长度，防止被调换、截断或拼接。
pub fn chunk_aad(vault_id: &str, blob_id: &str, index: u64, plain_len: usize) -> Vec<u8> {
    let mut aad = b"moss-pocket/chunk/v1|".to_vec();
    aad.extend_from_slice(vault_id.as_bytes());
    aad.push(b'|');
    aad.extend_from_slice(blob_id.as_bytes());
    aad.extend_from_slice(&index.to_le_bytes());
    aad.extend_from_slice(&(plain_len as u64).to_le_bytes());
    aad
}

/// 媒体文件头的字节数：标识 4 + 分块大小 4 + 随机数前缀 4 + 原始长度 8。
pub const BLOB_HEADER_LEN: usize = 20;

#[cfg(test)]
mod tests {
    use super::*;

    fn fast_params() -> KdfParams {
        KdfParams {
            m_cost_kib: 8,
            t_cost: 1,
            p_cost: 1,
            salt: base64_encode(&[7u8; 16]),
        }
    }

    #[test]
    fn wrap_and_unwrap_roundtrip() {
        let master = MasterKey::generate().unwrap();
        let params = fast_params();
        let kek = params.derive(b"correct horse").unwrap();
        let wrapped = wrap_master_key(&kek, "vault-1", "password", &master).unwrap();
        let back = unwrap_master_key(&kek, "vault-1", "password", &wrapped).unwrap();
        assert_eq!(back.as_bytes(), master.as_bytes());
    }

    #[test]
    fn wrong_secret_wrong_kind_wrong_vault_all_fail() {
        let master = MasterKey::generate().unwrap();
        let params = fast_params();
        let kek = params.derive(b"right").unwrap();
        let wrapped = wrap_master_key(&kek, "vault-1", "password", &master).unwrap();

        let other_kek = params.derive(b"wrong").unwrap();
        assert!(unwrap_master_key(&other_kek, "vault-1", "password", &wrapped).is_err());
        assert!(unwrap_master_key(&kek, "vault-2", "password", &wrapped).is_err());
        assert!(unwrap_master_key(&kek, "vault-1", "recovery", &wrapped).is_err());

        let mut tampered = wrapped.clone();
        let mut bytes = base64_decode(&tampered.ciphertext).unwrap();
        bytes[0] ^= 0x01;
        tampered.ciphertext = base64_encode(&bytes);
        assert!(unwrap_master_key(&kek, "vault-1", "password", &tampered).is_err());
    }

    #[test]
    fn encrypt_decrypt_and_tamper() {
        let key = [3u8; KEY_LEN];
        let nonce = [4u8; NONCE_LEN];
        let message = "你好，世界".as_bytes();
        let ciphertext = encrypt(&key, &nonce, message, b"aad").unwrap();
        assert_eq!(ciphertext.len(), message.len() + TAG_LEN);
        assert_eq!(decrypt(&key, &nonce, &ciphertext, b"aad").unwrap(), message);
        assert!(decrypt(&key, &nonce, &ciphertext, b"other").is_err());
        assert!(decrypt(&key, &[9u8; NONCE_LEN], &ciphertext, b"aad").is_err());
        assert!(decrypt(&[5u8; KEY_LEN], &nonce, &ciphertext, b"aad").is_err());

        let mut broken = ciphertext.clone();
        broken[3] ^= 0x80;
        assert!(decrypt(&key, &nonce, &broken, b"aad").is_err());
        assert!(decrypt(&key, &nonce, &ciphertext[..TAG_LEN], b"aad").is_err());
    }

    #[test]
    fn chunk_nonces_never_collide_and_aad_binds_everything() {
        let prefix = [1u8, 2, 3, 4];
        let first = chunk_nonce(&prefix, 0);
        let second = chunk_nonce(&prefix, 1);
        let last = chunk_nonce(&prefix, u64::MAX);
        assert_ne!(first, second);
        assert_ne!(first, last);
        assert_eq!(&first[..4], &prefix);
        assert_eq!(&last[4..], &u64::MAX.to_le_bytes());

        assert_ne!(chunk_aad("v", "b", 0, 10), chunk_aad("v", "b", 1, 10));
        assert_ne!(chunk_aad("v", "b", 0, 10), chunk_aad("v", "b", 0, 11));
        assert_ne!(chunk_aad("v", "b", 0, 10), chunk_aad("v", "c", 0, 10));
    }

    #[test]
    fn derived_keys_depend_on_every_part() {
        let master = MasterKey::generate().unwrap();
        let a = derive_key(&master, "v", "note", "");
        assert_ne!(a, derive_key(&master, "v", "index", ""));
        assert_ne!(a, derive_key(&master, "w", "note", ""));
        let other = MasterKey::generate().unwrap();
        assert_ne!(a, derive_key(&other, "v", "note", ""));
    }

    #[test]
    fn kdf_params_validation_rejects_absurd_values() {
        assert!(fast_params().validate().is_ok());
        assert!(KdfParams::generate().unwrap().validate().is_ok());
        let mut bad = fast_params();
        bad.m_cost_kib = u32::MAX;
        assert!(bad.validate().is_err());
        let mut bad = fast_params();
        bad.p_cost = 0;
        assert!(bad.validate().is_err());
        let mut bad = fast_params();
        bad.p_cost = u32::MAX;
        assert!(bad.validate().is_err());
        let mut bad = fast_params();
        bad.salt = "not base64!".into();
        assert!(bad.validate().is_err());
    }

    #[test]
    fn default_kdf_is_deterministic_per_salt() {
        let params = KdfParams::generate().unwrap();
        let first = params.derive("同一个密码".as_bytes()).unwrap();
        let second = params.derive("同一个密码".as_bytes()).unwrap();
        assert_eq!(first, second);
        let rekeyed = KdfParams::generate().unwrap();
        assert_ne!(first, rekeyed.derive("同一个密码".as_bytes()).unwrap());
    }
}
