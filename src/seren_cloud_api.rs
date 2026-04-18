use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, io::Read, path::Path, time::Duration};

const DEFAULT_API_BASE: &str = "https://api.serendb.com";
const DEFAULT_HTTP_TIMEOUT: Duration = Duration::from_secs(30);

pub struct SerenCloudClient {
    api_key: String,
    base_url: String,
}

impl fmt::Debug for SerenCloudClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never surface the Seren Cloud API key through Debug. A struct-level
        // `#[derive(Debug)]` would leak the key into panics, trace output, or
        // anywhere `{:?}` is used in operator-facing code paths.
        f.debug_struct("SerenCloudClient")
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .finish()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    pub id: String,
    pub status: String,
}

#[derive(Debug, Clone)]
pub enum RunPayload {
    Chat { input: String },
    Job { payload: String },
    Heartbeat { payload: Option<String> },
}

#[derive(Debug)]
pub struct RemoteRunEvent {
    pub role: Option<String>,
    pub content: String,
}

#[derive(Debug)]
pub struct RunResult {
    pub run_id: String,
    pub output: Option<String>,
    pub events: Vec<RemoteRunEvent>,
}

impl SerenCloudClient {
    pub fn from_env() -> Result<Self> {
        let api_key = std::env::var("SEREN_API_KEY")
            .context("SEREN_API_KEY is required for the seren-cloud courier")?;
        let base_url = std::env::var("SEREN_API_BASE")
            .unwrap_or_else(|_| DEFAULT_API_BASE.to_string())
            .trim_end_matches('/')
            .to_string();
        Ok(Self { api_key, base_url })
    }

    pub fn deploy(&self, digest: &str, manifest: &Value, parcel_dir: &Path) -> Result<Deployment> {
        let url = format!("{}/deploy", self.base_url);
        let body = self.post_json(
            &url,
            serde_json::json!({
                "parcel_digest": digest,
                "parcel_manifest": manifest,
                "source_parcel_dir": parcel_dir.display().to_string(),
            }),
            "failed to deploy parcel to Seren Cloud",
        )?;
        parse_deployment(&body, digest)
    }

    pub fn deployment_status(&self, deployment_id: &str) -> Result<Deployment> {
        let url = format!("{}/deployments/{}", self.base_url, deployment_id);
        let body = self.get_json(&url, "failed to get deployment status")?;
        parse_deployment(&body, deployment_id)
    }

    pub fn start_run(&self, deployment_id: &str, payload: &RunPayload) -> Result<RunResult> {
        let url = format!("{}/deployments/{}/runs", self.base_url, deployment_id);
        let body = self.post_json(
            &url,
            run_payload_json(payload),
            "failed to start run on Seren Cloud",
        )?;

        let run_id = body
            .get("id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("run response missing id"))?
            .to_string();
        let output = body.get("output").and_then(Value::as_str).map(String::from);

        let mut events = Vec::new();
        if let Some(items) = body.get("events").and_then(Value::as_array) {
            for item in items {
                let content = item
                    .get("content")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("run event missing content"))?;
                events.push(RemoteRunEvent {
                    role: item.get("role").and_then(Value::as_str).map(String::from),
                    content: content.to_string(),
                });
            }
        } else if let Some(output) = &output {
            events.push(RemoteRunEvent {
                role: Some("assistant".to_string()),
                content: output.clone(),
            });
        }

        Ok(RunResult {
            run_id,
            output,
            events,
        })
    }

    pub fn stop_deployment(&self, deployment_id: &str) -> Result<()> {
        let url = format!("{}/deployments/{}/stop", self.base_url, deployment_id);
        let _ = self.post_json(&url, serde_json::json!({}), "failed to stop deployment")?;
        Ok(())
    }

    fn get_json(&self, url: &str, context: &str) -> Result<Value> {
        let mut response = self
            .agent()
            .get(url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .call()
            .map_err(|error| anyhow!("{context}: {error}"))?;
        read_json_body(&mut response, context)
    }

    fn post_json(&self, url: &str, payload: Value, context: &str) -> Result<Value> {
        let mut response = self
            .agent()
            .post(url)
            .header("Authorization", &format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .send_json(payload)
            .map_err(|error| anyhow!("{context}: {error}"))?;
        read_json_body(&mut response, context)
    }

    fn agent(&self) -> ureq::Agent {
        ureq::Agent::config_builder()
            .timeout_global(Some(DEFAULT_HTTP_TIMEOUT))
            .build()
            .new_agent()
    }
}

fn run_payload_json(payload: &RunPayload) -> Value {
    match payload {
        RunPayload::Chat { input } => serde_json::json!({
            "operation": "chat",
            "input": input,
        }),
        RunPayload::Job { payload } => serde_json::json!({
            "operation": "job",
            "payload": payload,
        }),
        RunPayload::Heartbeat { payload } => serde_json::json!({
            "operation": "heartbeat",
            "payload": payload,
        }),
    }
}

fn parse_deployment(body: &Value, fallback_id: &str) -> Result<Deployment> {
    let id = body
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or(fallback_id)
        .to_string();
    let status = body
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_string();
    if id.is_empty() {
        bail!("deployment response missing id")
    }
    Ok(Deployment { id, status })
}

fn read_json_body(response: &mut ureq::http::Response<ureq::Body>, context: &str) -> Result<Value> {
    let status = response.status();
    let mut body = response
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .reader();
    let mut text = String::new();
    body.read_to_string(&mut text)
        .with_context(|| format!("{context}: failed to read response body"))?;
    if !status.is_success() {
        bail!("{context}: HTTP {}: {}", status.as_u16(), text);
    }
    serde_json::from_str(&text)
        .with_context(|| format!("{context}: failed to parse response body as JSON"))
}
