//! 核心库的错误类型。

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, thiserror::Error)]
pub enum CoreError {
    #[error("系统随机数不可用")]
    Random(#[from] getrandom::Error),
    #[error("密钥派生参数超出允许范围")]
    KdfParams,
    #[error("密钥派生失败")]
    Kdf(#[from] argon2::Error),
    #[error("加密失败")]
    Encrypt(aes_gcm::Error),
    #[error("解密失败：密钥不正确或文件已损坏")]
    Decrypt(#[from] aes_gcm::Error),
    #[error("文件格式无法识别：{0}")]
    BadFormat(String),
    #[error("读写文件失败：{path}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("数据内容无法解析")]
    Json(#[from] serde_json::Error),
    #[error("还没有创建笔记库")]
    VaultNotFound,
    #[error("这个库是用更新版本的 MOSS-Pocket 创建的（格式版本 {0}），请升级软件")]
    NewerVersion(u32),
    #[error("找不到这条笔记")]
    NoteNotFound,
    #[error("密码至少要 6 位")]
    PasswordTooShort,
    #[error("{0}")]
    Invalid(String),
}

impl CoreError {
    /// 把读写错误和出错的路径绑在一起。
    pub fn io(path: &std::path::Path, source: std::io::Error) -> Self {
        CoreError::Io {
            path: path.display().to_string(),
            source,
        }
    }
}
