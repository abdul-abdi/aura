use crate::provider::LlmProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

const DEFAULT_MODEL_FILENAME: &str = "intent-model.gguf";
const DEFAULT_N_CTX: u32 = 2048;
const DEFAULT_N_THREADS: u32 = 4;
const DEFAULT_MAX_TOKENS: u32 = 256;

static BACKEND: OnceLock<Result<Arc<LlamaBackend>, String>> = OnceLock::new();

fn get_backend() -> Result<Arc<LlamaBackend>> {
    let result = BACKEND.get_or_init(|| {
        LlamaBackend::init()
            .map(Arc::new)
            .map_err(|e| format!("{e}"))
    });
    match result {
        Ok(backend) => Ok(Arc::clone(backend)),
        Err(e) => Err(anyhow::anyhow!("Backend init failed: {e}")),
    }
}

#[derive(Debug, Clone)]
pub struct LlamaCppConfig {
    pub model_path: PathBuf,
    pub n_ctx: u32,
    pub n_threads: u32,
    pub max_tokens: u32,
}

impl Default for LlamaCppConfig {
    fn default() -> Self {
        let model_path = dirs::data_local_dir()
            .unwrap_or_else(|| {
                tracing::warn!("Could not determine local data directory, falling back to '.'");
                PathBuf::from(".")
            })
            .join("aura")
            .join("models")
            .join(DEFAULT_MODEL_FILENAME);

        Self {
            model_path,
            n_ctx: DEFAULT_N_CTX,
            n_threads: DEFAULT_N_THREADS,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

pub struct LlamaCppProvider {
    model: Arc<LlamaModel>,
    backend: Arc<LlamaBackend>,
    config: LlamaCppConfig,
}

impl LlamaCppProvider {
    pub fn new(config: LlamaCppConfig) -> Result<Self> {
        let backend = get_backend()?;

        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(
            &backend,
            config.model_path.to_str().context("Invalid model path")?,
            &model_params,
        )
        .context("Failed to load GGUF model")?;

        Ok(Self {
            model: Arc::new(model),
            backend,
            config,
        })
    }
}

#[async_trait]
impl LlmProvider for LlamaCppProvider {
    async fn complete(&self, prompt: &str) -> Result<String> {
        let ctx_params = LlamaContextParams::default()
            .with_n_ctx(std::num::NonZeroU32::new(self.config.n_ctx));

        let ctx = self
            .model
            .new_context(&self.backend, ctx_params)
            .context("Failed to create inference context")?;

        let tokens = self
            .model
            .str_to_token(prompt, llama_cpp_2::model::AddBos::Always)?;

        tracing::debug!(
            token_count = tokens.len(),
            "LlamaCpp inference started"
        );

        // TODO: implement actual token generation loop
        let _ = ctx;

        Ok(String::new())
    }
}
