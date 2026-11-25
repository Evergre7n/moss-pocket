//! 库文件读写的集成测试：真实落盘，跑在系统临时目录里，用完即删。

use mp_core::crypto::CHUNK_SIZE;
use mp_core::model::{Block, Note};
use mp_core::vault::{UnlockSecret, Vault};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const PASSWORD: &str = "正确的马电池钉";
const NEW_PASSWORD: &str = "换过的口令了";
const DAY: u64 = 24 * 60 * 60 * 1000;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!("moss-pocket-test-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

fn sample_note(marker: &str) -> Note {
    let mut note = Note::new().unwrap();
    note.blocks[0].as_text_mut().unwrap().text = format!("{marker}\n第二行正文");
    note.tags = vec!["灵感".into()];
    note
}

fn all_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut queue = vec![root.to_path_buf()];
    while let Some(dir) = queue.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                queue.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64
}

#[test]
fn create_unlock_with_password_or_recovery_and_reject_wrong_ones() {
    let dir = TempDir::new("create");
    assert!(Vault::create(&dir, "12345").is_err());
    let (vault, recovery) = Vault::create(&dir, PASSWORD).unwrap();
    assert_eq!(recovery.len(), 31);
    drop(vault);

    let mut vault = Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).unwrap();
    let note = sample_note("第一条想法");
    let id = note.id.clone();
    vault.save_note(&note).unwrap();
    drop(vault);

    assert!(Vault::unlock(&dir, UnlockSecret::Password("错误的马电池钉")).is_err());
    assert!(Vault::unlock(&dir, UnlockSecret::RecoveryKey("不是恢复密钥")).is_err());
    assert!(Vault::unlock(&dir, UnlockSecret::RecoveryKey("AAAAA-AAAAA-AAAAA-AAAAA-AAAAAA")).is_err());

    // 恢复密钥不区分大小写。
    let mut vault = Vault::unlock(&dir, UnlockSecret::RecoveryKey(&recovery.to_lowercase())).unwrap();
    assert_eq!(vault.summaries().len(), 1);
    assert_eq!(vault.summaries()[0].title, "第一条想法");
    assert_eq!(vault.load_note(&id).unwrap().tags, ["灵感"]);
    assert_eq!(vault.load_note("00000000000000000000000000000000").unwrap_err().to_string(), "找不到这条笔记");
}

#[test]
fn files_on_disk_are_ciphertext() {
    let dir = TempDir::new("cipher");
    let (mut vault, _recovery) = Vault::create(&dir, PASSWORD).unwrap();
    let marker = "绝密想法XYZ不可见";
    vault.save_note(&sample_note(marker)).unwrap();
    let blob_data: Vec<u8> = (0..9000).map(|i| (i % 7) as u8).collect();
    let (blob_id, len) = vault.import_blob(&blob_data[..]).unwrap();
    assert_eq!(len, 9000);
    assert!(dir.join("blobs").join(format!("{blob_id}.blob")).exists());
    drop(vault);

    for path in all_files(&dir) {
        let bytes = std::fs::read(&path).unwrap();
        assert!(!bytes.windows(9).any(|w| w == marker.as_bytes()), "{} 里有明文", path.display());
        if path.extension().is_some_and(|e| e == "blob") {
            assert!(!bytes.windows(64).any(|w| w == &blob_data[..64]), "媒体文件里有明文");
        }
    }
    // 库头是明文 JSON，但里面不能出现密码本身。
    let header = std::fs::read_to_string(dir.join("vault.json")).unwrap();
    assert!(!header.contains(PASSWORD));
    assert!(header.contains("argon2id") || header.contains("m_cost_kib"));
}

