use anyhow::{anyhow, bail, Context, Result};
use half::f16;
use hf_hub::api::sync::ApiBuilder;
use safetensors::{tensor::Dtype, SafeTensors};
use std::path::Path;
use std::sync::Mutex;
use tokenizers::Tokenizer;

pub const MODEL_ID: &str = "minishlab/potion-code-16M-v2";

enum ModelState {
    Unloaded,
    Loaded(Inner),
    Disabled,
}

pub struct Model2Vec {
    model: Mutex<ModelState>,
}

impl Model2Vec {
    pub fn new() -> Self {
        let is_offline = std::env::var("ATLAS_CODE_INTELIGENCE_OFFLINE")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
            || std::env::var("ATLAS_CODE_INTELIGENCE_LEXICAL_ONLY")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);

        let initial_state = if is_offline {
            eprintln!("AtlasCodeInteligence: Lexical-only mode active (semantic embeddings disabled via environment)");
            ModelState::Disabled
        } else {
            ModelState::Unloaded
        };

        Self {
            model: Mutex::new(initial_state),
        }
    }

    pub fn embed(&self, text: &str) -> Result<Option<Vec<f32>>> {
        let mut model = self
            .model
            .lock()
            .map_err(|error| anyhow!("Embedding model lock poisoned: {error}"))?;
        match &mut *model {
            ModelState::Disabled => Ok(None),
            ModelState::Loaded(inner) => inner.encode(text).map(Some),
            ModelState::Unloaded => match Inner::load() {
                Ok(inner) => {
                    let vec = inner.encode(text)?;
                    *model = ModelState::Loaded(inner);
                    Ok(Some(vec))
                }
                Err(error) => {
                    let require_semantic = std::env::var("ATLAS_CODE_INTELIGENCE_REQUIRE_SEMANTIC")
                        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                        .unwrap_or(false);
                    if require_semantic {
                        return Err(error).context(format!(
                            "Failed to load required embedding model '{MODEL_ID}' from Hugging Face"
                        ));
                    }
                    eprintln!(
                        "AtlasCodeInteligence warning: Could not download or load embedding model '{MODEL_ID}' from Hugging Face: {error:#}. Falling back to lexical-only mode. Set ATLAS_CODE_INTELIGENCE_LEXICAL_ONLY=1 to suppress model loading attempts."
                    );
                    *model = ModelState::Disabled;
                    Ok(None)
                }
            },
        }
    }
}

struct Inner {
    tokenizer: Tokenizer,
    embeddings: Vec<f32>,
    dimensions: usize,
    vocabulary_size: usize,
    normalize: bool,
}

impl Inner {
    fn load() -> Result<Self> {
        let api = ApiBuilder::new()
            .build()
            .map_err(|error| anyhow!("Connecting to Hugging Face Hub: {error}"))?;
        let repository = api.model(MODEL_ID.to_owned());
        let model_path = repository.get("model.safetensors").with_context(|| {
            format!("Downloading '{MODEL_ID}/model.safetensors' from Hugging Face Hub")
        })?;
        let tokenizer_path = repository.get("tokenizer.json").with_context(|| {
            format!("Downloading '{MODEL_ID}/tokenizer.json' from Hugging Face Hub")
        })?;
        let config_path = repository.get("config.json").ok();
        Self::from_files(&model_path, &tokenizer_path, config_path.as_deref())
    }

    fn from_files(
        model_path: &Path,
        tokenizer_path: &Path,
        config_path: Option<&Path>,
    ) -> Result<Self> {
        let tokenizer = Tokenizer::from_file(tokenizer_path)
            .map_err(|error| anyhow!("Loading tokenizer: {error}"))?;
        let normalize = config_path
            .and_then(|path| std::fs::File::open(path).ok())
            .and_then(|file| serde_json::from_reader::<_, serde_json::Value>(file).ok())
            .and_then(|config| config.get("normalize").and_then(serde_json::Value::as_bool))
            .unwrap_or(true);
        let model_bytes = std::fs::read(model_path)
            .with_context(|| format!("Reading model weights from {}", model_path.display()))?;
        let tensors =
            SafeTensors::deserialize(&model_bytes).context("Parsing Model2Vec weights")?;
        let tensor = tensors
            .tensor("embeddings")
            .or_else(|_| tensors.tensor("0"))
            .context("Model has no embeddings tensor")?;
        let shape = tensor.shape();
        if shape.len() != 2 {
            bail!("Expected a 2D embedding matrix, got shape {shape:?}");
        }
        let vocabulary_size = shape[0];
        let dimensions = shape[1];
        let raw = tensor.data();
        let embeddings: Vec<f32> = match tensor.dtype() {
            Dtype::F32 => raw
                .chunks_exact(4)
                .map(|bytes| f32::from_le_bytes(bytes.try_into().expect("four bytes")))
                .collect(),
            Dtype::F16 => raw
                .chunks_exact(2)
                .map(|bytes| f16::from_le_bytes(bytes.try_into().expect("two bytes")).to_f32())
                .collect(),
            Dtype::I8 => raw.iter().map(|byte| *byte as i8 as f32).collect(),
            dtype => bail!("Unsupported embedding tensor type: {dtype:?}"),
        };
        if embeddings.len() != vocabulary_size * dimensions {
            bail!("Embedding matrix dimensions do not match tensor data");
        }
        Ok(Self {
            tokenizer,
            embeddings,
            dimensions,
            vocabulary_size,
            normalize,
        })
    }

    fn encode(&self, text: &str) -> Result<Vec<f32>> {
        let encoding = self
            .tokenizer
            .encode(text, false)
            .map_err(|error| anyhow!("Tokenizing embedding input: {error}"))?;
        let mut vector = vec![0.0; self.dimensions];
        let mut count = 0usize;
        for token_id in encoding.get_ids() {
            let index = *token_id as usize;
            if index >= self.vocabulary_size {
                continue;
            }
            let offset = index * self.dimensions;
            for (target, value) in vector
                .iter_mut()
                .zip(&self.embeddings[offset..offset + self.dimensions])
            {
                *target += value;
            }
            count += 1;
        }
        if count == 0 {
            return Ok(vector);
        }
        if self.normalize {
            let norm = vector
                .iter()
                .map(|value| value * value)
                .sum::<f32>()
                .sqrt()
                .max(1e-12);
            vector.iter_mut().for_each(|value| *value /= norm);
        } else {
            let divisor = count as f32;
            vector.iter_mut().for_each(|value| *value /= divisor);
        }
        Ok(vector)
    }
}
