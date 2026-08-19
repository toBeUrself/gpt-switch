mod account;
mod auth;

use anyhow::Result;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "gpt-switch")]
#[command(version = "1.0.0")]
#[command(about = "A simple Codex account switcher")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// 保存当前登录的 Codex 账号
    Add {
        /// 账号别名，例如 work / personal
        name: String,
    },

    /// 查看已经保存的账号
    List,

    /// 查看当前正在使用的账号
    Current,

    /// 切换到指定账号
    Use {
        /// 要切换到的账号别名
        name: String,
    },

    /// 删除一个已经保存的账号
    Remove {
        /// 要删除的账号别名
        name: String,
    },

    /// 准备登录一个新的 Codex 账号
    LoginNew,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Commands::Add { name } => {
            account::add_account(&name)?;
        }

        Commands::List => {
            account::list_accounts()?;
        }

        Commands::Current => {
            account::show_current_account()?;
        }

        Commands::Use { name } => {
            account::switch_account(&name)?;
        }

        Commands::Remove { name } => {
            account::remove_account(&name)?;
        }

        Commands::LoginNew => {
            account::login_new()?;
        }
    }

    Ok(())
}
