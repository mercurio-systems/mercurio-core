use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mercurio_core::frontend::lexer::lex;
use mercurio_core::frontend::resolver::resolve_module;
use mercurio_core::frontend::sysml::parse_sysml;
use mercurio_core::frontend::transpile::{MappingBundle, transpile_module};
use mercurio_core::{KirDocument, default_stdlib_path, repo_path};
use mercurio_tools::default_pilot_root;
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = parse_args()?;
    let stdlib = KirDocument::from_path(&default_stdlib_path())?;
    let mappings = MappingBundle::load()?;
    let corpus_seed = PilotCorpusSeed::load()?;
    let cases = args
        .corpus
        .paths(&corpus_seed, &args.pilot_root)?
        .iter()
        .map(|relative_path| {
            audit_case(
                &args.pilot_root,
                relative_path,
                &stdlib,
                &mappings,
                &corpus_seed,
            )
        })
        .collect::<Vec<_>>();
    let summary = summarize(&cases);
    let report = AuditReport {
        corpus_name: args.corpus.report_name().to_string(),
        generated_at_utc: now_utc_rfc3339()?,
        pilot_root: args.pilot_root.display().to_string(),
        stdlib_path: default_stdlib_path().display().to_string(),
        cases,
        summary,
    };

    if let Some(parent) = args.output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&args.output_path, serde_json::to_string_pretty(&report)?)?;

    println!("pilot corpus audit");
    println!("  corpus: {}", report.corpus_name);
    println!("  pilot root: {}", report.pilot_root);
    println!("  output: {}", args.output_path.display());
    for (status, count) in &report.summary.status_counts {
        println!("  {status}: {count}");
    }

    Ok(())
}

struct Args {
    pilot_root: PathBuf,
    output_path: PathBuf,
    corpus: AuditCorpus,
}

#[derive(Clone, Copy)]
enum AuditCorpus {
    Small,
    Core,
    Advanced,
    Behavioral,
    Extended,
    Training,
}

impl AuditCorpus {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "small" => Some(Self::Small),
            "core" => Some(Self::Core),
            "advanced" => Some(Self::Advanced),
            "behavioral" => Some(Self::Behavioral),
            "extended" => Some(Self::Extended),
            "training" => Some(Self::Training),
            _ => None,
        }
    }

    fn report_name(self) -> &'static str {
        match self {
            Self::Small => "pilot-small",
            Self::Core => "pilot-core",
            Self::Advanced => "pilot-advanced",
            Self::Behavioral => "pilot-behavioral",
            Self::Extended => "pilot-extended",
            Self::Training => "pilot-training",
        }
    }

    fn output_path(self) -> PathBuf {
        match self {
            Self::Small => repo_path("target/pilot_corpus_audit.small.json"),
            Self::Core => repo_path("target/pilot_corpus_audit.core.json"),
            Self::Advanced => repo_path("target/pilot_corpus_audit.advanced.json"),
            Self::Behavioral => repo_path("target/pilot_corpus_audit.behavioral.json"),
            Self::Extended => repo_path("target/pilot_corpus_audit.extended.json"),
            Self::Training => repo_path("target/pilot_corpus_audit.training.json"),
        }
    }

    fn key(self) -> &'static str {
        match self {
            Self::Small => "small",
            Self::Core => "core",
            Self::Advanced => "advanced",
            Self::Behavioral => "behavioral",
            Self::Extended => "extended",
            Self::Training => "training",
        }
    }

    fn paths(
        self,
        seed: &PilotCorpusSeed,
        pilot_root: &Path,
    ) -> Result<Vec<String>, Box<dyn std::error::Error>> {
        match self {
            Self::Training => {
                let mut stack = vec![pilot_root.join("sysml/src/training")];
                let mut files = Vec::new();

                while let Some(path) = stack.pop() {
                    if path.is_dir() {
                        for entry in std::fs::read_dir(&path)? {
                            stack.push(entry?.path());
                        }
                        continue;
                    }

                    if path
                        .extension()
                        .is_some_and(|extension| extension == "sysml")
                    {
                        let relative = path
                            .strip_prefix(pilot_root)?
                            .to_string_lossy()
                            .replace('\\', "/");
                        files.push(relative);
                    }
                }

                files.sort();
                Ok(files)
            }
            _ => seed
                .corpus_paths(self.key())
                .map(|paths| paths.to_vec())
                .ok_or_else(|| {
                    format!(
                        "missing corpus tier `{}` in {}",
                        self.key(),
                        repo_path("crates/mercurio-tools/corpus/pilot_corpus.seed.json").display()
                    )
                    .into()
                }),
        }
    }
}

