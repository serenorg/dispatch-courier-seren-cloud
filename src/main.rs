use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead, Write},
    time::Instant,
};

mod parcel;
mod protocol;
mod seren_cloud_api;

use parcel::{
    canonical_parcel_dir, inspection_from_manifest, list_local_tools, load_manifest, manifest_json,
    resolve_prompt_text, validate_custom_reference,
};
use protocol::{
    COURIER_PLUGIN_PROTOCOL_VERSION, ConversationMessage, CourierEvent, CourierOperation,
    CourierSession, PluginRequest, PluginRequestEnvelope, PluginResponse, capabilities,
    parse_jsonrpc_request, plugin_error, response_to_jsonrpc,
};
use seren_cloud_api::{RunPayload, SerenCloudClient};

fn main() -> Result<()> {
    let stdin = io::stdin().lock();
    let mut stdout = io::stdout().lock();
    let mut client: Option<SerenCloudClient> = None;
    let mut active_backend: Option<BackendState> = None;

    for line in stdin.lines() {
        let line = line.context("failed to read stdin")?;
        if line.trim().is_empty() {
            continue;
        }

        let (request_id, envelope) = parse_jsonrpc_request(&line)
            .map_err(|error| anyhow!("failed to parse plugin request: {error}"))?;

        let responses = match handle_request(&envelope, &mut client, &mut active_backend) {
            Ok(responses) => responses,
            Err(error) => vec![plugin_error("internal_error", error.to_string())],
        };

        for response in responses {
            let json =
                response_to_jsonrpc(&request_id, &response).map_err(|error| anyhow!(error))?;
            writeln!(stdout, "{json}")?;
            stdout.flush()?;
        }
    }

    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BackendState {
    deployment_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_run_id: Option<String>,
}

fn handle_request(
    envelope: &PluginRequestEnvelope,
    client: &mut Option<SerenCloudClient>,
    active_backend: &mut Option<BackendState>,
) -> Result<Vec<PluginResponse>> {
    if envelope.protocol_version != COURIER_PLUGIN_PROTOCOL_VERSION {
        return Ok(vec![plugin_error(
            "unsupported_protocol_version",
            format!(
                "expected protocol_version {}, got {}",
                COURIER_PLUGIN_PROTOCOL_VERSION, envelope.protocol_version
            ),
        )]);
    }

    match &envelope.request {
        PluginRequest::Capabilities => Ok(vec![PluginResponse::Capabilities {
            capabilities: capabilities(),
        }]),
        PluginRequest::ValidateParcel { parcel_dir } => {
            let parcel_dir = canonical_parcel_dir(parcel_dir)?;
            let manifest = load_manifest(&parcel_dir)?;
            validate_custom_reference(&manifest)?;
            Ok(vec![PluginResponse::Ok])
        }
        PluginRequest::Inspect { parcel_dir } => {
            let parcel_dir = canonical_parcel_dir(parcel_dir)?;
            let manifest = load_manifest(&parcel_dir)?;
            validate_custom_reference(&manifest)?;
            Ok(vec![PluginResponse::Inspection {
                inspection: inspection_from_manifest(&manifest),
            }])
        }
        PluginRequest::OpenSession { parcel_dir } => {
            let parcel_dir = canonical_parcel_dir(parcel_dir)?;
            let manifest = load_manifest(&parcel_dir)?;
            validate_custom_reference(&manifest)?;
            let deployment = ensure_client(client)?.deploy(
                &manifest.digest,
                &manifest_json(&parcel_dir)?,
                &parcel_dir,
            )?;
            let backend = BackendState {
                deployment_id: deployment.id.clone(),
                last_run_id: None,
            };
            *active_backend = Some(backend.clone());

            Ok(vec![PluginResponse::Session {
                session: CourierSession {
                    id: format!("seren-cloud-{}", deployment.id),
                    parcel_digest: manifest.digest,
                    entrypoint: manifest.entrypoint,
                    label: None,
                    turn_count: 0,
                    elapsed_ms: 0,
                    history: Vec::new(),
                    resolved_mounts: Vec::new(),
                    backend_state: Some(serde_json::to_string(&backend)?),
                },
            }])
        }
        PluginRequest::ResumeSession {
            parcel_dir,
            session,
        } => {
            let parcel_dir = canonical_parcel_dir(parcel_dir)?;
            let manifest = load_manifest(&parcel_dir)?;
            validate_custom_reference(&manifest)?;
            if session.parcel_digest != manifest.digest {
                bail!(
                    "session parcel digest `{}` does not match parcel digest `{}`",
                    session.parcel_digest,
                    manifest.digest
                );
            }
            let backend = backend_state_from_session(session)?;
            let deployment = ensure_client(client)?.deployment_status(&backend.deployment_id)?;
            if is_terminal_deployment_status(&deployment.status) {
                bail!(
                    "seren cloud deployment `{}` is in terminal status `{}` and cannot be resumed",
                    backend.deployment_id,
                    deployment.status
                );
            }
            *active_backend = Some(backend.clone());

            let mut resumed = session.clone();
            resumed.parcel_digest = manifest.digest;
            resumed.entrypoint = manifest.entrypoint;
            resumed.backend_state = Some(serde_json::to_string(&backend)?);
            Ok(vec![PluginResponse::Session { session: resumed }])
        }
        PluginRequest::Run {
            parcel_dir,
            session,
            operation,
        } => run_request(parcel_dir, session, operation, client, active_backend),
        PluginRequest::Shutdown => {
            if let (Some(client), Some(state)) = (client.as_ref(), active_backend.as_ref())
                && let Err(error) = client.stop_deployment(&state.deployment_id)
            {
                eprintln!(
                    "seren-cloud: failed to stop deployment `{}`: {error:#}",
                    state.deployment_id
                );
            }
            *active_backend = None;
            Ok(vec![PluginResponse::Ok])
        }
    }
}

fn run_request(
    parcel_dir: &str,
    session: &CourierSession,
    operation: &CourierOperation,
    client: &mut Option<SerenCloudClient>,
    active_backend: &mut Option<BackendState>,
) -> Result<Vec<PluginResponse>> {
    let parcel_dir = canonical_parcel_dir(parcel_dir)?;
    let manifest = load_manifest(&parcel_dir)?;
    validate_custom_reference(&manifest)?;
    if session.parcel_digest != manifest.digest {
        bail!(
            "session parcel digest `{}` does not match parcel digest `{}`",
            session.parcel_digest,
            manifest.digest
        );
    }

    match operation {
        CourierOperation::ResolvePrompt => {
            let text = resolve_prompt_text(&parcel_dir, &manifest)?;
            Ok(done_with_events(
                session.clone(),
                vec![CourierEvent::PromptResolved { text }],
            ))
        }
        CourierOperation::ListLocalTools => Ok(done_with_events(
            session.clone(),
            vec![CourierEvent::LocalToolsListed {
                tools: list_local_tools(&manifest),
            }],
        )),
        CourierOperation::InvokeTool { invocation } => Ok(vec![plugin_error(
            "unsupported_operation",
            format!(
                "seren-cloud does not execute Dispatch invoke_tool operations yet (`{}`)",
                invocation.name
            ),
        )]),
        CourierOperation::Chat { input } => run_remote_turn(
            session,
            RunPayload::Chat {
                input: input.clone(),
            },
            client,
            active_backend,
        ),
        CourierOperation::Job { payload } => run_remote_turn(
            session,
            RunPayload::Job {
                payload: payload.clone(),
            },
            client,
            active_backend,
        ),
        CourierOperation::Heartbeat { payload } => run_remote_turn(
            session,
            RunPayload::Heartbeat {
                payload: payload.clone(),
            },
            client,
            active_backend,
        ),
    }
}

fn run_remote_turn(
    session: &CourierSession,
    payload: RunPayload,
    client: &mut Option<SerenCloudClient>,
    active_backend: &mut Option<BackendState>,
) -> Result<Vec<PluginResponse>> {
    let mut next_session = session.clone();
    let mut backend = session
        .backend_state
        .as_deref()
        .map(parse_backend_state)
        .transpose()?
        .or_else(|| active_backend.clone())
        .ok_or_else(|| anyhow!("run requires session backend_state with deployment metadata"))?;

    let started_at = Instant::now();
    let run_result = ensure_client(client)?.start_run(&backend.deployment_id, &payload)?;
    backend.last_run_id = Some(run_result.run_id);
    *active_backend = Some(backend.clone());

    next_session.turn_count += 1;
    next_session.elapsed_ms = next_session
        .elapsed_ms
        .saturating_add(started_at.elapsed().as_millis() as u64);
    next_session.backend_state = Some(serde_json::to_string(&backend)?);

    match &payload {
        RunPayload::Chat { input } => {
            next_session.history.push(ConversationMessage {
                role: "user".to_string(),
                content: input.clone(),
            });
        }
        RunPayload::Job { payload } => {
            next_session.history.push(ConversationMessage {
                role: "user".to_string(),
                content: payload.clone(),
            });
        }
        RunPayload::Heartbeat { .. } => {}
    }

    if let Some(output) = &run_result.output {
        next_session.history.push(ConversationMessage {
            role: "assistant".to_string(),
            content: output.clone(),
        });
    }

    let mut events = run_result
        .events
        .into_iter()
        .map(|event| match event.role {
            Some(role) => CourierEvent::Message {
                role,
                content: event.content,
            },
            None => CourierEvent::TextDelta {
                content: event.content,
            },
        })
        .collect::<Vec<_>>();

    if events.is_empty()
        && let Some(output) = &run_result.output
    {
        events.push(CourierEvent::Message {
            role: "assistant".to_string(),
            content: output.clone(),
        });
    }

    Ok(done_with_events(next_session, events))
}

fn done_with_events(session: CourierSession, mut events: Vec<CourierEvent>) -> Vec<PluginResponse> {
    events.push(CourierEvent::Done);
    let mut responses = events
        .into_iter()
        .map(|event| PluginResponse::Event { event })
        .collect::<Vec<_>>();
    responses.push(PluginResponse::Done { session });
    responses
}

fn ensure_client(client: &mut Option<SerenCloudClient>) -> Result<&SerenCloudClient> {
    if client.is_none() {
        *client = Some(SerenCloudClient::from_env()?);
    }
    Ok(client
        .as_ref()
        .expect("seren-cloud client was initialized above"))
}

fn is_terminal_deployment_status(status: &str) -> bool {
    matches!(
        status.to_ascii_lowercase().as_str(),
        "stopped" | "terminated" | "failed" | "error"
    )
}

fn backend_state_from_session(session: &CourierSession) -> Result<BackendState> {
    let backend_state = session
        .backend_state
        .as_deref()
        .ok_or_else(|| anyhow!("session is missing backend_state"))?;
    parse_backend_state(backend_state)
}

fn parse_backend_state(input: &str) -> Result<BackendState> {
    serde_json::from_str(input).context("failed to parse session backend_state")
}

#[cfg(test)]
mod tests {
    use super::is_terminal_deployment_status;
    use serde::Deserialize;
    use std::{fs, path::PathBuf};

    #[derive(Debug, Deserialize)]
    struct CatalogDocument {
        entries: Vec<CatalogEntry>,
    }

    #[derive(Debug, Deserialize)]
    struct CatalogEntry {
        name: String,
        kind: String,
        protocol: String,
        protocol_version: u32,
        manifest_path: String,
        manifest_url: String,
        install_hint: String,
        source: InstallSource,
        requirements: Requirements,
    }

    #[derive(Debug, Deserialize)]
    #[serde(tag = "type", rename_all = "snake_case")]
    enum InstallSource {
        GithubRelease {
            repo: String,
            tag: String,
            checksum_asset: String,
            binaries: Vec<ReleaseBinary>,
        },
    }

    #[derive(Debug, Deserialize)]
    struct ReleaseBinary {
        target: String,
        asset: String,
        binary_name: String,
    }

    #[derive(Debug, Deserialize)]
    struct Requirements {
        secrets: Vec<String>,
        optional_secrets: Vec<String>,
        network_domains: Vec<String>,
    }

    #[derive(Debug, Deserialize)]
    struct CourierManifest {
        kind: String,
        name: String,
        protocol_version: u32,
        transport: String,
    }

    fn repo_path(relative: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
    }

    #[test]
    fn terminal_statuses_are_recognized() {
        for status in ["stopped", "Terminated", "FAILED", "error"] {
            assert!(
                is_terminal_deployment_status(status),
                "expected `{status}` to be terminal"
            );
        }
    }

    #[test]
    fn non_terminal_statuses_pass_through() {
        for status in ["running", "pending", "deploying", "unknown", ""] {
            assert!(
                !is_terminal_deployment_status(status),
                "expected `{status}` to be non-terminal"
            );
        }
    }

    #[test]
    fn published_catalog_entry_matches_plugin_manifest_and_runtime_requirements() {
        let catalog: CatalogDocument = serde_json::from_slice(
            &fs::read(repo_path("catalog/extensions.json")).expect("catalog should exist"),
        )
        .expect("catalog should parse");
        let manifest: CourierManifest = serde_json::from_slice(
            &fs::read(repo_path("courier-plugin.json")).expect("manifest should exist"),
        )
        .expect("manifest should parse");

        let entry = catalog
            .entries
            .iter()
            .find(|entry| entry.name == "seren-cloud")
            .expect("seren-cloud catalog entry should exist");

        assert_eq!(entry.kind, "courier");
        assert_eq!(entry.name, manifest.name);
        assert_eq!(entry.protocol, manifest.transport);
        assert_eq!(entry.protocol_version, manifest.protocol_version);
        assert_eq!(entry.manifest_path, "courier-plugin.json");
        assert_eq!(
            entry.manifest_url,
            "https://raw.githubusercontent.com/serenorg/dispatch-courier-seren-cloud/v0.1.0/courier-plugin.json"
        );
        assert_eq!(entry.install_hint, "dispatch extension install seren-cloud");
        match &entry.source {
            InstallSource::GithubRelease {
                repo,
                tag,
                checksum_asset,
                binaries,
            } => {
                assert_eq!(repo, "serenorg/dispatch-courier-seren-cloud");
                assert_eq!(tag, "v0.1.0");
                assert_eq!(checksum_asset, "SHA256SUMS.txt");
                assert_eq!(binaries.len(), 5);
                assert!(binaries.iter().any(|binary| {
                    binary.target == "x86_64-unknown-linux-gnu"
                        && binary.asset == "dispatch-courier-seren-cloud-x86_64-unknown-linux-gnu"
                        && binary.binary_name == "dispatch-courier-seren-cloud"
                }));
                assert!(binaries.iter().any(|binary| {
                    binary.target == "x86_64-pc-windows-msvc"
                        && binary.asset == "dispatch-courier-seren-cloud-x86_64-pc-windows-msvc.exe"
                        && binary.binary_name == "dispatch-courier-seren-cloud.exe"
                }));
            }
        }
        assert_eq!(entry.requirements.secrets, vec!["SEREN_API_KEY"]);
        assert_eq!(entry.requirements.optional_secrets, vec!["SEREN_API_BASE"]);
        assert_eq!(entry.requirements.network_domains, vec!["api.serendb.com"]);
        assert_eq!(manifest.kind, "courier");
    }
}
