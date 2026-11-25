//! 磁盘上的库：文件布局、建库与解锁、笔记和媒体文件的读写。细则见仓库 `docs/format.md`。
//!
//! 布局（数据目录跟着 exe 走）：
//! - `vault.json`：明文头，记格式版本、库标识、密钥派生参数和被包装的主密钥
//! - `index.enc`：加密的条目索引，列表用它，不含正文
//! - `notes/<笔记标识>.note`：每条笔记一个加密文件
//! - `blobs/<媒体标识>.blob`：加密的媒体文件，分块存储，可流式解密
//!
//! 所有写入都走「写临时文件、旧文件改名备份、临时文件顶上」，断电或崩溃不会留下半个文件。

use crate::crypto::{
    self, BLOB_HEADER_LEN, CHUNK_SIZE, KEY_LEN, KdfParams, MasterKey, TAG_LEN, WrappedKey,
};
use crate::error::{CoreError, Result};
use crate::model::{normalize_tags, Note, NoteSummary};
use crate::spans;
use crate::util::{
    format_recovery_key, now_millis, parse_recovery_key, random_array, random_id, RECOVERY_KEY_LEN,
};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// 数据目录名，固定放在 exe 旁边（便携模式）。
pub const DATA_DIR_NAME: &str = "MOSS-Pocket-data";
/// 回收站保留时长：30 天。
pub const TRASH_RETENTION_MILLIS: u64 = 30 * 24 * 60 * 60 * 1000;
/// 密码最短长度。
pub const MIN_PASSWORD_LEN: usize = 6;

const HEADER_FILE: &str = "vault.json";
const INDEX_FILE: &str = "index.enc";
const NOTES_DIR: &str = "notes";
const BLOBS_DIR: &str = "blobs";
const FORMAT_NAME: &str = "moss-pocket-vault";
const FORMAT_VERSION: u32 = 1;
const NOTE_MAGIC: &[u8; 4] = b"MPN1";
const INDEX_MAGIC: &[u8; 4] = b"MPI1";
const BLOB_MAGIC: &[u8; 4] = b"MPB1";

/// 包装主密钥的两种口令。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WrapKind {
    Password,
    Recovery,
}

impl WrapKind {
    fn label(self) -> &'static str {
        match self {
            WrapKind::Password => "password",
            WrapKind::Recovery => "recovery",
        }
    }
}

/// 一份包装：用哪种口令、什么派生参数、包装后的主密钥。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeyWrap {
    pub id: String,
    pub kind: WrapKind,
    pub kdf: KdfParams,
    #[serde(flatten)]
    pub wrapped: WrappedKey,
}

/// 库头。明文存放，不含任何机密。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VaultHeader {
    pub format: String,
    pub version: u32,
    pub vault_id: String,
    pub created_at: u64,
    pub wraps: Vec<KeyWrap>,
}

/// 解锁用的口令。
#[derive(Clone, Copy)]
pub enum UnlockSecret<'a> {
    Password(&'a str),
    RecoveryKey(&'a str),
}

/// 已解锁的库。持有主密钥，析构时清零。
pub struct Vault {
    root: PathBuf,
    header: VaultHeader,
    master: MasterKey,
    index: Vec<NoteSummary>,
}

fn io_error(path: &Path, error: io::Error) -> CoreError {
    CoreError::io(path, error)
}

/// 同目录下的兄弟文件：`x.note` 加上 `.tmp` 变成 `x.note.tmp`。
fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".");
    name.push(suffix);
    path.with_file_name(name)
}

/// 原子写入：先写临时文件并落盘，旧文件改名成备份，再让临时文件顶上。
fn atomic_write(path: &Path, data: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|error| io_error(parent, error))?;
    let tmp = sibling(path, "tmp");
    let backup = sibling(path, "prev");
    {
        let mut file = File::create(&tmp).map_err(|error| io_error(&tmp, error))?;
        file.write_all(data).map_err(|error| io_error(&tmp, error))?;
        file.sync_all().map_err(|error| io_error(&tmp, error))?;
    }
    let had_old = path.exists();
    if had_old {
        if backup.exists() {
            fs::remove_file(&backup).map_err(|error| io_error(&backup, error))?;
        }
        fs::rename(path, &backup).map_err(|error| io_error(path, error))?;
    }
    if let Err(error) = fs::rename(&tmp, path) {
        // 顶替失败就把备份放回去，保持原样。
        if had_old {
            let _ = fs::rename(&backup, path);
        }
        let _ = fs::remove_file(&tmp);
        return Err(io_error(path, error));
    }
    Ok(())
}