#[test]
fn corrupt_note_falls_back_to_previous_version() {
    let dir = TempDir::new("backup");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let mut note = sample_note("第一版");
    let id = note.id.clone();
    vault.save_note(&note).unwrap();
    note.blocks[0].as_text_mut().unwrap().text = "第二版".into();
    vault.save_note(&note).unwrap();
    assert_eq!(vault.load_note(&id).unwrap().title(), "第二版");

    // 把主文件改坏：应当退回上一版，而不是报错丢数据。
    let path = dir.join("notes").join(format!("{id}.note"));
    let mut bytes = std::fs::read(&path).unwrap();
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xFF;
    std::fs::write(&path, &bytes).unwrap();
    assert_eq!(vault.load_note(&id).unwrap().title(), "第一版");

    // 备份也坏了才报错。
    let backup = dir.join("notes").join(format!("{id}.note.prev"));
    std::fs::write(&backup, b"garbage").unwrap();
    assert!(vault.load_note(&id).is_err());
}

#[test]
fn index_is_rebuilt_when_missing_or_corrupt() {
    let dir = TempDir::new("index");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let first = sample_note("甲");
    let mut second = sample_note("乙");
    // 让「乙」比「甲」新，顺序才是确定的。
    second.created_at += 10;
    second.updated_at += 10;
    vault.save_note(&first).unwrap();
    vault.save_note(&second).unwrap();
    drop(vault);

    std::fs::remove_file(dir.join("index.enc")).unwrap();
    let vault = Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).unwrap();
    assert_eq!(vault.summaries().len(), 2);

    std::fs::write(dir.join("index.enc"), b"not an index").unwrap();
    let vault = Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).unwrap();
    let summaries = vault.summaries();
    let titles: Vec<&str> = summaries.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["乙", "甲"]);
}

#[test]
fn blob_roundtrip_seek_and_tamper_detection() {
    let dir = TempDir::new("blob");
    let (vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let data: Vec<u8> = (0..CHUNK_SIZE * 2 + 12_345).map(|i| (i % 251) as u8).collect();
    let (blob_id, len) = vault.import_blob(&data[..]).unwrap();
    assert_eq!(len as usize, data.len());
    assert_eq!(vault.read_blob(&blob_id).unwrap(), data);

    let mut reader = vault.open_blob(&blob_id).unwrap();
    assert_eq!(reader.len(), len);
    // 从第一块尾部跨到第二块开头。
    reader.seek(SeekFrom::Start(CHUNK_SIZE as u64 - 5)).unwrap();
    let mut buffer = [0u8; 10];
    reader.read_exact(&mut buffer).unwrap();
    assert_eq!(&buffer[..], &data[CHUNK_SIZE - 5..CHUNK_SIZE + 5]);
    // 从结尾倒着定位。
    reader.seek(SeekFrom::End(-3)).unwrap();
    let mut tail = Vec::new();
    reader.read_to_end(&mut tail).unwrap();
    assert_eq!(tail, data[data.len() - 3..]);
    assert!(reader.seek(SeekFrom::Current(-1_000_000_000)).is_err());
    reader.seek(SeekFrom::Start(u64::MAX)).unwrap();
    assert!(reader.seek(SeekFrom::Current(1)).is_err());

    // 改动中间一个字节：解密时认证失败。
    drop(reader);
    let path = dir.join("blobs").join(format!("{blob_id}.blob"));
    let mut bytes = std::fs::read(&path).unwrap();
    let mid = 20 + CHUNK_SIZE + 40;
    bytes[mid] ^= 0x01;
    std::fs::write(&path, &bytes).unwrap();
    assert!(vault.read_blob(&blob_id).is_err());
}

/// 读了若干字节后报错的读取器，用来验证导入中途失败不会留下残缺文件。
struct FailingReader {
    remaining_before_failure: usize,
}

impl Read for FailingReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining_before_failure == 0 {
            return Err(std::io::Error::other("模拟的读取失败"));
        }
        let count = out.len().min(self.remaining_before_failure).min(64 * 1024);
        out[..count].fill(0xAB);
        self.remaining_before_failure -= count;
        Ok(count)
    }
}

