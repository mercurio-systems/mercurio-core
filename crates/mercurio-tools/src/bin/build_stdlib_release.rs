use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

use mercurio_core::frontend::transpile::{KirEmissionSeed, PilotConstructSeed};
use mercurio_core::{
    Graph, KparPackageBuild, MpackLanguageProfile, MpackLibrary, MpackManifest, MpackPythonPackage,
    MpackPythonWrapperBinding, MpackRequirements, MpackRulepack, RulePack,
    generate_python_wrappers, load_language_profile, load_pilot_export, normalize_pilot_export,
    repo_path, repo_root, validate_mpack_manifest, write_kpar_package,
};
use mercurio_tools::sha256_file;
use serde::Serialize;
use serde_json::{Value, json};
use zip::write::FileOptions;

const DEFAULT_PROFILE_ID: &str = "sysml-2.0-pilot-0.57.0";
const DEFAULT_SPEC_VERSION: &str = "2.0.0";
const DEFAULT_STDLIB_PACKAGE: &str = "org.omg/sysml-stdlib";
const DEFAULT_MPACK_ID: &str = "org.mercurio.sysml-stdlib-support";
const DEFAULT_WRAPPER_MODULE: &str = "mercurio_sysml_2_0";

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse()?;
    let raw_export_path = prepare_raw_export(&args)?;
    let export_digest = digest_file(&raw_export_path)?;
    let export_sha256 = sha256_file(&raw_export_path)?;
    let export = load_pilot_export(&raw_export_path)?;
    let source_id = args
        .source_id
        .clone()
        .or_else(|| infer_source_id(&args, export.metadata.as_ref()))
        .unwrap_or_else(|| "pilot-unknown".to_string());
    let release_root = args.out.clone().unwrap_or_else(|| {
        repo_path(&format!(
            "artifacts/stdlib/sysml-{}/{}",
            args.spec_version,
            safe_path_segment(&source_id)
        ))
    });

    create_release_dirs(&release_root)?;
    let locked_export_path = release_root.join("raw/pilot-stdlib-export.json");
    copy_if_different(&raw_export_path, &locked_export_path)?;

    let profile = load_language_profile(&args.profile_id)?;
    let mut kir = normalize_pilot_export(export.clone())?;
    kir.metadata = build_kir_metadata(&args, &source_id, &export_digest, &export_sha256, &export);
    if args.audit_profile {
        audit_profile_inputs(&args, &profile, &kir)?;
    }

    let kir_path = release_root.join("kir/stdlib.full.kir.json");
    kir.write_pretty_to_path(&kir_path)?;
    let kir_digest = digest_file(&kir_path)?;

    let rulepack = RulePack::metamodel_adapter_from_graph(&Graph::from_document(kir.clone())?);
    let rulepack_path = release_root.join("rules/stdlib.rulepack.json");
    rulepack.write_pretty_to_path(&rulepack_path)?;
    let rulepack_digest = digest_file(&rulepack_path)?;

    let kpar_path = release_root.join(format!("kpar/sysml-stdlib-{}.kpar", args.spec_version));
    write_kpar_package(
        &kpar_path,
        &KparPackageBuild {
            name: args.stdlib_package.clone(),
            version: Some(args.spec_version.clone()),
            sources: Vec::new(),
            precompiled_kir: Some(kir.clone()),
        },
    )?;
    let kpar_digest = digest_file(&kpar_path)?;

    let profile_source = repo_path(&format!(
        "resources/language-profiles/{}/profile.json",
        args.profile_id
    ));
    let profile_mapping_sources = [
        repo_path(&format!(
            "resources/language-profiles/{}/mappings/pilot_constructs.seed.json",
            args.profile_id
        )),
        repo_path(&format!(
            "resources/language-profiles/{}/mappings/kir_emission.seed.json",
            args.profile_id
        )),
    ];
    let profile_target = release_root
        .join("profiles")
        .join(&args.profile_id)
        .join("profile.json");
    copy_if_different(&profile_source, &profile_target)?;
    let profile_mapping_targets = profile_mapping_sources
        .iter()
        .map(|source| {
            let target = release_root
                .join("profiles")
                .join(&args.profile_id)
                .join("mappings")
                .join(source.file_name().expect("mapping file name"));
            copy_if_different(source, &target)?;
            Ok::<_, Box<dyn std::error::Error>>(target)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let profile_digest = digest_file(&profile_target)?;
    let profile_mappings_digest = digest_paths(&release_root, &profile_mapping_targets)?;

    let generated = generate_python_wrappers(&kir, &profile, &args.wrapper_module);
    let python_root = release_root.join("python");
    let mut wrapper_files = Vec::new();
    for (relative, content) in generated.files {
        let path = python_root.join(path_from_slashes(&relative));
        write_text(&path, &content)?;
        wrapper_files.push(path);
    }
    wrapper_files.sort();
    let wrappers_digest = digest_paths(&release_root, &wrapper_files)?;

    let mpack_manifest = build_mpack_manifest(
        &args,
        &source_id,
        &export_digest,
        &kir_digest,
        &kpar_digest,
        &profile_digest,
        &profile_mappings_digest,
        &rulepack_digest,
        &wrappers_digest,
    );
    if let Err(errors) = validate_mpack_manifest(&mpack_manifest) {
        let message = errors
            .into_iter()
            .map(|error| error.to_string())
            .collect::<Vec<_>>()
            .join("; ");
        return Err(message.into());
    }

    let mpack_path = release_root.join(format!(
        "mpack/{}-{}.mpack",
        args.mpack_id, args.mpack_version
    ));
    write_mpack(
        &mpack_path,
        &release_root,
        &mpack_manifest,
        &kpar_path,
        &profile_target,
        &profile_mapping_targets,
        &rulepack_path,
        &wrapper_files,
    )?;
    let mpack_digest = digest_file(&mpack_path)?;

    let source_lock = SourceLock {
        schema: "dev.mercurio.stdlib-source-lock.v1",
        spec_version: &args.spec_version,
        source_id: &source_id,
        raw_export: path_to_slash("raw/pilot-stdlib-export.json"),
        raw_export_digest: &export_digest,
        raw_export_sha256: &export_sha256,
        pilot_root: args
            .pilot_root
            .as_ref()
            .map(|path| path_to_slash_path(path)),
        pilot_commit: args
            .pilot_root
            .as_deref()
            .and_then(|path| git_stdout(path, ["rev-parse", "HEAD"])),
        pilot_dirty: args.pilot_root.as_deref().and_then(git_dirty),
    };
    let source_lock_path = release_root.join("source.lock.json");
    write_json(&source_lock_path, &source_lock)?;

    let release_artifacts = BTreeMap::from([
        (
            "raw_export".to_string(),
            Artifact {
                path: path_to_slash("raw/pilot-stdlib-export.json"),
                digest: export_digest.clone(),
            },
        ),
        (
            "kir".to_string(),
            Artifact {
                path: path_to_slash("kir/stdlib.full.kir.json"),
                digest: kir_digest.clone(),
            },
        ),
        (
            "rulepack".to_string(),
            Artifact {
                path: path_to_slash("rules/stdlib.rulepack.json"),
                digest: rulepack_digest.clone(),
            },
        ),
        (
            "kpar".to_string(),
            Artifact {
                path: path_to_slash_path(kpar_path.strip_prefix(&release_root)?),
                digest: kpar_digest.clone(),
            },
        ),
        (
            "profile".to_string(),
            Artifact {
                path: path_to_slash_path(profile_target.strip_prefix(&release_root)?),
                digest: profile_digest.clone(),
            },
        ),
        (
            "profile_mappings".to_string(),
            Artifact {
                path: format!("profiles/{}/mappings", args.profile_id),
                digest: profile_mappings_digest.clone(),
            },
        ),
        (
            "python_wrappers".to_string(),
            Artifact {
                path: path_to_slash("python"),
                digest: wrappers_digest.clone(),
            },
        ),
        (
            "mpack".to_string(),
            Artifact {
                path: path_to_slash_path(mpack_path.strip_prefix(&release_root)?),
                digest: mpack_digest.clone(),
            },
        ),
    ]);

    let provenance = BuildProvenance {
        schema: "dev.mercurio.stdlib-release.v1",
        spec_version: &args.spec_version,
        source_id: &source_id,
        profile_id: &args.profile_id,
        stdlib_package: &args.stdlib_package,
        mpack_id: &args.mpack_id,
        mpack_version: &args.mpack_version,
        wrapper_module: &args.wrapper_module,
        mercurio_tools_version: env!("CARGO_PKG_VERSION"),
        artifacts: release_artifacts.clone(),
    };
    let provenance_path = release_root.join("build.provenance.json");
    write_json(&provenance_path, &provenance)?;

    let release_lock = ReleaseLock {
        schema: "dev.mercurio.stdlib-release-lock.v1",
        spec_version: &args.spec_version,
        source_id: &source_id,
        profile_id: &args.profile_id,
        stdlib_package: &args.stdlib_package,
        mpack_id: &args.mpack_id,
        mpack_version: &args.mpack_version,
        wrapper_module: &args.wrapper_module,
        mercurio_tools_version: env!("CARGO_PKG_VERSION"),
        release_root: path_to_slash_path(&release_root),
        source: ReleaseSource {
            raw_export: Artifact {
                path: path_to_slash("raw/pilot-stdlib-export.json"),
                digest: export_digest,
            },
            raw_export_sha256: export_sha256.clone(),
            pilot_root: args
                .pilot_root
                .as_ref()
                .map(|path| path_to_slash_path(path)),
            pilot_commit: args
                .pilot_root
                .as_deref()
                .and_then(|path| git_stdout(path, ["rev-parse", "HEAD"])),
            pilot_dirty: args.pilot_root.as_deref().and_then(git_dirty),
        },
        profile: ReleaseProfile {
            profile: Artifact {
                path: path_to_slash_path(profile_target.strip_prefix(&release_root)?),
                digest: profile_digest,
            },
            mappings: Artifact {
                path: format!("profiles/{}/mappings", args.profile_id),
                digest: profile_mappings_digest,
            },
            pilot_grammars: pilot_grammar_artifacts(args.pilot_root.as_deref())?,
        },
        artifacts: release_artifacts,
    };
    let release_lock_path = release_root.join("release.lock.json");
    write_json(&release_lock_path, &release_lock)?;

    if args.promote {
        promote_release(
            &args,
            &release_root,
            &locked_export_path,
            &kir_path,
            &rulepack_path,
            &kpar_path,
            &release_lock_path,
        )?;
    }

    if args.check_reproducible {
        let second_kpar = release_root.join("target/repro/sysml-stdlib.kpar");
        write_kpar_package(
            &second_kpar,
            &KparPackageBuild {
                name: args.stdlib_package.clone(),
                version: Some(args.spec_version.clone()),
                sources: Vec::new(),
                precompiled_kir: Some(kir),
            },
        )?;
        let second_digest = digest_file(&second_kpar)?;
        if second_digest != provenance.artifacts["kpar"].digest {
            return Err(format!(
                "KPAR is not reproducible: first {}, second {}",
                provenance.artifacts["kpar"].digest, second_digest
            )
            .into());
        }
    }

    println!("release: {}", release_root.display());
    println!(
        "mpack: {}",
        release_root
            .join(&provenance.artifacts["mpack"].path)
            .display()
    );
    Ok(())
}

#[derive(Debug, Clone)]
struct Args {
    from_export: Option<PathBuf>,
    pilot_root: Option<PathBuf>,
    out: Option<PathBuf>,
    spec_version: String,
    profile_id: String,
    source_id: Option<String>,
    stdlib_package: String,
    mpack_id: String,
    mpack_version: String,
    wrapper_module: String,
    check_reproducible: bool,
    audit_profile: bool,
    promote: bool,
}

impl Args {
    fn parse() -> Result<Self, Box<dyn std::error::Error>> {
        let mut args = Self {
            from_export: None,
            pilot_root: None,
            out: None,
            spec_version: DEFAULT_SPEC_VERSION.to_string(),
            profile_id: DEFAULT_PROFILE_ID.to_string(),
            source_id: None,
            stdlib_package: DEFAULT_STDLIB_PACKAGE.to_string(),
            mpack_id: DEFAULT_MPACK_ID.to_string(),
            mpack_version: DEFAULT_SPEC_VERSION.to_string(),
            wrapper_module: DEFAULT_WRAPPER_MODULE.to_string(),
            check_reproducible: false,
            audit_profile: false,
            promote: false,
        };
        let raw = std::env::args().skip(1).collect::<Vec<_>>();
        let mut index = 0;
        while index < raw.len() {
            match raw[index].as_str() {
                "--from-export" => args.from_export = Some(next_path(&raw, &mut index)?),
                "--pilot-root" => args.pilot_root = Some(next_path(&raw, &mut index)?),
                "--out" => args.out = Some(next_path(&raw, &mut index)?),
                "--spec-version" => args.spec_version = next_string(&raw, &mut index)?,
                "--profile-id" => args.profile_id = next_string(&raw, &mut index)?,
                "--source-id" => args.source_id = Some(next_string(&raw, &mut index)?),
                "--stdlib-package" => args.stdlib_package = next_string(&raw, &mut index)?,
                "--mpack-id" => args.mpack_id = next_string(&raw, &mut index)?,
                "--mpack-version" => args.mpack_version = next_string(&raw, &mut index)?,
                "--wrapper-module" => args.wrapper_module = next_string(&raw, &mut index)?,
                "--check-reproducible" => args.check_reproducible = true,
                "--audit-profile" => args.audit_profile = true,
                "--promote" => args.promote = true,
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                unknown => return Err(format!("unknown argument: {unknown}").into()),
            }
            index += 1;
        }
        if args.from_export.is_none() && args.pilot_root.is_none() {
            return Err("expected --from-export PATH or --pilot-root PATH".into());
        }
        Ok(args)
    }
}

#[derive(Serialize)]
struct SourceLock<'a> {
    schema: &'a str,
    spec_version: &'a str,
    source_id: &'a str,
    raw_export: String,
    raw_export_digest: &'a str,
    raw_export_sha256: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pilot_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pilot_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pilot_dirty: Option<bool>,
}

