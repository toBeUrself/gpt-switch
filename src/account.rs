use crate::auth::{AuthFile, auth_email, auth_mode, copy_auth_atomically, read_auth_from};
use crate::usage::{
    UsageSnapshot, fetch_usage, format_countdown, format_remaining, format_reset_time,
    format_updated, format_usable, read_usage, save_usage, unix_now,
};

use anyhow::{Context, Result};
use comfy_table::Table;
use std::fs;
use std::path::PathBuf;

fn codex_home() -> Result<PathBuf> {
    let home = dirs::home_dir().context("无法获取当前用户的 Home 目录")?;

    Ok(home.join(".codex"))
}

fn auth_path() -> Result<PathBuf> {
    Ok(codex_home()?.join("auth.json"))
}

fn accounts_dir() -> Result<PathBuf> {
    Ok(codex_home()?.join("gpt-switch").join("accounts"))
}

fn account_dir(name: &str) -> Result<PathBuf> {
    Ok(accounts_dir()?.join(name))
}

fn saved_auth_path(name: &str) -> Result<PathBuf> {
    Ok(account_dir(name)?.join("auth.json"))
}

fn saved_usage_path(name: &str) -> Result<PathBuf> {
    Ok(account_dir(name)?.join("usage.json"))
}

fn login_backup_path() -> Result<PathBuf> {
    Ok(codex_home()?.join("gpt-switch").join("login-backup.json"))
}

fn validate_account_name(name: &str) -> Result<()> {
    if name.is_empty() {
        anyhow::bail!("账号名称不能为空");
    }

    let valid = name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');

    if !valid {
        anyhow::bail!("账号名称只能包含字母、数字、- 和 _");
    }

    Ok(())
}

fn find_saved_account_by_email(email: &str) -> Result<Option<String>> {
    let dir = accounts_dir()?;

    if !dir.exists() {
        return Ok(None);
    }

    for entry in
        fs::read_dir(&dir).with_context(|| format!("无法读取账号目录: {}", dir.display()))?
    {
        let entry = entry?;

        if !entry.file_type()?.is_dir() {
            continue;
        }

        let saved_auth_path = entry.path().join("auth.json");

        if !saved_auth_path.exists() {
            continue;
        }

        let saved_auth = read_auth_from(&saved_auth_path)?;

        let Some(saved_email) = auth_email(&saved_auth)? else {
            continue;
        };

        if saved_email.eq_ignore_ascii_case(email) {
            let name = entry.file_name().to_string_lossy().to_string();

            return Ok(Some(name));
        }
    }

    Ok(None)
}

fn sync_current_account() -> Result<(String, AuthFile)> {
    let current_path = auth_path()?;

    if !current_path.exists() {
        anyhow::bail!("当前没有找到 Codex 登录文件: {}", current_path.display());
    }

    let current_auth = read_auth_from(&current_path)?;

    let email = auth_email(&current_auth)?.context("无法识别当前 Codex 账号邮箱")?;

    let saved_name = find_saved_account_by_email(&email)?.with_context(|| {
        format!("当前账号 `{email}` 还没有保存，请先执行 `gpt-switch add <name>`")
    })?;

    let saved_path = saved_auth_path(&saved_name)?;

    copy_auth_atomically(&current_path, &saved_path)?;

    Ok((saved_name, current_auth))
}

fn refresh_usage(name: &str, auth: &AuthFile) -> Result<UsageSnapshot> {
    let usage = fetch_usage(auth)?;
    save_usage(&saved_usage_path(name)?, &usage)?;

    Ok(usage)
}

fn try_refresh_usage(name: &str, auth: &AuthFile) -> Option<UsageSnapshot> {
    match refresh_usage(name, auth) {
        Ok(usage) => Some(usage),
        Err(error) => {
            eprintln!("警告: 无法刷新账号 `{name}` 的额度信息: {error:#}");
            None
        }
    }
}

fn cached_usage(name: &str) -> Option<UsageSnapshot> {
    let path = match saved_usage_path(name) {
        Ok(path) => path,
        Err(error) => {
            eprintln!("警告: 无法确定账号 `{name}` 的额度文件路径: {error:#}");
            return None;
        }
    };

    match read_usage(&path) {
        Ok(usage) => usage,
        Err(error) => {
            eprintln!("警告: 无法读取账号 `{name}` 的额度缓存: {error:#}");
            None
        }
    }
}

pub fn show_current_account() -> Result<()> {
    let path = auth_path()?;

    println!("Codex auth 文件路径:");
    println!("{}", path.display());

    if !path.exists() {
        println!("状态: 文件不存在");
        println!("当前账号: 未登录");
        return Ok(());
    }

    let auth = read_auth_from(&path)?;

    println!("状态: 文件存在");

    match auth_mode(&auth) {
        Some(mode) => {
            println!("认证方式: {mode}");
        }

        None => {
            println!("认证方式: 未知");
        }
    }

    match auth_email(&auth)? {
        Some(email) => {
            println!("当前账号: {email}");
        }

        None => {
            println!("当前账号: 无法识别");
        }
    }

    Ok(())
}