#[test]
fn failed_import_leaves_no_partial_blob() {
    let dir = TempDir::new("partial");
    let (vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let reader = FailingReader {
        remaining_before_failure: CHUNK_SIZE + 5000,
    };
    assert!(vault.import_blob(reader).is_err());
    let leftover = std::fs::read_dir(dir.join("blobs")).unwrap().count();
    assert_eq!(leftover, 0);
}

#[test]
fn garbage_collection_removes_only_orphans_and_never_after_corruption() {
    let dir = TempDir::new("gc");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let (used, _) = vault.import_blob(&b"in use"[..]).unwrap();
    let mut note = Note::new().unwrap();
    note.blocks.push(Block::Divider { id: 7 });
    note.blocks[0] = Block::Image(mp_core::model::ImageBlock {
        id: 8,
        blob_id: used.clone(),
        width: 1,
        height: 1,
        display_width: None,
    });
    vault.save_note(&note).unwrap();
    let (orphan, _) = vault.import_blob(&b"orphan"[..]).unwrap();
    vault.collect_garbage().unwrap();
    assert!(dir.join("blobs").join(format!("{used}.blob")).exists());
    assert!(!dir.join("blobs").join(format!("{orphan}.blob")).exists());

    // 有笔记解不开时，什么都不删。
    let (another, _) = vault.import_blob(&b"another orphan"[..]).unwrap();
    let path = dir.join("notes").join(format!("{}.note", note.id));
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    std::fs::write(&path, &bytes).unwrap();
    std::fs::write(dir.join("notes").join(format!("{}.note.prev", note.id)), b"broken too").unwrap();
    assert!(vault.collect_garbage().is_err());
    assert!(dir.join("blobs").join(format!("{another}.blob")).exists());
    assert!(dir.join("blobs").join(format!("{used}.blob")).exists());
}

#[test]
fn change_password_keeps_data_and_recovery_key() {
    let dir = TempDir::new("password");
    let (mut vault, recovery) = Vault::create(&dir, PASSWORD).unwrap();
    let note = sample_note("改密码前的内容");
    let id = note.id.clone();
    vault.save_note(&note).unwrap();
    assert!(vault.change_password("错的旧密码", NEW_PASSWORD).is_err());
    assert!(vault.change_password(PASSWORD, "123").is_err());
    vault.change_password(PASSWORD, NEW_PASSWORD).unwrap();
    drop(vault);

    assert!(Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).is_err());
    let mut vault = Vault::unlock(&dir, UnlockSecret::Password(NEW_PASSWORD)).unwrap();
    assert_eq!(vault.load_note(&id).unwrap().title(), "改密码前的内容");
    assert!(vault.reset_password_with_recovery("AAAAA-AAAAA-AAAAA-AAAAA-AAAAAA", PASSWORD).is_err());
    vault.reset_password_with_recovery(&recovery, PASSWORD).unwrap();
    let fresh = vault.regenerate_recovery_key().unwrap();
    drop(vault);

    assert!(Vault::unlock(&dir, UnlockSecret::RecoveryKey(&recovery)).is_err());
    let vault = Vault::unlock(&dir, UnlockSecret::RecoveryKey(&fresh)).unwrap();
    assert_eq!(vault.summaries().len(), 1);
    assert!(Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).is_ok());
}

#[test]
fn blob_handle_can_be_moved_to_another_thread() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<mp_core::vault::BlobHandle>();

    let dir = TempDir::new("handle");
    let (vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let handle = vault.blob_handle();
    // 库本身已经释放，句柄照样能用：后台线程导入大文件时不占着整个库。
    drop(vault);
    let payload = b"from another thread";
    let imported = std::thread::spawn(move || handle.import(&payload[..])).join().unwrap();
    let (blob_id, len) = imported.unwrap();
    assert_eq!(len as usize, payload.len());

    let vault = Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).unwrap();
    assert_eq!(vault.read_blob(&blob_id).unwrap(), payload);
    // 媒体类型判定和句柄配合：认得出的文件才导入。
    assert_eq!(mp_core::media::classify("song.mp3"), Some(mp_core::media::MediaKind::Audio));
    assert_eq!(mp_core::media::classify("song.txt"), None);
}

