//! 中英文案。新增界面文字时在这里加一行，界面代码里用 `tr.xxx()` 取。

/// 实际使用的界面语言。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Zh,
    En,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tr(pub Lang);

macro_rules! texts {
    ($($name:ident => $zh:literal / $en:literal),* $(,)?) => {
        #[allow(dead_code)]
        impl Tr {
            $(pub fn $name(&self) -> &'static str {
                match self.0 { Lang::Zh => $zh, Lang::En => $en }
            })*
        }
    };
}

texts! {
    unlock_title => "解锁笔记库" / "Unlock your notes",
    unlock_sub => "输入密码打开你的想法" / "Enter your password to open your notes",
    unlock_recovery_sub => "输入你保存的恢复密钥" / "Enter the recovery key you saved",
    password => "密码" / "Password",
    password_hint => "输入密码" / "Enter password",
    recovery_key => "恢复密钥" / "Recovery key",
    recovery_key_hint => "输入恢复密钥" / "Enter recovery key",
    unlock => "解锁" / "Unlock",
    unlocking => "正在解锁…" / "Unlocking…",
    wrong_password => "密码不正确，请重试" / "Wrong password, try again",
    wrong_recovery_key => "恢复密钥不正确" / "Wrong recovery key",
    use_recovery_key => "使用恢复密钥" / "Use recovery key",
    use_password => "使用密码" / "Use password",
    vault_broken => "笔记库文件读不出来，无法解锁" / "The vault files cannot be read",

    setup_title => "设置密码" / "Set a password",
    setup_sub => "这个密码用来加密你的全部笔记。请务必牢记：没有它，任何人都打不开，包括开发者。" /
        "This password encrypts all your notes. Remember it: without it nobody can open them, not even the developer.",
    confirm_password => "再输一次" / "Repeat password",
    password_mismatch => "两次输入不一致" / "Passwords do not match",
    password_too_short => "密码至少要 6 位" / "Password must be at least 6 characters",
    create_vault => "创建笔记库" / "Create vault",
    creating => "正在创建…" / "Creating…",
    recovery_title => "保存好你的恢复密钥" / "Save your recovery key",
    recovery_sub => "忘记密码时，用它仍能打开笔记库。它只显示这一次，请抄下来或存到安全的地方。" /
        "If you forget the password, this key can still open the vault. It is shown only once, write it down or store it somewhere safe.",
    copy => "复制" / "Copy",
    copied => "已复制" / "Copied",
    saved_continue => "我已保存，继续" / "I saved it, continue",

    new_note => "新建" / "New",
    new_note_full => "新建笔记" / "New note",
    search_hint => "搜索笔记" / "Search notes",
    notes => "笔记" / "Notes",
    pinned_notes => "已置顶" / "Pinned",
    recent_notes => "按最近修改" / "Recently edited",
    trash => "回收站" / "Trash",
    back_to_notes => "返回笔记" / "Back to notes",
    empty_trash => "回收站是空的" / "Trash is empty",
    empty_notes => "还没有笔记。点「新建」记下第一个想法。" / "No notes yet. Click “New” to capture your first idea.",
    empty_page => "空白页" / "Blank page",
    no_note_open => "没有打开的笔记" / "No note open",
    trash_description => "可以在这里查看和恢复已删除的笔记。" / "View or restore deleted notes here.",
    no_note_selected => "从左侧选一条笔记，或新建一条" / "Pick a note on the left, or create a new one",
    untitled => "无标题" / "Untitled",
    new_note_status => "新笔记" / "New note",
    add_title => "添加标题" / "Add title",
    new_note_title => "给笔记起个标题" / "Give your note a title",
    editor => "编辑笔记" / "Note editor",
    find => "查找" / "Find",
    unsaved => "有未保存的更改" / "Unsaved changes",
    pin => "置顶" / "Pin",
    unpin => "取消置顶" / "Unpin",
    move_to_trash => "移到回收站" / "Move to trash",
    restore => "恢复" / "Restore",
    delete_forever => "彻底删除" / "Delete forever",
    lock => "锁定" / "Lock",
    settings => "设置" / "Settings",
    tags => "标签" / "Tags",
    tags_hint => "标签，用逗号分隔" / "Tags, use commas",
    note_deleted_hint => "这条笔记在回收站里。恢复后才能编辑。" / "This note is in the trash. Restore it to edit.",
    saved => "已保存" / "Saved",
    save_failed => "保存失败" / "Failed to save",
    save_before_close => "笔记仍未保存。应用已保持打开，请检查数据目录后重试。" / "Your note is unsaved. The app is still open. Check the data folder and try again.",
    import_in_progress => "正在导入媒体，完成后即可锁定或关闭。" / "Media is importing. You can lock or close the app when it finishes.",
    auto_locked => "闲置已久，已自动锁定" / "Locked after being idle",

    paragraph => "段落" / "Paragraph",
    heading1 => "标题一" / "Heading 1",
    heading2 => "标题二" / "Heading 2",
    heading3 => "标题三" / "Heading 3",
    bullet => "项目符号" / "Bullet list",
    numbered => "编号列表" / "Numbered list",
    quote => "引用" / "Quote",
    divider => "分割线" / "Divider",
    move_up => "上移" / "Move up",
    move_down => "下移" / "Move down",
    duplicate => "复制一块" / "Duplicate block",
    delete_block => "删除这一块" / "Delete block",
    insert_divider => "插入分割线" / "Insert divider",
    block_menu => "块菜单" / "Block menu",
    empty_block_hint => "写点什么…" / "Type something…",
    media_placeholder => "媒体块（即将支持）" / "Media block (coming soon)",

    today_at => "今天" / "Today",
    yesterday_at => "昨天" / "Yesterday",

    play => "播放" / "Play",
    pause => "暂停" / "Pause",
    stop => "停止" / "Stop",
    play_video => "播放视频" / "Play video",
    view_image => "查看图片" / "View image",
    open_audio_player => "打开播放器" / "Open player",
    rename_audio => "重命名音频" / "Rename audio",
    audio_renamed => "已重命名音频" / "Audio renamed",
    replay => "重播" / "Replay",
    loading_video => "正在载入视频…" / "Loading video…",
    video_startup_timed_out => "视频载入超时。关闭播放器后可重试。" / "The video took too long to load. Close the player to try again.",
    media_seek => "播放进度" / "Playback position",
    video_playback_failed => "无法播放此视频" / "Can't play this video",
    video_codec_hint => "请确认文件完整，或将视频转换为 H.264 编码的 MP4。" /
        "Check that the file is complete, or convert it to an H.264 MP4.",
    image_view_failed => "无法显示此图片" / "Can't show this image",
    save_as => "另存为" / "Save as…",
    insert => "插入" / "Insert",
    insert_image => "图片" / "Image",
    insert_audio => "音频" / "Audio",
    insert_video => "视频" / "Video",
    insert_media => "插入图片、音频或视频" / "Insert image, audio or video",
    media_filter => "图片、音频、视频" / "Images, audio and video",
    picker_insert_image => "插入图片" / "Insert image",
    picker_insert_audio => "插入音频" / "Insert audio",
    picker_insert_video => "插入视频" / "Insert video",
    picker_location => "位置" / "Location",
    picker_go => "前往文件夹" / "Go to folder",
    picker_parent => "上一级" / "Parent folder",
    picker_drives => "磁盘" / "Drives",
    picker_loading => "正在读取文件夹…" / "Loading folder…",
    picker_file_name => "文件名" / "File name",
    picker_all_files => "所有文件" / "All files",
    picker_open => "打开" / "Open",
    picker_save => "保存" / "Save",
    picker_cancel => "取消" / "Cancel",
    picker_replace => "替换" / "Replace",
    picker_keep_editing => "继续编辑" / "Keep editing",
    picker_replace_question => "要替换已有文件吗？" / "Replace the existing file?",
    picker_clear => "清除选择" / "Clear selection",
    picker_empty => "这里没有符合条件的项目" / "No matching items in this folder",
    picker_bad_folder => "无法打开文件夹" / "Can't open this folder",
    picker_partial_folder => "有些目录项无法读取" / "Some items in this folder couldn't be read",
    picker_bad_file_name => "请输入有效的文件名" / "Enter a valid file name",
    picker_bad_target => "无法保存到这个位置" / "Can't save to this location",
    picker_missing_file => "选中的文件已不可用" / "A selected file is no longer available",
    record_audio => "录音" / "Record",
    recording => "录音中" / "Recording",
    finish_recording => "完成" / "Finish",
    cancel_recording => "取消" / "Cancel",
    recording_failed => "录音失败" / "Recording failed",
    recording_limit_reached => "已达到录音上限，正在保存" / "Recording limit reached. Saving…",
    recording_file_name => "录音.wav" / "Recording.wav",
    copy_block => "复制这一块" / "Copy block",
    cut_block => "剪切这一块" / "Cut block",
    paste_block => "粘贴" / "Paste",
    loading_image => "正在载入图片…" / "Loading image…",
    image_resize_hint => "拖右下角缩放，双击恢复原大" / "Drag the corner to resize; double-click to reset",

    appearance => "外观" / "Appearance",
    typography => "排版" / "Typography",
    heading_font => "标题字体" / "Heading font",
    body_font => "正文字体" / "Body font",
    default_font => "应用默认" / "App default",
    font_unavailable => "这台电脑缺少所选字体，已使用默认字体。" / "The chosen font is unavailable on this PC. Using the default.",
    font_preview => "字体预览" / "Type preview",
    font_preview_title => "窗边手记" / "Notes by the window",
    font_preview_body => "把想法写下来，留一点空白。 Aa · 0123456789" / "Leave a little room for the next idea. Aa · 0123456789",
    theme => "主题" / "Theme",
    theme_system => "跟随系统" / "Follow system",
    theme_light => "浅色" / "Light",
    theme_dark => "深色" / "Dark",
    backdrop => "窗口背景" / "Window background",
    backdrop_opaque => "不透明" / "Opaque",
    backdrop_transparent => "纯半透明" / "Translucent",
    backdrop_acrylic => "磨砂玻璃（实时模糊）" / "Frosted glass (live blur)",
    backdrop_mica => "云母（取桌面壁纸）" / "Mica (desktop wallpaper)",
    backdrop_system_acrylic => "系统亚克力" / "System acrylic",
    tint_color => "底色" / "Tint",
    opacity => "不透明度" / "Opacity",
    accent_color => "强调色" / "Accent",
    corner_radius => "圆角" / "Corner radius",
    font_size => "笔记字号" / "Note text size",
    line_spacing => "行距" / "Line spacing",
    paragraph_spacing => "段间距" / "Paragraph spacing",
    language => "语言" / "Language",
    lang_system => "跟随系统" / "Follow system",
    security => "安全" / "Security",
    auto_lock => "闲置后自动锁定" / "Auto-lock when idle",
    auto_lock_off => "不自动锁定" / "Never",
    minutes => "分钟" / "minutes",
    change_password => "修改密码" / "Change password",
    old_password => "当前密码" / "Current password",
    new_password => "新密码" / "New password",
    apply => "应用" / "Apply",
    password_changed => "密码已修改" / "Password changed",
    regenerate_recovery => "重新生成恢复密钥" / "Regenerate recovery key",
    recovery_regenerated => "旧的恢复密钥已作废，请保存好这一枚。" / "The old key is now invalid. Save this one.",
    close => "关闭" / "Close",
    minimize_window => "最小化窗口" / "Minimize window",
    maximize_window => "最大化窗口" / "Maximize window",
    restore_window => "还原窗口" / "Restore window",
    close_window => "关闭窗口" / "Close window",
    export => "导出" / "Export",
    export_html => "网页 (HTML)" / "Web page (HTML)",
    export_docx => "Word 文档" / "Word document",
    export_done => "已导出" / "Exported",
    export_failed => "导出失败" / "Export failed",
    backdrop_acrylic_note => "磨砂会模糊窗口后的内容。系统无法提供材质时使用实色底。" /
        "Frosted glass blurs content behind the window. A solid surface is used when the material is unavailable.",

    find_placeholder => "在这条笔记里查找" / "Find in this note",
    replace_placeholder => "替换为" / "Replace with",
    find_match_case => "区分大小写" / "Match case",
    find_prev => "上一处" / "Previous",
    find_next => "下一处" / "Next",
    find_show_replace => "替换" / "Replace",
    find_replace => "换这一处" / "Replace one",
    find_replace_all => "全部替换" / "Replace all",
    find_none => "无结果" / "No results",

    err_random => "系统随机数不可用，无法继续" / "System random numbers are unavailable; cannot continue",
    err_kdf_params => "密钥派生参数超出允许范围" / "Key derivation parameters are out of range",
    err_kdf => "密钥派生失败" / "Key derivation failed",
    err_encrypt => "加密失败" / "Encryption failed",
    err_decrypt => "解密失败：密钥不正确，或文件已损坏" / "Decryption failed: wrong key, or the files are damaged",
    err_json => "数据内容无法解析" / "The data could not be parsed",
    err_vault_missing => "还没有创建笔记库" / "No vault has been created yet",
    err_note_missing => "找不到这条笔记" / "Note not found",
    err_password_short => "密码至少要 6 位" / "The password must be at least 6 characters",
    err_vault_locked => "笔记库已锁定" / "The vault is locked",
    err_password_mismatch => "两次输入的新密码不一致" / "The two new passwords don't match",
    err_audio_seek => "这段音频不能跳转" / "This audio can't seek",
    err_audio_decode => "这段音频没法播放：格式不支持，或文件已损坏" /
        "This audio can't be played: unsupported format or damaged file",
    err_pasted_image => "这张图片存不下来：尺寸不正常" / "This image can't be stored: invalid size",
    err_edge_missing => "没有找到 Microsoft Edge，无法生成 PDF。可以改用导出网页，再用浏览器打印。" /
        "Microsoft Edge was not found, so no PDF can be made. Export as a web page and print from a browser instead.",
    already_open => "这份数据已经被另一个 MOSS-Pocket 打开着。请先关掉那一个，再打开新的。" /
        "This data is already open in another MOSS-Pocket. Close that one first.",
    window_failed => "窗口没能创建" / "The window could not be created",
}

