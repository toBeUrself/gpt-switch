use crate::auth::{AuthFile, auth_access_token, auth_account_id};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageSnapshot {
    pub fetched_at: i64,
    pub plan_type: Option<String>,
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
    pub primary: Option<UsageWindow>,
    pub secondary: Option<UsageWindow>,
    pub credits: Option<CreditsSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UsageWindow {
    pub used_percent: f64,
    pub window_seconds: Option<u64>,
    pub resets_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CreditsSnapshot {
    pub unlimited: Option<bool>,
    pub balance: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UsageResponse {
    plan_type: Option<String>,
    rate_limit: Option<RateLimitResponse>,
    credits: Option<CreditsSnapshot>,
}

#[derive(Debug, Deserialize)]
struct RateLimitResponse {
    allowed: Option<bool>,
    limit_reached: Option<bool>,
    primary_window: Option<UsageWindowResponse>,
    secondary_window: Option<UsageWindowResponse>,
}

#[derive(Debug, Deserialize)]
struct UsageWindowResponse {
    used_percent: Option<f64>,
    limit_window_seconds: Option<u64>,
    reset_at: Option<i64>,
}

impl UsageResponse {
    fn into_snapshot(self, fetched_at: i64) -> UsageSnapshot {
        let rate_limit = self.rate_limit;

        UsageSnapshot {
            fetched_at,
            plan_type: self.plan_type,
            allowed: rate_limit.as_ref().and_then(|value| value.allowed),
            limit_reached: rate_limit.as_ref().and_then(|value| value.limit_reached),
            primary: rate_limit
                .as_ref()
                .and_then(|value| value.primary_window.as_ref())
                .and_then(UsageWindowResponse::to_snapshot),
            secondary: rate_limit
                .as_ref()
                .and_then(|value| value.secondary_window.as_ref())
                .and_then(UsageWindowResponse::to_snapshot),
            credits: self.credits,
        }
    }
}

impl UsageWindowResponse {
    fn to_snapshot(&self) -> Option<UsageWindow> {
        Some(UsageWindow {
            used_percent: self.used_percent?,
            window_seconds: self.limit_window_seconds,
            resets_at: self.reset_at,
        })
    }
}

pub fn fetch_usage(auth: &AuthFile) -> Result<UsageSnapshot> {
    fetch_usage_from_url(auth, USAGE_URL, unix_now())
}

fn fetch_usage_from_url(auth: &AuthFile, url: &str, fetched_at: i64) -> Result<UsageSnapshot> {
    let access_token = auth_access_token(auth).context("认证文件缺少 access_token")?;
    let account_id = auth_account_id(auth).context("认证文件缺少 account_id")?;

    let config = ureq::Agent::config_builder()
        .timeout_global(Some(REQUEST_TIMEOUT))
        .build();
    let agent: ureq::Agent = config.into();

    let mut response = agent
        .get(url)
        .header("Authorization", &format!("Bearer {access_token}"))
        .header("ChatGPT-Account-Id", account_id)
        .call()
        .map_err(|error| anyhow::anyhow!("额度接口请求失败: {error}"))?;

    let usage: UsageResponse = response
        .body_mut()
        .with_config()
        .limit(MAX_RESPONSE_BYTES)
        .read_json()
        .context("无法解析额度接口响应")?;

    Ok(usage.into_snapshot(fetched_at))
}

pub fn save_usage(path: &Path, usage: &UsageSnapshot) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建额度目录: {}", parent.display()))?;
    }

    let content = serde_json::to_vec_pretty(usage).context("无法序列化额度信息")?;
    let temp = temp_path_for(path)?;

    if temp.exists() {
        fs::remove_file(&temp)
            .with_context(|| format!("无法清理额度临时文件: {}", temp.display()))?;
    }

    fs::write(&temp, content)
        .with_context(|| format!("无法写入额度临时文件: {}", temp.display()))?;
    set_private_permissions(&temp)?;
    fs::rename(&temp, path).with_context(|| format!("无法保存额度文件: {}", path.display()))?;

    Ok(())
}

pub fn read_usage(path: &Path) -> Result<Option<UsageSnapshot>> {
    if !path.exists() {
        return Ok(None);
    }

    let content = fs::read_to_string(path)
        .with_context(|| format!("无法读取额度文件: {}", path.display()))?;
    let usage = serde_json::from_str(&content)
        .with_context(|| format!("无法解析额度文件: {}", path.display()))?;

    Ok(Some(usage))
}

pub fn format_remaining(window: Option<&UsageWindow>, now: i64) -> String {
    let Some(window) = window else {
        return "-".to_string();
    };

    if window.resets_at.is_some_and(|reset| reset <= now) {
        return "待刷新".to_string();
    }

    let remaining = (100.0 - window.used_percent).clamp(0.0, 100.0);

    format!("{remaining:.0}%")
}

pub fn format_reset(window: Option<&UsageWindow>, now: i64) -> String {
    let Some(resets_at) = window.and_then(|value| value.resets_at) else {
        return "-".to_string();
    };

    if resets_at <= now {
        return "待刷新".to_string();
    }

    format_duration((resets_at - now) as u64)
}

pub fn format_updated(fetched_at: i64, now: i64) -> String {
    if fetched_at >= now {
        return "刚刚".to_string();
    }

    format!("{}前", format_duration((now - fetched_at) as u64))
}

pub fn format_usable(usage: Option<&UsageSnapshot>, now: i64) -> String {
    let Some(usage) = usage else {
        return "-".to_string();
    };

    let reset_times: Vec<i64> = usage
        .primary
        .iter()
        .chain(usage.secondary.iter())
        .filter_map(|window| window.resets_at)
        .collect();
    let windows_are_stale = reset_times.iter().any(|reset| *reset <= now);

    if windows_are_stale {
        return "待刷新".to_string();
    }

    match (usage.allowed, usage.limit_reached) {
        (Some(false), _) | (_, Some(true)) => "NO".to_string(),
        (Some(true), _) | (_, Some(false)) => "YES".to_string(),
        _ => "-".to_string(),
    }
}

pub fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        return "<1分钟".to_string();
    }

    if seconds < 3600 {
        return format!("{}分钟", seconds / 60);
    }

    if seconds < 86_400 {
        let hours = seconds / 3600;
        let minutes = (seconds % 3600) / 60;

        return if minutes == 0 {
            format!("{hours}小时")
        } else {
            format!("{hours}小时{minutes}分")
        };
    }

    let days = seconds / 86_400;
    let hours = (seconds % 86_400) / 3600;

    if hours == 0 {
        format!("{days}天")
    } else {
        format!("{days}天{hours}小时")
    }
}

