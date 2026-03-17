use anyhow::Result;

#[cfg(feature = "embed-candle")]
pub mod candle;

#[cfg(feature = "embed-llamacpp")]
pub mod llamacpp;

/// Trait for embedding backends. Implement this to add a new backend.
pub trait EmbedBackend: Send + Sync {
    fn embed(&self, text: &str) -> Result<Vec<f32>>;
    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>>;
    fn dimensions(&self) -> usize;
}

#[derive(Debug, Clone)]
pub struct EmbedConfig {
    pub backend: BackendType,
    pub model_path: Option<String>,
}

#[derive(Debug, Clone)]
pub enum BackendType {
    Candle,
    LlamaCpp,
}

pub fn create_backend(_config: &EmbedConfig) -> Result<Box<dyn EmbedBackend>> {
    #[cfg(feature = "embed-candle")]
    if matches!(_config.backend, BackendType::Candle) {
        return Ok(Box::new(candle::CandleBackend::new(_config)?));
    }

    #[cfg(feature = "embed-llamacpp")]
    if matches!(_config.backend, BackendType::LlamaCpp) {
        return Ok(Box::new(llamacpp::LlamaCppBackend::new(_config)?));
    }

    anyhow::bail!("No embedding backend available. Build with --features embed-candle or embed-llamacpp")
}