/// 标识只能是 32 位十六进制，防止损坏的索引把路径引到目录外面去。
fn check_id(id: &str) -> Result<()> {
    let valid = id.len() == 32 && id.bytes().all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    if valid {
        Ok(())
    } else {
        Err(CoreError::BadFormat(format!("标识 `{id}`")))
    }
}

/// 封装一段明文：标识 4 字节 + 随机数 12 字节 + 密文（末尾带 16 字节标签）。
fn seal(key: &[u8; KEY_LEN], magic: &[u8; 4], aad: &[u8], plaintext: &[u8]) -> Result<Vec<u8>> {
    let nonce = random_array::<12>()?;
    let ciphertext = crypto::encrypt(key, &nonce, plaintext, aad)?;
    let mut out = Vec::with_capacity(16 + ciphertext.len());
    out.extend_from_slice(magic);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

/// [`seal`] 的逆操作。
fn open_sealed(key: &[u8; KEY_LEN], magic: &[u8; 4], aad: &[u8], bytes: &[u8]) -> Result<Vec<u8>> {
    if bytes.len() < 16 + TAG_LEN || &bytes[..4] != magic {
        return Err(CoreError::BadFormat(String::from_utf8_lossy(magic).to_string()));
    }
    let nonce: &[u8; 12] = bytes[4..16].try_into().expect("长度已校验为 16 字节以上");
    crypto::decrypt(key, nonce, &bytes[16..], aad)
}

fn seal_aad(vault_id: &str, kind: &str, id: &str) -> Vec<u8> {
    let mut aad = b"moss-pocket/v1|".to_vec();
    aad.extend_from_slice(kind.as_bytes());
    aad.push(b'|');
    aad.extend_from_slice(vault_id.as_bytes());
    aad.push(b'|');
    aad.extend_from_slice(id.as_bytes());
    aad
}

/// 读完直到填满缓冲区或遇到结尾，返回实际读到的字节数。
fn read_fill(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(count) => filled += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

/// 读满整块，不够就报错（处理到一半文件断了）。
fn read_exact_or_eof(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<()> {
    let filled = read_fill(reader, buffer)?;
    if filled != buffer.len() {
        return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "文件比头部声明的短"));
    }
    Ok(())
}

impl VaultHeader {
    pub fn path(root: &Path) -> PathBuf {
        root.join(HEADER_FILE)
    }

    /// 读库头。文件不存在返回 None（第一次使用）。
    pub fn load(root: &Path) -> Result<Option<VaultHeader>> {
        let path = Self::path(root);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let backup = sibling(&path, "prev");
                match fs::read(&backup) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
                    Err(error) => return Err(io_error(&backup, error)),
                }
            }
            Err(error) => return Err(io_error(&path, error)),
        };
        let header: VaultHeader = serde_json::from_slice(&bytes)?;
        header.validate()?;
        Ok(Some(header))
    }

    fn validate(&self) -> Result<()> {
        if self.format != FORMAT_NAME {
            return Err(CoreError::BadFormat("库头".into()));
        }
        if self.version > FORMAT_VERSION {
            return Err(CoreError::NewerVersion(self.version));
        }
        if self.version != FORMAT_VERSION || check_id(&self.vault_id).is_err() || self.wraps.is_empty() {
            return Err(CoreError::BadFormat("库头".into()));
        }
        for wrap in &self.wraps {
            check_id(&wrap.id)?;
            wrap.kdf.validate()?;
        }
        Ok(())
    }

    pub fn wraps_of_kind(&self, kind: WrapKind) -> impl Iterator<Item = &KeyWrap> {
        self.wraps.iter().filter(move |wrap| wrap.kind == kind)
    }
}

fn validate_password(password: &str) -> Result<()> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        Err(CoreError::PasswordTooShort)
    } else {
        Ok(())
    }
}