pub fn add_account(name: &str) -> Result<()> {
    validate_account_name(name)?;

    let source = auth_path()?;

    if !source.exists() {
        anyhow::bail!("当前没有找到 Codex 登录文件: {}", source.display());
    }

    let current_auth = read_auth_from(&source)?;

    let email = auth_email(&current_auth)?.context("无法从当前 Codex 登录信息中识别邮箱")?;

    let target_dir = account_dir(name)?;
    let target = target_dir.join("auth.json");

    if target.exists() {
        anyhow::bail!("账号名称 `{name}` 已经存在");
    }

    if let Some(existing_name) = find_saved_account_by_email(&email)? {
        anyhow::bail!("当前邮箱 `{email}` 已经保存为 `{existing_name}`");
    }

    fs::create_dir_all(&target_dir)
        .with_context(|| format!("无法创建账号目录: {}", target_dir.display()))?;

    copy_auth_atomically(&source, &target).with_context(|| format!("无法保存账号 `{name}`"))?;

    let usage = try_refresh_usage(name, &current_auth);

    println!("已保存账号");
    println!("Alias: {name}");
    println!("Email: {email}");
    if usage.is_some() {
        println!("额度信息: 已更新");
    }

    Ok(())
}

pub fn list_accounts(refresh: bool) -> Result<()> {
    let dir = accounts_dir()?;

    if !dir.exists() {
        println!("还没有保存任何账号");
        return Ok(());
    }

    let current_path = auth_path()?;

    let current_auth = if current_path.exists() {
        Some(read_auth_from(&current_path)?)
    } else {
        None
    };
    let current_email = current_auth.as_ref().map(auth_email).transpose()?.flatten();

    let mut accounts: Vec<(String, Option<String>, bool, Option<UsageSnapshot>)> = Vec::new();

    for entry in
        fs::read_dir(&dir).with_context(|| format!("无法读取账号目录: {}", dir.display()))?
    {
        let entry = entry?;

        if !entry.file_type()?.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().to_string();

        let saved_path = entry.path().join("auth.json");

        let saved_auth = if saved_path.exists() {
            Some(read_auth_from(&saved_path)?)
        } else {
            None
        };
        let saved_email = saved_auth.as_ref().map(auth_email).transpose()?.flatten();

        let is_current = match (&current_email, &saved_email) {
            (Some(current), Some(saved)) => current.eq_ignore_ascii_case(saved),

            _ => false,
        };

        let usage = if refresh {
            let refresh_auth = if is_current {
                current_auth.as_ref().or(saved_auth.as_ref())
            } else {
                saved_auth.as_ref()
            };

            refresh_auth
                .and_then(|auth| try_refresh_usage(&name, auth))
                .or_else(|| cached_usage(&name))
        } else {
            cached_usage(&name)
        };

        accounts.push((name, saved_email, is_current, usage));
    }

    accounts.sort_by(|a, b| a.0.cmp(&b.0));

    if accounts.is_empty() {
        println!("还没有保存任何账号");
        return Ok(());
    }

    let mut table = Table::new();

    table.set_header(vec![
        "NAME",
        "EMAIL",
        "PLAN",
        "PRIMARY LEFT",
        "PRIMARY RESET",
        "PRIMARY COUNTDOWN",
        "SECONDARY LEFT",
        "SECONDARY RESET",
        "SECONDARY COUNTDOWN",
        "USABLE",
        "UPDATED",
        "CURRENT",
    ]);

    let now = unix_now();

    for (name, email, is_current, usage) in accounts {
        table.add_row(vec![
            name,
            email.unwrap_or_else(|| "-".to_string()),
            usage
                .as_ref()
                .and_then(|value| value.plan_type.clone())
                .unwrap_or_else(|| "-".to_string()),
            format_remaining(usage.as_ref().and_then(|value| value.primary.as_ref()), now),
            format_reset_time(usage.as_ref().and_then(|value| value.primary.as_ref()), now),
            format_countdown(usage.as_ref().and_then(|value| value.primary.as_ref()), now),
            format_remaining(
                usage.as_ref().and_then(|value| value.secondary.as_ref()),
                now,
            ),
            format_reset_time(
                usage.as_ref().and_then(|value| value.secondary.as_ref()),
                now,
            ),
            format_countdown(
                usage.as_ref().and_then(|value| value.secondary.as_ref()),
                now,
            ),
            format_usable(usage.as_ref(), now),
            usage
                .as_ref()
                .map(|value| format_updated(value.fetched_at, now))
                .unwrap_or_else(|| "-".to_string()),
            if is_current {
                "*".to_string()
            } else {
                String::new()
            },
        ]);
    }

    println!("{table}");

    Ok(())
}