fn parse_args() -> Result<Args, Box<dyn std::error::Error>> {
    let mut pilot_root = default_pilot_root();
    let mut corpus = AuditCorpus::Small;
    let mut output_path = None;
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let mut index = 0;

    while index < args.len() {
        match args[index].as_str() {
            "--pilot-root" => {
                index += 1;
                let value = args.get(index).ok_or("missing value for --pilot-root")?;
                pilot_root = PathBuf::from(value);
            }
            "--out" => {
                index += 1;
                let value = args.get(index).ok_or("missing value for --out")?;
                output_path = Some(PathBuf::from(value));
            }
            "--corpus" => {
                index += 1;
                let value = args.get(index).ok_or("missing value for --corpus")?;
                corpus =
                    AuditCorpus::parse(value).ok_or_else(|| format!("unknown corpus: {value}"))?;
            }
            "--help" | "-h" => {
                print_usage();
                std::process::exit(0);
            }
            unknown => {
                return Err(format!("unknown argument: {unknown}").into());
            }
        }
        index += 1;
    }

    Ok(Args {
        pilot_root,
        output_path: output_path.unwrap_or_else(|| corpus.output_path()),
        corpus,
    })
}

fn print_usage() {
    println!(
        "Usage: cargo run -p mercurio-tools --bin audit_pilot_corpus -- [--corpus small|core|advanced|behavioral|extended|training] [--pilot-root PATH] [--out PATH]"
    );
}

fn audit_case(
    pilot_root: &Path,
    relative_path: &str,
    stdlib: &KirDocument,
    mappings: &MappingBundle,
    corpus_seed: &PilotCorpusSeed,
) -> AuditCase {
    let path = pilot_root.join(relative_path);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) => {
            return AuditCase {
                relative_path: relative_path.to_string(),
                absolute_path: path.display().to_string(),
                status: AuditStatus::Io,
                message: err.to_string(),
                feature_tags: Vec::new(),
            };
        }
    };
    let feature_tags = detect_feature_tags(&text, &corpus_seed.feature_tag_rules);
    let augmented_stdlib =
        match build_augmented_stdlib(pilot_root, relative_path, stdlib, mappings, corpus_seed) {
            Ok(document) => document,
            Err(err) => {
                return AuditCase {
                    relative_path: relative_path.to_string(),
                    absolute_path: path.display().to_string(),
                    status: AuditStatus::Io,
                    message: err.to_string(),
                    feature_tags,
                };
            }
        };

    if let Err(err) = lex(&text) {
        return AuditCase {
            relative_path: relative_path.to_string(),
            absolute_path: path.display().to_string(),
            status: AuditStatus::LexGap,
            message: err.to_string(),
            feature_tags,
        };
    }

    let module = match parse_sysml(&text) {
        Ok(module) => module,
        Err(err) => {
            return AuditCase {
                relative_path: relative_path.to_string(),
                absolute_path: path.display().to_string(),
                status: AuditStatus::ParseGap,
                message: err.to_string(),
                feature_tags,
            };
        }
    };

    let resolved = match resolve_module(&module, &augmented_stdlib, mappings) {
        Ok(resolved) => resolved,
        Err(err) => {
            return AuditCase {
                relative_path: relative_path.to_string(),
                absolute_path: path.display().to_string(),
                status: AuditStatus::ResolutionGap,
                message: err.to_string(),
                feature_tags,
            };
        }
    };

    match transpile_module(&resolved, relative_path, mappings) {
        Ok(_) => AuditCase {
            relative_path: relative_path.to_string(),
            absolute_path: path.display().to_string(),
            status: AuditStatus::Pass,
            message: "transpiled successfully".to_string(),
            feature_tags,
        },
        Err(err) => AuditCase {
            relative_path: relative_path.to_string(),
            absolute_path: path.display().to_string(),
            status: classify_transpile_error(&err.message),
            message: err.to_string(),
            feature_tags,
        },
    }
}

fn build_augmented_stdlib(
    pilot_root: &Path,
    relative_path: &str,
    stdlib: &KirDocument,
    mappings: &MappingBundle,
    corpus_seed: &PilotCorpusSeed,
) -> Result<KirDocument, Box<dyn std::error::Error>> {
    let mut augmented = stdlib.clone();

    for support_path in corpus_seed.support_paths_for(relative_path) {
        let support_text = std::fs::read_to_string(pilot_root.join(support_path))?;
        let support_module = parse_sysml(&support_text)?;
        let support_resolved = resolve_module(&support_module, &augmented, mappings)?;
        let support_kir = transpile_module(&support_resolved, support_path, mappings)?;

        for element in synthetic_support_elements(&support_kir) {
            augmented.elements.push(element);
        }
    }

    Ok(augmented)
}