/// 从磁盘读进来的笔记先过一遍规整，坏掉的字段修到可用，不让一条坏笔记拖垮整个库。
pub fn sanitize_note(note: &mut Note) {
    note.tags = normalize_tags(&note.tags);
    for block in &mut note.blocks {
        if let Some(text) = block.as_text_mut() {
            let len = crate::model::char_count(&text.text) as u32;
            text.spans = spans::normalize(std::mem::take(&mut text.spans), len);
        }
    }
    if note.blocks.is_empty() {
        if let Ok(empty) = crate::model::TextBlock::new_empty() {
            note.blocks.push(crate::model::Block::Text(empty));
        }
    }
}

/// 媒体文件的流式解密读取器。实现 `Read + Seek`，解码器和网络播放都直接拿它当普通文件用。
pub struct BlobReader<R> {
    inner: R,
    key: [u8; KEY_LEN],
    aad_prefix: Vec<u8>,
    prefix: [u8; 4],
    chunk_size: usize,
    plain_len: u64,
    chunk_index: u64,
    chunk: Vec<u8>,
    position: u64,
}

impl<R: Read + Seek> BlobReader<R> {
    fn open(mut inner: R, key: [u8; KEY_LEN], vault_id: &str, blob_id: &str) -> io::Result<Self> {
        let mut header = [0u8; BLOB_HEADER_LEN];
        inner.read_exact(&mut header)?;
        if &header[..4] != BLOB_MAGIC {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "不是媒体文件"));
        }
        let chunk_size = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        if !(4096..=64 * 1024 * 1024).contains(&chunk_size) {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "分块大小非法"));
        }
        let plain_len = u64::from_le_bytes(header[12..20].try_into().unwrap());
        let expected_len = plain_len.div_ceil(chunk_size as u64)
            .checked_mul(TAG_LEN as u64)
            .and_then(|tags| plain_len.checked_add(tags))
            .and_then(|len| len.checked_add(BLOB_HEADER_LEN as u64))
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "媒体长度溢出"))?;
        if inner.seek(SeekFrom::End(0))? != expected_len {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "媒体长度与头部不一致"));
        }
        let mut reader = Self {
            inner,
            key,
            aad_prefix: b"moss-pocket/chunk/v1|".to_vec(),
            prefix: header[8..12].try_into().unwrap(),
            chunk_size,
            plain_len,
            chunk_index: u64::MAX,
            chunk: Vec::new(),
            position: 0,
        };
        reader.aad_prefix.extend_from_slice(vault_id.as_bytes());
        reader.aad_prefix.push(b'|');
        reader.aad_prefix.extend_from_slice(blob_id.as_bytes());
        Ok(reader)
    }

    /// 原始（解密后）长度。
    pub fn len(&self) -> u64 {
        self.plain_len
    }

    fn chunk_plain_len(&self, index: u64) -> usize {
        let remaining = self.plain_len.saturating_sub(index.saturating_mul(self.chunk_size as u64));
        remaining.min(self.chunk_size as u64) as usize
    }

    fn chunk_aad(&self, index: u64, plain_len: usize) -> Vec<u8> {
        let mut aad = self.aad_prefix.clone();
        aad.extend_from_slice(&index.to_le_bytes());
        aad.extend_from_slice(&(plain_len as u64).to_le_bytes());
        aad
    }

    fn load_chunk(&mut self, index: u64) -> io::Result<()> {
        let plain_len = self.chunk_plain_len(index);
        let start = BLOB_HEADER_LEN as u64 + index * (self.chunk_size as u64 + TAG_LEN as u64);
        self.inner.seek(SeekFrom::Start(start))?;
        let mut ciphertext = vec![0u8; plain_len + TAG_LEN];
        read_exact_or_eof(&mut self.inner, &mut ciphertext)?;
        let plain = crypto::decrypt(
            &self.key,
            &crypto::chunk_nonce(&self.prefix, index),
            &ciphertext,
            &self.chunk_aad(index, plain_len),
        )
        .map_err(io::Error::other)?;
        self.chunk = plain;
        self.chunk_index = index;
        Ok(())
    }
}

impl<R: Read + Seek> Read for BlobReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() || self.position >= self.plain_len {
            return Ok(0);
        }
        let index = self.position / self.chunk_size as u64;
        if index != self.chunk_index {
            self.load_chunk(index)?;
        }
        let offset = (self.position % self.chunk_size as u64) as usize;
        let count = out.len().min(self.chunk.len().saturating_sub(offset));
        out[..count].copy_from_slice(&self.chunk[offset..offset + count]);
        self.position += count as u64;
        Ok(count)
    }
}