/// 带参数的文案：中英各拼一遍，取当前语言的那一份。
fn zh_or_en(tr: Tr, zh: String, en: String) -> String {
    match tr.0 {
        Lang::Zh => zh,
        Lang::En => en,
    }
}

pub fn picker_selected(tr: Tr, count: usize) -> String {
    zh_or_en(tr, format!("已选 {count} 项"), format!("{count} selected"))
}

pub fn unsupported_file(tr: Tr, name: &str) -> String {
    zh_or_en(tr, format!("不支持的文件类型：{name}"), format!("Unsupported file type: {name}"))
}

pub fn cannot_open_file(tr: Tr, name: &str, error: &str) -> String {
    zh_or_en(tr, format!("打不开文件 {name}：{error}"), format!("Could not open {name}: {error}"))
}

pub fn image_unreadable(tr: Tr, name: &str) -> String {
    zh_or_en(tr, format!("图片无法识别：{name}"), format!("Unrecognized image: {name}"))
}

pub fn file_io_failed(tr: Tr, path: &str) -> String {
    zh_or_en(tr, format!("读写文件失败：{path}"), format!("Could not read or write file: {path}"))
}

pub fn bad_format(tr: Tr, detail: &str) -> String {
    zh_or_en(tr, format!("文件格式无法识别：{detail}"), format!("Unrecognized file format: {detail}"))
}

