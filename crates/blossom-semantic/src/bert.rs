//! A built-in model run on the CPU with candle's BERT.
//!
//! Texts get the model's prefix, are tokenized with its own tokenizer,
//! truncated to [`MAX_TOKENS`] and padded per batch; the attention mask
//! keeps padding out of the result. Token vectors are pooled as the model
//! was trained ([`Pooling`]).

use std::fs;
use std::path::Path;

use candle_core::{DType, Device, IndexOp, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config, DTYPE};
use tokenizers::{PaddingParams, PaddingStrategy, Tokenizer, TruncationParams};

use crate::model::{Model, Pooling};
use crate::{Embedder, SemanticError};

/// Tokens per text at most. The models take 512; a heading's opening says
/// most of what it is about, and shorter is much faster on a CPU.
pub const MAX_TOKENS: usize = 256;

/// A loaded built-in model.
pub struct BertEmbedder {
    model: Model,
    id: String,
    bert: BertModel,
    tokenizer: Tokenizer,
    dims: usize,
}

impl std::fmt::Debug for BertEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BertEmbedder")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

fn err(what: impl std::fmt::Display) -> SemanticError {
    SemanticError::Embed(what.to_string())
}

impl BertEmbedder {
    /// Loads `model` from its folder under `data_dir`.
    ///
    /// # Errors
    ///
    /// When the model isn't downloaded or its files can't be read.
    pub fn load(model: Model, data_dir: &Path) -> Result<BertEmbedder, SemanticError> {
        if !model.is_downloaded(data_dir) {
            return Err(SemanticError::NotDownloaded(model));
        }
        let dir = model.dir(data_dir);
        let config: Config = serde_json::from_slice(&fs::read(dir.join("config.json"))?)
            .map_err(|e| err(format!("config.json: {e}")))?;
        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json"))
            .map_err(|e| err(format!("tokenizer.json: {e}")))?;
        let pad_id = ["[PAD]", "<pad>"]
            .iter()
            .find_map(|t| tokenizer.token_to_id(t))
            .unwrap_or(0);
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            pad_id,
            ..PaddingParams::default()
        }));
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_TOKENS,
                ..TruncationParams::default()
            }))
            .map_err(err)?;
        let weights = fs::read(dir.join("model.safetensors"))?;
        let vb = VarBuilder::from_buffered_safetensors(weights, DTYPE, &Device::Cpu)
            .map_err(|e| err(format!("model.safetensors: {e}")))?;
        let bert = BertModel::load(vb, &config).map_err(|e| err(format!("model: {e}")))?;
        Ok(BertEmbedder {
            model,
            id: model.id(),
            bert,
            tokenizer,
            dims: config.hidden_size,
        })
    }

    fn embed(&self, prefix: &str, texts: &[&str]) -> Result<Vec<Vec<f32>>, SemanticError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let inputs: Vec<String> = texts.iter().map(|t| format!("{prefix}{t}")).collect();
        let encodings = self.tokenizer.encode_batch(inputs, true).map_err(err)?;
        let rows =
            |f: &dyn Fn(&tokenizers::Encoding) -> Vec<u32>| -> Result<Tensor, SemanticError> {
                let rows = encodings
                    .iter()
                    .map(|e| Tensor::new(f(e).as_slice(), &Device::Cpu))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(err)?;
                Tensor::stack(&rows, 0).map_err(err)
            };
        let ids = rows(&|e| e.get_ids().to_vec())?;
        let mask = rows(&|e| e.get_attention_mask().to_vec())?;
        let types = ids.zeros_like().map_err(err)?;
        let out = self.bert.forward(&ids, &types, Some(&mask)).map_err(err)?;
        let pooled = match self.model.pooling() {
            Pooling::Cls => out.i((.., 0, ..)).map_err(err)?,
            Pooling::Mean => {
                let m = mask
                    .to_dtype(DType::F32)
                    .and_then(|m| m.unsqueeze(2))
                    .map_err(err)?;
                let sum = out.broadcast_mul(&m).and_then(|t| t.sum(1)).map_err(err)?;
                let count = m.sum(1).map_err(err)?;
                sum.broadcast_div(&count).map_err(err)?
            }
        };
        pooled.to_vec2::<f32>().map_err(err)
    }
}

impl Embedder for BertEmbedder {
    fn id(&self) -> &str {
        &self.id
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SemanticError> {
        self.embed(self.model.passage_prefix(), texts)
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, SemanticError> {
        self.embed(self.model.query_prefix(), &[text])?
            .pop()
            .ok_or(SemanticError::Shape {
                expected: 1,
                got: 0,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine(a: &[f32], b: &[f32]) -> f32 {
        let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let n = |v: &[f32]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        dot / (n(a) * n(b))
    }

    #[test]
    fn needs_the_files() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            BertEmbedder::load(Model::English, dir.path()),
            Err(SemanticError::NotDownloaded(Model::English))
        ));
    }

    /// Runs the real English model from `BLOSSOM_TEST_DATA_DIR` (a data
    /// folder holding `models/<id>/…`): `cargo test -p blossom-semantic
    /// --features builtin-model -- --ignored`.
    #[test]
    #[ignore = "needs the downloaded model"]
    fn english_model_ranks_by_meaning() {
        let data = std::env::var("BLOSSOM_TEST_DATA_DIR").expect("BLOSSOM_TEST_DATA_DIR");
        let e = BertEmbedder::load(Model::English, Path::new(&data)).unwrap();
        assert_eq!(e.dims(), 384);
        let passages = e
            .embed_passages(&[
                "Groceries\nmilk, eggs, bread and butter",
                "Fix the bicycle\nthe rear brake squeaks and the chain is loose",
                "Quarterly report\nrevenue grew 12% while costs stayed flat",
            ])
            .unwrap();
        // A single text and a padded batch give the same vector.
        let alone = e
            .embed_passages(&["Groceries\nmilk, eggs, bread and butter"])
            .unwrap();
        assert!(cosine(&alone[0], &passages[0]) > 0.999);
        let q = e.embed_query("what food do I need to buy").unwrap();
        let scores: Vec<f32> = passages.iter().map(|p| cosine(&q, p)).collect();
        assert!(scores[0] > scores[1] && scores[0] > scores[2], "{scores:?}");
        let q = e.embed_query("bike repair").unwrap();
        let scores: Vec<f32> = passages.iter().map(|p| cosine(&q, p)).collect();
        assert!(scores[1] > scores[0] && scores[1] > scores[2], "{scores:?}");
    }
}
