use std::collections::BTreeMap;

use mercurio_core::{
    CURRENT_DEFAULT_PROFILE_ID, CoreMutationFeasibilityService, ElementRef, Graph, KirDocument,
    KirElement, LocalPackageRepository, MutationContext, MutationFeasibilityService,
    MutationProposal, SemanticConcept, SemanticMutation, compile_sysml_text,
    default_language_profile, default_metamodel_registry, default_stdlib_path, diff_kir_documents,
    elements_with_metadata, generate_python_wrappers, load_authoring_project_from_sysml,
    load_language_profile, requirement_traces, workspace_revision_for_kir_document,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("verify_core_contracts: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let profile = default_language_profile()?;
    check(
        profile.id == CURRENT_DEFAULT_PROFILE_ID,
        format!("default profile is {}", profile.id),
    )?;

    let loaded_by_id = load_language_profile(&profile.id)?;
    check(
        loaded_by_id == profile,
        "profile loads reproducibly by id".to_string(),
    )?;

    let stdlib_path = default_stdlib_path();
    let stdlib = KirDocument::from_path(&stdlib_path)?;
    check(
        stdlib.elements.len() > 10_000,
        format!("stdlib loads with {} elements", stdlib.elements.len()),
    )?;

    let graph = Graph::from_document(stdlib.clone())?;
    let registry = default_metamodel_registry()?;
    check(
        registry
            .canonical_kind(SemanticConcept::RequirementUsage)
            .is_some(),
        "requirement usage concept is registered".to_string(),
    )?;
    check(
        graph.edge_count() > 10_000,
        format!("stdlib graph builds with {} edges", graph.edge_count()),
    )?;

    let revision_a = workspace_revision_for_kir_document(&stdlib)?;
    let revision_b = workspace_revision_for_kir_document(&stdlib)?;
    check(
        revision_a == revision_b,
        format!("stdlib revision is stable ({})", revision_a.fingerprint),
    )?;

    let wrappers_a = generate_python_wrappers(&stdlib, &profile, "mercurio_sysml");
    let wrappers_b = generate_python_wrappers(&stdlib, &profile, "mercurio_sysml");
    check(
        wrappers_a == wrappers_b,
        format!(
            "python wrappers generate deterministically ({} files)",
            wrappers_a.files.len()
        ),
    )?;
    check(
        wrappers_a.profile_id == profile.id && wrappers_a.stdlib_version == profile.stdlib_version,
        "python wrapper provenance matches language profile".to_string(),
    )?;

    let empty_diff = diff_kir_documents(&stdlib, &stdlib);
    check(
        empty_diff.added_elements.is_empty()
            && empty_diff.removed_elements.is_empty()
            && empty_diff.changed_attributes.is_empty()
            && empty_diff.added_relationships.is_empty()
            && empty_diff.removed_relationships.is_empty(),
        "semantic diff is empty for identical documents".to_string(),
    )?;

    let package =
        LocalPackageRepository::bundled().verify_package("org.omg/sysml-stdlib", "2.0.0")?;
    check(
        package.has_precompiled_kir,
        format!(
            "bundled stdlib package verifies ({} source files, digest {})",
            package.source_count, package.digest
        ),
    )?;

    verify_metadata_round_trip()?;

    println!("core contracts: ok");
    Ok(())
}

fn check(condition: bool, message: String) -> Result<(), Box<dyn std::error::Error>> {
    if condition {
        println!("OK    {message}");
        Ok(())
    } else {
        Err(format!("FAILED {message}").into())
    }
}

fn verify_metadata_round_trip() -> Result<(), Box<dyn std::error::Error>> {
    let source = r#"
package Demo {
  metadata def ReviewTag {
    attribute status : String;
    attribute owner : String;
  }

  requirement safeStart;
}
"#;
    let project = load_authoring_project_from_sysml(BTreeMap::from([(
        "model.sysml".to_string(),
        source.to_string(),
    )]))?;
    let context = MutationContext::from_project(project);
    let proposal = MutationProposal {
        intent: "tag requirement metadata".to_string(),
        affected_elements: vec![ElementRef::new("Demo.safeStart")],
        operations: vec![SemanticMutation::AddMetadataAnnotation {
            element: ElementRef::new("Demo.safeStart"),
            metadata_type: "ReviewTag".to_string(),
            properties: BTreeMap::from([
                ("owner".to_string(), "Safety Team".to_string()),
                ("status".to_string(), "draft".to_string()),
            ]),
        }],
        evidence: Vec::new(),
        rationale: None,
        workspace_revision: context.workspace_revision.clone(),
    };
    let service = CoreMutationFeasibilityService::new();
    let report = service.check(&context, &proposal);
    check(
        report.normalized_plan.is_some(),
        "metadata annotation mutation normalizes".to_string(),
    )?;
    let applied = service
        .apply_checked_plan(
            &context,
            report.normalized_plan.as_ref().expect("checked above"),
        )
        .map_err(|issue| issue.message)?;
    let edited = applied
        .edited_files
        .get("model.sysml")
        .ok_or("metadata mutation did not edit model.sysml")?;
    let stdlib = KirDocument::from_path(&default_stdlib_path())?;
    let document = compile_sysml_text(edited, "model.sysml", &stdlib)?;
    check(
        elements_with_metadata(&document, "ReviewTag")
            .iter()
            .any(|element| element.id == "requirement.Demo.safeStart"),
        "metadata annotation round-trips through SysML source".to_string(),
    )?;
    check(
        applied
            .semantic_diff
            .changed_attributes
            .iter()
            .any(|change| {
                change.element == ElementRef::new("Demo.safeStart")
                    && change.attribute == "metadata"
            }),
        "metadata mutation reports semantic diff".to_string(),
    )?;

    let trace_document = KirDocument {
        metadata: BTreeMap::new(),
        elements: vec![
            KirElement {
                id: "req.safeStart".to_string(),
                kind: "RequirementUsage".to_string(),
                layer: 2,
                properties: BTreeMap::new(),
            },
            KirElement {
                id: "case.verifySafeStart".to_string(),
                kind: "VerificationCaseUsage".to_string(),
                layer: 2,
                properties: BTreeMap::from([(
                    "verified_requirement".to_string(),
                    serde_json::json!("req.safeStart"),
                )]),
            },
        ],
    };
    check(
        requirement_traces(&trace_document, "req.safeStart")?
            .iter()
            .any(|trace| trace.source == "case.verifySafeStart"),
        "requirement trace query follows verification relationships".to_string(),
    )?;

    Ok(())
}