impl<R: Read + Seek> Seek for BlobReader<R> {
    fn seek(&mut self, target: SeekFrom) -> io::Result<u64> {
        let base: i128 = match target {
            SeekFrom::Start(value) => value as i128,
            SeekFrom::Current(delta) => self.position as i128 + delta as i128,
            SeekFrom::End(delta) => self.plain_len as i128 + delta as i128,
        };
        if !(0..=u64::MAX as i128).contains(&base) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "定位超出文件位置范围"));
        }
        self.position = base as u64;
        Ok(self.position)
    }
}

impl Vault {
    /// exe 旁边的数据目录。
    pub fn data_dir(exe_dir: &Path) -> PathBuf {
        exe_dir.join(DATA_DIR_NAME)
    }

    /// 新建一个库，返回已解锁的库和恢复密钥（已排好版，直接给用户看）。
    pub fn create(root: &Path, password: &str) -> Result<(Vault, String)> {
        validate_password(password)?;
        // A missing header can mean an interrupted write, not an empty vault.
        // The application acquires its .lock before first-time creation.
        if root.exists() {
            for entry in fs::read_dir(root).map_err(|error| io_error(root, error))? {
                let entry = entry.map_err(|error| io_error(root, error))?;
                if entry.file_name() != ".lock" {
                    return Err(CoreError::Invalid("数据目录已有文件，不能覆盖创建笔记库".into()));
                }
            }
        }
        let master = MasterKey::generate()?;
        let recovery_bytes = random_array::<RECOVERY_KEY_LEN>()?;
        let now = now_millis();
        let header = VaultHeader {
            format: FORMAT_NAME.into(),
            version: FORMAT_VERSION,
            vault_id: random_id()?,
            created_at: now,
            wraps: Vec::new(),
        };
        let dirs = [root.to_path_buf(), root.join(NOTES_DIR), root.join(BLOBS_DIR)];
        for dir in &dirs {
            fs::create_dir_all(dir).map_err(|error| io_error(dir, error))?;
        }
        let mut vault = Vault {
            root: root.to_path_buf(),
            header,
            master,
            index: Vec::new(),
        };
        vault.replace_wrap(WrapKind::Password, password.as_bytes())?;
        vault.replace_wrap(WrapKind::Recovery, &recovery_bytes)?;
        vault.save_header()?;
        vault.save_index()?;
        Ok((vault, format_recovery_key(&recovery_bytes)))
    }

    /// 用密码或恢复密钥解锁。口令不对返回 [`CoreError::Decrypt`]。
    pub fn unlock(root: &Path, secret: UnlockSecret<'_>) -> Result<Vault> {
        let header = VaultHeader::load(root)?.ok_or(CoreError::VaultNotFound)?;
        let kind = match secret {
            UnlockSecret::Password(_) => WrapKind::Password,
            UnlockSecret::RecoveryKey(_) => WrapKind::Recovery,
        };
        let secret_bytes: Vec<u8> = match secret {
            UnlockSecret::Password(text) => text.as_bytes().to_vec(),
            UnlockSecret::RecoveryKey(text) => parse_recovery_key(text)?.to_vec(),
        };
        let mut master = None;
        let mut last_error = CoreError::Decrypt(aes_gcm::Error);
        for wrap in header.wraps_of_kind(kind) {
            let wrapping_key = match wrap.kdf.derive(&secret_bytes) {
                Ok(key) => key,
                Err(error) => {
                    last_error = error;
                    continue;
                }
            };
            match crypto::unwrap_master_key(&wrapping_key, &header.vault_id, kind.label(), &wrap.wrapped) {
                Ok(key) => {
                    master = Some(key);
                    break;
                }
                Err(error) => last_error = error,
            }
        }
        let master = master.ok_or(last_error)?;
        for dir in [root.join(NOTES_DIR), root.join(BLOBS_DIR)] {
            fs::create_dir_all(&dir).map_err(|error| io_error(&dir, error))?;
        }
        let mut vault = Vault {
            root: root.to_path_buf(),
            header,
            master,
            index: Vec::new(),
        };
        vault.load_index()?;
        Ok(vault)
    }

