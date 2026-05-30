use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use mercurio_core::frontend::transpile::{KirEmissionSeed, PilotConstructSeed};
use mercurio_core::ir::KirDocument;
use mercurio_core::language::profile::{CURRENT_DEFAULT_PROFILE_ID, LanguageProfile};
use mercurio_core::paths::repo_path;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse()?;
    let profile_dir = args.profile_root.join(&args.profile_id);
    let profile_path = profile_dir.join("profile.json");
    let constructs_path = profile_dir
        .join("mappings")
        .join("pilot_constructs.seed.json");
    let emission_path = profile_dir.join("mappings").join("kir_emission.seed.json");

    let profile = LanguageProfile::from_path(&profile_path)?;
    let constructs: PilotConstructSeed =
        serde_json::from_str(&fs::read_to_string(&constructs_path)?)?;
    let emission: KirEmissionSeed = serde_json::from_str(&fs::read_to_string(&emission_path)?)?;
    let stdlib_path = args
        .stdlib_path
        .clone()
        .unwrap_or_else(|| repo_path(&profile.stdlib_path));
    let stdlib = KirDocument::from_path(&stdlib_path)?;

    let mut audit = Audit::default();
    audit_profile_identity(&mut audit, &args, &profile, &profile_path);
    let stdlib_ids = audit_stdlib(&mut audit, &profile, &stdlib, &stdlib_path);
    audit_constructs(&mut audit, &constructs, &emission);
    audit_emission(&mut audit, &constructs, &emission);
    audit_stdlib_references(&mut audit, &profile, &constructs, &stdlib_ids);
    if let Some(pilot_root) = &args.pilot_root {
        audit_pilot_grammar_alignment(&mut audit, &constructs, pilot_root);
    }

    audit.print(
        &args.profile_id,
        &profile_path,
        &constructs_path,
        &emission_path,
        &stdlib_path,
    );

    if audit.errors > 0 || (args.deny_warnings && audit.warnings > 0) {
        std::process::exit(1);
    }
    Ok(())
}

#[derive(Debug)]
struct Args {
    profile_id: String,
    profile_root: PathBuf,
    stdlib_path: Option<PathBuf>,
    pilot_root: Option<PathBuf>,
    deny_warnings: bool,
}

impl Args {
    fn parse() -> Result<Self> {
        let mut profile_id = CURRENT_DEFAULT_PROFILE_ID.to_string();
        let mut profile_root = repo_path("resources/language-profiles");
        let mut stdlib_path = None;
        let mut pilot_root = None;
        let mut deny_warnings = false;

        let mut args = env::args().skip(1).collect::<Vec<_>>();
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--profile-id" | "--profile" => {
                    index += 1;
                    profile_id = take_arg(&args, index, "--profile-id")?;
                }
                "--profile-root" => {
                    index += 1;
                    profile_root = PathBuf::from(take_arg(&args, index, "--profile-root")?);
                }
                "--stdlib" => {
                    index += 1;
                    stdlib_path = Some(PathBuf::from(take_arg(&args, index, "--stdlib")?));
                }
                "--pilot-root" => {
                    index += 1;
                    pilot_root = Some(PathBuf::from(take_arg(&args, index, "--pilot-root")?));
                }
                "--deny-warnings" => {
                    deny_warnings = true;
                }
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => {
                    return Err(format!("unknown argument `{other}`").into());
                }
            }
            index += 1;
        }

        args.clear();
        Ok(Self {
            profile_id,
            profile_root,
            stdlib_path,
            pilot_root,
            deny_warnings,
        })
    }
}

fn take_arg(args: &[String], index: usize, name: &str) -> Result<String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("missing value for {name}").into())
}

fn print_usage() {
    println!(
        "Usage: cargo run -p mercurio-tools --bin audit_language_profile -- [--profile-id ID] [--profile-root PATH] [--stdlib PATH] [--pilot-root PATH] [--deny-warnings]"
    );
}

#[derive(Default)]
struct Audit {
    errors: usize,
    warnings: usize,
    infos: Vec<String>,
}

impl Audit {
    fn error(&mut self, message: impl Into<String>) {
        self.errors += 1;
        self.infos.push(format!("ERROR {}", message.into()));
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warnings += 1;
        self.infos.push(format!("WARN  {}", message.into()));
    }

    fn ok(&mut self, message: impl Into<String>) {
        self.infos.push(format!("OK    {}", message.into()));
    }