pub fn newer_version(tr: Tr, version: u32) -> String {
    zh_or_en(
        tr,
        format!("这个库是用更新版本的 MOSS-Pocket 创建的（格式版本 {version}），请升级软件"),
        format!("This vault was created by a newer MOSS-Pocket (format version {version}); please update the app"),
    )
}

pub fn audio_device_failed(tr: Tr, error: &str) -> String {
    zh_or_en(tr, format!("音频设备打不开：{error}"), format!("The audio device could not be opened: {error}"))
}

pub fn audio_decode_failed(tr: Tr, error: &str) -> String {
    zh_or_en(tr, format!("{}（{error}）", tr.err_audio_decode()), format!("{} ({error})", tr.err_audio_decode()))
}

pub fn edge_pdf_failed(tr: Tr, code: i32) -> String {
    zh_or_en(tr, format!("Edge 没能生成 PDF（退出码 {code}）"), format!("Edge could not produce the PDF (exit code {code})"))
}

pub fn edge_failed_to_start(tr: Tr, error: &str) -> String {
    zh_or_en(tr, format!("Edge 没能启动：{error}"), format!("Edge could not be started: {error}"))
}

impl Tr {
    /// 核心库的错误，换成用户看得懂的话。
    pub fn core_error(&self, error: &mp_core::CoreError) -> String {
        use mp_core::CoreError;
        match error {
            CoreError::Random(_) => self.err_random().to_string(),
            CoreError::KdfParams => self.err_kdf_params().to_string(),
            CoreError::Kdf(_) => self.err_kdf().to_string(),
            CoreError::Encrypt(_) => self.err_encrypt().to_string(),
            CoreError::Decrypt(_) => self.err_decrypt().to_string(),
            CoreError::BadFormat(detail) => bad_format(*self, detail),
            CoreError::Io { path, .. } => file_io_failed(*self, path),
            CoreError::Json(_) => self.err_json().to_string(),
            CoreError::VaultNotFound => self.err_vault_missing().to_string(),
            CoreError::NewerVersion(version) => newer_version(*self, *version),
            CoreError::NoteNotFound => self.err_note_missing().to_string(),
            CoreError::PasswordTooShort => self.err_password_short().to_string(),
            CoreError::Invalid(text) => text.clone(),
        }
    }
}

