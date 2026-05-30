use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use mercurio_core::{
    AuthoringProject, ContainerSelector, ElementView, Graph, KirDocument, MetamodelAttributeRegistry,
    Mutation, QualifiedName, WriteBackMode, WriteBackResult, compile_sysml_text,
    create_empty_model, default_language_profile, default_stdlib_path, generate_python_wrappers,
    load_authoring_project_from_sysml,
};
use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyType;

static DEFAULT_STDLIB_DOCUMENT: OnceLock<Result<KirDocument, String>> = OnceLock::new();

#[pyclass(name = "WriteBackResult")]
#[derive(Debug, Clone)]
struct PyWriteBackResult {
    #[pyo3(get)]
    edited_files: BTreeMap<String, String>,
    #[pyo3(get)]
    changed_files: Vec<String>,
    #[pyo3(get)]
    changed_declarations: Vec<String>,
    #[pyo3(get)]
    mode: String,
    #[pyo3(get)]
    validation_ok: bool,
    #[pyo3(get)]
    validation_message: Option<String>,
}

#[pymethods]
impl PyWriteBackResult {
    fn __repr__(&self) -> String {
        format!(
            "WriteBackResult(changed_files={:?}, mode={:?}, validation_ok={})",
            self.changed_files, self.mode, self.validation_ok
        )
    }
}

#[pyclass(name = "SemanticModel")]
#[derive(Clone)]
struct PySemanticModel {
    document: Arc<KirDocument>,
    graph: Arc<Graph>,
    registry: Arc<MetamodelAttributeRegistry>,
}

#[pymethods]
impl PySemanticModel {
    #[classmethod]
    fn from_kir_json(_cls: &Bound<'_, PyType>, content: String) -> PyResult<Self> {
        let document = KirDocument::from_str(&content)
            .map_err(|err| PyValueError::new_err(err.to_string()))?;
        py_semantic_model(document)
    }

    fn element(&self, element_id: String) -> PyResult<PyElementView> {
        let node_id = self
            .graph
            .node_id(&element_id)
            .ok_or_else(|| PyValueError::new_err(format!("element not found: {element_id}")))?;
        Ok(PyElementView {
            graph: self.graph.clone(),
            registry: self.registry.clone(),
            node_id,
        })
    }

    fn elements(&self) -> Vec<PyElementView> {
        self.graph
            .elements()
            .iter()
            .map(|element| PyElementView {
                graph: self.graph.clone(),
                registry: self.registry.clone(),
                node_id: element.id,
            })
            .collect()
    }

    fn element_count(&self) -> usize {
        self.document.elements.len()
    }

    fn generate_python_wrappers(&self, module_name: String) -> PyResult<BTreeMap<String, String>> {
        let profile = default_language_profile()
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
        Ok(generate_python_wrappers(&self.document, &profile, &module_name).files)
    }

    fn __repr__(&self) -> String {
        format!("SemanticModel(elements={})", self.document.elements.len())
    }
}

#[pyclass(name = "ElementView")]
#[derive(Clone)]
struct PyElementView {
    graph: Arc<Graph>,
    registry: Arc<MetamodelAttributeRegistry>,
    node_id: mercurio_core::NodeId,
}

#[pymethods]
impl PyElementView {
    #[getter]
    fn id(&self) -> PyResult<String> {
        Ok(self.view()?.id().to_string())
    }

    #[getter]
    fn kind(&self) -> PyResult<String> {
        Ok(self.view()?.kind().to_string())
    }

    #[getter]
    fn layer(&self) -> PyResult<u8> {
        Ok(self.view()?.layer())
    }

    fn metatype_id(&self) -> PyResult<Option<String>> {
        Ok(self.view()?.metatype().map(|summary| summary.id))
    }

    fn get_json(&self, name: String) -> PyResult<Option<String>> {
        self.view()?
            .get(&name)
            .map(serde_json::to_string)
            .transpose()
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))
    }

    fn effective_json(&self, name: String) -> PyResult<Option<String>> {
        self.view()?
            .effective(&name)
            .map(|value| serde_json::to_string(&value))
            .transpose()
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))
    }

    fn get_str(&self, name: String) -> PyResult<Option<String>> {
        Ok(self.view()?.get_str(&name).map(str::to_string))
    }

    fn effective_str(&self, name: String) -> PyResult<Option<String>> {
        Ok(self
            .view()?
            .effective(&name)
            .and_then(|value| value.as_str().map(str::to_string)))
    }

    fn references(&self, relation: String) -> PyResult<Vec<PyElementView>> {
        Ok(self
            .view()?
            .references(&relation)
            .into_iter()
            .map(|view| PyElementView {
                graph: self.graph.clone(),
                registry: self.registry.clone(),
                node_id: view.node_id(),
            })
            .collect())
    }

    fn attribute_names(&self) -> PyResult<Vec<String>> {
        Ok(self
            .view()?
            .attributes()
            .map(|query| query.rows.into_iter().map(|row| row.name).collect())
            .unwrap_or_default())
    }

    fn __repr__(&self) -> PyResult<String> {
        let view = self.view()?;
        Ok(format!("ElementView(id={:?}, kind={:?})", view.id(), view.kind()))
    }
}