pub fn switch_account(name: &str) -> Result<()> {
    validate_account_name(name)?;

    let source = saved_auth_path(name)?;

    if !source.exists() {
        anyhow::bail!("没有找到已保存账号 `{name}`");
    }

    // 切换之前先确认目标账号正常
    let target_auth =
        read_auth_from(&source).with_context(|| format!("账号 `{name}` 的 auth.json 无效"))?;

    let target_email =
        auth_email(&target_auth)?.with_context(|| format!("无法识别账号 `{name}` 的邮箱"))?;

    let current_path = auth_path()?;
    let mut target_usage_refreshed = false;

    // 如果当前有登录账号，
    // 先同步当前最新 credential。
    if current_path.exists() {
        let (current_name, current_auth) = sync_current_account()?;

        try_refresh_usage(&current_name, &current_auth);
        target_usage_refreshed = current_name == name;

        println!("已同步当前账号: {current_name}");
    } else {
        println!("当前没有活动中的 Codex 账号");

        println!("将直接恢复已保存账号 `{name}`");
    }

    copy_auth_atomically(&source, &current_path)?;

    // 切换后再次验证
    let switched_auth = read_auth_from(&current_path)?;

    let switched_email = auth_email(&switched_auth)?.context("切换后无法识别账号邮箱")?;

    if !switched_email.eq_ignore_ascii_case(&target_email) {
        anyhow::bail!("账号切换验证失败，预期 `{target_email}`，实际 `{switched_email}`");
    }

    if !target_usage_refreshed {
        try_refresh_usage(name, &switched_auth);
    }

    println!();
    println!("已切换账号");
    println!("Alias: {name}");
    println!("Email: {target_email}");
    println!("请完全退出并重新打开 Codex App / CLI");

    Ok(())
}

pub fn remove_account(name: &str) -> Result<()> {
    validate_account_name(name)?;

    let dir = account_dir(name)?;
    let saved_path = dir.join("auth.json");

    if !dir.exists() {
        anyhow::bail!("没有找到已保存账号 `{name}`");
    }

    if !saved_path.exists() {
        anyhow::bail!("账号 `{name}` 缺少 auth.json，目录可能已经损坏");
    }

    let saved_auth = read_auth_from(&saved_path)?;

    let saved_email =
        auth_email(&saved_auth)?.with_context(|| format!("无法识别账号 `{name}` 的邮箱"))?;

    let current_path = auth_path()?;

    if current_path.exists() {
        let current_auth = read_auth_from(&current_path)?;

        if let Some(current_email) = auth_email(&current_auth)?
            && current_email.eq_ignore_ascii_case(&saved_email)
        {
            anyhow::bail!("账号 `{name}` 当前正在使用，不能删除。请先切换到其他账号");
        }
    }

    fs::remove_dir_all(&dir).with_context(|| format!("无法删除账号目录: {}", dir.display()))?;

    println!("已删除账号");
    println!("Alias: {name}");
    println!("Email: {saved_email}");

    Ok(())
}

pub fn login_new() -> Result<()> {
    let current_path = auth_path()?;

    if !current_path.exists() {
        anyhow::bail!("当前没有 auth.json，可能已经处于等待登录新账号的状态");
    }

    let current_auth = read_auth_from(&current_path)?;

    let email = auth_email(&current_auth)?.context("无法识别当前 Codex 账号邮箱")?;

    let saved_name = find_saved_account_by_email(&email)?.with_context(|| {
        format!("当前账号 `{email}` 还没有保存，请先执行 `gpt-switch add <name>`")
    })?;

    // 保存当前最新 token
    let (_, current_auth) = sync_current_account()?;
    try_refresh_usage(&saved_name, &current_auth);

    // 再做一份额外备份
    let backup_path = login_backup_path()?;

    copy_auth_atomically(&current_path, &backup_path)?;

    // 这里只删除本地活动文件，
    // 不调用 Codex logout。
    fs::remove_file(&current_path)
        .with_context(|| format!("无法移除当前认证文件: {}", current_path.display()))?;

    println!("当前账号已经安全保存");
    println!("Alias: {saved_name}");
    println!("Email: {email}");

    println!();
    println!("已移除当前 ~/.codex/auth.json");
    println!("没有执行 Codex logout");

    println!();
    println!("接下来：");
    println!("1. 完全退出 Codex App / CLI");
    println!("2. 重新打开 Codex");
    println!("3. 登录新的 ChatGPT 账号");
    println!("4. 执行 `gpt-switch current` 确认邮箱");
    println!("5. 执行 `gpt-switch add <name>` 保存新账号");

    println!();
    println!("如果不想登录新账号，可以直接执行：");
    println!("gpt-switch use {saved_name}");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::validate_account_name;

    #[test]
    fn accepts_safe_account_names() {
        for name in ["work", "personal-2", "team_alpha", "A1"] {
            assert!(validate_account_name(name).is_ok(), "name: {name}");
        }
    }

    #[test]
    fn rejects_empty_account_name() {
        let error = validate_account_name("").unwrap_err();

        assert!(error.to_string().contains("不能为空"));
    }

    #[test]
    fn rejects_path_traversal_and_unsupported_characters() {
        for name in ["../work", "work/personal", "work account", "工作"] {
            assert!(validate_account_name(name).is_err(), "name: {name}");
        }
    }
}
