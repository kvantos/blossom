//! Embeddings from an OpenAI-compatible server (`POST {base}/embeddings`),
//! such as Ollama, LM Studio, llama.cpp's server or `OpenAI` itself.

use std::time::Duration;

use serde_json::{Value, json};

use crate::{Embedder, SemanticError};

/// Where and what to ask.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerSettings {
    /// API base, e.g. `http://localhost:11434/v1`; a URL already ending in
    /// `/embeddings` is used as is.
    pub url: String,
    /// The model name, e.g. `nomic-embed-text`.
    pub model: String,
    /// Sent as `Authorization: Bearer …` when set.
    pub api_key: Option<String>,
}

/// The default API base: Ollama on this machine.
pub const DEFAULT_URL: &str = "http://localhost:11434/v1";

/// The default model: Ollama's common embedding model.
pub const DEFAULT_MODEL: &str = "nomic-embed-text";

/// An embedder backed by a server. Asks once on connect to learn the
/// vector length.
#[derive(Debug)]
pub struct ServerEmbedder {
    agent: ureq::Agent,
    endpoint: String,
    settings: ServerSettings,
    id: String,
    dims: usize,
}

impl ServerEmbedder {
    /// Connects: embeds a probe text to check the server and learn the
    /// vector length.
    ///
    /// # Errors
    ///
    /// When the server can't be reached or answers with an error.
    pub fn connect(settings: ServerSettings) -> Result<ServerEmbedder, SemanticError> {
        let config = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(Duration::from_secs(300)))
            .http_status_as_error(false)
            .build();
        let base = settings.url.trim().trim_end_matches('/');
        let endpoint = if base.ends_with("/embeddings") {
            base.to_string()
        } else {
            format!("{base}/embeddings")
        };
        let mut e = ServerEmbedder {
            agent: ureq::Agent::new_with_config(config),
            endpoint,
            id: format!("server-{}", settings.model),
            settings,
            dims: 0,
        };
        let probe = e.request(&["probe"])?;
        e.dims = probe.first().map_or(0, Vec::len);
        if e.dims == 0 {
            return Err(SemanticError::Embed(format!(
                "{} returned an empty embedding",
                e.endpoint
            )));
        }
        Ok(e)
    }

    fn request(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SemanticError> {
        let fail = |what: String| SemanticError::Embed(format!("{}: {what}", self.endpoint));
        let mut req = self
            .agent
            .post(&self.endpoint)
            .header("Content-Type", "application/json");
        if let Some(key) = &self.settings.api_key {
            req = req.header("Authorization", &format!("Bearer {key}"));
        }
        let body = json!({ "model": self.settings.model, "input": texts });
        let mut resp = req.send_json(&body).map_err(|e| fail(e.to_string()))?;
        let status = resp.status();
        let text = resp
            .body_mut()
            .with_config()
            .limit(256 * 1024 * 1024)
            .read_to_string()
            .map_err(|e| fail(e.to_string()))?;
        if !status.is_success() {
            return Err(fail(format!("{status}: {}", error_message(&text))));
        }
        let value: Value = serde_json::from_str(&text).map_err(|e| fail(e.to_string()))?;
        let vectors = parse_embeddings(&value).ok_or_else(|| fail("unexpected answer".into()))?;
        if vectors.len() == texts.len() {
            Ok(vectors)
        } else {
            Err(SemanticError::Shape {
                expected: texts.len(),
                got: vectors.len(),
            })
        }
    }
}

impl Embedder for ServerEmbedder {
    fn id(&self) -> &str {
        &self.id
    }

    fn dims(&self) -> usize {
        self.dims
    }

    fn embed_passages(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SemanticError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        self.request(texts)
    }

    fn embed_query(&self, text: &str) -> Result<Vec<f32>, SemanticError> {
        self.request(&[text])?.pop().ok_or(SemanticError::Shape {
            expected: 1,
            got: 0,
        })
    }
}