#[derive(Serialize)]
struct BuildProvenance<'a> {
    schema: &'a str,
    spec_version: &'a str,
    source_id: &'a str,
    profile_id: &'a str,
    stdlib_package: &'a str,
    mpack_id: &'a str,
    mpack_version: &'a str,
    wrapper_module: &'a str,
    mercurio_tools_version: &'a str,
    artifacts: BTreeMap<String, Artifact>,
}

#[derive(Serialize)]
struct ReleaseLock<'a> {
    schema: &'a str,
    spec_version: &'a str,
    source_id: &'a str,
    profile_id: &'a str,
    stdlib_package: &'a str,
    mpack_id: &'a str,
    mpack_version: &'a str,
    wrapper_module: &'a str,
    mercurio_tools_version: &'a str,
    release_root: String,
    source: ReleaseSource,
    profile: ReleaseProfile,
    artifacts: BTreeMap<String, Artifact>,
}

#[derive(Serialize)]
struct ReleaseSource {
    raw_export: Artifact,
    raw_export_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pilot_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pilot_commit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pilot_dirty: Option<bool>,
}

#[derive(Serialize)]
struct ReleaseProfile {
    profile: Artifact,
    mappings: Artifact,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pilot_grammars: BTreeMap<String, Artifact>,
}

#[derive(Serialize)]
struct PromotedPackageManifest<'a> {
    schema: &'a str,
    name: &'a str,
    version: &'a str,
    kind: &'a str,
    file: String,
    digest: String,
    created_at: String,
    source: PromotedPackageSource,
}

