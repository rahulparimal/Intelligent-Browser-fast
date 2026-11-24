//! `intelli-rust`: search your documents and manage the index, from a single small binary.

mod browse;
mod commands;
mod provider;

use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "intelli-rust", version, about = "Local search, admin UI and browser agent in one small binary")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Open the search home page (the default when no command is given)
    Ui(commands::UiArgs),
    /// Open the search settings and admin page
    Admin(commands::UiArgs),
    /// Add files or folders to the search index (txt, md, html, pdf, docx)
    Index(commands::IndexArgs),
    /// Restrict documents to certain users or groups (list, set, clear)
    Acl(commands::AclArgs),
    /// Search your documents from the terminal
    Search(commands::SearchArgs),
    /// Score search quality on a file of judged queries (keyword, fuzzy, dense, hybrid)
    Eval(commands::EvalArgs),
    /// Do a task in a real browser from plain English
    Run(browse::RunArgs),
    /// Show what the model would see for a page, and how many tokens that is
    Distill(browse::DistillArgs),
    /// Check the installation
    Doctor,
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("INTELLI_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let rt = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: could not start the async runtime: {e}");
            return ExitCode::FAILURE;
        }
    };
    let result = rt.block_on(async {
        match cli.command.unwrap_or(Command::Ui(commands::UiArgs::default())) {
            Command::Ui(a) => commands::ui(a, "/").await,
            Command::Admin(a) => commands::ui(a, "/admin").await,
            Command::Index(a) => commands::index(&a),
            Command::Acl(a) => commands::acl(&a),
            Command::Search(a) => commands::search(&a),
            Command::Eval(a) => commands::eval(&a),
            Command::Run(a) => browse::run(a).await,
            Command::Distill(a) => browse::distill_cmd(a).await,
            Command::Doctor => commands::doctor(),
        }
    });
    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