impl PyElementView {
    fn view(&self) -> PyResult<ElementView<'_>> {
        ElementView::new(&self.graph, &self.registry, self.node_id)
            .ok_or_else(|| PyRuntimeError::new_err("stale element view"))
    }
}

#[pyclass(name = "ModelBuilder")]
struct PyModelBuilder {
    project: AuthoringProject,
    validate_each_mutation: bool,
    pending_changed_files: BTreeSet<String>,
    pending_changed_declarations: BTreeSet<String>,
}

#[pymethods]
impl PyModelBuilder {
    #[new]
    #[pyo3(signature = (validate_each_mutation=true))]
    fn new(validate_each_mutation: bool) -> Self {
        Self {
            project: create_empty_model(),
            validate_each_mutation,
            pending_changed_files: BTreeSet::new(),
            pending_changed_declarations: BTreeSet::new(),
        }
    }

    #[classmethod]
    #[pyo3(signature = (files, validate_each_mutation=true))]
    fn from_sysml_files(
        _cls: &Bound<'_, PyType>,
        files: BTreeMap<String, String>,
        validate_each_mutation: bool,
    ) -> PyResult<Self> {
        let project = load_authoring_project_from_sysml(files).map_err(authoring_error)?;
        Ok(Self {
            project,
            validate_each_mutation,
            pending_changed_files: BTreeSet::new(),
            pending_changed_declarations: BTreeSet::new(),
        })
    }

    fn add_package(&mut self, target_file: String, name: String) -> PyResult<PyWriteBackResult> {
        self.apply_and_write_back(Mutation::AddPackage {
            target_file,
            package_name: qname(&name),
        })
    }

    #[pyo3(signature = (container, keyword, name, specializes=None))]
    fn add_definition(
        &mut self,
        container: String,
        keyword: String,
        name: String,
        specializes: Option<Vec<String>>,
    ) -> PyResult<PyWriteBackResult> {
        self.apply_and_write_back(Mutation::AddDefinition {
            container: selector(&container),
            keyword,
            name,
            specializes: qnames(specializes),
        })
    }

    #[pyo3(signature = (container, keyword, name, ty=None, specializes=None))]
    fn add_usage(
        &mut self,
        container: String,
        keyword: String,
        name: String,
        ty: Option<String>,
        specializes: Option<Vec<String>>,
    ) -> PyResult<PyWriteBackResult> {
        self.apply_and_write_back(Mutation::AddUsage {
            container: selector(&container),
            keyword,
            name,
            ty: ty.as_deref().map(qname),
            specializes: qnames(specializes),
        })
    }

    fn rename(&mut self, element: String, new_name: String) -> PyResult<PyWriteBackResult> {
        self.apply_and_write_back(Mutation::RenameDeclaration {
            qualified_name: qname(&element),
            new_name,
        })
    }

    #[pyo3(signature = (element, ty=None))]
    fn set_usage_type(
        &mut self,
        element: String,
        ty: Option<String>,
    ) -> PyResult<PyWriteBackResult> {
        self.apply_and_write_back(Mutation::UpdateUsageType {
            qualified_name: qname(&element),
            ty: ty.as_deref().map(qname),
        })
    }

    #[pyo3(signature = (element, expression=None))]
    fn set_expression(
        &mut self,
        element: String,
        expression: Option<String>,
    ) -> PyResult<PyWriteBackResult> {
        self.apply_and_write_back(Mutation::SetExpression {
            qualified_name: qname(&element),
            expression,
        })
    }

    fn move_declaration(
        &mut self,
        element: String,
        destination: String,
    ) -> PyResult<PyWriteBackResult> {
        self.apply_and_write_back(Mutation::MoveDeclaration {
            qualified_name: qname(&element),
            destination: selector(&destination),
        })
    }

    fn render_file(&self, path: String) -> PyResult<String> {
        self.project.render_new_file(&path).map_err(authoring_error)
    }

    fn files(&self) -> Vec<String> {
        self.project
            .files()
            .map(|(path, _)| path.to_string())
            .collect()
    }

    fn rendered_files(&self) -> PyResult<BTreeMap<String, String>> {
        let mut rendered = BTreeMap::new();
        for (path, _) in self.project.files() {
            rendered.insert(
                path.to_string(),
                self.project
                    .render_new_file(path)
                    .map_err(authoring_error)?,
            );
        }
        Ok(rendered)
    }

