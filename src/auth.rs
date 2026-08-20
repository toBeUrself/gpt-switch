use anyhow::{Context, Result};
use base64::{
    Engine as _,
    engine::general_purpose::{URL_SAFE, URL_SAFE_NO_PAD},
};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
pub struct AuthFile {
    auth_mode: Option<String>,
    account_id: Option<String>,
    tokens: Option<Tokens>,
}

#[derive(Debug, Deserialize)]
struct Tokens {
    id_token: Option<String>,
    access_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct IdTokenClaims {
    email: Option<String>,
}

pub fn read_auth_from(path: &Path) -> Result<AuthFile> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("无法读取认证文件: {}", path.display()))?;

    let auth: AuthFile = serde_json::from_str(&content)
        .with_context(|| format!("无法解析认证文件: {}", path.display()))?;

    Ok(auth)
}

pub fn auth_mode(auth: &AuthFile) -> Option<&str> {
    auth.auth_mode.as_deref()
}

pub fn auth_account_id(auth: &AuthFile) -> Option<&str> {
    auth.account_id.as_deref()
}

pub fn auth_access_token(auth: &AuthFile) -> Option<&str> {
    auth.tokens.as_ref()?.access_token.as_deref()
}

pub fn auth_email(auth: &AuthFile) -> Result<Option<String>> {
    let Some(tokens) = auth.tokens.as_ref() else {
        return Ok(None);
    };

    let Some(id_token) = tokens.id_token.as_deref() else {
        return Ok(None);
    };

    email_from_id_token(id_token)
}

/// 从 id_token 的 JWT payload 中读取 email。
///
/// 注意：这里只用于本地识别账号，不做 JWT 签名验证，
/// 不能用于服务器认证、权限判断或安全授权。
fn email_from_id_token(id_token: &str) -> Result<Option<String>> {
    let parts: Vec<&str> = id_token.split('.').collect();

    if parts.len() != 3 {
        anyhow::bail!("id_token 不是有效的 JWT 格式");
    }

    let payload = parts[1];

    let decoded = URL_SAFE_NO_PAD
        .decode(payload)
        .or_else(|_| URL_SAFE.decode(payload))
        .context("无法解码 id_token payload")?;

    let claims: IdTokenClaims =
        serde_json::from_slice(&decoded).context("无法解析 id_token payload")?;

    Ok(claims.email)
}

pub fn copy_auth_atomically(source: &Path, target: &Path) -> Result<()> {
    // 复制前先确认源文件有效
    read_auth_from(source).with_context(|| format!("源认证文件无效: {}", source.display()))?;

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录: {}", parent.display()))?;
    }

    let temp = temp_path_for(target)?;

    // 清理之前异常退出留下的临时文件
    if temp.exists() {
        fs::remove_file(&temp).with_context(|| format!("无法清理临时文件: {}", temp.display()))?;
    }

    // 先复制到临时文件，不直接覆盖正式文件
    fs::copy(source, &temp).with_context(|| {
        format!(
            "无法复制认证文件: {} -> {}",
            source.display(),
            temp.display()
        )
    })?;

    set_private_permissions(&temp)?;

    // 再检查复制出来的内容
    read_auth_from(&temp).with_context(|| format!("临时认证文件无效: {}", temp.display()))?;

    // macOS/Linux 同一文件系统中的 rename
    // 可以避免直接写坏正式 auth.json
    fs::rename(&temp, target).with_context(|| format!("无法替换认证文件: {}", target.display()))?;

    Ok(())
}