#[test]
fn trash_keeps_notes_until_retention_expires() {
    let dir = TempDir::new("trash");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let note = sample_note("要删的想法");
    let id = note.id.clone();
    vault.save_note(&note).unwrap();

    let mut loaded = vault.load_note(&id).unwrap();
    loaded.deleted_at = Some(now() - 29 * DAY);
    vault.save_note(&loaded).unwrap();
    assert_eq!(vault.purge_trash(now()).unwrap(), 0);
    assert_eq!(vault.summaries().len(), 1);

    loaded.deleted_at = Some(now() - 31 * DAY);
    vault.save_note(&loaded).unwrap();
    assert_eq!(vault.purge_trash(now()).unwrap(), 1);
    assert!(vault.summaries().is_empty());
    assert!(matches!(vault.load_note(&id), Err(mp_core::CoreError::NoteNotFound)));
    assert!(!dir.join("notes").join(format!("{id}.note")).exists());
    assert!(!dir.join("notes").join(format!("{id}.note.prev")).exists());
}

#[test]
fn ids_cannot_escape_their_directories() {
    let dir = TempDir::new("ids");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    assert!(vault.load_note("../../vault").is_err());
    assert!(vault.load_note("0000000000000000000000000000000A").is_err());
    assert!(vault.read_blob("..").is_err());
    assert!(vault.save_note(&{
        let mut note = sample_note("x");
        note.id = "../evil".into();
        note
    })
    .is_err());
}

#[test]
fn saving_is_durable_and_sorted() {
    let dir = TempDir::new("sorted");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let older = sample_note("旧");
    let mut newer = sample_note("新");
    newer.created_at = older.created_at + 5_000;
    newer.updated_at = older.updated_at + 5_000;
    let mut pinned = sample_note("置顶");
    pinned.pinned = true;
    pinned.updated_at = older.updated_at - 9_000;
    for note in [&older, &newer, &pinned] {
        vault.save_note(note).unwrap();
    }
    let summaries = vault.summaries();
    let titles: Vec<&str> = summaries.iter().map(|s| s.title.as_str()).collect();
    assert_eq!(titles, ["置顶", "新", "旧"]);

    // 写盘确实落了：重新解锁还在。
    drop(vault);
    let vault = Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).unwrap();
    let summaries = vault.summaries();
    assert_eq!(summaries.iter().map(|s| s.title.clone()).collect::<Vec<_>>(), ["置顶", "新", "旧"]);
}

#[test]
fn creating_again_preserves_existing_vault() {
    let dir = TempDir::new("no-overwrite");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let note = sample_note("保留原库");
    vault.save_note(&note).unwrap();
    let header = std::fs::read(dir.join("vault.json")).unwrap();
    assert!(Vault::create(&dir, NEW_PASSWORD).is_err());
    assert_eq!(std::fs::read(dir.join("vault.json")).unwrap(), header);
    let mut reopened = Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).unwrap();
    assert_eq!(reopened.load_note(&note.id).unwrap().title(), "保留原库");
}

#[test]
fn previous_only_header_and_note_recover_after_interrupted_rename() {
    let dir = TempDir::new("rename-gap");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let note = sample_note("完整旧版本");
    vault.save_note(&note).unwrap();
    drop(vault);
    std::fs::rename(dir.join("vault.json"), dir.join("vault.json.prev")).unwrap();
    let notes = dir.join("notes");
    std::fs::rename(notes.join(format!("{}.note", note.id)), notes.join(format!("{}.note.prev", note.id))).unwrap();
    std::fs::remove_file(dir.join("index.enc")).unwrap();
    assert!(Vault::create(&dir, NEW_PASSWORD).is_err());
    let mut reopened = Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).unwrap();
    assert_eq!(reopened.summaries().len(), 1);
    assert_eq!(reopened.load_note(&note.id).unwrap().title(), "完整旧版本");
    reopened.destroy_note(&note.id).unwrap();
    assert!(reopened.summaries().is_empty());
    reopened.rebuild_index().unwrap();
    assert!(reopened.summaries().is_empty());
}

