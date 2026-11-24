use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Args;
use ibos_mem::MemoryBudget;
use ibos_retrieval::eval::{Config, Judged, evaluate, table};
use ibos_retrieval::{Embedder, HashingEmbedder, Mode, Query, RemoteEmbedder, Retriever};
use ibos_search::{IndexOptions, LiteStore, paths};
use ibos_server::ServeOptions;

#[derive(Debug, Args, Default)]
pub(crate) struct UiArgs {
    /// Port for the local page (0 picks a free one)
    #[arg(long, default_value_t = 8765)]
    port: u16,
    /// Do not open a browser tab
    #[arg(long)]
    no_open: bool,
    /// Quit shortly after the browser tab is closed
    #[arg(long)]
    exit_when_closed: bool,
    /// Show documents restricted to this user or group (repeatable). Without it the page sees only unrestricted documents.
    #[arg(long = "as")]
    principals: Vec<String>,
}

#[derive(Debug, clap::Subcommand)]
pub(crate) enum AclCommand {
    /// List restricted documents and who may see them
    List,
    /// Restrict a document to the given users or groups (none = hidden from everyone)
    Set { path: PathBuf, principals: Vec<String> },
    /// Remove a document's restriction
    Clear { path: PathBuf },
}

#[derive(Debug, Args)]
pub(crate) struct AclArgs {
    #[command(subcommand)]
    command: AclCommand,
}

#[derive(Debug, Args)]
pub(crate) struct IndexArgs {
    /// Files or folders to index
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Re-read every file even if unchanged
    #[arg(long)]
    force: bool,
    /// Also compute vectors for semantic search (see `INTELLI_EMBED_URL`; built-in hashing embedder otherwise)
    #[arg(long)]
    embed: bool,
}

#[derive(Debug, Args)]
pub(crate) struct SearchArgs {
    query: String,
    #[arg(long, default_value_t = 5)]
    limit: usize,
    /// Print JSON instead of text
    #[arg(long)]
    json: bool,
    /// auto (default: typo-tolerant keywords, plus vectors when a real embedding service is configured), keyword (exact words
    /// only), fuzzy, dense (vectors only) or hybrid (keywords + vectors)
    #[arg(long, default_value = "auto")]
    mode: String,
    /// Act as this user or group when documents have access lists (repeatable)
    #[arg(long = "as")]
    principals: Vec<String>,
    /// Also print the evidence block that would go to a model, limited to this many tokens
    #[arg(long)]
    context_tokens: Option<usize>,
}

#[derive(Debug, Args)]
pub(crate) struct EvalArgs {
    /// JSON file: a list of {"query", "relevant": [path substrings], "principals": [], "forbidden": []}
    judgments: PathBuf,
}

/// Remote model when `INTELLI_EMBED_URL` and `INTELLI_EMBED_MODEL` are set (key from `INTELLI_EMBED_KEY` or `OPENAI_API_KEY`);
/// otherwise the built-in hashing embedder, which finds typos and word forms but not synonyms.
fn embedder() -> Result<Arc<dyn Embedder>> {
    match (std::env::var("INTELLI_EMBED_URL"), std::env::var("INTELLI_EMBED_MODEL")) {
        (Ok(url), Ok(model)) if !url.is_empty() && !model.is_empty() => {
            let key = std::env::var("INTELLI_EMBED_KEY").or_else(|_| std::env::var("OPENAI_API_KEY")).ok();
            Ok(Arc::new(RemoteEmbedder::connect(&url, &model, key).context("could not reach the embedding service")?))
        }
        _ => Ok(Arc::new(HashingEmbedder::default())),
    }
}

fn retriever(store: &LiteStore, with_embedder: bool) -> Result<Retriever> {
    let budget = MemoryBudget::new(256 << 20, 512 << 20);
    Ok(Retriever::open(store, if with_embedder { Some(embedder()?) } else { None }, budget)?)
}