    /// 重新包装主密钥：密码和恢复密钥各包一份。只重写库头，不碰任何笔记。
    fn replace_wrap(&mut self, kind: WrapKind, secret: &[u8]) -> Result<()> {
        let kdf = KdfParams::generate()?;
        let wrapping_key = kdf.derive(secret)?;
        let wrapped = crypto::wrap_master_key(
            &wrapping_key,
            &self.header.vault_id,
            kind.label(),
            &self.master,
        )?;
        let new_wrap = KeyWrap {
            id: random_id()?,
            kind,
            kdf,
            wrapped,
        };
        self.header.wraps.retain(|wrap| wrap.kind != kind);
        self.header.wraps.push(new_wrap);
        Ok(())
    }

    fn save_header(&self) -> Result<()> {
        let path = VaultHeader::path(&self.root);
        atomic_write(&path, &serde_json::to_vec_pretty(&self.header)?)
    }

    fn note_key(&self) -> [u8; KEY_LEN] {
        crypto::derive_key(&self.master, &self.header.vault_id, "note", "")
    }

    fn index_key(&self) -> [u8; KEY_LEN] {
        crypto::derive_key(&self.master, &self.header.vault_id, "index", "")
    }

    fn index_path(&self) -> PathBuf {
        self.root.join(INDEX_FILE)
    }

    fn note_path(&self, note_id: &str) -> PathBuf {
        self.root.join(NOTES_DIR).join(format!("{note_id}.note"))
    }

    /// 读索引；索引坏了或不存在，就把所有笔记解密一遍重建。
    fn load_index(&mut self) -> Result<()> {
        let aad = seal_aad(&self.header.vault_id, "index", "");
        if let Ok(bytes) = fs::read(self.index_path()) {
            if let Ok(plain) = open_sealed(&self.index_key(), INDEX_MAGIC, &aad, &bytes) {
                if let Ok(index) = serde_json::from_slice::<Vec<NoteSummary>>(&plain) {
                    let disk_ids = self.disk_note_ids()?;
                    let index_ids: std::collections::HashSet<_> = index.iter().map(|item| item.id.clone()).collect();
                    if index.len() == index_ids.len() && disk_ids == index_ids {
                        self.index = index;
                        return Ok(());
                    }
                }
            }
        }
        self.rebuild_index()
    }

    /// 扫描笔记目录重建索引。任何一条笔记解不开都停下来报告，不做任何删除。
    pub fn rebuild_index(&mut self) -> Result<()> {
        let mut index = Vec::new();
        for id in self.disk_note_ids()? {
            index.push(NoteSummary::from(&self.load_note(&id)?));
        }
        self.index = index;
        self.save_index()
    }

    // Include previous-only files left between the two renames of a durable write.
    fn disk_note_ids(&self) -> Result<std::collections::HashSet<String>> {
        let dir = self.root.join(NOTES_DIR);
        let mut ids = std::collections::HashSet::new();
        for entry in fs::read_dir(&dir).map_err(|error| io_error(&dir, error))? {
            let entry = entry.map_err(|error| io_error(&dir, error))?;
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if let Some(id) = name.strip_suffix(".note").or_else(|| name.strip_suffix(".note.prev")) {
                check_id(id)?;
                ids.insert(id.to_string());
            }
        }
        Ok(ids)
    }

    fn save_index(&self) -> Result<()> {
        let aad = seal_aad(&self.header.vault_id, "index", "");
        let sealed = seal(&self.index_key(), INDEX_MAGIC, &aad, &serde_json::to_vec(&self.index)?)?;
        atomic_write(&self.index_path(), &sealed)
    }

