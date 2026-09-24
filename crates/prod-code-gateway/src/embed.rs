//! Dense vectors for intent search (roadmap 8.4).
//!
//! The lexical index finds a declaration when the question shares a word with its name,
//! signature or doc comment. "Where do we re-establish the socket after the link drops" shares
//! none with `fn reconnect_on_close`. A small sentence-embedding model maps both to vectors
//! whose dot product is high when they mean the same thing, and the search fuses that ranking
//! with the lexical one.
//!
//! The model is an ONNX export run in process by ONNX Runtime: BGE-small (English, 384
//! dimensions, 34 MB quantized) by default, found in `models/bge-small-en-v1.5` next to the
//! workspaces directory (`~/prod-code-storage/models/…`, beside the metrics) or
//! wherever `PROD_CODE_EMBED_MODEL` points. It is optional: without it the search is lexical
//! only and says so.

use anyhow::{Context, Result, anyhow};
use std::path::{Path, PathBuf};

/// Declarations are short; the head of a long doc comment carries its topic.
const MAX_TOKENS: usize = 256;

/// Something that turns text into L2-normalized vectors: the model, or a stand-in in tests.
pub trait Embed: Send {
    /// One vector per declaration text.
    fn passages(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    /// The vector of a question.
    fn query(&mut self, text: &str) -> Result<Vec<f32>>;
}

/// How a model turns per-token states into one vector, and what it wants in front of the text.
/// Both are part of how it was trained, and a model used any other way ranks worse.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Recipe {
    /// The first token's state (BGE), or the mean over the tokens (e5).
    cls: bool,
    query_prefix: &'static str,
    passage_prefix: &'static str,
}

/// The recipe of the model in `dir`, told by its name: e5 and Jina models by theirs, BGE
/// otherwise.
fn recipe_for(dir: &Path) -> Recipe {
    let name = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.contains("jina") {
        Recipe {
            cls: false,
            query_prefix: "",
            passage_prefix: "",
        }
    } else if name.contains("e5") {
        Recipe {
            cls: false,
            query_prefix: "query: ",
            passage_prefix: "passage: ",
        }
    } else {
        Recipe {
            cls: true,
            query_prefix: "Represent this sentence for searching relevant passages: ",
            passage_prefix: "",
        }
    }
}

/// Where the model is looked for: `PROD_CODE_EMBED_MODEL`, or `models/bge-small-en-v1.5` beside
/// the workspaces directory. It is read once, on the first search; a model installed later is
/// used after a restart.
pub fn model_dir(storage_root: &Path) -> PathBuf {
    match std::env::var_os("PROD_CODE_EMBED_MODEL") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => storage_root
            .parent()
            .unwrap_or(storage_root)
            .join("models")
            .join("bge-small-en-v1.5"),
    }
}

/// A sentence-embedding model run by ONNX Runtime.
pub struct OnnxEmbedder {
    session: ort::session::Session,
    tokenizer: tokenizers::Tokenizer,
    /// BERT exports take `token_type_ids`, others do not; read from the model, not assumed.
    needs_token_types: bool,
    recipe: Recipe,
}

impl OnnxEmbedder {
    /// Loads `model.onnx` and `tokenizer.json` from `dir`.
    pub fn load(dir: &Path) -> Result<Self> {
        let model = dir.join("model.onnx");
        let tokenizer = dir.join("tokenizer.json");
        anyhow::ensure!(
            model.is_file() && tokenizer.is_file(),
            "no model.onnx and tokenizer.json in {}",
            dir.display()
        );
        let threads = std::thread::available_parallelism()
            .map(|n| n.get().min(8))
            .unwrap_or(4);
        let session = ort::session::Session::builder()
            .map_err(|e| anyhow!("onnx runtime: {e}"))?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| anyhow!("onnx runtime: {e}"))?
            .with_intra_threads(threads)
            .map_err(|e| anyhow!("onnx runtime: {e}"))?
            .commit_from_file(&model)
            .map_err(|e| anyhow!("loading {}: {e}", model.display()))?;
        let mut tokenizer = tokenizers::Tokenizer::from_file(&tokenizer)
            .map_err(|e| anyhow!("loading {}: {e}", tokenizer.display()))?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams {
                max_length: MAX_TOKENS,
                ..Default::default()
            }))
            .map_err(|e| anyhow!("tokenizer truncation: {e}"))?;
        tokenizer.with_padding(None);
        let needs_token_types = session
            .inputs()
            .iter()
            .any(|input| input.name() == "token_type_ids");
        Ok(Self {
            session,
            tokenizer,
            needs_token_types,
            recipe: recipe_for(dir),
        })
    }

    fn embed(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        let encodings = texts
            .iter()
            .map(|t| {
                self.tokenizer
                    .encode(t.as_str(), true)
                    .map_err(|e| anyhow!("tokenizing: {e}"))
            })
            .collect::<Result<Vec<_>>>()?;
        let batch = encodings.len();
        let width = encodings
            .iter()
            .map(|e| e.get_ids().len())
            .max()
            .unwrap_or(1)
            .max(1);
        let mut ids = ndarray::Array2::<i64>::zeros((batch, width));
        let mut mask = ndarray::Array2::<i64>::zeros((batch, width));
        for (row, encoding) in encodings.iter().enumerate() {
            for (col, (&id, &m)) in encoding
                .get_ids()
                .iter()
                .zip(encoding.get_attention_mask())
                .enumerate()
            {
                ids[[row, col]] = id as i64;
                mask[[row, col]] = m as i64;
            }
        }
        let types = ndarray::Array2::<i64>::zeros((batch, width));
        let ids_t = ort::value::TensorRef::from_array_view(&ids)
            .map_err(|e| anyhow!("input tensor: {e}"))?;
        let mask_t = ort::value::TensorRef::from_array_view(&mask)
            .map_err(|e| anyhow!("mask tensor: {e}"))?;
        let outputs = if self.needs_token_types {
            let types_t = ort::value::TensorRef::from_array_view(&types)
                .map_err(|e| anyhow!("type tensor: {e}"))?;
            self.session
                .run(ort::inputs![
                    "input_ids" => ids_t,
                    "attention_mask" => mask_t,
                    "token_type_ids" => types_t
                ])
                .map_err(|e| anyhow!("running the model: {e}"))?
        } else {
            self.session
                .run(ort::inputs!["input_ids" => ids_t, "attention_mask" => mask_t])
                .map_err(|e| anyhow!("running the model: {e}"))?
        };
        let hidden = outputs["last_hidden_state"]
            .try_extract_array::<f32>()
            .map_err(|e| anyhow!("reading the model's output: {e}"))?;
        let hidden = hidden
            .into_dimensionality::<ndarray::Ix3>()
            .context("the model's output is not [batch, tokens, dim]")?;
        let dim = hidden.shape()[2];
        let mut out = Vec::with_capacity(batch);
        for (row, encoding) in encodings.iter().enumerate() {
            let mut v = vec![0f32; dim];
            if self.recipe.cls {
                for (d, x) in v.iter_mut().enumerate() {
                    *x = hidden[[row, 0, d]];
                }
            } else {
                let mut n = 0f32;
                for (col, m) in encoding.get_attention_mask().iter().enumerate() {
                    if *m == 0 {
                        continue;
                    }
                    n += 1.0;
                    for (d, x) in v.iter_mut().enumerate() {
                        *x += hidden[[row, col, d]];
                    }
                }
                if n > 0.0 {
                    v.iter_mut().for_each(|x| *x /= n);
                }
            }
            normalize(&mut v);
            out.push(v);
        }
        Ok(out)
    }
}

