//! OpenAI-compatible streaming LLM client (plan todo 6).
//!
//! `POST {SKB_LLM_BASE_URL}/chat/completions` with `stream: true`; the SSE
//! body is parsed incrementally and text fragments (`choices[0].delta.content`)
//! are yielded as they arrive until `data: [DONE]`. Connection/HTTP/protocol
//! failures surface as typed [`LlmError`]s so the chat handler can emit them
//! as in-band SSE error events. The client caps hostile-upstream blast
//! radius: error bodies and single SSE frames are size-capped, idle chunk
//! reads time out, and an API key is never sent over plain HTTP.

use reqwest::Client;
use serde_json::json;
use std::fmt;
use std::time::Duration;

/// Environment variable selecting the OpenAI-compatible base URL.
pub const ENV_BASE_URL: &str = "SKB_LLM_BASE_URL";
/// Environment variable selecting the chat model.
pub const ENV_MODEL: &str = "SKB_LLM_MODEL";
/// Environment variable carrying the optional bearer token.
pub const ENV_API_KEY: &str = "SKB_LLM_API_KEY";

const DEFAULT_BASE_URL: &str = "http://localhost:11434/v1";
const DEFAULT_MODEL: &str = "llama3.1";

/// Cap on the non-2xx error body captured into `LlmError::Status` — a
/// hostile upstream must not turn one failed request into unbounded memory.
const MAX_ERROR_BODY: usize = 8 * 1024;
/// Cap on a single SSE line held in memory before its newline arrives.
const MAX_SSE_FRAME: usize = 64 * 1024;
/// Idle cap between SSE chunks — a silent upstream must not pin the chat
/// pipeline's task and connection forever.
const READ_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug)]
pub enum LlmError {
    /// DNS/connect/timeout/body-read failure against the LLM upstream.
    Connection(reqwest::Error),
    /// Upstream answered with a non-2xx status.
    Status { status: u16, body: String },
    /// The SSE body violated the OpenAI streaming contract.
    Protocol(String),
    /// The configured client options are unusable (e.g. an API key over
    /// plain HTTP).
    Config(String),
}

impl LlmError {
    /// Stable in-band SSE error code (`event: error` payload).
    pub fn code(&self) -> &'static str {
        match self {
            LlmError::Connection(_) => "E_LLM_CONNECTION",
            LlmError::Status { .. } => "E_LLM_STATUS",
            LlmError::Protocol(_) => "E_LLM_PROTOCOL",
            LlmError::Config(_) => "E_LLM_CONFIG",
        }
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::Connection(e) => write!(f, "LLM connection failed: {e}"),
            LlmError::Status { status, body } => {
                write!(f, "LLM returned HTTP {status}: {body}")
            }
            LlmError::Protocol(detail) => write!(f, "LLM stream protocol error: {detail}"),
            LlmError::Config(detail) => write!(f, "LLM client misconfigured: {detail}"),
        }
    }
}

impl std::error::Error for LlmError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            LlmError::Connection(e) => Some(e),
            _ => None,
        }
    }
}

pub struct LlmClient {
    http: Client,
    base_url: String,
    model: String,
    api_key: Option<String>,
}

impl LlmClient {
    /// Resolve the client from the environment. Read per request so tests and
    /// E2E scripts can point `SKB_LLM_BASE_URL` at a mock before calling.
    pub fn from_env() -> Result<Self, LlmError> {
        let base_url = std::env::var(ENV_BASE_URL).unwrap_or_else(|_| DEFAULT_BASE_URL.into());
        let model = std::env::var(ENV_MODEL).unwrap_or_else(|_| DEFAULT_MODEL.into());
        let api_key = std::env::var(ENV_API_KEY).ok().filter(|k| !k.is_empty());
        if api_key.is_some() && !base_url.trim_start().starts_with("https://") {
            // A bearer token over plain HTTP leaks the credential and every
            // prompt to whoever can sniff the wire — including a local
            // process that grabbed the port before the LLM came up.
            return Err(LlmError::Config(
                "SKB_LLM_API_KEY requires an https:// SKB_LLM_BASE_URL".to_string(),
            ));
        }
        let http = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .read_timeout(READ_TIMEOUT)
            .build()
            .map_err(LlmError::Connection)?;
        Ok(Self {
            http,
            base_url,
            model,
            api_key,
        })
    }