#[derive(Serialize)]
struct PromotedPackageSource {
    kind: String,
    path: String,
    digest: String,
}

#[derive(Clone, Serialize)]
struct Artifact {
    path: String,
    digest: String,
}

fn print_usage() {
    println!(
        "Usage: cargo run -p mercurio-tools --bin build_stdlib_release -- [--from-export PATH | --pilot-root PATH] [--out PATH] [--spec-version VERSION] [--profile-id ID] [--source-id ID] [--check-reproducible] [--audit-profile] [--promote]"
    );
}

fn next_string(args: &[String], index: &mut usize) -> Result<String, Box<dyn std::error::Error>> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| "missing argument value".into())
}

fn next_path(args: &[String], index: &mut usize) -> Result<PathBuf, Box<dyn std::error::Error>> {
    Ok(PathBuf::from(next_string(args, index)?))
}

fn prepare_raw_export(args: &Args) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = &args.from_export {
        return Ok(path.clone());
    }
    let pilot_root = args
        .pilot_root
        .as_deref()
        .ok_or("expected --from-export or --pilot-root")?;
    let export_path = repo_path("target/stdlib-release/pilot-stdlib-export.json");
    export_from_pilot(pilot_root, &export_path)?;
    Ok(export_path)
}

fn create_release_dirs(root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    for name in [
        "raw",
        "kir",
        "rules",
        "kpar",
        "profiles",
        "python",
        "mpack",
        "target/repro",
    ] {
        std::fs::create_dir_all(root.join(name))?;
    }
    Ok(())
}