    fn print(
        &self,
        profile_id: &str,
        profile_path: &Path,
        constructs_path: &Path,
        emission_path: &Path,
        stdlib_path: &Path,
    ) {
        println!("Language profile audit");
        println!("  profile: {profile_id}");
        println!("  profile file: {}", profile_path.display());
        println!("  constructs: {}", constructs_path.display());
        println!("  emission: {}", emission_path.display());
        println!("  stdlib: {}", stdlib_path.display());
        println!();
        for info in &self.infos {
            println!("{info}");
        }
        println!();
        println!(
            "summary: {} error(s), {} warning(s)",
            self.errors, self.warnings
        );
    }
}

fn audit_profile_identity(
    audit: &mut Audit,
    args: &Args,
    profile: &LanguageProfile,
    profile_path: &Path,
) {
    if profile.id == args.profile_id {
        audit.ok(format!("profile id matches `{}`", profile.id));
    } else {
        audit.error(format!(
            "profile id `{}` does not match requested id `{}` in {}",
            profile.id,
            args.profile_id,
            profile_path.display()
        ));
    }
}

fn audit_stdlib(
    audit: &mut Audit,
    profile: &LanguageProfile,
    stdlib: &KirDocument,
    stdlib_path: &Path,
) -> BTreeSet<String> {
    if stdlib.elements.is_empty() {
        audit.error(format!("stdlib has no elements: {}", stdlib_path.display()));
    } else {
        audit.ok(format!(
            "stdlib loads with {} elements",
            stdlib.elements.len()
        ));
    }

    let ids = stdlib
        .elements
        .iter()
        .map(|element| element.id.clone())
        .collect::<BTreeSet<_>>();

    if Path::new(&profile.stdlib_path).is_absolute() {
        audit.warn("profile stdlib_path is absolute; profile artifacts are less relocatable");
    } else {
        audit.ok("profile stdlib_path is relative");
    }

    ids
}

fn audit_constructs(
    audit: &mut Audit,
    constructs: &PilotConstructSeed,
    emission: &KirEmissionSeed,
) {
    let mut construct_to_metaclass = BTreeMap::<String, String>::new();
    for entry in &constructs.constructs {
        if let Some(previous) =
            construct_to_metaclass.insert(entry.construct.clone(), entry.metaclass.clone())
        {
            audit.error(format!(
                "construct `{}` is mapped twice: `{previous}` and `{}`",
                entry.construct, entry.metaclass
            ));
        }
    }
    audit.ok(format!(
        "construct seed has {} construct mappings",
        construct_to_metaclass.len()
    ));

    let mut keyword_count = 0;
    for (keyword, construct) in &constructs.keyword_registry.definitions {
        keyword_count += 1;
        audit_keyword_construct(
            audit,
            "definition",
            keyword,
            construct,
            &construct_to_metaclass,
            emission,
        );
    }
    for (keyword, construct) in &constructs.keyword_registry.usages {
        keyword_count += 1;
        audit_keyword_construct(
            audit,
            "usage",
            keyword,
            construct,
            &construct_to_metaclass,
            emission,
        );
    }
    audit.ok(format!("keyword registry has {keyword_count} entries"));
}

fn audit_keyword_construct(
    audit: &mut Audit,
    group: &str,
    keyword: &str,
    construct: &str,
    construct_to_metaclass: &BTreeMap<String, String>,
    emission: &KirEmissionSeed,
) {
    let metaclass = construct_to_metaclass
        .get(construct)
        .map(String::as_str)
        .or_else(|| fallback_metaclass_for_construct(construct));

    let Some(metaclass) = metaclass else {
        audit.error(format!(
            "{group} keyword `{keyword}` points at construct `{construct}` with no construct mapping"
        ));
        return;
    };

    if !emission.metaclasses.contains_key(metaclass) {
        audit.error(format!(
            "{group} keyword `{keyword}` resolves to metaclass `{metaclass}` with no KIR emission rule"
        ));
    }
}