fn open_store() -> Result<Arc<LiteStore>> {
    let store = Arc::new(LiteStore::open(None, None).context("could not open the search index")?);
    // Documents are untrusted input: read them in a restricted worker process, unless that is switched off for debugging.
    if std::env::var("INTELLI_NO_EXTRACT_SANDBOX").is_err_and(|_| true) {
        store.set_sandbox(ibos_search::sandbox::Sandbox::current_exe().ok());
    }
    Ok(store)
}

pub(crate) async fn ui(a: UiArgs, path: &str) -> Result<ExitCode> {
    let store = open_store()?;
    let opts = ServeOptions {
        port: a.port,
        open_browser: !a.no_open,
        exit_when_closed: a.exit_when_closed,
        path: path.to_owned(),
        principals: a.principals.clone(),
        ..ServeOptions::default()
    };
    let running = ibos_server::start(Arc::clone(&store), &opts).await?;
    let docs = store.stats()?.documents;
    eprintln!(
        "Search home page: {}  ({docs} documents indexed; Ctrl+C to stop)\nThe link contains a one-time launch token: keep it private.",
        running.open_url(path)
    );
    tokio::select! {
        r = running.wait() => r?,
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn index(a: &IndexArgs) -> Result<ExitCode> {
    let store = open_store()?;
    for p in &a.paths {
        let stats = store
            .index_path(p, IndexOptions { prune: true, force: a.force }, None)
            .with_context(|| format!("{} not found or unreadable", p.display()))?;
        println!("{}: {}", p.display(), serde_json::to_string(&stats)?);
    }
    let s = store.stats()?;
    println!("index now holds {{documents: {}, chunks: {}}}", s.documents, s.chunks);
    if a.embed {
        let r = retriever(&store, true)?;
        let st = r.sync_vectors(Some(&|done, total| eprint!("\rembedding {done}/{total}")))?;
        eprintln!();
        println!("vectors: {} computed, {} unchanged, {} removed", st.embedded, st.kept, st.removed);
    }
    Ok(ExitCode::SUCCESS)
}

fn search_passages(a: &SearchArgs) -> Result<ExitCode> {
    let store = open_store()?;
    // The built-in hashing embedder finds typos and word forms but ranks long passages poorly (measured: it lowers exact-query
    // quality in hybrid mode), so `auto` only adds vectors when a real embedding service is configured.
    let semantic = std::env::var("INTELLI_EMBED_URL").is_ok_and(|u| !u.is_empty());
    let (mode, fuzzy) = match a.mode.as_str() {
        "auto" if semantic => (Mode::Hybrid, true),
        "auto" | "fuzzy" => (Mode::Lexical, true),
        m => (m.parse::<Mode>().map_err(anyhow::Error::msg)?, true),
    };
    let r = retriever(&store, mode != Mode::Lexical)?;
    let q = Query::new(&a.query).mode(mode).fuzzy(fuzzy).limit(a.limit).principals(a.principals.clone());
    let (out, evidence) = match a.context_tokens {
        Some(t) => r.context(&q, t, 2)?,
        None => (r.search(&q)?, Vec::new()),
    };
    if a.json {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &serde_json::json!({ "mode_used": out.mode_used, "notes": out.notes, "passages": out.passages, "evidence": evidence })
            )?
        );
        return Ok(ExitCode::SUCCESS);
    }
    for n in &out.notes {
        eprintln!("note: {n}");
    }
    if out.passages.is_empty() {
        println!("No results.");
    }
    for (i, p) in out.passages.iter().enumerate() {
        let text: String = p.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let excerpt: String = text.chars().take(200).collect();
        println!("[{}] {} ({}) score={:.2}\n    {excerpt}", i + 1, p.title, p.path, p.score);
    }
    if !evidence.is_empty() {
        println!("\n--- evidence for the model ({} tokens) ---", evidence.iter().map(|e| e.tokens).sum::<usize>());
        for e in &evidence {
            println!("{}\n", e.render());
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn eval(a: &EvalArgs) -> Result<ExitCode> {
    let judged: Vec<Judged> = serde_json::from_slice(
        &std::fs::read(&a.judgments).with_context(|| format!("cannot read {}", a.judgments.display()))?,
    )
    .context("judgments file is not valid")?;
    let store = open_store()?;
    let r = retriever(&store, true)?;
    let configs = [
        Config::new("keyword", Mode::Lexical, false, false),
        Config::new("keyword + fuzzy", Mode::Lexical, false, true),
        Config::new("dense", Mode::Dense, false, false),
        Config::new("hybrid", Mode::Hybrid, false, true),
        Config::new("hybrid + rerank", Mode::Hybrid, true, true),
    ];
    println!("{}", table(&evaluate(&r, &judged, &configs)?));
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn search(a: &SearchArgs) -> Result<ExitCode> {
    if a.mode == "keyword" {
        return search_keyword_only(a);
    }
    search_passages(a)
}

fn search_keyword_only(a: &SearchArgs) -> Result<ExitCode> {
    let access = ibos_search::acl::Access::new(a.principals.iter().cloned());
    let hits = open_store()?.search_as(&a.query, Some(a.limit), None, &access)?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&hits)?);
    } else if hits.is_empty() {
        println!("No results.");
    } else {
        for (i, h) in hits.iter().enumerate() {
            let text = h.snippet.replace("<mark>", "").replace("</mark>", "");
            println!(
                "[{}] {} ({}) score={:.2}\n    {}",
                i + 1,
                h.title,
                h.path,
                h.score,
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            );
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn acl(a: &AclArgs) -> Result<ExitCode> {
    let store = open_store()?;
    match &a.command {
        AclCommand::List => {
            let entries = store.acl_entries()?;
            if entries.is_empty() {
                println!("No restricted documents: every document is visible to everyone.");
            }
            for (path, who) in entries {
                println!("{path}  ->  {}", if who.is_empty() { "nobody".to_owned() } else { who.join(", ") });
            }
        }
        AclCommand::Set { path, principals } => {
            let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let who: Vec<&str> = principals.iter().map(String::as_str).collect();
            store.set_acl(&abs.to_string_lossy(), &who)?;
            println!(
                "{} is now visible to: {}",
                abs.display(),
                if who.is_empty() { "nobody".to_owned() } else { who.join(", ") }
            );
        }
        AclCommand::Clear { path } => {
            let abs = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            store.clear_acl(&abs.to_string_lossy())?;
            println!("{} is no longer restricted", abs.display());
        }
    }
    Ok(ExitCode::SUCCESS)
}

pub(crate) fn doctor() -> Result<ExitCode> {
    println!("[ok  ] intelli-rust {}", env!("CARGO_PKG_VERSION"));
    println!("[ok  ] config dir: {}", paths::config_dir().display());
    let keys: Vec<&str> = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "GOOGLE_API_KEY"]
        .into_iter()
        .filter(|k| std::env::var(k).is_ok_and(|v| !v.is_empty()))
        .collect();
    if keys.is_empty() {
        println!(
            "[warn] api key: none of ANTHROPIC_API_KEY/OPENAI_API_KEY/GOOGLE_API_KEY is set\n       fix: set one, or use a local model endpoint"
        );
    } else {
        println!("[ok  ] api key: {}", keys.join(", "));
    }
    match ibos_cdp::find_browser() {
        Some(p) => println!("[ok  ] browser: {}", p.display()),
        None => println!(
            "[FAIL] browser: no Chrome, Chromium or Edge found\n       fix: install one, or set INTELLI_BROWSER to its path"
        ),
    }
    let store = LiteStore::open(None, None);
    match store {
        Ok(s) => println!("[ok  ] search index: {} ({} documents)", s.path().display(), s.stats()?.documents),
        Err(e) => bail!("search index cannot be opened: {e}"),
    }
    Ok(ExitCode::SUCCESS)
}