/// 「已替换 N 处」。
pub fn replaced(tr: Tr, count: usize) -> String {
    zh_or_en(tr, format!("已替换 {count} 处"), format!("{count} replaced"))
}

/// 把时间戳排成本地的「日期 时间」。
pub fn format_time(tr: &Tr, unix_millis: u64, now_millis: u64) -> String {
    let (date, time) = crate::win::local_date_time(unix_millis);
    let (today, _) = crate::win::local_date_time(now_millis);
    // 粗略判断今天和昨天：同一日期，或相差不到 48 小时且日期不同。
    let label = if date == today {
        tr.today_at().to_string()
    } else if now_millis.abs_diff(unix_millis) < 48 * 3600 * 1000 {
        tr.yesterday_at().to_string()
    } else {
        date
    };
    format!("{label} {time}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_text_has_both_languages() {
        for lang in [Lang::Zh, Lang::En] {
            let tr = Tr(lang);
            assert!(!tr.unlock_title().is_empty());
            assert!(!tr.setup_sub().is_empty());
            assert!(!tr.empty_notes().is_empty());
        }
        assert_ne!(Tr(Lang::Zh).unlock_title(), Tr(Lang::En).unlock_title());
    }

    #[test]
    fn core_errors_are_explained_in_both_languages() {
        let (zh, en) = (Tr(Lang::Zh), Tr(Lang::En));
        assert_eq!(zh.core_error(&mp_core::CoreError::PasswordTooShort), "密码至少要 6 位");
        assert_eq!(en.core_error(&mp_core::CoreError::PasswordTooShort), "The password must be at least 6 characters");
        assert_eq!(
            en.core_error(&mp_core::CoreError::NewerVersion(7)),
            "This vault was created by a newer MOSS-Pocket (format version 7); please update the app"
        );
        // 核心库自己带的说明文字原样保留。
        assert_eq!(en.core_error(&mp_core::CoreError::Invalid("keep me".into())), "keep me");
        assert_eq!((replaced(zh, 3), replaced(en, 3)), ("已替换 3 处".to_string(), "3 replaced".to_string()));
        assert_ne!(unsupported_file(zh, "a.txt"), unsupported_file(en, "a.txt"));
    }
}