    pub fn header(&self) -> &VaultHeader {
        &self.header
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 条目摘要，已排好序：置顶在前，其余按修改时间从新到旧。
    pub fn summaries(&self) -> Vec<NoteSummary> {
        let mut list = self.index.clone();
        crate::model::sort_summaries(&mut list);
        list
    }

    pub fn load_note(&mut self, note_id: &str) -> Result<Note> {
        check_id(note_id)?;
        let path = self.note_path(note_id);
        let key = self.note_key();
        let aad = seal_aad(&self.header.vault_id, "note", note_id);
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let backup = sibling(&path, "prev");
                match fs::read(&backup) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == io::ErrorKind::NotFound => return Err(CoreError::NoteNotFound),
                    Err(error) => return Err(io_error(&backup, error)),
                }
            }
            Err(error) => return Err(io_error(&path, error)),
        };
        let plaintext = match open_sealed(&key, NOTE_MAGIC, &aad, &bytes) {
            Ok(plain) => plain,
            Err(_) => {
                // 主文件坏了，退回上一版。
                let backup = sibling(&path, "prev");
                let bytes = fs::read(&backup).map_err(|_| CoreError::Decrypt(aes_gcm::Error))?;
                open_sealed(&key, NOTE_MAGIC, &aad, &bytes)?
            }
        };
        let mut note: Note = serde_json::from_slice(&plaintext)?;
        if note.id != note_id {
            return Err(CoreError::BadFormat("笔记标识与文件名不一致".into()));
        }
        sanitize_note(&mut note);
        Ok(note)
    }

    /// 存一条笔记，同时更新索引。
    pub fn save_note(&mut self, note: &Note) -> Result<()> {
        check_id(&note.id)?;
        let mut copy = note.clone();
        sanitize_note(&mut copy);
        let aad = seal_aad(&self.header.vault_id, "note", &copy.id);
        let sealed = seal(&self.note_key(), NOTE_MAGIC, &aad, &serde_json::to_vec(&copy)?)?;
        atomic_write(&self.note_path(&copy.id), &sealed)?;
        let summary = NoteSummary::from(&copy);
        match self.index.iter_mut().find(|item| item.id == summary.id) {
            Some(item) => *item = summary,
            None => self.index.push(summary),
        }
        self.save_index()
    }

    /// 列表里只改了元数据（置顶、标签），而正文还没解密时的快捷入口。
    pub fn summary_mut(&mut self, note_id: &str) -> Option<&mut NoteSummary> {
        self.index.iter_mut().find(|item| item.id == note_id)
    }

    /// 永久删除一条笔记，连同备份文件。
    pub fn destroy_note(&mut self, note_id: &str) -> Result<()> {
        check_id(note_id)?;
        let path = self.note_path(note_id);
        for target in [sibling(&path, "prev"), sibling(&path, "tmp"), path.clone()] {
            if target.exists() {
                fs::remove_file(&target).map_err(|error| io_error(&target, error))?;
            }
        }
        self.index.retain(|item| item.id != note_id);
        self.save_index()
    }

    /// 清掉过了保留期的回收站条目。
    pub fn purge_trash(&mut self, now: u64) -> Result<usize> {
        let expired: Vec<String> = self
            .index
            .iter()
            .filter(|item| item.deleted_at.is_some_and(|at| now.saturating_sub(at) > TRASH_RETENTION_MILLIS))
            .map(|item| item.id.clone())
            .collect();
        // Cached metadata can predate a restored note after an interrupted save.
        // Confirm all candidates before making any permanent deletion.
        let mut confirmed = Vec::new();
        for id in expired {
            if self.load_note(&id)?.deleted_at.is_some_and(|at| now.saturating_sub(at) > TRASH_RETENTION_MILLIS) {
                confirmed.push(id);
            }
        }
        for id in &confirmed {
            self.destroy_note(id)?;
        }
        Ok(confirmed.len())
    }

    /// 删掉没有任何笔记引用的媒体文件。
    ///
    /// 只要有一条笔记解不开就什么都不删，宁可多占空间也不误删还在用的文件。
    pub fn collect_garbage(&mut self) -> Result<()> {
        let mut referenced = std::collections::HashSet::new();
        // The index is a cache and may predate a completed note write.
        // Scan every durable note before deleting any media.
        for id in self.disk_note_ids()? {
            let note = self.load_note(&id)?;
            referenced.extend(note.blob_ids().into_iter().map(str::to_string));
            let backup = sibling(&self.note_path(&id), "prev");
            if backup.exists() {
                let bytes = fs::read(&backup).map_err(|error| io_error(&backup, error))?;
                let aad = seal_aad(&self.header.vault_id, "note", &id);
                let plain = open_sealed(&self.note_key(), NOTE_MAGIC, &aad, &bytes)?;
                let note: Note = serde_json::from_slice(&plain)?;
                if note.id != id { return Err(CoreError::BadFormat("备份笔记标识不一致".into())); }
                referenced.extend(note.blob_ids().into_iter().map(str::to_string));
            }
        }
        let dir = self.root.join(BLOBS_DIR);
        let entries = match fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(io_error(&dir, error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| io_error(&dir, error))?;
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let is_blob = path.extension().is_some_and(|ext| ext == "blob");
            // An import may still be writing its .tmp on a worker thread.
            if is_blob && check_id(stem).is_ok() && !referenced.contains(stem) {
                let _ = fs::remove_file(&path);
            }
        }
        Ok(())
    }

    /// 媒体文件的轻量句柄。导入、解码、流式播放都可以拿到别的线程上做，不占界面线程。
    pub fn blob_handle(&self) -> BlobHandle {
        BlobHandle {
            root: self.root.clone(),
            vault_id: self.header.vault_id.clone(),
            master: self.master.clone(),
        }
    }

    /// 导入一个媒体文件：分块加密后存进库，返回 (媒体标识, 原始字节数)。
    pub fn import_blob(&self, reader: impl Read) -> Result<(String, u64)> {
        self.blob_handle().import(reader)
    }

    /// 打开一个媒体文件，拿到可定位的解密读取器。
    pub fn open_blob(&self, blob_id: &str) -> Result<BlobReader<BufReader<File>>> {
        self.blob_handle().open(blob_id)
    }

    /// 把一个媒体文件整个读进内存。图片这类小文件用。
    pub fn read_blob(&self, blob_id: &str) -> Result<Vec<u8>> {
        self.blob_handle().read(blob_id)
    }

    /// 改密码：先验证旧密码，再只重写包装。恢复密钥不受影响。
    pub fn change_password(&mut self, old_password: &str, new_password: &str) -> Result<()> {
        let kind = WrapKind::Password;
        let verified = header::verify_wrap(&self.header, kind, old_password.as_bytes())?;
        if !verified {
            return Err(CoreError::Decrypt(aes_gcm::Error));
        }
        validate_password(new_password)?;
        self.persist_wrap(kind, new_password.as_bytes())
    }

    /// 忘记密码时用恢复密钥重新设密码。
    pub fn reset_password_with_recovery(&mut self, recovery_key: &str, new_password: &str) -> Result<()> {
        let kind = WrapKind::Recovery;
        let bytes = parse_recovery_key(recovery_key)?;
        if !header::verify_wrap(&self.header, kind, &bytes)? {
            return Err(CoreError::Decrypt(aes_gcm::Error));
        }
        validate_password(new_password)?;
        self.persist_wrap(WrapKind::Password, new_password.as_bytes())
    }

    /// 重新生成恢复密钥，旧的那枚立即作废。返回新密钥。
    pub fn regenerate_recovery_key(&mut self) -> Result<String> {
        let bytes = random_array::<RECOVERY_KEY_LEN>()?;
        self.persist_wrap(WrapKind::Recovery, &bytes)?;
        Ok(format_recovery_key(&bytes))
    }

    fn persist_wrap(&mut self, kind: WrapKind, secret: &[u8]) -> Result<()> {
        let old_header = self.header.clone();
        let result = self.replace_wrap(kind, secret).and_then(|()| self.save_header());
        if result.is_err() {
            self.header = old_header;
        }
        result
    }
}