fn audit_profile_inputs(
    args: &Args,
    profile: &mercurio_core::LanguageProfile,
    kir: &mercurio_core::KirDocument,
) -> Result<(), Box<dyn std::error::Error>> {
    let profile_dir = repo_path(&format!("resources/language-profiles/{}", args.profile_id));
    let constructs_path = profile_dir
        .join("mappings")
        .join("pilot_constructs.seed.json");
    let emission_path = profile_dir.join("mappings").join("kir_emission.seed.json");
    let constructs: PilotConstructSeed =
        serde_json::from_str(&std::fs::read_to_string(&constructs_path)?)?;
    let emission: KirEmissionSeed =
        serde_json::from_str(&std::fs::read_to_string(&emission_path)?)?;

    let mut errors = Vec::new();
    if profile.id != args.profile_id {
        errors.push(format!(
            "profile id `{}` does not match requested profile `{}`",
            profile.id, args.profile_id
        ));
    }

    let stdlib_ids = kir
        .elements
        .iter()
        .map(|element| element.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for (concept, target) in &profile.canonical_kinds {
        if !stdlib_ids.contains(target.as_str()) {
            errors.push(format!(
                "canonical concept `{concept:?}` points at missing stdlib element `{target}`"
            ));
        }
    }
    for (alias, target) in &profile.aliases {
        if !stdlib_ids.contains(target.as_str()) {
            errors.push(format!(
                "profile alias `{alias}` points at missing stdlib element `{target}`"
            ));
        }
    }

    let mut construct_to_metaclass = BTreeMap::new();
    for entry in &constructs.constructs {
        if let Some(previous) =
            construct_to_metaclass.insert(entry.construct.clone(), entry.metaclass.clone())
        {
            errors.push(format!(
                "construct `{}` maps to both `{previous}` and `{}`",
                entry.construct, entry.metaclass
            ));
        }
    }
    for entry in &constructs.constructs {
        if !emission.metaclasses.contains_key(&entry.metaclass) {
            errors.push(format!(
                "construct `{}` maps to metaclass `{}` with no KIR emission rule",
                entry.construct, entry.metaclass
            ));
        }
    }
    for (keyword, construct) in constructs
        .keyword_registry
        .definitions
        .iter()
        .chain(constructs.keyword_registry.usages.iter())
    {
        let Some(metaclass) = construct_to_metaclass
            .get(construct)
            .map(String::as_str)
            .or_else(|| fallback_metaclass_for_construct(construct))
        else {
            errors.push(format!(
                "keyword `{keyword}` points at construct `{construct}` with no construct mapping"
            ));
            continue;
        };
        if !emission.metaclasses.contains_key(metaclass) {
            errors.push(format!(
                "keyword `{keyword}` resolves to metaclass `{metaclass}` with no KIR emission rule"
            ));
        }
    }

    audit_stdlib_refs(
        &mut errors,
        "package default specialization",
        &constructs.default_specialization_anchors.packages,
        &stdlib_ids,
    );
    audit_stdlib_refs(
        &mut errors,
        "definition default specialization",
        &constructs.default_specialization_anchors.definitions,
        &stdlib_ids,
    );
    audit_stdlib_refs(
        &mut errors,
        "usage default specialization",
        &constructs.default_specialization_anchors.usages,
        &stdlib_ids,
    );
    audit_stdlib_vec_refs(
        &mut errors,
        "definition semantic specialization",
        &constructs.semantic_specialization_defaults.definitions,
        &stdlib_ids,
    );
    audit_stdlib_vec_refs(
        &mut errors,
        "usage semantic specialization",
        &constructs.semantic_specialization_defaults.usages,
        &stdlib_ids,
    );
    for (construct, overrides) in &constructs.usage_semantic_specialization_overrides.usages {
        audit_stdlib_vec_refs(
            &mut errors,
            &format!("usage semantic override for `{construct}`"),
            overrides,
            &stdlib_ids,
        );
    }
    audit_stdlib_refs(
        &mut errors,
        "stdlib alias",
        &constructs.stdlib_aliases.ids,
        &stdlib_ids,
    );

    if errors.is_empty() {
        println!("profile audit: ok");
        Ok(())
    } else {
        Err(format!("profile audit failed:\n  {}", errors.join("\n  ")).into())
    }
}

fn audit_stdlib_refs(
    errors: &mut Vec<String>,
    label: &str,
    refs: &BTreeMap<String, String>,
    stdlib_ids: &std::collections::BTreeSet<&str>,
) {
    for (key, target) in refs {
        if !stdlib_ids.contains(target.as_str()) {
            errors.push(format!(
                "{label} `{key}` points at missing stdlib element `{target}`"
            ));
        }
    }
}

fn audit_stdlib_vec_refs(
    errors: &mut Vec<String>,
    label: &str,
    refs: &BTreeMap<String, Vec<String>>,
    stdlib_ids: &std::collections::BTreeSet<&str>,
) {
    for (key, targets) in refs {
        for target in targets {
            if !stdlib_ids.contains(target.as_str()) {
                errors.push(format!(
                    "{label} `{key}` points at missing stdlib element `{target}`"
                ));
            }
        }
    }
}

fn fallback_metaclass_for_construct(construct: &str) -> Option<&'static str> {
    if construct.ends_with("Usage") {
        Some("KerML::Feature")
    } else if construct.ends_with("Definition") {
        Some("KerML::Classifier")
    } else {
        None
    }
}