fn temp_path_for(target: &Path) -> Result<PathBuf> {
    let file_name = target
        .file_name()
        .context("额度目标路径缺少文件名")?
        .to_string_lossy();

    Ok(target.with_file_name(format!("{file_name}.tmp")))
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("无法读取额度文件权限: {}", path.display()))?
        .permissions();
    permissions.set_mode(0o600);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("无法设置额度文件权限: {}", path.display()))?;

    Ok(())
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "gpt-switch-usage-test-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn auth() -> AuthFile {
        serde_json::from_value(json!({
            "auth_mode": "chatgpt",
            "tokens": {
                "access_token": "test-token",
                "account_id": "account-123"
            }
        }))
        .unwrap()
    }

    fn sample_response() -> String {
        json!({
            "plan_type": "plus",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 25,
                    "limit_window_seconds": 18000,
                    "reset_at": 10_000
                },
                "secondary_window": {
                    "used_percent": 40.5,
                    "limit_window_seconds": 604800,
                    "reset_at": 20_000
                }
            },
            "credits": {
                "unlimited": false,
                "balance": "0"
            }
        })
        .to_string()
    }

    #[test]
    fn fetches_usage_with_expected_auth_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let body = sample_response();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0_u8; 4096];
            let size = stream.read(&mut request).unwrap();
            let request = String::from_utf8_lossy(&request[..size]);

            assert!(request.starts_with("GET /usage HTTP/1.1"));
            assert!(request.contains("authorization: Bearer test-token"));
            assert!(request.contains("chatgpt-account-id: account-123"));

            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });

        let usage =
            fetch_usage_from_url(&auth(), &format!("http://{address}/usage"), 1_000).unwrap();
        server.join().unwrap();

        assert_eq!(usage.plan_type.as_deref(), Some("plus"));
        assert_eq!(usage.primary.as_ref().unwrap().used_percent, 25.0);
        assert_eq!(usage.secondary.as_ref().unwrap().used_percent, 40.5);
        assert_eq!(usage.fetched_at, 1_000);
    }

    #[test]
    fn saves_and_reads_usage_snapshot() {
        let dir = TestDir::new();
        let path = dir.0.join("account").join("usage.json");
        let response: UsageResponse = serde_json::from_str(&sample_response()).unwrap();
        let expected = response.into_snapshot(1_000);

        save_usage(&path, &expected).unwrap();

        assert_eq!(read_usage(&path).unwrap(), Some(expected));
        assert!(!path.with_file_name("usage.json.tmp").exists());
    }

    #[test]
    fn formats_remaining_and_reset_countdown() {
        let window = UsageWindow {
            used_percent: 25.4,
            window_seconds: Some(18_000),
            resets_at: Some(10_000),
        };

        assert_eq!(format_remaining(Some(&window), 1_000), "75%");
        assert_eq!(format_reset(Some(&window), 1_000), "2小时30分");
    }

    #[test]
    fn marks_expired_snapshot_as_stale() {
        let window = UsageWindow {
            used_percent: 100.0,
            window_seconds: Some(18_000),
            resets_at: Some(999),
        };
        let usage = UsageSnapshot {
            fetched_at: 500,
            plan_type: Some("plus".to_string()),
            allowed: Some(false),
            limit_reached: Some(true),
            primary: Some(window.clone()),
            secondary: None,
            credits: None,
        };

        assert_eq!(format_remaining(Some(&window), 1_000), "待刷新");
        assert_eq!(format_reset(Some(&window), 1_000), "待刷新");
        assert_eq!(format_usable(Some(&usage), 1_000), "待刷新");
    }

    #[test]
    fn reached_limit_takes_precedence_over_allowed_flag() {
        let usage = UsageSnapshot {
            fetched_at: 500,
            plan_type: Some("plus".to_string()),
            allowed: Some(true),
            limit_reached: Some(true),
            primary: Some(UsageWindow {
                used_percent: 100.0,
                window_seconds: Some(18_000),
                resets_at: Some(10_000),
            }),
            secondary: None,
            credits: None,
        };

        assert_eq!(format_usable(Some(&usage), 1_000), "NO");
    }

    #[test]
    fn usable_status_is_stale_when_any_window_has_reset() {
        let usage = UsageSnapshot {
            fetched_at: 500,
            plan_type: Some("plus".to_string()),
            allowed: Some(false),
            limit_reached: Some(true),
            primary: Some(UsageWindow {
                used_percent: 100.0,
                window_seconds: Some(18_000),
                resets_at: Some(999),
            }),
            secondary: Some(UsageWindow {
                used_percent: 50.0,
                window_seconds: Some(604_800),
                resets_at: Some(20_000),
            }),
            credits: None,
        };

        assert_eq!(format_usable(Some(&usage), 1_000), "待刷新");
    }

    #[test]
    fn requires_access_token_and_account_id() {
        let missing: AuthFile = serde_json::from_value(json!({
            "auth_mode": "chatgpt",
            "tokens": {}
        }))
        .unwrap();

        assert!(
            fetch_usage_from_url(&missing, "http://127.0.0.1:1", 1_000)
                .unwrap_err()
                .to_string()
                .contains("access_token")
        );
    }
}