    fn compile_json(&self) -> PyResult<String> {
        let document = self.compile_document()?;
        serde_json::to_string_pretty(&document)
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))
    }

    fn compile_model(&self) -> PyResult<PySemanticModel> {
        py_semantic_model(self.compile_document()?)
    }

    fn validate(&mut self) -> PyResult<PyWriteBackResult> {
        let changed_files = if self.pending_changed_files.is_empty() {
            self.project
                .files()
                .map(|(path, _)| path.to_string())
                .collect::<BTreeSet<_>>()
        } else {
            self.pending_changed_files.clone()
        };
        let changed_declarations = self
            .pending_changed_declarations
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        let write_back = self
            .project
            .write_back_changed_files_and_update(&changed_files)
            .map_err(authoring_error)?;
        self.pending_changed_files.clear();
        self.pending_changed_declarations.clear();
        Ok(py_write_back_result(
            write_back,
            changed_files.into_iter().collect(),
            changed_declarations,
        ))
    }

    #[pyo3(signature = (directory, result_name=None))]
    fn write_handoff(&self, directory: PathBuf, result_name: Option<String>) -> PyResult<PathBuf> {
        let rendered = self.rendered_files()?;
        std::fs::create_dir_all(&directory).map_err(io_error)?;
        let files_dir = directory.join("files");
        std::fs::create_dir_all(&files_dir).map_err(io_error)?;

        let mut changed_files = Vec::new();
        for (path, content) in &rendered {
            let output_path = files_dir.join(path);
            if let Some(parent) = output_path.parent() {
                std::fs::create_dir_all(parent).map_err(io_error)?;
            }
            write_atomic(&output_path, content)?;
            changed_files.push(format!("files/{}", path.replace('\\', "/")));
        }

        let result_path = directory.join(result_name.unwrap_or_else(|| "result.json".to_string()));
        let primary_file = changed_files.first().cloned();
        let result = serde_json::json!({
            "status": "ok",
            "changed_files": changed_files,
            "primary_file": primary_file,
            "diagnostics": []
        });
        let result_text = serde_json::to_string_pretty(&result)
            .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
        write_atomic(&result_path, &result_text)?;
        Ok(result_path)
    }

    fn __repr__(&self) -> String {
        format!(
            "ModelBuilder(files={:?}, validate_each_mutation={})",
            self.files(),
            self.validate_each_mutation
        )
    }
}

impl PyModelBuilder {
    fn apply_and_write_back(&mut self, mutation: Mutation) -> PyResult<PyWriteBackResult> {
        let result = self
            .project
            .apply_mutation(mutation)
            .map_err(authoring_error)?;
        let changed_files = result.changed_files.iter().cloned().collect::<Vec<_>>();
        let changed_declarations = result
            .changed_declarations
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if !self.validate_each_mutation {
            self.pending_changed_files
                .extend(result.changed_files.iter().cloned());
            self.pending_changed_declarations
                .extend(result.changed_declarations.iter().cloned());
            return self.deferred_write_back_result(changed_files, changed_declarations);
        }
        let write_back = self
            .project
            .write_back_mutation(&result)
            .map_err(authoring_error)?;
        Ok(py_write_back_result(
            write_back,
            changed_files,
            changed_declarations,
        ))
    }

    fn compile_document(&self) -> PyResult<KirDocument> {
        let rendered = self.rendered_files()?;
        let stdlib = default_stdlib_document()?;
        let mut documents = Vec::new();
        for (path, content) in rendered {
            documents.push(
                compile_sysml_text(&content, &path, stdlib)
                    .map_err(|err| PyValueError::new_err(err.to_string()))?,
            );
        }
        KirDocument::merge(documents).map_err(|err| PyRuntimeError::new_err(err.to_string()))
    }

    fn deferred_write_back_result(
        &self,
        changed_files: Vec<String>,
        changed_declarations: Vec<String>,
    ) -> PyResult<PyWriteBackResult> {
        let mut edited_files = BTreeMap::new();
        for path in &changed_files {
            edited_files.insert(
                path.clone(),
                self.project
                    .render_new_file(path)
                    .map_err(authoring_error)?,
            );
        }
        Ok(PyWriteBackResult {
            edited_files,
            changed_files,
            changed_declarations,
            mode: "deferred".to_string(),
            validation_ok: true,
            validation_message: Some(
                "validation deferred; call validate() to compile and round-trip check".to_string(),
            ),
        })
    }
}

fn default_stdlib_document() -> PyResult<&'static KirDocument> {
    DEFAULT_STDLIB_DOCUMENT
        .get_or_init(|| {
            KirDocument::from_path(&default_stdlib_path()).map_err(|err| err.to_string())
        })
        .as_ref()
        .map_err(|err| PyRuntimeError::new_err(err.clone()))
}

