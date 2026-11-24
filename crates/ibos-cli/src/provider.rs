//! Choosing a model provider from `--model provider:name`, `--base-url` and the environment.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use ibos_llm::{Anthropic, Llm, OpenAiCompat};

pub(crate) struct Resolved {
    pub llm: Arc<dyn Llm>,
    pub model: String,
    pub provider: &'static str,
}

impl std::fmt::Debug for Resolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Resolved").field("provider", &self.provider).field("model", &self.model).finish_non_exhaustive()
    }
}

const TIMEOUT: Duration = Duration::from_mins(2);

fn nonempty(env: &dyn Fn(&str) -> Option<String>, key: &str) -> Option<String> {
    env(key).filter(|v| !v.trim().is_empty())
}

/// `spec` is `provider`, `provider:model` or nothing (auto-detect from API keys). `env` looks up environment variables, so the
/// logic is testable without touching the real environment.
pub(crate) fn resolve(
    spec: Option<&str>,
    base_url: Option<&str>,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<Resolved> {
    let (provider, name) = spec.map_or(("", ""), |s| s.split_once(':').unwrap_or((s, "")));
    let provider = if provider.is_empty() {
        if nonempty(env, "ANTHROPIC_API_KEY").is_some() {
            "anthropic"
        } else if nonempty(env, "OPENAI_API_KEY").is_some() {
            "openai"
        } else {
            bail!(
                "No LLM configured: set ANTHROPIC_API_KEY or OPENAI_API_KEY, or pass --model ollama:<model> (or --base-url for another OpenAI-compatible server)."
            );
        }
    } else {
        provider
    };
    let pick = |default: &str| if name.is_empty() { default.to_owned() } else { name.to_owned() };
    match provider {
        "anthropic" => {
            let Some(key) = nonempty(env, "ANTHROPIC_API_KEY") else { bail!("ANTHROPIC_API_KEY is not set") };
            let client = match base_url {
                Some(u) => Anthropic::with_base_url(u, &key, TIMEOUT)?,
                None => Anthropic::new(&key, TIMEOUT)?,
            };
            Ok(Resolved { llm: Arc::new(client), model: pick("claude-sonnet-5-5"), provider: "anthropic" })
        }
        "openai" => {
            let Some(key) = nonempty(env, "OPENAI_API_KEY") else { bail!("OPENAI_API_KEY is not set") };
            let base = base_url
                .map(str::to_owned)
                .or_else(|| nonempty(env, "OPENAI_BASE_URL"))
                .unwrap_or_else(|| "https://api.openai.com/v1".into());
            Ok(Resolved {
                llm: Arc::new(OpenAiCompat::new(&base, Some(key), TIMEOUT)?),
                model: pick("gpt-4o"),
                provider: "openai",
            })
        }
        "ollama" => {
            let base = base_url.unwrap_or("http://localhost:11434/v1");
            Ok(Resolved {
                llm: Arc::new(OpenAiCompat::new(base, None, TIMEOUT)?),
                model: pick("llama3.1"),
                provider: "ollama",
            })
        }
        "compat" | "openai-compatible" => {
            let Some(base) = base_url else { bail!("--base-url is required for an OpenAI-compatible server") };
            if name.is_empty() {
                bail!("give the model name: --model compat:<model>");
            }
            Ok(Resolved {
                llm: Arc::new(OpenAiCompat::new(base, nonempty(env, "OPENAI_API_KEY"), TIMEOUT)?),
                model: name.to_owned(),
                provider: "compat",
            })
        }
        other => {
            bail!("Unknown provider {other:?}; use anthropic, openai, ollama or compat (Google is not supported yet)")
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn env<'a>(pairs: &'a [(&str, &str)]) -> impl Fn(&str) -> Option<String> + 'a {
        let m: HashMap<String, String> = pairs.iter().map(|(k, v)| ((*k).into(), (*v).into())).collect();
        move |k| m.get(k).cloned()
    }

    #[test]
    fn auto_detects_from_keys_with_anthropic_first() {
        let r = resolve(None, None, &env(&[("OPENAI_API_KEY", "sk-1")])).unwrap();
        assert_eq!((r.provider, r.model.as_str()), ("openai", "gpt-4o"));
        let r = resolve(None, None, &env(&[("OPENAI_API_KEY", "sk-1"), ("ANTHROPIC_API_KEY", "sk-ant")])).unwrap();
        assert_eq!(r.provider, "anthropic");
        let e = resolve(None, None, &env(&[("OPENAI_API_KEY", "  ")])).unwrap_err().to_string();
        assert!(e.contains("No LLM configured") && e.contains("ollama"), "{e}");
    }

    #[test]
    fn explicit_providers_and_models() {
        let k = env(&[("OPENAI_API_KEY", "sk-1"), ("ANTHROPIC_API_KEY", "sk-ant")]);
        assert_eq!(resolve(Some("openai:gpt-4o-mini"), None, &k).unwrap().model, "gpt-4o-mini");
        assert_eq!(resolve(Some("anthropic"), None, &k).unwrap().model, "claude-sonnet-5-5");
        let o = resolve(Some("ollama:qwen2.5"), None, &env(&[])).unwrap();
        assert_eq!((o.provider, o.model.as_str()), ("ollama", "qwen2.5"));
        assert_eq!(
            resolve(Some("ollama"), None, &env(&[])).unwrap().model,
            "llama3.1",
            "no key needed for a local model"
        );
        let c = resolve(Some("compat:my-model"), Some("http://127.0.0.1:1/v1"), &env(&[])).unwrap();
        assert_eq!((c.provider, c.model.as_str()), ("compat", "my-model"));
    }

    #[test]
    fn mistakes_get_actionable_errors() {
        let none = env(&[]);
        for (spec, base, needle) in [
            (Some("openai"), None, "OPENAI_API_KEY"),
            (Some("anthropic"), None, "ANTHROPIC_API_KEY"),
            (Some("google"), None, "Google"),
            (Some("nope:x"), None, "Unknown provider"),
            (Some("compat:m"), None, "--base-url"),
            (Some("compat"), Some("http://x/v1"), "model name"),
        ] {
            let e = resolve(spec, base, &none).unwrap_err().to_string();
            assert!(e.contains(needle), "{spec:?} -> {e}");
        }
    }
}