/// `data[].embedding`, ordered by `data[].index` when given.
#[allow(clippy::cast_possible_truncation)] // JSON numbers are f64; vectors are f32
fn parse_embeddings(value: &Value) -> Option<Vec<Vec<f32>>> {
    let mut items: Vec<(u64, Vec<f32>)> = Vec::new();
    for (i, item) in value.get("data")?.as_array()?.iter().enumerate() {
        let index = item
            .get("index")
            .and_then(Value::as_u64)
            .unwrap_or(i as u64);
        let v = item
            .get("embedding")?
            .as_array()?
            .iter()
            .map(|x| x.as_f64().map(|f| f as f32))
            .collect::<Option<Vec<f32>>>()?;
        items.push((index, v));
    }
    items.sort_by_key(|(i, _)| *i);
    Some(items.into_iter().map(|(_, v)| v).collect())
}

/// The server's error message (`error.message` or `error`), else the body.
fn error_message(body: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let msg = parsed.as_ref().and_then(|v| {
        let e = v.get("error")?;
        e.get("message")
            .and_then(Value::as_str)
            .or_else(|| e.as_str())
            .map(String::from)
    });
    let msg = msg.unwrap_or_else(|| body.trim().to_string());
    msg.chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;
    use std::thread;

    use super::*;

    /// A one-thread HTTP server: answers each request with `answer(body)`
    /// as `(status, json)`, and reports each request's headers and body.
    fn serve(
        answer: impl Fn(&Value) -> (u16, String) + Send + 'static,
    ) -> (String, mpsc::Receiver<(String, Value)>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = stream.unwrap();
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut head = String::new();
                let mut len = 0;
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if let Some(v) = line.to_lowercase().strip_prefix("content-length:") {
                        len = v.trim().parse().unwrap();
                    }
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    head.push_str(&line);
                }
                let mut body = vec![0; len];
                reader.read_exact(&mut body).unwrap();
                let body: Value = serde_json::from_slice(&body).unwrap();
                let (status, json) = answer(&body);
                let _ = tx.send((head, body));
                write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{json}",
                    json.len()
                )
                .unwrap();
            }
        });
        (url, rx)
    }

    /// Embeds each input as `[len, 1, 0]`, listing items in reverse order.
    fn lengths(body: &Value) -> (u16, String) {
        let input = body["input"].as_array().unwrap();
        let data: Vec<Value> = input
            .iter()
            .enumerate()
            .rev()
            .map(|(i, t)| json!({"index": i, "embedding": [t.as_str().unwrap().len(), 1, 0]}))
            .collect();
        (200, json!({ "data": data }).to_string())
    }

    #[test]
    fn embeds_in_order_with_key() {
        let (url, seen) = serve(lengths);
        let e = ServerEmbedder::connect(ServerSettings {
            url: format!("{url}/"),
            model: "m".into(),
            api_key: Some("secret".into()),
        })
        .unwrap();
        assert_eq!((e.id(), e.dims()), ("server-m", 3));
        let (head, body) = seen.recv().unwrap();
        assert!(head.starts_with("POST /v1/embeddings "));
        assert!(head.contains("Bearer secret"));
        assert_eq!(body["model"], "m");
        let v = e.embed_passages(&["a", "abc"]).unwrap();
        assert_eq!(v, [vec![1.0, 1.0, 0.0], vec![3.0, 1.0, 0.0]]);
        assert_eq!(e.embed_query("ab").unwrap(), [2.0, 1.0, 0.0]);
    }

    #[test]
    fn reports_server_errors() {
        let (url, _seen) = serve(|_| {
            (
                404,
                r#"{"error":{"message":"model \"m\" not found"}}"#.into(),
            )
        });
        let err = ServerEmbedder::connect(ServerSettings {
            url,
            model: "m".into(),
            api_key: None,
        })
        .unwrap_err()
        .to_string();
        assert!(
            err.contains("404") && err.contains("model \"m\" not found"),
            "{err}"
        );
    }

    #[test]
    fn unreachable_server_is_an_error() {
        // Bind and drop: nothing listens there now.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let r = ServerEmbedder::connect(ServerSettings {
            url: format!("http://127.0.0.1:{port}/v1/embeddings"),
            model: "m".into(),
            api_key: None,
        });
        assert!(matches!(r, Err(SemanticError::Embed(m)) if m.contains("/v1/embeddings:")));
    }

    #[test]
    fn error_messages() {
        assert_eq!(error_message(r#"{"error":"bad"}"#), "bad");
        assert_eq!(error_message("plain\n"), "plain");
    }
}
