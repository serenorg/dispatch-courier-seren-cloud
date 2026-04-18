use crate::protocol::{
    A2aAuthConfig, A2aEndpointMode, CourierInspection, CourierKind, LocalToolSpec, LocalToolTarget,
    MountConfig, ToolApprovalPolicy, ToolRiskLevel,
};
use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Deserialize)]
pub struct ParcelManifest {
    pub digest: String,
    pub courier: CourierTarget,
    pub entrypoint: Option<String>,
    pub instructions: Vec<InstructionConfig>,
    #[serde(default)]
    pub inline_prompts: Vec<String>,
    #[serde(default)]
    pub secrets: Vec<SecretSpec>,
    #[serde(default)]
    pub mounts: Vec<MountConfig>,
    #[serde(default)]
    pub tools: Vec<ToolConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CourierTarget {
    Native {
        reference: String,
    },
    Docker {
        reference: String,
    },
    Wasm {
        reference: String,
        #[allow(dead_code)]
        component: Option<serde_json::Value>,
    },
    Custom {
        reference: String,
    },
}

impl CourierTarget {
    pub fn reference(&self) -> &str {
        match self {
            Self::Native { reference }
            | Self::Docker { reference }
            | Self::Wasm { reference, .. }
            | Self::Custom { reference } => reference,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InstructionKind {
    Identity,
    Soul,
    Skill,
    Agents,
    User,
    Tools,
    Memory,
    Heartbeat,
    Eval,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InstructionConfig {
    pub kind: InstructionKind,
    pub packaged_path: String,
    #[serde(default)]
    pub skill_name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SecretSpec {
    pub name: String,
    pub required: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ToolConfig {
    Local(LocalToolConfig),
    Builtin(()),
    Mcp(()),
    A2a(A2aToolConfig),
}

#[derive(Debug, Clone, Deserialize)]
pub struct LocalToolConfig {
    pub alias: String,
    pub packaged_path: String,
    pub runner: CommandSpec,
    pub approval: Option<ToolApprovalPolicy>,
    pub risk: Option<ToolRiskLevel>,
    pub description: Option<String>,
    pub input_schema: Option<ToolInputSchemaRef>,
    #[serde(default)]
    pub skill_source: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct A2aToolConfig {
    pub alias: String,
    pub url: String,
    #[serde(default)]
    pub endpoint_mode: Option<A2aEndpointMode>,
    #[serde(default)]
    pub auth: Option<A2aAuthConfig>,
    #[serde(default)]
    pub expected_agent_name: Option<String>,
    #[serde(default)]
    pub expected_card_sha256: Option<String>,
    pub approval: Option<ToolApprovalPolicy>,
    pub risk: Option<ToolRiskLevel>,
    pub description: Option<String>,
    pub input_schema: Option<ToolInputSchemaRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolInputSchemaRef {
    pub packaged_path: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CommandSpec {
    pub command: String,
    pub args: Vec<String>,
}

pub fn load_manifest(parcel_dir: &Path) -> Result<ParcelManifest> {
    let manifest_path = parcel_dir.join("manifest.json");
    let body = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    serde_json::from_str(&body)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))
}

pub fn validate_custom_reference(manifest: &ParcelManifest) -> Result<()> {
    if !matches!(manifest.courier, CourierTarget::Custom { .. }) {
        bail!(
            "seren-cloud requires a custom courier target; got `{}`",
            manifest.courier.reference()
        );
    }

    let reference = manifest.courier.reference().to_ascii_lowercase();
    let supported = [
        "dispatch/custom",
        "dispatch/custom:<tag>",
        "dispatch/custom@<digest>",
        "custom",
    ];
    let matches = reference == "custom"
        || reference == "dispatch/custom"
        || reference.starts_with("dispatch/custom:")
        || reference.starts_with("dispatch/custom@");
    if matches {
        return Ok(());
    }

    bail!(
        "seren-cloud only accepts generic custom courier references; got `{}` (supported: {})",
        manifest.courier.reference(),
        supported.join(", ")
    )
}

pub fn inspection_from_manifest(manifest: &ParcelManifest) -> CourierInspection {
    CourierInspection {
        courier_id: "seren-cloud".to_string(),
        kind: CourierKind::Custom,
        entrypoint: manifest.entrypoint.clone(),
        required_secrets: manifest
            .secrets
            .iter()
            .filter(|secret| secret.required)
            .map(|secret| secret.name.clone())
            .collect(),
        mounts: manifest.mounts.clone(),
        local_tools: list_local_tools(manifest),
    }
}

pub fn list_local_tools(manifest: &ParcelManifest) -> Vec<LocalToolSpec> {
    manifest
        .tools
        .iter()
        .filter_map(|tool| match tool {
            ToolConfig::Local(local) => Some(LocalToolSpec {
                alias: local.alias.clone(),
                approval: local.approval,
                risk: local.risk,
                description: local.description.clone(),
                input_schema_packaged_path: local
                    .input_schema
                    .as_ref()
                    .map(|schema| schema.packaged_path.clone()),
                input_schema_sha256: local
                    .input_schema
                    .as_ref()
                    .map(|schema| schema.sha256.clone()),
                skill_source: local.skill_source.clone(),
                target: LocalToolTarget::Local {
                    packaged_path: local.packaged_path.clone(),
                    command: local.runner.command.clone(),
                    args: local.runner.args.clone(),
                },
            }),
            ToolConfig::A2a(a2a) => Some(LocalToolSpec {
                alias: a2a.alias.clone(),
                approval: a2a.approval,
                risk: a2a.risk,
                description: a2a.description.clone(),
                input_schema_packaged_path: a2a
                    .input_schema
                    .as_ref()
                    .map(|schema| schema.packaged_path.clone()),
                input_schema_sha256: a2a
                    .input_schema
                    .as_ref()
                    .map(|schema| schema.sha256.clone()),
                skill_source: None,
                target: LocalToolTarget::A2a {
                    endpoint_url: a2a.url.clone(),
                    endpoint_mode: a2a.endpoint_mode,
                    auth: a2a.auth.clone(),
                    expected_agent_name: a2a.expected_agent_name.clone(),
                    expected_card_sha256: a2a.expected_card_sha256.clone(),
                },
            }),
            ToolConfig::Builtin(_) | ToolConfig::Mcp(_) => None,
        })
        .collect()
}

pub fn resolve_prompt_text(parcel_dir: &Path, manifest: &ParcelManifest) -> Result<String> {
    let mut sections = Vec::new();
    for instruction in &manifest.instructions {
        if !matches!(
            instruction.kind,
            InstructionKind::Identity
                | InstructionKind::Soul
                | InstructionKind::Skill
                | InstructionKind::Agents
                | InstructionKind::User
                | InstructionKind::Tools
                | InstructionKind::Memory
                | InstructionKind::Heartbeat
        ) {
            continue;
        }

        let path = resolve_packaged_context_path(parcel_dir, &instruction.packaged_path)?;
        let body = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let body = if instruction.kind == InstructionKind::Skill && instruction.skill_name.is_some()
        {
            strip_skill_frontmatter(&body)
        } else {
            body
        };
        sections.push(format!(
            "# {}\n\n{}",
            instruction_heading(instruction.kind),
            body.trim_end()
        ));
    }

    for prompt in &manifest.inline_prompts {
        sections.push(format!("# PROMPT\n\n{}", prompt.trim_end()));
    }

    Ok(sections.join("\n\n"))
}

fn instruction_heading(kind: InstructionKind) -> &'static str {
    match kind {
        InstructionKind::Identity => "IDENTITY",
        InstructionKind::Soul => "SOUL",
        InstructionKind::Skill => "SKILL",
        InstructionKind::Agents => "AGENTS",
        InstructionKind::User => "USER",
        InstructionKind::Tools => "TOOLS",
        InstructionKind::Memory => "MEMORY",
        InstructionKind::Heartbeat => "HEARTBEAT",
        InstructionKind::Eval => "EVAL",
    }
}

fn strip_skill_frontmatter(body: &str) -> String {
    let mut lines = body.lines();
    if lines.next() != Some("---") {
        return body.to_string();
    }

    let mut seen_end = false;
    let mut remaining = Vec::new();
    for line in lines {
        if !seen_end && line == "---" {
            seen_end = true;
            continue;
        }
        if seen_end {
            remaining.push(line);
        }
    }

    if seen_end {
        remaining.join("\n")
    } else {
        body.to_string()
    }
}

pub fn manifest_json(parcel_dir: &Path) -> Result<serde_json::Value> {
    let manifest_path = parcel_dir.join("manifest.json");
    let body = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    serde_json::from_str(&body)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))
}

pub fn canonical_parcel_dir(parcel_dir: &str) -> Result<PathBuf> {
    Path::new(parcel_dir)
        .canonicalize()
        .with_context(|| format!("failed to resolve parcel dir `{parcel_dir}`"))
}

/// Defense-in-depth: guarantee that a manifest-declared `packaged_path` cannot
/// escape the parcel's context directory via `..` or an absolute root. The
/// dispatch build pipeline already rejects such paths at build time, but the
/// courier plugin reads whichever parcel the host hands it, so it validates
/// the shape again before touching the filesystem.
fn ensure_packaged_path_in_parcel(packaged_path: &str) -> Result<()> {
    let path = Path::new(packaged_path);
    if path.is_absolute() {
        bail!("packaged_path `{packaged_path}` must be relative to the parcel context directory");
    }
    for component in path.components() {
        match component {
            std::path::Component::ParentDir => {
                bail!("packaged_path `{packaged_path}` must not contain `..` components");
            }
            std::path::Component::Prefix(_) | std::path::Component::RootDir => {
                bail!(
                    "packaged_path `{packaged_path}` must be relative to the parcel context directory"
                );
            }
            _ => {}
        }
    }
    Ok(())
}

fn resolve_packaged_context_path(parcel_dir: &Path, packaged_path: &str) -> Result<PathBuf> {
    ensure_packaged_path_in_parcel(packaged_path)?;
    let context_dir = parcel_dir.join("context").canonicalize().with_context(|| {
        format!(
            "failed to resolve parcel context directory in `{}`",
            parcel_dir.display()
        )
    })?;
    let path = context_dir.join(packaged_path);
    let canonical_path = path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    if !canonical_path.starts_with(&context_dir) {
        bail!("packaged_path `{packaged_path}` escaped the parcel context directory");
    }
    Ok(canonical_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::time::{SystemTime, UNIX_EPOCH};

    fn base_manifest(courier: CourierTarget) -> ParcelManifest {
        ParcelManifest {
            digest: "digest".to_string(),
            courier,
            entrypoint: Some("chat".to_string()),
            instructions: Vec::new(),
            inline_prompts: Vec::new(),
            secrets: Vec::new(),
            mounts: Vec::new(),
            tools: Vec::new(),
        }
    }

    #[test]
    fn validate_custom_reference_accepts_generic_custom_targets() {
        let manifest = base_manifest(CourierTarget::Custom {
            reference: "dispatch/custom".to_string(),
        });
        validate_custom_reference(&manifest).unwrap();
    }

    #[test]
    fn validate_custom_reference_rejects_non_custom_targets() {
        let manifest = base_manifest(CourierTarget::Native {
            reference: "dispatch/custom".to_string(),
        });
        let error = validate_custom_reference(&manifest)
            .unwrap_err()
            .to_string();
        assert!(error.contains("requires a custom courier target"));
    }

    #[test]
    fn validate_custom_reference_accepts_tagged_custom_reference() {
        let manifest = base_manifest(CourierTarget::Custom {
            reference: "dispatch/custom:seren-cloud".to_string(),
        });
        validate_custom_reference(&manifest).unwrap();
    }

    #[test]
    fn ensure_packaged_path_rejects_parent_dir_escape() {
        let error = ensure_packaged_path_in_parcel("../../etc/passwd")
            .unwrap_err()
            .to_string();
        assert!(error.contains("must not contain `..`"));
    }

    #[test]
    fn ensure_packaged_path_rejects_absolute_path() {
        let error = ensure_packaged_path_in_parcel("/etc/passwd")
            .unwrap_err()
            .to_string();
        assert!(error.contains("must be relative"));
    }

    #[test]
    fn ensure_packaged_path_accepts_nested_relative_paths() {
        ensure_packaged_path_in_parcel("skills/telegram/skill.md").unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn resolve_packaged_context_path_rejects_symlink_escape() {
        use std::os::unix::fs::symlink;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("dispatch-seren-cloud-{unique}"));
        let context_dir = root.join("context");
        let outside_dir = root.join("outside");
        fs::create_dir_all(&context_dir).unwrap();
        fs::create_dir_all(&outside_dir).unwrap();
        fs::write(outside_dir.join("secret.txt"), "nope").unwrap();
        symlink(&outside_dir, context_dir.join("link")).unwrap();

        let error = resolve_packaged_context_path(&root, "link/secret.txt")
            .unwrap_err()
            .to_string();
        assert!(error.contains("escaped the parcel context directory"));

        fs::remove_dir_all(&root).unwrap();
    }
}
