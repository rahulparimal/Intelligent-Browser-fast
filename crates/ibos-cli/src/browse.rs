//! `run` (the agent) and `distill` (what the model would see), the two commands that use a browser.

use std::io::{IsTerminal, Write as _};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Args;
use ibos_agent::{Agent, AgentConfig, ConfirmMode, Guard, GuardConfig, RunReport, StopReason};
use ibos_cdp::{Browser, LaunchOptions};
use ibos_ctx::observe::render_full;
use ibos_ctx::{ApproxTokenizer, Tokenizer};
use ibos_dom::{DistillOptions, distill};
use ibos_mem::StepArena;
use ibos_search::paths::config_dir;

use crate::provider::resolve;

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    /// What to do, in plain English
    task: String,
    /// `provider[:model]` (anthropic, openai, ollama, compat). Default: from your API keys
    #[arg(long)]
    model: Option<String>,
    /// Endpoint for ollama / compat / openai-compatible servers
    #[arg(long)]
    base_url: Option<String>,
    /// Run without a visible browser window
    #[arg(long)]
    headless: bool,
    #[arg(long, default_value_t = 30)]
    max_steps: u32,
    /// Stop when the task has used this many tokens (prompt + completion)
    #[arg(long, default_value_t = 200_000)]
    budget: u64,
    /// Only browse these domains (repeatable; `*.example.com` allowed)
    #[arg(long = "allow-domain")]
    allow_domain: Vec<String>,
    /// Never browse these domains (repeatable)
    #[arg(long = "block-domain")]
    block_domain: Vec<String>,
    /// `risky`: ask before purchases, deletions and sends (denied when there is no terminal). `off`: never ask
    #[arg(long, value_enum, default_value_t = Confirm::Risky)]
    confirm: Confirm,
    /// Where to write the audit log (default: the config directory)
    #[arg(long)]
    audit_log: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
enum Confirm {
    Risky,
    Off,
}

#[derive(Debug, Args)]
pub(crate) struct DistillArgs {
    /// A URL, or the path of an HTML file
    target: String,
    /// Include everything: elements below the viewport and every repeated item
    #[arg(long)]
    full: bool,
    /// Show the browser window
    #[arg(long)]
    headed: bool,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
}

fn tokenizer() -> Arc<dyn Tokenizer> {
    #[cfg(feature = "exact-tokens")]
    if let Ok(t) = ibos_ctx::tokens::OpenAiTokenizer::o200k() {
        return Arc::new(t);
    }
    Arc::new(ApproxTokenizer)
}

fn ask_on_terminal(reason: &str) -> bool {
    if !std::io::stdin().is_terminal() {
        return false;
    }
    eprint!("\n[guardrail] The agent {reason}. Allow? [y/N] ");
    let _ = std::io::stderr().flush();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).is_ok() && matches!(line.trim().to_lowercase().as_str(), "y" | "yes")
}

fn default_audit_path() -> PathBuf {
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
    config_dir().join("audit").join(format!("{secs}.jsonl"))
}