impl Embed for OnnxEmbedder {
    fn passages(&mut self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let prefixed: Vec<String> = texts
            .iter()
            .map(|t| format!("{}{t}", self.recipe.passage_prefix))
            .collect();
        self.embed(&prefixed)
    }

    fn query(&mut self, text: &str) -> Result<Vec<f32>> {
        let text = format!("{}{text}", self.recipe.query_prefix);
        Ok(self.embed(&[text])?.pop().unwrap_or_default())
    }
}

/// Scales `v` to unit length, so that a dot product is the cosine.
pub fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
}

/// The cosine of two unit vectors.
pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recipe_follows_the_model_and_vectors_are_unit_length() {
        let bge = recipe_for(Path::new("/m/bge-small-en-v1.5"));
        assert!(bge.cls);
        assert!(bge.query_prefix.starts_with("Represent"));
        let e5 = recipe_for(Path::new("/m/multilingual-e5-small"));
        assert!(!e5.cls);
        assert_eq!(
            (e5.query_prefix, e5.passage_prefix),
            ("query: ", "passage: ")
        );
        let jina = recipe_for(Path::new("/m/jina-embeddings-v2-base-code"));
        assert!(!jina.cls);
        assert_eq!((jina.query_prefix, jina.passage_prefix), ("", ""));
        let mut v = vec![3.0, 4.0];
        normalize(&mut v);
        assert_eq!(v, vec![0.6, 0.8]);
        let mut zero = vec![0.0, 0.0];
        normalize(&mut zero);
        assert_eq!(zero, vec![0.0, 0.0]);
        assert!((dot(&v, &v) - 1.0).abs() < 1e-6);
        assert_eq!(
            model_dir(Path::new("/s/workspaces")),
            std::env::var_os("PROD_CODE_EMBED_MODEL")
                .filter(|d| !d.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("/s/models/bge-small-en-v1.5"))
        );
        let missing = OnnxEmbedder::load(Path::new("/no/such/model"))
            .err()
            .unwrap();
        assert!(format!("{missing}").contains("no model.onnx"), "{missing}");
    }

    /// The real model, where it is installed (the build nodes): a question and the code that
    /// answers it share no word, and still score above an unrelated declaration.
    #[test]
    fn the_model_matches_a_question_to_code_by_meaning() {
        let dir = model_dir(&dirs_storage());
        let Ok(mut model) = OnnxEmbedder::load(&dir) else {
            eprintln!("no model at {}; skipped", dir.display());
            return;
        };
        let passages = model
            .passages(&[
                "fn reconnect_on_close. Re-establishes the websocket after the link drops."
                    .to_string(),
                "fn parse_color. Reads a CSS hex color such as #ff8800.".to_string(),
            ])
            .unwrap();
        assert_eq!(passages.len(), 2);
        assert_eq!(passages[0].len(), 384);
        let q = model
            .query("where do we restore the connection when the server goes away")
            .unwrap();
        assert!((dot(&q, &q) - 1.0).abs() < 1e-3);
        assert!(
            dot(&q, &passages[0]) > dot(&q, &passages[1]) + 0.05,
            "{} vs {}",
            dot(&q, &passages[0]),
            dot(&q, &passages[1])
        );
    }

    /// The gateway's default workspaces directory; the nodes keep the model beside it.
    fn dirs_storage() -> PathBuf {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
            .join("prod-code-storage")
            .join("workspaces")
    }
}