#[test]
fn stale_index_cannot_hide_durable_note_or_its_media() {
    let dir = TempDir::new("stale-index");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let old_index = std::fs::read(dir.join("index.enc")).unwrap();
    let (blob_id, _) = vault.import_blob(&b"durable media"[..]).unwrap();
    let mut note = sample_note("写完笔记但索引未更新");
    note.blocks.push(Block::Image(mp_core::model::ImageBlock {
        id: 20, blob_id: blob_id.clone(), width: 1, height: 1, display_width: None,
    }));
    vault.save_note(&note).unwrap();
    std::fs::write(dir.join("index.enc"), old_index).unwrap();
    drop(vault);
    let mut reopened = Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).unwrap();
    assert_eq!(reopened.summaries().len(), 1);
    reopened.collect_garbage().unwrap();
    assert_eq!(reopened.read_blob(&blob_id).unwrap(), b"durable media");
}

#[test]
fn garbage_collection_preserves_backup_media_and_active_imports() {
    let dir = TempDir::new("gc-backup");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let (blob_id, _) = vault.import_blob(&b"previous media"[..]).unwrap();
    let mut note = sample_note("旧版本的媒体");
    note.blocks.push(Block::Image(mp_core::model::ImageBlock {
        id: 20, blob_id: blob_id.clone(), width: 1, height: 1, display_width: None,
    }));
    vault.save_note(&note).unwrap();
    note.blocks.pop();
    vault.save_note(&note).unwrap();
    let active_tmp = dir.join("blobs").join("in-progress.blob.tmp");
    std::fs::write(&active_tmp, b"still importing").unwrap();
    vault.collect_garbage().unwrap();
    assert!(active_tmp.exists());
    assert_eq!(vault.read_blob(&blob_id).unwrap(), b"previous media");
    std::fs::remove_file(dir.join("notes").join(format!("{}.note", note.id))).unwrap();
    assert_eq!(vault.load_note(&note.id).unwrap().blob_ids(), [blob_id.as_str()]);
}

#[test]
fn malformed_blob_lengths_are_rejected_before_reading_or_allocating() {
    let dir = TempDir::new("blob-length");
    let (vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let (id, _) = vault.import_blob(&b"original bytes"[..]).unwrap();
    let path = dir.join("blobs").join(format!("{id}.blob"));
    let original = std::fs::read(&path).unwrap();
    for len in [0u64, u64::MAX] {
        let mut bytes = original.clone();
        bytes[12..20].copy_from_slice(&len.to_le_bytes());
        std::fs::write(&path, bytes).unwrap();
        assert!(vault.open_blob(&id).is_err());
    }
    std::fs::write(&path, &original[..original.len() - 1]).unwrap();
    assert!(vault.open_blob(&id).is_err());
    let (empty, _) = vault.import_blob(&b""[..]).unwrap();
    assert!(vault.read_blob(&empty).unwrap().is_empty());
}

#[test]
fn failed_password_write_keeps_in_memory_credentials() {
    let dir = TempDir::new("password-rollback");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let blocked_tmp = dir.join("vault.json.tmp");
    std::fs::create_dir(&blocked_tmp).unwrap();
    assert!(vault.change_password(PASSWORD, NEW_PASSWORD).is_err());
    std::fs::remove_dir(&blocked_tmp).unwrap();
    // This would reject PASSWORD if the failed attempt had changed memory only.
    vault.change_password(PASSWORD, NEW_PASSWORD).unwrap();
    assert!(Vault::unlock(&dir, UnlockSecret::Password(NEW_PASSWORD)).is_ok());
}

#[test]
fn stale_trash_summary_cannot_destroy_a_restored_note() {
    let dir = TempDir::new("trash-cache");
    let (mut vault, _) = Vault::create(&dir, PASSWORD).unwrap();
    let mut note = sample_note("已恢复");
    note.deleted_at = Some(now() - 31 * DAY);
    vault.save_note(&note).unwrap();
    let stale_index = std::fs::read(dir.join("index.enc")).unwrap();
    note.deleted_at = None;
    vault.save_note(&note).unwrap();
    std::fs::write(dir.join("index.enc"), stale_index).unwrap();
    drop(vault);
    let mut reopened = Vault::unlock(&dir, UnlockSecret::Password(PASSWORD)).unwrap();
    assert_eq!(reopened.purge_trash(now()).unwrap(), 0);
    assert_eq!(reopened.load_note(&note.id).unwrap().deleted_at, None);
}