fn synthetic_support_elements(document: &KirDocument) -> Vec<mercurio_core::KirElement> {
    document
        .elements
        .iter()
        .filter_map(|element| {
            let synthetic_id = if let Some(path) = element.id.strip_prefix("pkg.") {
                Some(path.replace('.', "::"))
            } else if let Some(path) = element.id.strip_prefix("type.") {
                Some(path.replace('.', "::"))
            } else {
                None
            }?;

            Some(mercurio_core::KirElement {
                id: synthetic_id,
                kind: if element.kind.contains("Package") {
                    "LibraryPackage".to_string()
                } else {
                    element.kind.clone()
                },
                layer: element.layer,
                properties: element.properties.clone(),
            })
        })
        .collect()
}

fn classify_transpile_error(message: &str) -> AuditStatus {
    if message.contains("missing construct mapping") {
        AuditStatus::MissingConstructMapping
    } else if message.contains("missing emission mapping") {
        AuditStatus::MissingEmissionMapping
    } else if message.contains("duplicate emitted KIR id") {
        AuditStatus::KirCollision
    } else {
        AuditStatus::TranspileGap
    }
}

fn detect_feature_tags(text: &str, rules: &[FeatureTagRule]) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut tags = Vec::new();

    for rule in rules {
        push_tag_if(&mut tags, rule.matches(text, &lower), &rule.tag);
    }

    tags
}

fn push_tag_if(tags: &mut Vec<String>, predicate: bool, tag: &str) {
    if predicate {
        tags.push(tag.to_string());
    }
}

fn contains_term(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(index, _)| {
        let before = text[..index].chars().next_back();
        let after = text[index + needle.len()..].chars().next();
        !is_word_char(before) && !is_word_char(after)
    })
}

fn is_word_char(ch: Option<char>) -> bool {
    ch.is_some_and(|value| value.is_ascii_alphanumeric() || value == '_')
}

fn summarize(cases: &[AuditCase]) -> AuditSummary {
    let mut status_counts = BTreeMap::new();
    let mut feature_counts = BTreeMap::new();

    for case in cases {
        *status_counts
            .entry(case.status.as_str().to_string())
            .or_insert(0) += 1;
        for feature in &case.feature_tags {
            *feature_counts.entry(feature.clone()).or_insert(0) += 1;
        }
    }

    AuditSummary {
        total_cases: cases.len(),
        status_counts,
        feature_counts,
    }
}

fn now_utc_rfc3339() -> Result<String, Box<dyn std::error::Error>> {
    Ok(OffsetDateTime::now_utc().format(&Rfc3339)?)
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
enum AuditStatus {
    Pass,
    Io,
    LexGap,
    ParseGap,
    ResolutionGap,
    MissingConstructMapping,
    MissingEmissionMapping,
    KirCollision,
    TranspileGap,
}

impl AuditStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Io => "io",
            Self::LexGap => "lex_gap",
            Self::ParseGap => "parse_gap",
            Self::ResolutionGap => "resolution_gap",
            Self::MissingConstructMapping => "missing_construct_mapping",
            Self::MissingEmissionMapping => "missing_emission_mapping",
            Self::KirCollision => "kir_collision",
            Self::TranspileGap => "transpile_gap",
        }
    }
}

#[derive(Debug, Serialize)]
struct AuditReport {
    corpus_name: String,
    generated_at_utc: String,
    pilot_root: String,
    stdlib_path: String,
    cases: Vec<AuditCase>,
    summary: AuditSummary,
}

#[derive(Debug, Serialize)]
struct AuditCase {
    relative_path: String,
    absolute_path: String,
    status: AuditStatus,
    message: String,
    feature_tags: Vec<String>,
}

#[derive(Debug, Serialize)]
struct AuditSummary {
    total_cases: usize,
    status_counts: BTreeMap<String, usize>,
    feature_counts: BTreeMap<String, usize>,
}

#[derive(Debug, Deserialize)]
struct PilotCorpusSeed {
    corpora: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    support_dependencies: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    feature_tag_rules: Vec<FeatureTagRule>,
}