fn build_kir_metadata(
    args: &Args,
    source_id: &str,
    export_digest: &str,
    export_sha256: &str,
    export: &mercurio_core::PilotExportDocument,
) -> BTreeMap<String, Value> {
    let mut metadata = BTreeMap::new();
    metadata.insert("import_source".to_string(), json!("pilot"));
    metadata.insert("source_id".to_string(), json!(source_id));
    metadata.insert("profile_id".to_string(), json!(args.profile_id));
    metadata.insert("stdlib_version".to_string(), json!(args.spec_version));
    metadata.insert("kir_schema_version".to_string(), json!("0.2"));
    metadata.insert("input_export_digest".to_string(), json!(export_digest));
    metadata.insert("input_export_sha256".to_string(), json!(export_sha256));
    metadata.insert(
        "input_export_path".to_string(),
        json!(
            args.from_export
                .as_ref()
                .map(|path| path_to_slash_path(path))
                .unwrap_or_else(|| path_to_slash("target/stdlib-release/pilot-stdlib-export.json"))
        ),
    );
    metadata.insert("element_count".to_string(), json!(export.elements.len()));
    metadata.insert(
        "relationship_count".to_string(),
        json!(export.relationships.len()),
    );
    if let Some(source_metadata) = &export.metadata {
        metadata.insert("source_export".to_string(), source_metadata.clone());
    }
    metadata
}

