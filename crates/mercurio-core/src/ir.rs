pub use mercurio_kir::{
    KIR_SCHEMA_VERSION, KirDocument, KirElement, KirError, KirFieldKind, KirFieldRegistry,
    KirFieldSpec, KirValidationDiagnostic,
};

use std::path::Path;

use crate::paths::default_stdlib_path;

pub fn load_model_stack(model_path: &Path) -> Result<KirDocument, KirError> {
    let stdlib_path = default_stdlib_path();
    if paths_equivalent(model_path, &stdlib_path) {
        return KirDocument::from_path(model_path);
    }

    let stdlib_document = KirDocument::from_path(&stdlib_path)?;

    let user_document = match model_path.extension().and_then(|value| value.to_str()) {
        Some("sysml") => {
            crate::frontend::sysml::load_sysml_document_with_stdlib(model_path, &stdlib_document)
                .map_err(|err| KirError::Frontend(err.to_string()))?
        }
        Some("kerml") => {
            crate::frontend::kerml::load_kerml_document_with_stdlib(model_path, &stdlib_document)
                .map_err(|err| KirError::Frontend(err.to_string()))?
        }
        _ => KirDocument::from_path(model_path)?,
    };

    KirDocument::merge([stdlib_document, user_document])
}

fn paths_equivalent(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn load_model_stack_accepts_kerml_sources() {
        let document = super::load_model_stack(&crate::paths::repo_path(
            "test_files/kerml/minimal_classifier.kerml",
        ))
        .unwrap();

        assert!(
            document
                .elements
                .iter()
                .any(|element| element.id == "type.Demo.Vehicle")
        );
        assert!(
            document
                .elements
                .iter()
                .any(|element| element.id == "feature.Demo.Vehicle.engine")
        );
    }
}