    /// Start a streaming chat completion; returns a handle yielding text
    /// fragments as they arrive.
    pub async fn stream_chat(&self, prompt: &str) -> Result<LlmStream, LlmError> {
        let url = format!("{}/chat/completions", self.base_url.trim_end_matches('/'));
        let mut request = self.http.post(url).json(&json!({
            "model": self.model,
            "messages": [{"role": "user", "content": prompt}],
            "stream": true,
        }));
        if let Some(key) = &self.api_key {
            request = request.bearer_auth(key);
        }
        let response = request.send().await.map_err(LlmError::Connection)?;
        let status = response.status();
        if !status.is_success() {
            let body = read_body_capped(response, MAX_ERROR_BODY).await;
            return Err(LlmError::Status {
                status: status.as_u16(),
                body,
            });
        }
        Ok(LlmStream {
            response,
            buffer: Vec::new(),
            finished: false,
        })
    }
}

/// Incremental reader over an OpenAI SSE body. Lines are decoded only at
/// newline boundaries so multibyte UTF-8 split across TCP chunks stays intact.
pub struct LlmStream {
    response: reqwest::Response,
    buffer: Vec<u8>,
    finished: bool,
}

/// Read at most `cap` bytes of a response body (lossy-UTF8), dropping the
/// rest — error bodies are diagnostic, never a data source.
async fn read_body_capped(response: reqwest::Response, cap: usize) -> String {
    let mut response = response;
    let mut body: Vec<u8> = Vec::new();
    while body.len() < cap {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = cap - body.len();
                let end = chunk.len().min(remaining);
                body.extend_from_slice(&chunk[..end]);
                if end < chunk.len() {
                    break;
                }
            }
            Ok(None) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

enum SseLine {
    Delta(String),
    Done,
    Skip,
}

impl LlmStream {
    /// Next `choices[0].delta.content` fragment, or `None` at `data: [DONE]`
    /// / end of body.
    pub async fn next_fragment(&mut self) -> Result<Option<String>, LlmError> {
        loop {
            if self.finished {
                return Ok(None);
            }
            if let Some(pos) = self.buffer.iter().position(|&b| b == b'\n') {
                let line = String::from_utf8_lossy(&self.buffer[..pos]).into_owned();
                self.buffer.drain(..=pos);
                match Self::parse_line(line.trim_end_matches('\r'))? {
                    SseLine::Delta(text) => return Ok(Some(text)),
                    SseLine::Done => {
                        self.finished = true;
                        return Ok(None);
                    }
                    SseLine::Skip => continue,
                }
            }
            match self.response.chunk().await {
                Ok(Some(chunk)) => {
                    if self.buffer.len() + chunk.len() > MAX_SSE_FRAME
                        && !chunk.contains(&b'\n')
                    {
                        self.finished = true;
                        return Err(LlmError::Protocol(format!(
                            "SSE frame exceeds {MAX_SSE_FRAME} bytes without a newline"
                        )));
                    }
                    self.buffer.extend_from_slice(&chunk);
                }
                Ok(None) => {
                    self.finished = true;
                    if self.buffer.is_empty() {
                        return Ok(None);
                    }
                    let rest = String::from_utf8_lossy(&self.buffer).into_owned();
                    self.buffer.clear();
                    return match Self::parse_line(rest.trim_end_matches('\r'))? {
                        SseLine::Delta(text) => Ok(Some(text)),
                        SseLine::Done | SseLine::Skip => Ok(None),
                    };
                }
                Err(e) => {
                    self.finished = true;
                    return Err(LlmError::Connection(e));
                }
            }
        }
    }

    fn parse_line(line: &str) -> Result<SseLine, LlmError> {
        let Some(data) = line.strip_prefix("data:") else {
            return Ok(SseLine::Skip); // event:/id:/`:` comments/blank lines
        };
        let data = data.strip_prefix(' ').unwrap_or(data);
        if data.trim() == "[DONE]" {
            return Ok(SseLine::Done);
        }
        let value: serde_json::Value = serde_json::from_str(data)
            .map_err(|e| LlmError::Protocol(format!("malformed data line: {e}")))?;
        let text = value["choices"][0]["delta"]["content"]
            .as_str()
            .unwrap_or_default();
        if text.is_empty() {
            Ok(SseLine::Skip) // role-only or usage-only chunk
        } else {
            Ok(SseLine::Delta(text.to_string()))
        }
    }
}