fn build_mpack_manifest(
    args: &Args,
    source_id: &str,
    export_digest: &str,
    kir_digest: &str,
    kpar_digest: &str,
    profile_digest: &str,
    profile_mappings_digest: &str,
    rulepack_digest: &str,
    wrappers_digest: &str,
) -> MpackManifest {
    MpackManifest {
        id: args.mpack_id.clone(),
        version: args.mpack_version.clone(),
        name: "SysML Standard Library Support".to_string(),
        kind: Some("stdlib_support".to_string()),
        description: Some(
            "Mercurio support package for the OMG SysML standard library: KPAR content, language profile, rulepack, and generated Python typed wrappers.".to_string(),
        ),
        requires: Some(MpackRequirements {
            mercurio: Some(">=0.1.0".to_string()),
            kir: Some("0.2".to_string()),
            plugin_abi: Some("mpack-0.1".to_string()),
        }),
        libraries: vec![MpackLibrary {
            id: Some(args.stdlib_package.clone()),
            path: Some(format!("libraries/sysml-stdlib-{}.kpar", args.spec_version)),
            locator: Some(format!("kpar:{}:{}", args.stdlib_package, args.spec_version)),
            sha256: None,
            role: Some("baseline".to_string()),
        }],
        language_profiles: vec![MpackLanguageProfile {
            id: args.profile_id.clone(),
            path: format!("profiles/{}/profile.json", args.profile_id),
            stdlib: Some(format!("libraries/sysml-stdlib-{}.kpar", args.spec_version)),
            python_wrappers: Some(MpackPythonWrapperBinding {
                module: args.wrapper_module.clone(),
                path: "python".to_string(),
                entrypoint: Some(format!("{}:register", args.wrapper_module)),
            }),
        }],
        rulepacks: vec![MpackRulepack {
            path: "rules/stdlib.rulepack.json".to_string(),
            id: Some("org.mercurio.sysml-stdlib-rules".to_string()),
        }],
        python_packages: vec![MpackPythonPackage {
            module: args.wrapper_module.clone(),
            path: "python".to_string(),
            profile: Some(args.profile_id.clone()),
            entrypoint: Some(format!("{}:register", args.wrapper_module)),
        }],
        services: Vec::new(),
        metadata: BTreeMap::from([(
            "release".to_string(),
            json!({
                "source_id": source_id,
                "raw_export_digest": export_digest,
                "kir_digest": kir_digest,
                "kpar_digest": kpar_digest,
                "profile_digest": profile_digest,
                "profile_mappings_digest": profile_mappings_digest,
                "rulepack_digest": rulepack_digest,
                "python_wrappers_digest": wrappers_digest
            }),
        )]),
    }
}

fn write_mpack(
    path: &Path,
    release_root: &Path,
    manifest: &MpackManifest,
    kpar_path: &Path,
    profile_path: &Path,
    profile_mapping_paths: &[PathBuf],
    rulepack_path: &Path,
    wrapper_files: &[PathBuf],
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = File::create(path)?;
    let mut archive = zip::ZipWriter::new(file);
    let options = FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    write_zip_json(&mut archive, "extension.json", manifest, options)?;
    write_zip_file(
        &mut archive,
        &format!(
            "libraries/{}",
            kpar_path.file_name().unwrap().to_string_lossy()
        ),
        kpar_path,
        options,
    )?;
    write_zip_file(
        &mut archive,
        &path_to_slash_path(profile_path.strip_prefix(release_root)?),
        profile_path,
        options,
    )?;
    for mapping_path in profile_mapping_paths {
        write_zip_file(
            &mut archive,
            &path_to_slash_path(mapping_path.strip_prefix(release_root)?),
            mapping_path,
            options,
        )?;
    }
    write_zip_file(
        &mut archive,
        "rules/stdlib.rulepack.json",
        rulepack_path,
        options,
    )?;
    for wrapper_file in wrapper_files {
        write_zip_file(
            &mut archive,
            &path_to_slash_path(wrapper_file.strip_prefix(release_root)?),
            wrapper_file,
            options,
        )?;
    }
    archive.finish()?;
    Ok(())
}

fn write_zip_json<W, T>(
    archive: &mut zip::ZipWriter<W>,
    path: &str,
    value: &T,
    options: FileOptions,
) -> Result<(), Box<dyn std::error::Error>>
where
    W: Write + Seek,
    T: Serialize,
{
    archive.start_file(path, options)?;
    archive.write_all(&serde_json::to_vec_pretty(value)?)?;
    archive.write_all(b"\n")?;
    Ok(())
}

fn write_zip_file<W>(
    archive: &mut zip::ZipWriter<W>,
    archive_path: &str,
    source_path: &Path,
    options: FileOptions,
) -> Result<(), Box<dyn std::error::Error>>
where
    W: Write + Seek,
{
    archive.start_file(archive_path, options)?;
    archive.write_all(&std::fs::read(source_path)?)?;
    Ok(())
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{}\n", serde_json::to_string_pretty(value)?))?;
    Ok(())
}