fn audit_emission(audit: &mut Audit, constructs: &PilotConstructSeed, emission: &KirEmissionSeed) {
    let construct_metaclasses = constructs
        .constructs
        .iter()
        .map(|entry| entry.metaclass.as_str())
        .collect::<BTreeSet<_>>();

    for entry in &constructs.constructs {
        if !emission.metaclasses.contains_key(&entry.metaclass) {
            audit.error(format!(
                "construct `{}` maps to metaclass `{}` with no KIR emission rule",
                entry.construct, entry.metaclass
            ));
        }
    }

    let mut unused_emission_rules = 0;
    for metaclass in emission.metaclasses.keys() {
        if !construct_metaclasses.contains(metaclass.as_str())
            && metaclass != "KerML::Classifier"
            && metaclass != "KerML::Feature"
        {
            unused_emission_rules += 1;
        }
    }
    if unused_emission_rules == 0 {
        audit.ok("all concrete emission rules are referenced by construct mappings");
    } else {
        audit.warn(format!(
            "{unused_emission_rules} emission rule(s) are not directly referenced by construct mappings"
        ));
    }

    let mut placeholder_errors = 0;
    for (metaclass, rule) in &emission.metaclasses {
        for template in std::iter::once(&rule.id_template).chain(rule.emit.properties.values()) {
            for placeholder in placeholders(template) {
                if placeholder.trim().is_empty() {
                    audit.error(format!(
                        "emission rule `{metaclass}` contains an empty placeholder in `{template}`"
                    ));
                    placeholder_errors += 1;
                }
            }
            if has_unbalanced_braces(template) {
                audit.error(format!(
                    "emission rule `{metaclass}` has unbalanced braces in `{template}`"
                ));
                placeholder_errors += 1;
            }
        }
    }
    if placeholder_errors == 0 {
        audit.ok("emission templates have balanced placeholders");
    }
}

fn audit_stdlib_references(
    audit: &mut Audit,
    profile: &LanguageProfile,
    constructs: &PilotConstructSeed,
    stdlib_ids: &BTreeSet<String>,
) {
    for (concept, target) in &profile.canonical_kinds {
        if !stdlib_ids.contains(target) {
            audit.error(format!(
                "canonical concept `{concept:?}` points at missing stdlib element `{target}`"
            ));
        }
    }
    audit.ok(format!(
        "profile declares {} canonical concept binding(s)",
        profile.canonical_kinds.len()
    ));

    for (alias, target) in &profile.aliases {
        if !stdlib_ids.contains(target) {
            audit.warn(format!(
                "profile alias `{alias}` points at `{target}`, which is not an element id in the stdlib"
            ));
        }
    }

    audit_anchor_group(
        audit,
        "package default specialization",
        &constructs.default_specialization_anchors.packages,
        stdlib_ids,
    );
    audit_anchor_group(
        audit,
        "definition default specialization",
        &constructs.default_specialization_anchors.definitions,
        stdlib_ids,
    );
    audit_anchor_group(
        audit,
        "usage default specialization",
        &constructs.default_specialization_anchors.usages,
        stdlib_ids,
    );
    audit_semantic_specialization_group(
        audit,
        "definition semantic specialization",
        &constructs.semantic_specialization_defaults.definitions,
        stdlib_ids,
    );
    audit_semantic_specialization_group(
        audit,
        "usage semantic specialization",
        &constructs.semantic_specialization_defaults.usages,
        stdlib_ids,
    );
    for (construct, overrides) in &constructs.usage_semantic_specialization_overrides.usages {
        audit_semantic_specialization_group(
            audit,
            &format!("usage semantic override for `{construct}`"),
            overrides,
            stdlib_ids,
        );
    }
    audit_anchor_group(
        audit,
        "stdlib alias",
        &constructs.stdlib_aliases.ids,
        stdlib_ids,
    );
}

fn audit_anchor_group(
    audit: &mut Audit,
    label: &str,
    refs: &BTreeMap<String, String>,
    stdlib_ids: &BTreeSet<String>,
) {
    for (key, target) in refs {
        if !stdlib_ids.contains(target) {
            audit.error(format!(
                "{label} `{key}` points at missing stdlib element `{target}`"
            ));
        }
    }
    audit.ok(format!("{label} references checked: {}", refs.len()));
}

fn audit_semantic_specialization_group(
    audit: &mut Audit,
    label: &str,
    refs: &BTreeMap<String, Vec<String>>,
    stdlib_ids: &BTreeSet<String>,
) {
    let mut count = 0;
    for (key, targets) in refs {
        for target in targets {
            count += 1;
            if !stdlib_ids.contains(target) {
                audit.error(format!(
                    "{label} `{key}` points at missing stdlib element `{target}`"
                ));
            }
        }
    }
    audit.ok(format!("{label} references checked: {count}"));
}