/// 媒体文件的轻量句柄：只拿着主密钥和数据目录，不带笔记索引。
///
/// 导入大文件、解码、给外部播放器流式供数都可能在别的线程上跑，用一个只管媒体的句柄，
/// 这些线程就不会和界面线程争用整个库。可以克隆，主密钥析构时照样清零。
#[derive(Clone)]
pub struct BlobHandle {
    root: PathBuf,
    vault_id: String,
    master: MasterKey,
}

impl BlobHandle {
    fn blob_path(&self, blob_id: &str) -> PathBuf {
        self.root.join(BLOBS_DIR).join(format!("{blob_id}.blob"))
    }

    /// 导入一个媒体文件：分块加密后存进库，返回 (媒体标识, 原始字节数)。
    pub fn import(&self, mut reader: impl Read) -> Result<(String, u64)> {
        let blob_id = random_id()?;
        let key = crypto::derive_key(&self.master, &self.vault_id, "blob", &blob_id);
        let prefix = random_array::<4>()?;
        let path = self.blob_path(&blob_id);
        let tmp = sibling(&path, "tmp");
        let dir = self.root.join(BLOBS_DIR);
        fs::create_dir_all(&dir).map_err(|error| io_error(&dir, error))?;
        // 中途出任何错都把半成品删掉，库里不会留下残缺的媒体文件。
        let plain_len = match self.stream_blob_to(&tmp, &mut reader, &blob_id, &key, &prefix) {
            Ok(len) => len,
            Err(error) => {
                let _ = fs::remove_file(&tmp);
                return Err(error);
            }
        };
        if let Err(error) = fs::rename(&tmp, &path) {
            let _ = fs::remove_file(&tmp);
            return Err(io_error(&path, error));
        }
        Ok((blob_id, plain_len))
    }