pub(crate) async fn run(a: RunArgs) -> Result<ExitCode> {
    let resolved = resolve(a.model.as_deref(), a.base_url.as_deref(), &|k| std::env::var(k).ok())?;
    let browser = Browser::launch(LaunchOptions { headless: a.headless, ..LaunchOptions::default() })
        .await
        .context("could not start a browser")?;
    let page = browser.new_page().await?;
    let guard = Guard::new(
        GuardConfig {
            allowed_domains: a.allow_domain,
            blocked_domains: a.block_domain,
            confirm_mode: if matches!(a.confirm, Confirm::Off) { ConfirmMode::Off } else { ConfirmMode::Risky },
            audit_path: Some(a.audit_log.unwrap_or_else(default_audit_path)),
        },
        Box::new(ask_on_terminal),
    );
    let cfg = AgentConfig {
        model: resolved.model.clone(),
        max_steps: a.max_steps,
        token_budget: a.budget,
        tokenizer: Some(tokenizer()),
        on_step: Some(Arc::new(|s| {
            eprintln!("step {}: {} -> {}", s.n, s.action, s.outcome.lines().next().unwrap_or(""));
        })),
        ..AgentConfig::default()
    };
    eprintln!("model: {}:{}", resolved.provider, resolved.model);
    let report = Agent::new(page, resolved.llm, cfg, guard).run(&a.task).await;
    print_report(&report);
    browser.close().await;
    Ok(if report.success { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}

fn print_report(r: &RunReport) {
    match &r.stop {
        StopReason::Done if r.success => println!("{}", r.answer),
        StopReason::Done => println!("(the agent reported it could not complete the task) {}", r.answer),
        StopReason::MaxSteps => println!("(stopped: step limit reached)"),
        StopReason::Budget => println!("(stopped: token budget reached)"),
        StopReason::TooManyInvalidReplies => {
            println!("(stopped: the model kept sending replies that are not valid actions)");
        }
        StopReason::ModelError(e) => println!("(stopped: the model call failed: {e})"),
        StopReason::BrowserError(e) => println!("(stopped: the browser failed: {e})"),
    }
    let u = r.usage.total();
    eprintln!(
        "tokens: prompt={} (cached={}, {:.0}%) completion={} over {} calls; {} blocked by guardrails",
        u.prompt,
        u.cached_prompt,
        r.usage.cache_hit_rate() * 100.0,
        u.completion,
        r.usage.calls(),
        r.denied
    );
}

fn target_url(target: &str) -> Result<String> {
    if target.contains("://") || target.starts_with("data:") || target == "about:blank" {
        return Ok(target.to_owned());
    }
    let abs =
        Path::new(target).canonicalize().with_context(|| format!("{target} is neither a URL nor an existing file"))?;
    // `Url::from_file_path` gets Windows drive letters and escaping right (`file:///C:/dir/page.html`)
    url::Url::from_file_path(&abs)
        .map(String::from)
        .map_err(|()| anyhow::anyhow!("{} cannot be turned into a file URL", abs.display()))
}

pub(crate) async fn distill_cmd(a: DistillArgs) -> Result<ExitCode> {
    let url = target_url(&a.target)?;
    let browser = Browser::launch(LaunchOptions { headless: !a.headed, ..LaunchOptions::default() })
        .await
        .context("could not start a browser")?;
    let page = browser.new_page().await?;
    page.navigate(&url).await?;
    let obs = page.observe_raw().await?;
    let opts = if a.full {
        DistillOptions {
            keep_repeat: usize::MAX,
            include_offscreen: true,
            max_elements: usize::MAX,
            ..DistillOptions::default()
        }
    } else {
        DistillOptions::default()
    };
    let arena = StepArena::new();
    let t0 = std::time::Instant::now();
    let d = distill(
        &obs.url,
        &obs.title,
        obs.ax.get(),
        Some(obs.dom.get()),
        (obs.viewport.width, obs.viewport.height),
        &opts,
        &arena,
    )?;
    let took = t0.elapsed();
    let text = render_full(&d.snapshot);
    let tok = tokenizer();
    let tokens = tok.count(&text);
    if a.json {
        let elements: Vec<_> =
            d.snapshot.elems.iter().map(|(h, e)| serde_json::json!({"h": h, "role": e.role, "text": e.text})).collect();
        let out = serde_json::json!({"url": d.snapshot.url, "title": d.snapshot.title, "more_below": d.snapshot.more_below, "elements": elements,
            "stats": {"input_bytes": d.stats.input_bytes, "ax_nodes": d.stats.ax_nodes, "kept": d.stats.kept, "collapsed_items": d.stats.collapsed_items, "below_viewport": d.stats.below_viewport,
                      "chars": text.len(), "tokens": tokens, "tokenizer": tok.name(), "distill_micros": took.as_micros()}});
        println!("{}", serde_json::to_string_pretty(&out)?);
    } else {
        println!("{text}");
        eprintln!(
            "raw: {} bytes from {} accessibility nodes -> distilled: {} chars, {tokens} tokens ({}), {} elements, {} collapsed, {} below the viewport; distilled in {took:?}",
            d.stats.input_bytes,
            d.stats.ax_nodes,
            text.len(),
            tok.name(),
            d.stats.kept,
            d.stats.collapsed_items,
            d.stats.below_viewport
        );
    }
    browser.close().await;
    Ok(ExitCode::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::target_url;

    #[test]
    fn urls_pass_through_and_files_become_file_urls() {
        assert_eq!(target_url("https://example.com/a?b=1").unwrap(), "https://example.com/a?b=1");
        assert_eq!(target_url("about:blank").unwrap(), "about:blank");
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("my page.html");
        std::fs::write(&f, "x").unwrap();
        let u = target_url(f.to_str().unwrap()).unwrap();
        assert!(u.starts_with("file:///") && u.ends_with("my%20page.html"), "{u}");
        assert!(target_url("/no/such/page.html").is_err());
    }
}