fn write_text(path: &Path, content: &str) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

fn copy_if_different(source: &Path, target: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if paths_equivalent(source, target) {
        return Ok(());
    }
    let source_bytes = std::fs::read(source)
        .map_err(|err| format!("failed to read source `{}`: {err}", source.display()))?;
    if target.exists()
        && std::fs::read(target).ok().as_deref() == Some(source_bytes.as_slice())
    {
        return Ok(());
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| format!("failed to create `{}`: {err}", parent.display()))?;
    }
    std::fs::write(target, source_bytes)
        .map_err(|err| format!("failed to write `{}`: {err}", target.display()))?;
    Ok(())
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn promote_release(
    args: &Args,
    release_root: &Path,
    raw_export_path: &Path,
    kir_path: &Path,
    rulepack_path: &Path,
    kpar_path: &Path,
    release_lock_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let resource_root = repo_path(&format!("resources/stdlib-sources/{}", args.profile_id));
    copy_if_different(
        raw_export_path,
        &resource_root.join("pilot-stdlib-export.json"),
    )?;
    copy_if_different(kir_path, &resource_root.join("stdlib.full.kir.json"))?;
    copy_if_different(rulepack_path, &resource_root.join("stdlib.rulepack.json"))?;
    copy_if_different(release_lock_path, &resource_root.join("release.lock.json"))?;
    promote_bundled_kpar(args, release_root, kpar_path)?;
    println!(
        "promoted stdlib resources: {}",
        path_to_slash_path(
            resource_root
                .strip_prefix(repo_root())
                .unwrap_or(resource_root.as_path())
        )
    );
    println!("release artifacts remain: {}", release_root.display());
    Ok(())
}

fn promote_bundled_kpar(
    args: &Args,
    release_root: &Path,
    kpar_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let package_dir = repo_path("packages")
        .join(path_from_slashes(&args.stdlib_package))
        .join(&args.spec_version);
    let file_name = kpar_path
        .file_name()
        .ok_or("KPAR path has no file name")?
        .to_string_lossy()
        .to_string();
    let package_path = package_dir.join(&file_name);
    copy_if_different(kpar_path, &package_path)?;
    let package_digest = digest_file(&package_path)?;
    let release_relative = path_to_slash_path(kpar_path.strip_prefix(release_root)?);
    let manifest = PromotedPackageManifest {
        schema: "dev.mercurio.local-package.v1",
        name: &args.stdlib_package,
        version: &args.spec_version,
        kind: "kpar",
        file: file_name,
        digest: package_digest,
        created_at: format!(
            "release:{}",
            args.source_id.as_deref().unwrap_or(&args.profile_id)
        ),
        source: PromotedPackageSource {
            kind: "stdlib-release".to_string(),
            path: release_relative,
            digest: digest_file(kpar_path)?,
        },
    };
    write_json(&package_dir.join("manifest.json"), &manifest)?;
    println!(
        "promoted bundled KPAR: {}",
        path_to_slash_path(
            package_path
                .strip_prefix(repo_root())
                .unwrap_or(package_path.as_path())
        )
    );
    Ok(())
}

fn digest_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(format_stable_digest([(
        "file".as_bytes(),
        bytes.as_slice(),
    )]))
}

fn digest_paths(root: &Path, paths: &[PathBuf]) -> Result<String, Box<dyn std::error::Error>> {
    let mut chunks = Vec::new();
    for path in paths {
        let relative = path_to_slash_path(path.strip_prefix(root)?);
        chunks.push(("path".to_string(), relative.into_bytes()));
        chunks.push(("file".to_string(), std::fs::read(path)?));
    }
    Ok(format_stable_digest(chunks.iter().map(|(label, bytes)| {
        (label.as_bytes(), bytes.as_slice())
    })))
}

fn pilot_grammar_artifacts(
    pilot_root: Option<&Path>,
) -> Result<BTreeMap<String, Artifact>, Box<dyn std::error::Error>> {
    let Some(pilot_root) = pilot_root else {
        return Ok(BTreeMap::new());
    };
    let candidates = [
        (
            "sysml_xtext",
            "org.omg.sysml.xtext/src/org/omg/sysml/xtext/SysML.xtext",
        ),
        (
            "kerml_xtext",
            "org.omg.kerml.xtext/src/org/omg/kerml/xtext/KerML.xtext",
        ),
    ];

    let mut artifacts = BTreeMap::new();
    for (name, relative) in candidates {
        let path = pilot_root.join(path_from_slashes(relative));
        if path.exists() {
            artifacts.insert(
                name.to_string(),
                Artifact {
                    path: path_to_slash(relative),
                    digest: digest_file(&path)?,
                },
            );
        }
    }
    Ok(artifacts)
}

