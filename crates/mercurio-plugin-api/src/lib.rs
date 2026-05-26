use std::collections::BTreeMap;

use mercurio_reasoner_api::CapabilityDescriptor;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const PLUGIN_ABI_VERSION: &str = "0.1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginManifest {
    pub id: String,
    pub version: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub requires: PluginRequirements,
    #[serde(default)]
    pub libraries: Vec<String>,
    #[serde(default)]
    pub rulepacks: Vec<String>,
    #[serde(default)]
    pub views: Vec<String>,
    #[serde(default)]
    pub ui_contributions: Vec<String>,
    #[serde(default)]
    pub services: Vec<PluginServiceDeclaration>,
    #[serde(default)]
    pub verification_actions: Vec<VerificationActionDeclaration>,
    #[serde(default)]
    pub capabilities: Vec<PluginCapabilityDeclaration>,
    pub permissions: PluginPermissions,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginRequirements {
    pub mercurio: String,
    pub kir: String,
    pub plugin_abi: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_api: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCapabilityDeclaration {
    pub capability: CapabilityDescriptor,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default)]
    pub input_schemas: Vec<String>,
    #[serde(default)]
    pub output_schemas: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginServiceDeclaration {
    pub id: String,
    pub runtime: PluginServiceRuntime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub command: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginServiceRuntime {
    Wasm,
    ExternalProcess,
    Http,
    InProcess,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationActionDeclaration {
    pub id: String,
    pub service: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginPermissions {
    pub filesystem: FilesystemPermission,
    pub network: NetworkPermission,
    pub source_mutation: bool,
    pub nondeterminism: bool,
}

impl PluginPermissions {
    pub fn pure() -> Self {
        Self {
            filesystem: FilesystemPermission::None,
            network: NetworkPermission::None,
            source_mutation: false,
            nondeterminism: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemPermission {
    None,
    ReadDeclared,
    WorkspaceRead,
    WorkspaceWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NetworkPermission {
    None,
    DeclaredHosts,
    Any,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginValidationError {
    pub code: String,
    pub message: String,
}

pub fn validate_manifest(manifest: &PluginManifest) -> Result<(), Vec<PluginValidationError>> {
    let mut errors = Vec::new();

    require_non_empty(&mut errors, "id", &manifest.id);
    require_non_empty(&mut errors, "version", &manifest.version);
    require_non_empty(&mut errors, "name", &manifest.name);
    require_non_empty(
        &mut errors,
        "requires.mercurio",
        &manifest.requires.mercurio,
    );
    require_non_empty(&mut errors, "requires.kir", &manifest.requires.kir);
    require_non_empty(
        &mut errors,
        "requires.plugin_abi",
        &manifest.requires.plugin_abi,
    );

    if manifest.requires.plugin_abi != PLUGIN_ABI_VERSION {
        errors.push(PluginValidationError {
            code: "unsupported_plugin_abi".to_string(),
            message: format!(
                "plugin requires ABI `{}`, host supports `{}`",
                manifest.requires.plugin_abi, PLUGIN_ABI_VERSION
            ),
        });
    }

    for service in &manifest.services {
        validate_service(&mut errors, service);
    }

    for declaration in &manifest.capabilities {
        require_non_empty(
            &mut errors,
            "capabilities[].capability.id",
            &declaration.capability.id,
        );
        if let Some(service_id) = &declaration.service {
            if !manifest
                .services
                .iter()
                .any(|service| &service.id == service_id)
            {
                errors.push(PluginValidationError {
                    code: "missing_capability_service".to_string(),
                    message: format!(
                        "capability `{}` references undeclared service `{service_id}`",
                        declaration.capability.id
                    ),
                });
            }
        }
    }

    for action in &manifest.verification_actions {
        if !manifest
            .services
            .iter()
            .any(|service| service.id == action.service)
        {
            errors.push(PluginValidationError {
                code: "missing_verification_service".to_string(),
                message: format!(
                    "verification action `{}` references undeclared service `{}`",
                    action.id, action.service
                ),
            });
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

fn validate_service(errors: &mut Vec<PluginValidationError>, service: &PluginServiceDeclaration) {
    require_non_empty(errors, "services[].id", &service.id);

    match service.runtime {
        PluginServiceRuntime::Wasm => {
            if service.module.as_deref().is_none_or(str::is_empty) {
                errors.push(PluginValidationError {
                    code: "missing_wasm_module".to_string(),
                    message: format!("WASM service `{}` must declare module", service.id),
                });
            }
            if service.function.as_deref().is_none_or(str::is_empty) {
                errors.push(PluginValidationError {
                    code: "missing_wasm_function".to_string(),
                    message: format!("WASM service `{}` must declare function", service.id),
                });
            }
        }
        PluginServiceRuntime::ExternalProcess => {
            if service.command.as_ref().is_none_or(Vec::is_empty) {
                errors.push(PluginValidationError {
                    code: "missing_external_command".to_string(),
                    message: format!(
                        "external process service `{}` must declare command",
                        service.id
                    ),
                });
            }
        }
        PluginServiceRuntime::Http => {
            if service.endpoint.as_deref().is_none_or(str::is_empty) {
                errors.push(PluginValidationError {
                    code: "missing_http_endpoint".to_string(),
                    message: format!("HTTP service `{}` must declare endpoint", service.id),
                });
            }
        }
        PluginServiceRuntime::InProcess => {}
    }
}

fn require_non_empty(errors: &mut Vec<PluginValidationError>, field: &str, value: &str) {
    if value.trim().is_empty() {
        errors.push(PluginValidationError {
            code: "missing_required_field".to_string(),
            message: format!("manifest field `{field}` is required"),
        });
    }
}

#[cfg(test)]
mod tests {
    use mercurio_reasoner_api::{CapabilityKind, REASONING_API_VERSION};

    use super::*;

    #[test]
    fn validates_contract_analysis_manifest() {
        let manifest = PluginManifest {
            id: "org.mercurio.contracts.pacti".to_string(),
            version: "0.1.0".to_string(),
            name: "Pacti Contract Analysis".to_string(),
            description: Some("Assume-guarantee contract analysis.".to_string()),
            requires: PluginRequirements {
                mercurio: ">=0.1.0".to_string(),
                kir: ">=0.1".to_string(),
                plugin_abi: PLUGIN_ABI_VERSION.to_string(),
                reasoning_api: Some(REASONING_API_VERSION.to_string()),
            },
            libraries: Vec::new(),
            rulepacks: vec!["rules/contracts.rulepack.json".to_string()],
            views: Vec::new(),
            ui_contributions: Vec::new(),
            services: vec![PluginServiceDeclaration {
                id: "contract.compatibility".to_string(),
                runtime: PluginServiceRuntime::Wasm,
                module: Some("wasm/contracts.wasm".to_string()),
                function: Some("compatibility".to_string()),
                command: None,
                endpoint: None,
            }],
            verification_actions: vec![VerificationActionDeclaration {
                id: "contract_compatibility".to_string(),
                service: "contract.compatibility".to_string(),
                input_schema: None,
                output_schema: None,
            }],
            capabilities: vec![PluginCapabilityDeclaration {
                capability: CapabilityDescriptor {
                    id: "contract.compatibility".to_string(),
                    kind: CapabilityKind::ContractAnalysis,
                    name: "Contract Compatibility".to_string(),
                    version: "0.1.0".to_string(),
                    api_version: REASONING_API_VERSION.to_string(),
                    deterministic: true,
                    input_artifact_kinds: vec!["kir".to_string(), "facts".to_string()],
                    output_artifact_kinds: vec!["finding".to_string(), "evidence".to_string()],
                },
                service: Some("contract.compatibility".to_string()),
                input_schemas: Vec::new(),
                output_schemas: Vec::new(),
            }],
            permissions: PluginPermissions::pure(),
            metadata: BTreeMap::new(),
        };

        validate_manifest(&manifest).expect("manifest should be valid");

        let encoded = serde_json::to_string(&manifest).expect("manifest serializes");
        let decoded: PluginManifest =
            serde_json::from_str(&encoded).expect("manifest deserializes");
        assert_eq!(decoded.id, "org.mercurio.contracts.pacti");
        assert_eq!(
            decoded.capabilities[0].capability.kind,
            CapabilityKind::ContractAnalysis
        );
    }

    #[test]
    fn rejects_capability_with_missing_service() {
        let mut manifest = PluginManifest {
            id: "org.example.bad".to_string(),
            version: "0.1.0".to_string(),
            name: "Bad Plugin".to_string(),
            description: None,
            requires: PluginRequirements {
                mercurio: ">=0.1.0".to_string(),
                kir: ">=0.1".to_string(),
                plugin_abi: PLUGIN_ABI_VERSION.to_string(),
                reasoning_api: Some(REASONING_API_VERSION.to_string()),
            },
            libraries: Vec::new(),
            rulepacks: Vec::new(),
            views: Vec::new(),
            ui_contributions: Vec::new(),
            services: Vec::new(),
            verification_actions: Vec::new(),
            capabilities: Vec::new(),
            permissions: PluginPermissions::pure(),
            metadata: BTreeMap::new(),
        };
        manifest.capabilities.push(PluginCapabilityDeclaration {
            capability: CapabilityDescriptor {
                id: "custom.bad".to_string(),
                kind: CapabilityKind::CustomReasoning,
                name: "Bad".to_string(),
                version: "0.1.0".to_string(),
                api_version: REASONING_API_VERSION.to_string(),
                deterministic: false,
                input_artifact_kinds: Vec::new(),
                output_artifact_kinds: Vec::new(),
            },
            service: Some("missing.service".to_string()),
            input_schemas: Vec::new(),
            output_schemas: Vec::new(),
        });

        let errors = validate_manifest(&manifest).expect_err("manifest should be invalid");
        assert!(
            errors
                .iter()
                .any(|error| error.code == "missing_capability_service")
        );
    }
}