fn audit_pilot_grammar_alignment(
    audit: &mut Audit,
    constructs: &PilotConstructSeed,
    pilot_root: &Path,
) {
    let grammar_paths = [
        pilot_root
            .join("org.omg.sysml.xtext")
            .join("src")
            .join("org")
            .join("omg")
            .join("sysml")
            .join("xtext")
            .join("SysML.xtext"),
        pilot_root
            .join("org.omg.kerml.xtext")
            .join("src")
            .join("org")
            .join("omg")
            .join("kerml")
            .join("xtext")
            .join("KerML.xtext"),
    ];

    let seed = constructs
        .constructs
        .iter()
        .map(|entry| (entry.construct.as_str(), entry.metaclass.as_str()))
        .collect::<BTreeSet<_>>();

    let mut grammar_entries = BTreeSet::new();
    for path in &grammar_paths {
        match fs::read_to_string(path) {
            Ok(text) => {
                grammar_entries.extend(extract_xtext_returns(&text));
            }
            Err(err) => {
                audit.warn(format!(
                    "could not read Pilot grammar `{}`: {err}",
                    path.display()
                ));
            }
        }
    }

    if grammar_entries.is_empty() {
        audit.warn("Pilot grammar alignment skipped because no `returns` rules were found");
        return;
    }

    let mut missing_from_seed = 0;
    for (construct, metaclass) in &grammar_entries {
        if !seed.contains(&(construct.as_str(), metaclass.as_str())) {
            missing_from_seed += 1;
        }
    }

    let mut missing_from_grammar = 0;
    for (construct, metaclass) in &seed {
        if !grammar_entries.contains(&((*construct).to_string(), (*metaclass).to_string())) {
            missing_from_grammar += 1;
        }
    }

    if missing_from_seed == 0 && missing_from_grammar == 0 {
        audit.ok(format!(
            "Pilot grammar alignment matched {} return-rule mappings",
            grammar_entries.len()
        ));
    } else {
        audit.warn(format!(
            "Pilot grammar alignment drift: {missing_from_seed} grammar mapping(s) missing from seed, {missing_from_grammar} seed mapping(s) not found in grammar"
        ));
    }
}

fn extract_xtext_returns(text: &str) -> BTreeSet<(String, String)> {
    let mut entries = BTreeSet::new();
    for line in text.lines() {
        let normalized = line.trim();
        if normalized.starts_with("//") {
            continue;
        }
        let tokens = normalized.split_whitespace().collect::<Vec<_>>();
        let Some(returns_index) = tokens.iter().position(|token| *token == "returns") else {
            continue;
        };
        if returns_index == 0 || returns_index + 1 >= tokens.len() {
            continue;
        }
        let construct = tokens[returns_index - 1].trim_matches(':').to_string();
        let metaclass = tokens[returns_index + 1]
            .trim_end_matches(|ch| ch == ':' || ch == ';')
            .to_string();
        if construct.is_empty() {
            continue;
        }
        if metaclass.starts_with("SysML::") || metaclass.starts_with("KerML::") {
            entries.insert((construct, metaclass));
        }
    }
    entries
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

fn placeholders(template: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let after_start = &rest[start + 1..];
        let Some(end) = after_start.find('}') else {
            break;
        };
        result.push(&after_start[..end]);
        rest = &after_start[end + 1..];
    }
    result
}

fn has_unbalanced_braces(template: &str) -> bool {
    template.matches('{').count() != template.matches('}').count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_xtext_return_rules() {
        let text = r#"
            Package returns SysML::Package:
            DefinitionElement returns KerML::Classifier:
            // Ignored returns SysML::Ignored:
        "#;

        let entries = extract_xtext_returns(text);

        assert!(entries.contains(&("Package".to_string(), "SysML::Package".to_string())));
        assert!(entries.contains(&(
            "DefinitionElement".to_string(),
            "KerML::Classifier".to_string()
        )));
        assert_eq!(entries.len(), 2);
    }

    #[test]
    fn finds_placeholders_in_templates() {
        assert_eq!(
            placeholders("type.{qualified_name}.{start_line}"),
            vec!["qualified_name", "start_line"]
        );
        assert!(!has_unbalanced_braces("{owner_id}"));
        assert!(has_unbalanced_braces("{owner_id"));
    }
}
