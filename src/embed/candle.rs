use anyhow::{Context, Result};
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use hf_hub::api::sync::ApiBuilder;
use tokenizers::Tokenizer;

use super::{EmbedBackend, EmbedConfig};

const MODEL_ID: &str = "sentence-transformers/all-MiniLM-L6-v2";
const DIMENSIONS: usize = 384;

pub struct CandleBackend {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl CandleBackend {
    pub fn new(config: &EmbedConfig) -> Result<Self> {
        let device = Self::select_device();

        let (tokenizer_path, weights_path, config_path) = if let Some(ref path) = config.model_path
        {
            let dir = std::path::PathBuf::from(path);
            (
                dir.join("tokenizer.json"),
                dir.join("model.safetensors"),
                dir.join("config.json"),
            )
        } else {
            let model_dir = dirs::home_dir()
                .context("Could not determine home directory")?
                .join(".rememora")
                .join("models");
            std::fs::create_dir_all(&model_dir)?;
            let cache = hf_hub::Cache::new(model_dir);
            let api = ApiBuilder::from_cache(cache).build()?;
            let repo = api.model(MODEL_ID.to_string());

            (
                repo.get("tokenizer.json")?,
                repo.get("model.safetensors")?,
                repo.get("config.json")?,
            )
        };

        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Failed to load tokenizer: {e}"))?;

        let config_str = std::fs::read_to_string(&config_path)?;
        let bert_config: Config = serde_json::from_str(&config_str)?;

        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], DType::F32, &device)?
        };
        let model = BertModel::load(vb, &bert_config)?;

        Ok(Self {
            model,
            tokenizer,
            device,
        })
    }

    fn select_device() -> Device {
        #[cfg(feature = "metal")]
        {
            if let Ok(device) = Device::new_metal(0) {
                return device;
            }
        }
        Device::Cpu
    }

    fn encode(&self, text: &str) -> Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, true)
            .map_err(|e| anyhow::anyhow!("Tokenization failed: {e}"))?;

        let ids = encoding.get_ids();
        let type_ids = encoding.get_type_ids();
        let attention_mask = encoding.get_attention_mask();

        let token_ids = Tensor::new(ids, &self.device)?.unsqueeze(0)?;
        let token_type_ids = Tensor::new(type_ids, &self.device)?.unsqueeze(0)?;
        let mask_tensor = Tensor::new(attention_mask, &self.device)?
            .to_dtype(DType::F32)?
            .unsqueeze(0)?;

        let output = self
            .model
            .forward(&token_ids, &token_type_ids, Some(&mask_tensor))?;

        // Mean pooling: sum token embeddings weighted by attention mask, divide by count
        let mask_expanded = mask_tensor.unsqueeze(2)?.broadcast_as(output.shape())?;
        let summed = (output * &mask_expanded)?.sum(1)?;
        let count = mask_expanded.sum(1)?;
        let mean_pooled = summed.broadcast_div(&count)?;

        // L2 normalize
        let mean_vec = mean_pooled.squeeze(0)?;
        let l2_norm = mean_vec.sqr()?.sum_all()?.sqrt()?;
        let normalized = mean_vec.broadcast_div(&l2_norm)?;

        let result: Vec<f32> = normalized.to_vec1()?;
        Ok(result)
    }
}

impl EmbedBackend for CandleBackend {
    fn embed(&self, text: &str) -> Result<Vec<f32>> {
        self.encode(text)
    }

    fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>> {
        texts.iter().map(|t| self.encode(t)).collect()
    }

    fn dimensions(&self) -> usize {
        DIMENSIONS
    }
}