fn temp_path_for(target: &Path) -> Result<PathBuf> {
    let file_name = target
        .file_name()
        .context("目标路径缺少文件名")?
        .to_string_lossy();

    Ok(target.with_file_name(format!("{file_name}.tmp")))
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = fs::metadata(path)
        .with_context(|| format!("无法读取文件权限: {}", path.display()))?
        .permissions();

    permissions.set_mode(0o600);

    fs::set_permissions(path, permissions)
        .with_context(|| format!("无法设置文件权限: {}", path.display()))?;

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
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

    struct TestDir(PathBuf);

    impl TestDir {
        fn new() -> Self {
            let sequence = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "gpt-switch-auth-test-{}-{sequence}",
                std::process::id()
            ));

            fs::create_dir(&path).expect("failed to create test directory");

            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn id_token_with_email(email: &str, padded: bool) -> String {
        let header = URL_SAFE_NO_PAD.encode(br#"{"alg":"none"}"#);
        let claims = serde_json::to_vec(&json!({ "email": email })).unwrap();
        let payload = if padded {
            URL_SAFE.encode(claims)
        } else {
            URL_SAFE_NO_PAD.encode(claims)
        };

        format!("{header}.{payload}.signature")
    }

    fn auth_json(email: &str) -> String {
        json!({
            "auth_mode": "chatgpt",
            "account_id": "account-123",
            "tokens": {
                "id_token": id_token_with_email(email, false),
                "access_token": "preserved-secret"
            },
            "unknown_future_field": true
        })
        .to_string()
    }

    #[test]
    fn extracts_email_from_unpadded_id_token() {
        let token = id_token_with_email("user@example.com", false);

        assert_eq!(
            email_from_id_token(&token).unwrap(),
            Some("user@example.com".to_string())
        );
    }

    #[test]
    fn extracts_email_from_padded_id_token() {
        let token = id_token_with_email("padded@example.com", true);

        assert_eq!(
            email_from_id_token(&token).unwrap(),
            Some("padded@example.com".to_string())
        );
    }

    #[test]
    fn rejects_token_with_wrong_number_of_parts() {
        let error = email_from_id_token("header.payload").unwrap_err();

        assert!(error.to_string().contains("JWT 格式"));
    }

    #[test]
    fn rejects_token_with_invalid_payload() {
        let error = email_from_id_token("header.not-base64!.signature").unwrap_err();

        assert!(error.to_string().contains("解码 id_token payload"));
    }

    #[test]
    fn returns_none_when_auth_has_no_tokens() {
        let auth: AuthFile = serde_json::from_str(r#"{"auth_mode":"api_key"}"#).unwrap();

        assert_eq!(auth_mode(&auth), Some("api_key"));
        assert_eq!(auth_email(&auth).unwrap(), None);
    }

    #[test]
    fn reads_account_id_and_access_token() {
        let auth: AuthFile = serde_json::from_str(&auth_json("user@example.com")).unwrap();

        assert_eq!(auth_account_id(&auth), Some("account-123"));
        assert_eq!(auth_access_token(&auth), Some("preserved-secret"));
    }

    #[test]
    fn atomic_copy_preserves_complete_auth_file() {
        let dir = TestDir::new();
        let source = dir.path().join("source.json");
        let target = dir.path().join("nested").join("auth.json");
        let content = auth_json("copy@example.com");
        fs::write(&source, &content).unwrap();

        copy_auth_atomically(&source, &target).unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), content);
        assert!(!target.with_file_name("auth.json.tmp").exists());
    }

    #[test]
    fn invalid_source_does_not_replace_existing_target() {
        let dir = TestDir::new();
        let source = dir.path().join("invalid.json");
        let target = dir.path().join("auth.json");
        let original = auth_json("original@example.com");
        fs::write(&source, "not-json").unwrap();
        fs::write(&target, &original).unwrap();

        assert!(copy_auth_atomically(&source, &target).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), original);
    }

    #[cfg(unix)]
    #[test]
    fn atomic_copy_sets_private_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TestDir::new();
        let source = dir.path().join("source.json");
        let target = dir.path().join("auth.json");
        fs::write(&source, auth_json("private@example.com")).unwrap();

        copy_auth_atomically(&source, &target).unwrap();

        let mode = fs::metadata(target).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }
}
