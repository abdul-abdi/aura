use crate::provider::LlmProvider;
use anyhow::{Context, Result};
use async_trait::async_trait;
use llama_cpp_2::context::params::LlamaContextParams;
use llama_cpp_2::llama_backend::LlamaBackend;
use llama_cpp_2::model::params::LlamaModelParams;
use llama_cpp_2::model::LlamaModel;
use std::path::PathBuf;
use std::sync::Arc;

pub struct LlamaCppConfig {
    pub model_path: PathBuf,
    pub n_ctx: u32,
    pub n_threads: u32,
    pub max_tokens: u32,
}

impl Default for LlamaCppConfig {
    fn default() -> Self {
        Self {
            model_path: dirs::data_local_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("aura")
                .join("models")
                .join("intent-model.gguf"),
            n_ctx: 2048,
            n_threads: 4,
            max_tokens: 256,
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
        let backend = LlamaBackend::init()?;

        let model_params = LlamaModelParams::default();
        let model = LlamaModel::load_from_file(
            &backend,
            config.model_path.to_str().context("Invalid model path")?,
            &model_params,
        )
        .context("Failed to load GGUF model")?;

        Ok(Self {
            model: Arc::new(model),
            backend: Arc::new(backend),
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
            "LlamaCpp inference with {} input tokens",
            tokens.len()
        );

        // TODO: implement actual token generation loop
        let _ = ctx;

        Ok(String::new())
    }
}
