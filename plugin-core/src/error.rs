use std::fmt;

#[derive(Debug)]
pub enum PluginError {
    LoadError(String),
    InitError(String),
    NotFound(String),
    AlreadyLoaded(String),
    VersionMismatch(String),
    Io(std::io::Error),
}

impl fmt::Display for PluginError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PluginError::LoadError(s) => write!(f, "加载失败: {}", s),
            PluginError::InitError(s) => write!(f, "初始化失败: {}", s),
            PluginError::NotFound(s) => write!(f, "未找到: {}", s),
            PluginError::AlreadyLoaded(s) => write!(f, "已加载: {}", s),
            PluginError::VersionMismatch(s) => write!(f, "版本不匹配: {}", s),
            PluginError::Io(e) => write!(f, "IO 错误: {}", e),
        }
    }
}

impl std::error::Error for PluginError {}

impl From<std::io::Error> for PluginError {
    fn from(e: std::io::Error) -> Self {
        PluginError::Io(e)
    }
}
