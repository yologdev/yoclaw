//! Embedding engine for semantic memory search.
//! Only compiled when the `semantic` feature flag is enabled.

use candle_core::{Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config as BertConfig};
use hf_hub::{api::sync::Api, Repo, RepoType};
use std::sync::OnceLock;
use tokenizers::Tokenizer;

const MODEL_REPO: &str = "google/embedding-gemma-300m";
const TARGET_DIMS: usize = 384; // Matryoshka truncation from 768

/// Lazily-initialized embedding engine. Created once, shared via Arc.
pub struct EmbeddingEngine {
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

static ENGINE: OnceLock<Result<EmbeddingEngine, String>> = OnceLock::new();

impl EmbeddingEngine {
    /// Get or create the global embedding engine.
    /// Model weights are downloaded on first use (~200MB) to ~/.cache/huggingface/.
    pub fn global() -> Result<&'static EmbeddingEngine, String> {
        ENGINE
            .get_or_init(|| Self::load().map_err(|e| e.to_string()))
            .as_ref()
            .map_err(|e| e.clone())
    }

    fn load() -> anyhow::Result<Self> {
        tracing::info!("Loading EmbeddingGemma-300M (first time may download ~200MB)...");

        let device = Device::Cpu;
        let api = Api::new()?;
        let repo = api.repo(Repo::new(MODEL_REPO.to_string(), RepoType::Model));

        // Download model files
        let config_path = repo.get("config.json")?;
        let tokenizer_path = repo.get("tokenizer.json")?;
        let weights_path = repo.get("model.safetensors")?;

        // Load config
        let config_str = std::fs::read_to_string(&config_path)?;
        let config: BertConfig = serde_json::from_str(&config_str)?;

        // Load tokenizer
        let tokenizer = Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow::anyhow!("Tokenizer error: {}", e))?;

        // Load weights
        let vb = unsafe {
            VarBuilder::from_mmaped_safetensors(&[weights_path], candle_core::DType::F32, &device)?
        };
        let model = BertModel::load(vb, &config)?;

        tracing::info!("EmbeddingGemma-300M loaded successfully");
        Ok(Self {
            model,
            tokenizer,
            device,
        })
    }

    /// Generate embeddings for a batch of texts.
    /// Output is truncated to 384 dimensions (Matryoshka property).
    pub fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        let mut all_embeddings = Vec::with_capacity(texts.len());

        for text in texts {
            let encoding = self
                .tokenizer
                .encode(*text, true)
                .map_err(|e| anyhow::anyhow!("Tokenize error: {}", e))?;

            let ids = encoding.get_ids().to_vec();
            let type_ids = encoding.get_type_ids().to_vec();
            let attention_mask = encoding.get_attention_mask().to_vec();

            let len = ids.len();
            let input_ids = Tensor::new(ids, &self.device)?.unsqueeze(0)?;
            let token_type_ids = Tensor::new(type_ids, &self.device)?.unsqueeze(0)?;
            let attention = Tensor::new(attention_mask.clone(), &self.device)?
                .to_dtype(candle_core::DType::F32)?
                .unsqueeze(0)?;

            // Forward pass
            let output = self
                .model
                .forward(&input_ids, &token_type_ids, Some(&attention))?;

            // Mean pooling over token dimension
            let mask_expanded = attention.unsqueeze(2)?.broadcast_as(output.shape())?;
            let sum = (output * mask_expanded)?.sum(1)?;
            let count = Tensor::new(vec![len as f32], &self.device)?
                .unsqueeze(0)?
                .broadcast_as(sum.shape())?;
            let mean = (sum / count)?;

            // L2 normalize
            let norm = mean.sqr()?.sum_keepdim(1)?.sqrt()?;
            let normalized = (mean / norm)?;

            // Truncate to target dims (Matryoshka)
            let embedding = normalized
                .narrow(1, 0, TARGET_DIMS.min(normalized.dim(1)?))?
                .squeeze(0)?
                .to_vec1::<f32>()?;

            all_embeddings.push(embedding);
        }

        Ok(all_embeddings)
    }
}

/// Initialize the sqlite-vec extension on a connection.
pub fn load_sqlite_vec(conn: &rusqlite::Connection) -> Result<(), rusqlite::Error> {
    // sqlite-vec is loaded as a runtime extension
    // The extension must be available in the system library path
    unsafe {
        conn.load_extension_enable()?;
        match conn.load_extension("vec0", None) {
            Ok(()) => {
                tracing::info!("sqlite-vec extension loaded");
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to load sqlite-vec extension: {}. Semantic search will use FTS5 only.",
                    e
                );
            }
        }
        conn.load_extension_disable()?;
    }
    Ok(())
}

/// Create the memory_vec virtual table if sqlite-vec is available.
pub fn create_vec_table(conn: &rusqlite::Connection) -> Result<bool, rusqlite::Error> {
    match conn.execute_batch(&format!(
        "CREATE VIRTUAL TABLE IF NOT EXISTS memory_vec USING vec0(
            memory_id INTEGER PRIMARY KEY,
            embedding float[{}]
        );",
        TARGET_DIMS
    )) {
        Ok(()) => Ok(true),
        Err(e) => {
            tracing::debug!("Cannot create memory_vec table (vec0 not available): {}", e);
            Ok(false)
        }
    }
}

/// Check if the memory_vec virtual table exists.
pub fn vec_table_exists(conn: &rusqlite::Connection) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name='memory_vec'",
        [],
        |_| Ok(()),
    )
    .is_ok()
}

/// Insert or update an embedding for a memory entry.
/// Uses DELETE + INSERT since vec0 virtual tables may not support INSERT OR REPLACE.
pub fn vec_insert(
    conn: &rusqlite::Connection,
    memory_id: i64,
    embedding: &[f32],
) -> Result<(), rusqlite::Error> {
    let blob: Vec<u8> = embedding.iter().flat_map(|f| f.to_le_bytes()).collect();
    conn.execute(
        "DELETE FROM memory_vec WHERE memory_id = ?1",
        rusqlite::params![memory_id],
    )?;
    conn.execute(
        "INSERT INTO memory_vec (memory_id, embedding) VALUES (?1, ?2)",
        rusqlite::params![memory_id, blob],
    )?;
    Ok(())
}

/// Delete an embedding for a memory entry.
pub fn vec_delete(conn: &rusqlite::Connection, memory_id: i64) -> Result<(), rusqlite::Error> {
    conn.execute(
        "DELETE FROM memory_vec WHERE memory_id = ?1",
        rusqlite::params![memory_id],
    )?;
    Ok(())
}

/// KNN search: find the closest embeddings to the query. Returns (memory_id, distance).
pub fn vec_search(
    conn: &rusqlite::Connection,
    query_embedding: &[f32],
    limit: usize,
) -> Result<Vec<(i64, f64)>, rusqlite::Error> {
    let blob: Vec<u8> = query_embedding
        .iter()
        .flat_map(|f| f.to_le_bytes())
        .collect();
    let mut stmt = conn.prepare(
        "SELECT memory_id, distance FROM memory_vec WHERE embedding MATCH ?1 ORDER BY distance LIMIT ?2",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![blob, limit as i64], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_dims() {
        assert_eq!(TARGET_DIMS, 384);
    }

    #[test]
    fn test_vec_table_exists_false() {
        // Without loading sqlite-vec, the table doesn't exist
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        assert!(!vec_table_exists(&conn));
    }
}