    /// 边读边加密边写临时文件，返回原始字节数。
    fn stream_blob_to(
        &self,
        tmp: &Path,
        reader: &mut impl Read,
        blob_id: &str,
        key: &[u8; KEY_LEN],
        prefix: &[u8; 4],
    ) -> Result<u64> {
        let write = |file: &mut BufWriter<File>, bytes: &[u8]| -> Result<()> {
            file.write_all(bytes).map_err(|error| io_error(tmp, error))
        };
        let mut file = BufWriter::new(File::create(tmp).map_err(|error| io_error(tmp, error))?);
        write(&mut file, BLOB_MAGIC)?;
        write(&mut file, &(CHUNK_SIZE as u32).to_le_bytes())?;
        write(&mut file, prefix)?;
        write(&mut file, &0u64.to_le_bytes())?;

        let mut buffer = vec![0u8; CHUNK_SIZE];
        let mut plain_len: u64 = 0;
        let mut index: u64 = 0;
        loop {
            let count = read_fill(reader, &mut buffer)
                .map_err(|error| CoreError::Invalid(format!("读取待导入文件失败：{error}")))?;
            if count == 0 {
                break;
            }
            let aad = crypto::chunk_aad(&self.vault_id, blob_id, index, count);
            let ciphertext =
                crypto::encrypt(key, &crypto::chunk_nonce(prefix, index), &buffer[..count], &aad)?;
            write(&mut file, &ciphertext)?;
            plain_len += count as u64;
            index += 1;
        }
        // 回到头部把真实长度填上。
        file.seek(SeekFrom::Start(12)).map_err(|error| io_error(tmp, error))?;
        write(&mut file, &plain_len.to_le_bytes())?;
        let file = file.into_inner().map_err(|error| io_error(tmp, error.into_error()))?;
        file.sync_all().map_err(|error| io_error(tmp, error))?;
        Ok(plain_len)
    }

    /// 打开一个媒体文件，拿到可定位的解密读取器。
    pub fn open(&self, blob_id: &str) -> Result<BlobReader<BufReader<File>>> {
        check_id(blob_id)?;
        let path = self.blob_path(blob_id);
        let file = File::open(&path).map_err(|error| io_error(&path, error))?;
        let key = crypto::derive_key(&self.master, &self.vault_id, "blob", blob_id);
        BlobReader::open(BufReader::new(file), key, &self.vault_id, blob_id)
            .map_err(|error| io_error(&path, error))
    }

    /// 把一个媒体文件整个读进内存。图片这类小文件用。
    pub fn read(&self, blob_id: &str) -> Result<Vec<u8>> {
        let mut reader = self.open(blob_id)?;
        let mut out = Vec::new();
        reader.read_to_end(&mut out).map_err(|error| io_error(&self.blob_path(blob_id), error))?;
        Ok(out)
    }
}

mod header {
    //! 库头上的小工具，单独放一层方便复用。

    use super::{crypto, Result, VaultHeader, WrapKind};

    /// 验证口令能不能解开某一种包装。
    pub fn verify_wrap(header: &VaultHeader, kind: WrapKind, secret: &[u8]) -> Result<bool> {
        for wrap in header.wraps_of_kind(kind) {
            let wrapping_key = wrap.kdf.derive(secret)?;
            if crypto::unwrap_master_key(&wrapping_key, &header.vault_id, kind.label(), &wrap.wrapped).is_ok() {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

impl Drop for Vault {
    fn drop(&mut self) {
        // MasterKey 自己会清零；这里再把派生用不到的索引清掉，避免残留标题。
        self.index.clear();
    }
}
