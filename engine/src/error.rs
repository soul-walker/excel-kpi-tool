//! 引擎错误类型。
//!
//! 库内错误使用 thiserror 定义；应用/入口层（GUI、CLI）可再包 anyhow。

/// 引擎统一错误。
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// 底层 IO 错误（文件读写、目录访问）。
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    /// 方案 JSON 解析错误。
    #[error("方案 JSON 解析失败: {0}")]
    Json(#[from] serde_json::Error),

    /// 方案 schema 版本不受支持。
    #[error("不支持的方案 schemaVersion={0}（当前支持 v{1}）")]
    UnsupportedSchema(u32, u32),

    /// 方案静态校验失败（含多条问题，见 message）。
    #[error("方案校验失败: {0}")]
    InvalidScheme(String),

    /// 运行期数据/引用问题。
    #[error("{0}")]
    Runtime(String),

    /// 尚未实现（占位，避免静默错算）。
    #[error("尚未实现: {0}")]
    NotImplemented(&'static str),
}

/// 引擎返回结果的别名。
pub type Result<T> = std::result::Result<T, Error>;