impl PilotCorpusSeed {
    fn load() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(serde_json::from_str(&std::fs::read_to_string(repo_path(
            "crates/mercurio-tools/corpus/pilot_corpus.seed.json",
        ))?)?)
    }

    fn corpus_paths(&self, key: &str) -> Option<&[String]> {
        self.corpora.get(key).map(Vec::as_slice)
    }

    fn support_paths_for(&self, relative_path: &str) -> &[String] {
        self.support_dependencies
            .get(relative_path)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

#[derive(Debug, Deserialize)]
struct FeatureTagRule {
    tag: String,
    mode: FeatureTagRuleMode,
    #[serde(default)]
    needle: Option<String>,
    #[serde(default)]
    needles: Vec<String>,
}

impl FeatureTagRule {
    fn matches(&self, _text: &str, lower: &str) -> bool {
        match self.mode {
            FeatureTagRuleMode::Contains => self
                .needle
                .as_deref()
                .is_some_and(|needle| lower.contains(&needle.to_ascii_lowercase())),
            FeatureTagRuleMode::Term => self
                .needle
                .as_deref()
                .is_some_and(|needle| contains_term(lower, &needle.to_ascii_lowercase())),
            FeatureTagRuleMode::AnyTerm => self
                .needles
                .iter()
                .any(|needle| contains_term(lower, &needle.to_ascii_lowercase())),
            FeatureTagRuleMode::ContainsAll => self
                .needles
                .iter()
                .all(|needle| lower.contains(&needle.to_ascii_lowercase())),
            FeatureTagRuleMode::PackageCountGtOne => lower.matches("package ").count() > 1,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum FeatureTagRuleMode {
    Contains,
    Term,
    AnyTerm,
    ContainsAll,
    PackageCountGtOne,
}

#[cfg(test)]
mod tests {
    use mercurio_tools::default_pilot_root;

    use super::{AuditStatus, PilotCorpusSeed, classify_transpile_error, detect_feature_tags};

    #[test]
    fn tags_obvious_language_features() {
        let seed = PilotCorpusSeed::load().unwrap();
        let tags = detect_feature_tags(
            "package Demo { private import Pkg::*; item def A { part x[0..1]: B; } }",
            &seed.feature_tag_rules,
        );

        assert!(tags.contains(&"import".to_string()));
        assert!(tags.contains(&"wildcard-import".to_string()));
        assert!(tags.contains(&"visibility".to_string()));
        assert!(tags.contains(&"item".to_string()));
        assert!(tags.contains(&"multiplicity-or-index".to_string()));
    }

    #[test]
    fn tags_advanced_behavioral_features() {
        let seed = PilotCorpusSeed::load().unwrap();
        let tags = detect_feature_tags(
            "interface def FuelInterface { bind x = y; flow from a to b; transition t first s then q; action run; accept trigger; perform run; send x; loop action charging; if ok { exhibit active; } state on; }",
            &seed.feature_tag_rules,
        );

        assert!(tags.contains(&"interface".to_string()));
        assert!(tags.contains(&"binding".to_string()));
        assert!(tags.contains(&"flow".to_string()));
        assert!(tags.contains(&"transition".to_string()));
        assert!(tags.contains(&"action".to_string()));
        assert!(tags.contains(&"accept".to_string()));
        assert!(tags.contains(&"perform".to_string()));
        assert!(tags.contains(&"send".to_string()));
        assert!(tags.contains(&"if".to_string()));
        assert!(tags.contains(&"loop".to_string()));
        assert!(tags.contains(&"exhibit".to_string()));
        assert!(tags.contains(&"state".to_string()));
    }

    #[test]
    fn classifies_known_transpile_failures() {
        assert!(matches!(
            classify_transpile_error("missing construct mapping `PortDefinition`"),
            AuditStatus::MissingConstructMapping
        ));
        assert!(matches!(
            classify_transpile_error("duplicate emitted KIR id `feature.A.x`"),
            AuditStatus::KirCollision
        ));
    }

    #[test]
    fn loads_seeded_corpora_and_support_dependencies() {
        let seed = PilotCorpusSeed::load().unwrap();

        assert!(
            seed.corpus_paths("behavioral")
                .unwrap()
                .iter()
                .any(|path| path.contains("Conditional Succession Example-1.sysml"))
        );
        assert!(
            seed.corpus_paths("extended")
                .unwrap()
                .iter()
                .any(|path| path.contains("Requirement Definitions.sysml"))
        );
        assert!(
            seed.support_paths_for("sysml/src/training/11. Interfaces/Interface Example.sysml")
                .iter()
                .any(|path| path.contains("Port Example.sysml"))
        );
        assert!(
            seed.feature_tag_rules
                .iter()
                .any(|rule| rule.tag == "perform")
        );
        assert!(
            seed.feature_tag_rules
                .iter()
                .any(|rule| rule.tag == "requirement")
        );
    }

    #[test]
    fn discovers_training_corpus_from_pilot_root() {
        let pilot_root = default_pilot_root();
        if !pilot_root.exists() {
            return;
        }

        let seed = PilotCorpusSeed::load().unwrap();
        let paths = super::AuditCorpus::Training
            .paths(&seed, &pilot_root)
            .unwrap();

        assert!(paths.len() >= 100);
        assert!(
            paths
                .iter()
                .any(|path| path.contains("31. Constraints/Constraints Example-1.sysml"))
        );
    }
}
