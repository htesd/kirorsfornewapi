//! 请求日志配置

use serde::{Deserialize, Serialize};

/// 日志记录模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogMode {
    /// 记录所有请求（成功 + 失败）
    All,
    /// 仅记录失败请求
    ErrorsOnly,
}

impl Default for LogMode {
    fn default() -> Self {
        Self::ErrorsOnly
    }
}

/// 请求日志配置
///
/// 在 config.json 的 `requestLog` 节点下配置：
///
/// ```json
/// {
///   "requestLog": {
///     "enabled": true,
///     "dbPath": "kiro_requests.db",
///     "mode": "all",
///     "maxRecords": 2000,
///     "saveRequestBody": true,
///     "saveResponseBodyOnError": true
///   }
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestLogConfig {
    /// 是否启用请求日志（默认 true）
    #[serde(default = "default_enabled")]
    pub enabled: bool,

    /// SQLite 文件路径（默认 `kiro_requests.db`）
    #[serde(default = "default_db_path")]
    pub db_path: String,

    /// 记录模式：仅错误 / 全部
    #[serde(default)]
    pub mode: LogMode,

    /// 表中最多保留的记录数（默认 2000）
    #[serde(default = "default_max_records")]
    pub max_records: usize,

    /// 是否保存请求体原文（默认 true）
    #[serde(default = "default_save_request_body")]
    pub save_request_body: bool,

    /// 出错时是否保存响应体原文（默认 true）
    #[serde(default = "default_save_response_body_on_error")]
    pub save_response_body_on_error: bool,
}

fn default_enabled() -> bool {
    true
}
fn default_db_path() -> String {
    "kiro_requests.db".to_string()
}
fn default_max_records() -> usize {
    2000
}
fn default_save_request_body() -> bool {
    true
}
fn default_save_response_body_on_error() -> bool {
    true
}

impl Default for RequestLogConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            db_path: default_db_path(),
            mode: LogMode::default(),
            max_records: default_max_records(),
            save_request_body: default_save_request_body(),
            save_response_body_on_error: default_save_response_body_on_error(),
        }
    }
}

impl RequestLogConfig {
    /// 给定记录的成功/失败状态，判定是否应当写入
    pub fn should_record(&self, success: bool) -> bool {
        if !self.enabled {
            return false;
        }
        match self.mode {
            LogMode::All => true,
            LogMode::ErrorsOnly => !success,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_mode_is_errors_only() {
        assert_eq!(LogMode::default(), LogMode::ErrorsOnly);
    }

    #[test]
    fn should_record_when_disabled_returns_false() {
        let cfg = RequestLogConfig {
            enabled: false,
            ..Default::default()
        };
        assert!(!cfg.should_record(true));
        assert!(!cfg.should_record(false));
    }

    #[test]
    fn errors_only_mode_records_only_failures() {
        let cfg = RequestLogConfig {
            mode: LogMode::ErrorsOnly,
            ..Default::default()
        };
        assert!(!cfg.should_record(true));
        assert!(cfg.should_record(false));
    }

    #[test]
    fn all_mode_records_everything() {
        let cfg = RequestLogConfig {
            mode: LogMode::All,
            ..Default::default()
        };
        assert!(cfg.should_record(true));
        assert!(cfg.should_record(false));
    }

    #[test]
    fn deserialize_from_json() {
        let json = r#"{
            "enabled": true,
            "dbPath": "/tmp/test.db",
            "mode": "all",
            "maxRecords": 5000,
            "saveRequestBody": false,
            "saveResponseBodyOnError": false
        }"#;
        let cfg: RequestLogConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.db_path, "/tmp/test.db");
        assert_eq!(cfg.mode, LogMode::All);
        assert_eq!(cfg.max_records, 5000);
        assert!(!cfg.save_request_body);
        assert!(!cfg.save_response_body_on_error);
    }

    #[test]
    fn deserialize_missing_fields_uses_defaults() {
        let json = "{}";
        let cfg: RequestLogConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.enabled);
        assert_eq!(cfg.mode, LogMode::ErrorsOnly);
        assert_eq!(cfg.max_records, 2000);
    }
}