fn format_stable_digest<'a, I>(chunks: I) -> String
where
    I: IntoIterator<Item = (&'a [u8], &'a [u8])>,
{
    let mut hash = 0xcbf29ce484222325u64;
    for (label, bytes) in chunks {
        hash = digest_bytes(hash, label);
        hash = digest_bytes(hash, &(bytes.len() as u64).to_le_bytes());
        hash = digest_bytes(hash, bytes);
    }
    format!("fnv1a64:{hash:016x}")
}

fn digest_bytes(mut hash: u64, bytes: &[u8]) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn infer_source_id(args: &Args, export_metadata: Option<&Value>) -> Option<String> {
    if let Some(pilot_root) = args.pilot_root.as_deref() {
        let describe = git_stdout(pilot_root, ["describe", "--tags", "--always", "--dirty"]);
        if let Some(describe) = describe {
            return Some(format!("pilot-{describe}"));
        }
        if let Some(commit) = git_stdout(pilot_root, ["rev-parse", "--short=12", "HEAD"]) {
            return Some(format!("pilot-{commit}"));
        }
    }
    export_metadata
        .and_then(|value| value.get("pilot_version"))
        .and_then(Value::as_str)
        .map(|version| format!("pilot-{version}"))
}

fn safe_path_segment(value: &str) -> String {
    let mut segment = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>();
    while segment.contains("--") {
        segment = segment.replace("--", "-");
    }
    segment.trim_matches('-').to_string()
}

fn path_from_slashes(value: &str) -> PathBuf {
    value.split('/').collect()
}

fn path_to_slash(value: &str) -> String {
    value.replace('\\', "/")
}

fn path_to_slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn export_from_pilot(
    pilot_root: &Path,
    export_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let pilot_root = pilot_root.canonicalize()?;
    let library_root = pilot_root.join("sysml.library");
    let interactive_jar = find_interactive_jar(&pilot_root)?;
    let classes_dir = repo_path("target/pilot-exporter-classes");
    let java_source =
        repo_path("tools/pilot-exporter/src/main/java/dev/mercurio/pilot/PilotStdlibExporter.java");

    compile_java_exporter(&interactive_jar, &java_source, &classes_dir)?;
    run_java_exporter(&interactive_jar, &classes_dir, &library_root, export_path)?;
    Ok(())
}

fn find_interactive_jar(pilot_root: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let target_dir = pilot_root.join("org.omg.sysml.interactive/target");
    let mut jars = std::fs::read_dir(&target_dir)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| {
                    name.starts_with("org.omg.sysml.interactive-") && name.ends_with("-all.jar")
                })
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    jars.sort();

    jars.into_iter().last().ok_or_else(|| {
        format!(
            "could not find org.omg.sysml.interactive-*-all.jar under {}",
            target_dir.display()
        )
        .into()
    })
}

fn compile_java_exporter(
    interactive_jar: &Path,
    java_source: &Path,
    classes_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let class_file = classes_dir.join("dev/mercurio/pilot/PilotStdlibExporter.class");
    let should_compile = match (
        std::fs::metadata(java_source),
        std::fs::metadata(&class_file),
    ) {
        (Ok(source), Ok(class)) => source.modified()? > class.modified()?,
        _ => true,
    };
    if !should_compile {
        return Ok(());
    }

    std::fs::create_dir_all(classes_dir)?;
    let status = Command::new("javac")
        .arg("-cp")
        .arg(interactive_jar)
        .arg("-d")
        .arg(classes_dir)
        .arg(java_source)
        .status()?;
    if !status.success() {
        return Err("failed to compile Java pilot exporter".into());
    }
    Ok(())
}

fn run_java_exporter(
    interactive_jar: &Path,
    classes_dir: &Path,
    library_root: &Path,
    export_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = export_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let separator = if cfg!(windows) { ";" } else { ":" };
    let lib_dir = interactive_jar
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("lib");
    let classpath = format!(
        "{}{}{}{}{}",
        classes_dir.display(),
        separator,
        interactive_jar.display(),
        separator,
        lib_dir.join("*").display()
    );
    let status = Command::new("java")
        .arg("-cp")
        .arg(classpath)
        .arg("dev.mercurio.pilot.PilotStdlibExporter")
        .arg(library_root)
        .arg(export_path)
        .status()?;
    if !status.success() {
        return Err("failed to run Java pilot exporter".into());
    }
    Ok(())
}

fn git_stdout<const N: usize>(repo: &Path, args: [&str; N]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let value = String::from_utf8(output.stdout).ok()?.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn git_dirty(repo: &Path) -> Option<bool> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(!output.stdout.is_empty())
}

#[allow(dead_code)]
fn relative_to_repo(path: &Path) -> String {
    path.canonicalize()
        .ok()
        .and_then(|absolute| {
            repo_root()
                .canonicalize()
                .ok()
                .and_then(|root| absolute.strip_prefix(root).ok().map(Path::to_path_buf))
        })
        .map(|path| path_to_slash_path(&path))
        .unwrap_or_else(|| path_to_slash_path(path))
}
