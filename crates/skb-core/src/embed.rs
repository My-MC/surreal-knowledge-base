use crate::error::SkbError;

pub trait Embed: Send + Sync {
    fn dimension(&self) -> usize;
    fn max_input_tokens(&self) -> usize;
    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SkbError>;
}

pub struct MockEmbedder {
    pub dimension: usize,
}

impl Embed for MockEmbedder {
    fn dimension(&self) -> usize {
        self.dimension
    }
    fn max_input_tokens(&self) -> usize {
        8192
    }

    fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SkbError> {
        Ok(texts
            .iter()
            .enumerate()
            .map(|(i, _)| {
                let mut v = vec![0.0f32; self.dimension];
                v[i % self.dimension] = 1.0;
                l2_normalize(&mut v);
                v
            })
            .collect())
    }
}

#[cfg(feature = "ort")]
pub mod ort_embedder {
    use crate::config::EmbeddingConfig;
    use crate::error::{ErrorCode, SkbError};
    use crate::tokenize::Tokenize;
    use ndarray::Array2;
    use ort::session::Session;
    use std::sync::{Arc, Mutex};

    pub struct OrtEmbedder {
        session: Arc<Mutex<Session>>,
        tokenizer: Arc<dyn Tokenize>,
        dimension: usize,
        max_input_tokens: usize,
        batch_size: usize,
    }

    impl OrtEmbedder {
        pub fn load(
            config: &EmbeddingConfig,
            tokenizer: Arc<dyn Tokenize>,
        ) -> Result<Self, SkbError> {
            let onnx_path = if config.onnx_path == "auto" {
                let client = hf_hub::HFClientSync::new()
                    .map_err(|e| SkbError::new(ErrorCode::Embedding, format!("hf-hub: {e}")))?;
                let (owner, name) = parse_hf_model(&config.model);
                let repo = client.model(owner, name);
                repo.download_file()
                    .filename("onnx/model.onnx")
                    .send()
                    .map_err(|e| {
                        SkbError::new(ErrorCode::Embedding, format!("download onnx: {e}"))
                    })?
            } else {
                std::path::PathBuf::from(&config.onnx_path)
            };

            if config.onnx_path == "auto" {
                let client = hf_hub::HFClientSync::new()
                    .map_err(|e| SkbError::new(ErrorCode::Embedding, format!("hf-hub: {e}")))?;
                let (owner, name) = parse_hf_model(&config.model);
                let repo = client.model(owner, name);
                let _ = repo.download_file().filename("onnx/model.onnx_data").send();
            }

            let mut builder = Session::builder().map_err(|e| {
                SkbError::new(ErrorCode::Embedding, format!("ort builder: {:?}", e))
            })?;
            let session = builder
                .commit_from_file(&onnx_path)
                .map_err(|e| SkbError::new(ErrorCode::Embedding, format!("ort load: {:?}", e)))?;
            let session = Arc::new(Mutex::new(session));

            let dimension = if config.dimension > 0 {
                config.dimension
            } else {
                1024
            };
            let max_input_tokens = if config.max_input_tokens > 0 {
                config.max_input_tokens
            } else {
                8192
            };

            Ok(Self {
                session,
                tokenizer,
                dimension,
                max_input_tokens,
                batch_size: config.batch_size.max(1),
            })
        }
    }

    impl super::Embed for OrtEmbedder {
        fn dimension(&self) -> usize {
            self.dimension
        }
        fn max_input_tokens(&self) -> usize {
            self.max_input_tokens
        }

        fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SkbError> {
            if texts.is_empty() {
                return Ok(vec![]);
            }

            let mut all_embeddings = Vec::with_capacity(texts.len());

            for chunk in texts.chunks(self.batch_size) {
                let batch_embeddings = self.embed_chunk(chunk)?;
                all_embeddings.extend(batch_embeddings);
            }

            Ok(all_embeddings)
        }
    }

    impl OrtEmbedder {
        fn embed_chunk(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, SkbError> {
            let batch = texts.len();

            let mut all_ids: Vec<Vec<i64>> = Vec::with_capacity(batch);
            let mut max_len = 0;

            for text in texts {
                let ids = self.tokenizer.encode(text)?;
                let ids_i64: Vec<i64> = ids.iter().map(|&x| x as i64).collect();
                max_len = max_len.max(ids_i64.len());
                all_ids.push(ids_i64);
            }

            let seq_len = max_len.min(self.max_input_tokens);

            let mut input_ids_arr = Array2::<i64>::zeros((batch, seq_len));
            let mut attention_mask_arr = Array2::<i64>::zeros((batch, seq_len));

            for (i, ids) in all_ids.iter().enumerate() {
                let len = ids.len().min(seq_len);
                for j in 0..len {
                    input_ids_arr[[i, j]] = ids[j];
                    attention_mask_arr[[i, j]] = 1;
                }
            }

            let input_tensor = ort::value::Tensor::from_array(input_ids_arr).map_err(|e| {
                SkbError::new(ErrorCode::Embedding, format!("input tensor: {:?}", e))
            })?;
            let mask_tensor = ort::value::Tensor::from_array(attention_mask_arr).map_err(|e| {
                SkbError::new(ErrorCode::Embedding, format!("mask tensor: {:?}", e))
            })?;

            let mut session = self.session.lock().unwrap();
            let outputs = session
                .run(ort::inputs! {
                    "input_ids" => input_tensor,
                    "attention_mask" => mask_tensor,
                })
                .map_err(|e| SkbError::new(ErrorCode::Embedding, format!("inference: {:?}", e)))?;

            let (_shape, data) = outputs["sentence_embedding"]
                .try_extract_tensor::<f32>()
                .map_err(|e| SkbError::new(ErrorCode::Embedding, format!("extract: {:?}", e)))?;

            let dim = self.dimension;

            let mut result = Vec::with_capacity(batch);
            for i in 0..batch {
                let start = i * dim;
                let end = start + dim;
                let mut vec = data[start..end].to_vec();
                super::l2_normalize(&mut vec);
                result.push(vec);
            }

            Ok(result)
        }
    }

    fn parse_hf_model(model: &str) -> (&str, &str) {
        let parts: Vec<&str> = model.splitn(2, '/').collect();
        (parts[0], parts.get(1).copied().unwrap_or(parts[0]))
    }
}

pub(crate) fn l2_normalize(vec: &mut [f32]) {
    let norm: f32 = vec.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        for v in vec.iter_mut() {
            *v /= norm;
        }
    }
}