fn py_semantic_model(document: KirDocument) -> PyResult<PySemanticModel> {
    let graph = Graph::from_document(document.clone())
        .map_err(|err| PyRuntimeError::new_err(err.to_string()))?;
    let registry = MetamodelAttributeRegistry::build(&graph);
    Ok(PySemanticModel {
        document: Arc::new(document),
        graph: Arc::new(graph),
        registry: Arc::new(registry),
    })
}

#[pymodule]
fn mercurio_core_native(_py: Python<'_>, module: &Bound<'_, PyModule>) -> PyResult<()> {
    module.add_class::<PyModelBuilder>()?;
    module.add_class::<PyWriteBackResult>()?;
    module.add_class::<PySemanticModel>()?;
    module.add_class::<PyElementView>()?;
    Ok(())
}

fn py_write_back_result(
    write_back: WriteBackResult,
    changed_files: Vec<String>,
    changed_declarations: Vec<String>,
) -> PyWriteBackResult {
    PyWriteBackResult {
        edited_files: write_back.edited_files,
        changed_files,
        changed_declarations,
        mode: match write_back.mode {
            WriteBackMode::LocalizedPatch => "localized_patch",
            WriteBackMode::CanonicalRewrite => "canonical_rewrite",
        }
        .to_string(),
        validation_ok: write_back.validation.ok,
        validation_message: write_back.validation.message,
    }
}

fn qname(value: &str) -> QualifiedName {
    QualifiedName::parse(value)
}

fn qnames(values: Option<Vec<String>>) -> Vec<QualifiedName> {
    values
        .unwrap_or_default()
        .iter()
        .map(|value| qname(value))
        .collect()
}

fn selector(value: &str) -> ContainerSelector {
    if let Some(path) = value.strip_prefix("file:") {
        return ContainerSelector::File {
            target_file: path.to_string(),
        };
    }
    let qualified_name = qname(value);
    if qualified_name.0.len() <= 1 {
        ContainerSelector::Package { qualified_name }
    } else {
        ContainerSelector::Declaration { qualified_name }
    }
}

fn authoring_error(err: mercurio_core::AuthoringError) -> PyErr {
    PyValueError::new_err(err.to_string())
}

fn io_error(err: std::io::Error) -> PyErr {
    PyRuntimeError::new_err(err.to_string())
}

fn write_atomic(path: &Path, content: &str) -> PyResult<()> {
    let mut tmp = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("output");
    tmp.set_file_name(format!("{file_name}.tmp"));
    std::fs::write(&tmp, content).map_err(io_error)?;
    if path.exists() {
        std::fs::remove_file(path).map_err(io_error)?;
    }
    std::fs::rename(&tmp, path).map_err(io_error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::PyModelBuilder;

    #[test]
    fn builder_creates_renders_and_compiles_model() {
        let mut builder = PyModelBuilder::new(true);

        builder
            .add_package("model.sysml".to_string(), "Demo".to_string())
            .unwrap();
        builder
            .add_definition(
                "Demo".to_string(),
                "part".to_string(),
                "Engine".to_string(),
                None,
            )
            .unwrap();
        builder
            .add_definition(
                "Demo".to_string(),
                "part".to_string(),
                "Vehicle".to_string(),
                None,
            )
            .unwrap();
        builder
            .add_usage(
                "Demo.Vehicle".to_string(),
                "part".to_string(),
                "engine".to_string(),
                Some("Engine".to_string()),
                None,
            )
            .unwrap();

        let rendered = builder.render_file("model.sysml".to_string()).unwrap();
        assert!(rendered.contains("package Demo"));
        assert!(rendered.contains("part def Vehicle"));
        assert!(rendered.contains("part engine: Engine"));

        let compiled = builder.compile_json().unwrap();
        assert!(compiled.contains("type.Demo.Vehicle"));
        assert!(compiled.contains("feature.Demo.Vehicle.engine"));
    }

    #[test]
    fn builder_can_defer_validation_until_requested() {
        let mut builder = PyModelBuilder::new(false);

        let package_result = builder
            .add_package("model.sysml".to_string(), "Demo".to_string())
            .unwrap();
        assert_eq!(package_result.mode, "deferred");

        builder
            .add_definition(
                "Demo".to_string(),
                "part".to_string(),
                "Vehicle".to_string(),
                None,
            )
            .unwrap();

        let rendered = builder.rendered_files().unwrap();
        assert!(rendered["model.sysml"].contains("part def Vehicle"));

        let validation = builder.validate().unwrap();
        assert_eq!(validation.mode, "canonical_rewrite");
        assert!(validation.validation_ok);
        assert_eq!(validation.changed_files, vec!["model.sysml"]);
    }
}
