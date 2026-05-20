use std::path::Path;

use crate::frontend::ast::{
    AliasDecl, BinaryOp, Declaration, Expr, GenericDefinitionDecl, GenericUsageDecl, ImportDecl,
    LiteralExpr, MultiplicityRange, PackageDecl, PartDefinitionDecl, PartUsageDecl, QualifiedName,
    SourceSpan, SysmlModule, UnaryOp,
};
use crate::frontend::diagnostics::Diagnostic;
use crate::frontend::lexer::{Token, TokenKind, lex};
use crate::frontend::resolver::{
    ResolverContext, resolve_module, resolve_module_with_context,
    resolve_module_with_resolver_context,
};
use crate::frontend::transpile::{MappingBundle, transpile_module};
use crate::ir::KirDocument;
use crate::logging::{compile_timer_start, log_compile_timed_event};
use crate::paths::default_stdlib_path;

#[derive(Debug)]
pub enum SysmlError {
    Io(std::io::Error),
    Diagnostic(Diagnostic),
    Kir(crate::ir::KirError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticCompileStatus {
    Ok,
    Partial,
    Failed,
}

#[derive(Debug, Clone)]
pub struct SemanticCompileReport {
    pub status: SemanticCompileStatus,
    pub diagnostics: Vec<Diagnostic>,
    pub document: Option<KirDocument>,
}

#[derive(Debug, Clone)]
pub struct ParseReport {
    pub module: SysmlModule,
    pub diagnostics: Vec<Diagnostic>,
}

impl std::fmt::Display for SysmlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(err) => write!(f, "failed to read sysml file: {err}"),
            Self::Diagnostic(err) => write!(f, "{err}"),
            Self::Kir(err) => write!(f, "{err}"),
        }
    }
}

impl std::error::Error for SysmlError {}

impl From<std::io::Error> for SysmlError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<Diagnostic> for SysmlError {
    fn from(value: Diagnostic) -> Self {
        Self::Diagnostic(value)
    }
}

impl From<crate::ir::KirError> for SysmlError {
    fn from(value: crate::ir::KirError) -> Self {
        Self::Kir(value)
    }
}

pub fn load_sysml_document(path: &Path) -> Result<KirDocument, SysmlError> {
    let stdlib = KirDocument::from_path(&default_stdlib_path())?;
    load_sysml_document_with_stdlib(path, &stdlib)
}

pub fn load_sysml_document_with_stdlib(
    path: &Path,
    stdlib: &KirDocument,
) -> Result<KirDocument, SysmlError> {
    let input = std::fs::read_to_string(path)?;
    compile_sysml_text(&input, &path.display().to_string(), stdlib).map_err(Into::into)
}

pub fn compile_sysml_text(
    input: &str,
    source_name: &str,
    stdlib: &KirDocument,
) -> Result<KirDocument, Diagnostic> {
    let parse_start = compile_timer_start();
    let module = parse_sysml(input)?;
    log_compile_timed_event(
        "sysml.compile.parse",
        parse_start,
        "ok",
        format!("source={} bytes={}", source_name, input.len()),
    );
    compile_sysml_module(&module, source_name, stdlib)
}

pub fn compile_sysml_module(
    module: &SysmlModule,
    source_name: &str,
    stdlib: &KirDocument,
) -> Result<KirDocument, Diagnostic> {
    let mapping_start = compile_timer_start();
    let mappings = MappingBundle::load()?;
    log_compile_timed_event(
        "sysml.compile.mapping_load",
        mapping_start,
        "ok",
        format!("source={}", source_name),
    );

    let resolve_start = compile_timer_start();
    let resolved = resolve_module(module, stdlib, &mappings)?;
    log_compile_timed_event(
        "sysml.compile.resolve",
        resolve_start,
        "ok",
        format!("source={} context_modules=1", source_name),
    );

    let transpile_start = compile_timer_start();
    let document = transpile_module(&resolved, source_name, &mappings)?;
    log_compile_timed_event(
        "sysml.compile.transpile",
        transpile_start,
        "ok",
        format!(
            "source={} elements={}",
            source_name,
            document.elements.len()
        ),
    );
    Ok(document)
}

pub fn compile_sysml_text_with_context(
    input: &str,
    source_name: &str,
    context_modules: &[SysmlModule],
    stdlib: &KirDocument,
) -> Result<KirDocument, Diagnostic> {
    let parse_start = compile_timer_start();
    let module = parse_sysml(input)?;
    log_compile_timed_event(
        "sysml.compile.parse",
        parse_start,
        "ok",
        format!("source={} bytes={}", source_name, input.len()),
    );
    compile_sysml_module_with_context(&module, source_name, context_modules, stdlib)
}

pub fn compile_sysml_text_with_context_report(
    input: &str,
    source_name: &str,
    context_modules: &[SysmlModule],
    stdlib: &KirDocument,
) -> SemanticCompileReport {
    let parse_start = compile_timer_start();
    let parse_report = match parse_sysml_recovering(input) {
        Ok(report) => report,
        Err(diagnostic) => {
            log_compile_timed_event(
                "sysml.compile.parse_recovering",
                parse_start,
                "error",
                format!("source={} bytes={}", source_name, input.len()),
            );
            return SemanticCompileReport {
                status: SemanticCompileStatus::Failed,
                diagnostics: vec![diagnostic],
                document: None,
            };
        }
    };
    log_compile_timed_event(
        "sysml.compile.parse_recovering",
        parse_start,
        "ok",
        format!(
            "source={} bytes={} diagnostics={}",
            source_name,
            input.len(),
            parse_report.diagnostics.len()
        ),
    );

    let mut compile_report = compile_sysml_module_with_context_report_with_limit(
        &parse_report.module,
        source_name,
        context_modules,
        stdlib,
        partial_compile_attempt_limit(input.len()),
    );
    if parse_report.diagnostics.is_empty() {
        return compile_report;
    }

    let mut diagnostics = parse_report.diagnostics;
    diagnostics.extend(compile_report.diagnostics);
    compile_report.diagnostics = diagnostics;
    if compile_report.document.is_some() {
        compile_report.status = SemanticCompileStatus::Partial;
    }
    compile_report
}

pub fn compile_sysml_module_with_context_report(
    module: &SysmlModule,
    source_name: &str,
    context_modules: &[SysmlModule],
    stdlib: &KirDocument,
) -> SemanticCompileReport {
    compile_sysml_module_with_context_report_with_limit(
        module,
        source_name,
        context_modules,
        stdlib,
        MAX_PARTIAL_COMPILE_ATTEMPTS,
    )
}

pub(crate) fn compile_sysml_module_with_context_report_with_limit(
    module: &SysmlModule,
    source_name: &str,
    context_modules: &[SysmlModule],
    stdlib: &KirDocument,
    max_attempts: usize,
) -> SemanticCompileReport {
    let mappings = match MappingBundle::load() {
        Ok(mappings) => mappings,
        Err(diagnostic) => {
            return SemanticCompileReport {
                status: SemanticCompileStatus::Failed,
                diagnostics: vec![diagnostic],
                document: None,
            };
        }
    };
    let working_context_modules =
        replace_equivalent_context_module(context_modules, module, module);
    let resolver_context =
        match ResolverContext::from_modules(&working_context_modules, stdlib, mappings) {
            Ok(context) => context,
            Err(diagnostic) => {
                return SemanticCompileReport {
                    status: SemanticCompileStatus::Failed,
                    diagnostics: vec![diagnostic],
                    document: None,
                };
            }
        };
    compile_sysml_module_with_resolver_context_report_with_limit(
        module,
        source_name,
        context_modules,
        stdlib,
        &resolver_context,
        mappings,
        max_attempts,
    )
}

pub fn compile_sysml_module_with_resolver_context_report_with_limit(
    module: &SysmlModule,
    source_name: &str,
    context_modules: &[SysmlModule],
    stdlib: &KirDocument,
    resolver_context: &ResolverContext,
    mappings: &MappingBundle,
    max_attempts: usize,
) -> SemanticCompileReport {
    let mut diagnostics = Vec::new();
    let mut working_module = module.clone();
    let mut owned_resolver_context = None;

    for attempt in 0..max_attempts {
        let attempt_start = compile_timer_start();
        let active_resolver_context = owned_resolver_context.as_ref().unwrap_or(resolver_context);
        match compile_sysml_module_with_resolver_context(
            &working_module,
            source_name,
            active_resolver_context,
            mappings,
        ) {
            Ok(document) => {
                log_compile_timed_event(
                    "sysml.compile.partial_attempt",
                    attempt_start,
                    "ok",
                    format!(
                        "source={} attempt={} max_attempts={} diagnostics={} elements={}",
                        source_name,
                        attempt + 1,
                        max_attempts,
                        diagnostics.len(),
                        document.elements.len()
                    ),
                );
                return SemanticCompileReport {
                    status: if diagnostics.is_empty() {
                        SemanticCompileStatus::Ok
                    } else {
                        SemanticCompileStatus::Partial
                    },
                    diagnostics,
                    document: Some(document),
                };
            }
            Err(diagnostic) => {
                log_compile_timed_event(
                    "sysml.compile.partial_attempt",
                    attempt_start,
                    "error",
                    format!(
                        "source={} attempt={} max_attempts={} diagnostics={}",
                        source_name,
                        attempt + 1,
                        max_attempts,
                        diagnostics.len() + 1
                    ),
                );
                let Some(span) = diagnostic.span.clone() else {
                    diagnostics.push(diagnostic);
                    break;
                };
                diagnostics.push(diagnostic);

                if !prune_declaration_for_span(&mut working_module, &span) {
                    break;
                }
                let working_context_modules =
                    replace_equivalent_context_module(context_modules, module, &working_module);
                match ResolverContext::from_modules(&working_context_modules, stdlib, mappings) {
                    Ok(context) => owned_resolver_context = Some(context),
                    Err(diagnostic) => {
                        diagnostics.push(diagnostic);
                        break;
                    }
                }
            }
        }
    }

    if max_attempts < MAX_PARTIAL_COMPILE_ATTEMPTS && diagnostics.len() >= max_attempts {
        diagnostics.push(Diagnostic::new(
            format!(
                "partial semantic recovery stopped after {max_attempts} attempts for a large source file"
            ),
            None,
        ));
    }

    SemanticCompileReport {
        status: SemanticCompileStatus::Failed,
        diagnostics,
        document: None,
    }
}

const MAX_PARTIAL_COMPILE_ATTEMPTS: usize = 32;

pub fn partial_compile_attempt_limit(input_bytes: usize) -> usize {
    if input_bytes >= 50_000 {
        8
    } else if input_bytes >= 20_000 {
        16
    } else {
        MAX_PARTIAL_COMPILE_ATTEMPTS
    }
}

pub fn compile_sysml_module_with_context(
    module: &SysmlModule,
    source_name: &str,
    context_modules: &[SysmlModule],
    stdlib: &KirDocument,
) -> Result<KirDocument, Diagnostic> {
    let mapping_start = compile_timer_start();
    let mappings = MappingBundle::load()?;
    log_compile_timed_event(
        "sysml.compile.mapping_load",
        mapping_start,
        "ok",
        format!("source={}", source_name),
    );

    let resolve_start = compile_timer_start();
    let resolved = resolve_module_with_context(module, context_modules, stdlib, &mappings)?;
    log_compile_timed_event(
        "sysml.compile.resolve",
        resolve_start,
        "ok",
        format!(
            "source={} context_modules={}",
            source_name,
            context_modules.len()
        ),
    );

    let transpile_start = compile_timer_start();
    let document = transpile_module(&resolved, source_name, &mappings)?;
    log_compile_timed_event(
        "sysml.compile.transpile",
        transpile_start,
        "ok",
        format!(
            "source={} elements={}",
            source_name,
            document.elements.len()
        ),
    );
    Ok(document)
}

pub(crate) fn compile_sysml_module_with_resolver_context(
    module: &SysmlModule,
    source_name: &str,
    resolver_context: &ResolverContext,
    mappings: &MappingBundle,
) -> Result<KirDocument, Diagnostic> {
    let resolve_start = compile_timer_start();
    let resolved = resolve_module_with_resolver_context(module, resolver_context, mappings)?;
    log_compile_timed_event(
        "sysml.compile.resolve",
        resolve_start,
        "ok",
        format!(
            "source={} context_modules={}",
            source_name,
            resolver_context.module_count()
        ),
    );

    let transpile_start = compile_timer_start();
    let document = transpile_module(&resolved, source_name, mappings)?;
    log_compile_timed_event(
        "sysml.compile.transpile",
        transpile_start,
        "ok",
        format!(
            "source={} elements={}",
            source_name,
            document.elements.len()
        ),
    );
    Ok(document)
}

fn replace_equivalent_context_module(
    context_modules: &[SysmlModule],
    original: &SysmlModule,
    replacement: &SysmlModule,
) -> Vec<SysmlModule> {
    let mut replaced = false;
    let mut modules = context_modules
        .iter()
        .map(|module| {
            if !replaced && module == original {
                replaced = true;
                replacement.clone()
            } else {
                module.clone()
            }
        })
        .collect::<Vec<_>>();

    if !replaced {
        modules.push(replacement.clone());
    }

    modules
}

fn prune_declaration_for_span(module: &mut SysmlModule, span: &SourceSpan) -> bool {
    if let Some(package) = module.package.as_mut() {
        let pruned = prune_package_for_span(package, span);
        module.members = vec![Declaration::Package(package.clone())];
        module.imports = package.imports.clone();
        module.definitions = package.definitions.clone();
        return pruned;
    }

    let pruned = prune_declarations_for_span(&mut module.members, span);
    module.imports = module
        .members
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Import(import) => Some(import.clone()),
            _ => None,
        })
        .collect();
    module.definitions = module
        .members
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::PartDefinition(definition) => Some(definition.clone()),
            _ => None,
        })
        .collect();
    pruned
}

fn prune_package_for_span(package: &mut PackageDecl, span: &SourceSpan) -> bool {
    let pruned = prune_declarations_for_span(&mut package.members, span);
    package.imports = package
        .members
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::Import(import) => Some(import.clone()),
            _ => None,
        })
        .collect();
    package.definitions = package
        .members
        .iter()
        .filter_map(|declaration| match declaration {
            Declaration::PartDefinition(definition) => Some(definition.clone()),
            _ => None,
        })
        .collect();
    pruned
}

fn prune_declarations_for_span(declarations: &mut Vec<Declaration>, span: &SourceSpan) -> bool {
    for declaration in declarations.iter_mut() {
        if prune_child_declaration_for_span(declaration, span) {
            return true;
        }
    }

    if let Some(index) = declarations
        .iter()
        .position(|declaration| span_contains(declaration_span(declaration), span))
    {
        declarations.remove(index);
        return true;
    }

    false
}

fn prune_child_declaration_for_span(declaration: &mut Declaration, span: &SourceSpan) -> bool {
    match declaration {
        Declaration::Package(package) => prune_package_for_span(package, span),
        Declaration::PartDefinition(definition) => {
            prune_declarations_for_span(&mut definition.members, span)
                || prune_part_usages_for_span(&mut definition.part_members, span)
        }
        Declaration::GenericDefinition(definition) => {
            prune_declarations_for_span(&mut definition.members, span)
        }
        Declaration::PartUsage(usage) => prune_declarations_for_span(&mut usage.body_members, span),
        Declaration::GenericUsage(usage) => {
            prune_declarations_for_span(&mut usage.body_members, span)
        }
        Declaration::Import(_) | Declaration::Alias(_) => false,
    }
}

fn prune_part_usages_for_span(usages: &mut Vec<PartUsageDecl>, span: &SourceSpan) -> bool {
    for usage in usages.iter_mut() {
        if prune_declarations_for_span(&mut usage.body_members, span) {
            return true;
        }
    }

    if let Some(index) = usages
        .iter()
        .position(|usage| span_contains(&usage.span, span))
    {
        usages.remove(index);
        return true;
    }

    false
}

fn declaration_span(declaration: &Declaration) -> &SourceSpan {
    match declaration {
        Declaration::Package(declaration) => &declaration.span,
        Declaration::Import(declaration) => &declaration.span,
        Declaration::PartDefinition(declaration) => &declaration.span,
        Declaration::PartUsage(declaration) => &declaration.span,
        Declaration::GenericDefinition(declaration) => &declaration.span,
        Declaration::GenericUsage(declaration) => &declaration.span,
        Declaration::Alias(declaration) => &declaration.span,
    }
}

fn span_contains(outer: &SourceSpan, inner: &SourceSpan) -> bool {
    span_position_before_or_equal(
        outer.start_line,
        outer.start_col,
        inner.start_line,
        inner.start_col,
    ) && span_position_before_or_equal(inner.end_line, inner.end_col, outer.end_line, outer.end_col)
}

fn span_position_before_or_equal(
    left_line: usize,
    left_col: usize,
    right_line: usize,
    right_col: usize,
) -> bool {
    left_line < right_line || (left_line == right_line && left_col <= right_col)
}

pub fn parse_sysml(input: &str) -> Result<SysmlModule, Diagnostic> {
    let tokens = lex(input)?;
    Parser::new(tokens, false)
        .parse()
        .map(|report| report.module)
}

pub fn parse_sysml_recovering(input: &str) -> Result<ParseReport, Diagnostic> {
    let tokens = lex(input)?;
    Parser::new(tokens, true).parse()
}

struct Parser {
    tokens: Vec<Token>,
    index: usize,
    pending_docs: Vec<String>,
    diagnostics: Vec<Diagnostic>,
    recover: bool,
}

impl Parser {
    fn new(tokens: Vec<Token>, recover: bool) -> Self {
        Self {
            tokens,
            index: 0,
            pending_docs: Vec::new(),
            diagnostics: Vec::new(),
            recover,
        }
    }

    fn parse(mut self) -> Result<ParseReport, Diagnostic> {
        let mut module = SysmlModule::default();

        while !self.at_end() {
            self.collect_docs();
            let parsed = match self.parse_declaration() {
                Ok(parsed) => parsed,
                Err(diagnostic) if self.recover => {
                    self.diagnostics.push(diagnostic);
                    self.recover_declaration();
                    continue;
                }
                Err(diagnostic) => return Err(diagnostic),
            };
            match parsed {
                Some(Declaration::Package(package)) => {
                    if module.package.is_none() {
                        module.package = Some(package.clone());
                    }
                    module.members.push(Declaration::Package(package));
                }
                Some(declaration) => append_module_member(&mut module, declaration),
                None => break,
            }
        }

        Ok(ParseReport {
            module,
            diagnostics: self.diagnostics,
        })
    }

    fn parse_declaration(&mut self) -> Result<Option<Declaration>, Diagnostic> {
        let docs = std::mem::take(&mut self.pending_docs);
        let modifiers = self.consume_declaration_modifiers();
        if let Some(keyword) = self.modifier_led_definition_keyword(&modifiers) {
            let start = self.current().clone();
            self.expect(TokenKind::Def, "expected `def`")?;
            return Ok(Some(self.parse_definition_after_keyword(
                &keyword, start, docs, modifiers,
            )?));
        }
        if self.starts_control_flow_reference(&modifiers) {
            return Ok(Some(self.parse_control_flow_reference(docs, modifiers)?));
        }
        if !modifiers.is_empty()
            && matches!(
                self.peek_kind(),
                TokenKind::Semicolon
                    | TokenKind::Colon
                    | TokenKind::Specializes
                    | TokenKind::Redefines
                    | TokenKind::Equals
                    | TokenKind::LBrace
            )
        {
            return Ok(Some(self.parse_modifier_only_declaration(docs, modifiers)?));
        }
        let current_kind = self.peek_kind().clone();
        let declaration = match current_kind {
            TokenKind::Package => Declaration::Package(self.parse_package(docs, modifiers)?),
            TokenKind::Import => Declaration::Import(self.parse_import(docs, modifiers)?),
            TokenKind::At => self.parse_annotation_usage(docs, modifiers)?,
            TokenKind::Part => self.parse_feature_declaration("part", docs, modifiers)?,
            TokenKind::Specializes | TokenKind::Redefines => {
                self.parse_relation_led_declaration(docs, modifiers)?
            }
            TokenKind::Identifier(ref value) if value == "alias" => {
                Declaration::Alias(self.parse_alias(docs, modifiers)?)
            }
            TokenKind::Identifier(ref value)
                if value == "use"
                    && matches!(self.next_kind(), Some(TokenKind::Identifier(next)) if next == "case") =>
            {
                self.parse_composite_feature_declaration(
                    "use", "case", "use-case", docs, modifiers,
                )?
            }
            TokenKind::Identifier(ref value)
                if value == "event"
                    && matches!(self.next_kind(), Some(TokenKind::Identifier(next)) if next == "occurrence") =>
            {
                self.parse_composite_feature_declaration(
                    "event",
                    "occurrence",
                    "occurrence",
                    docs,
                    modifiers,
                )?
            }
            TokenKind::Identifier(ref value)
                if matches!(value.as_str(), "assert" | "assume" | "require")
                    && matches!(self.next_kind(), Some(TokenKind::Identifier(next)) if next == "constraint") =>
            {
                self.parse_composite_feature_declaration(
                    value,
                    "constraint",
                    value,
                    docs,
                    modifiers,
                )?
            }
            TokenKind::Hash => self.parse_hashed_declaration(docs, modifiers)?,
            TokenKind::Identifier(ref value) if self.should_parse_as_feature_keyword(value) => {
                self.parse_feature_declaration(value, docs, modifiers)?
            }
            TokenKind::Identifier(_) => self.parse_implicit_usage(docs, modifiers)?,
            TokenKind::Eof => return Ok(None),
            _ => {
                return Err(self.error_here(
                    "expected a declaration such as `package`, `import`, `part`, `alias`, or another SysML declaration keyword",
                ));
            }
        };

        Ok(Some(declaration))
    }

    fn parse_relation_led_declaration(
        &mut self,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        let start = self.current().clone();
        let relation_kind = self.peek_kind().clone();
        self.advance();

        let relation_target = self.parse_qualified_name()?;
        let name = relation_target
            .segments
            .last()
            .cloned()
            .unwrap_or_else(|| "feature".to_string());
        let mut tail = self.parse_usage_tail(&[])?;

        match relation_kind {
            TokenKind::Specializes => tail.specializes.insert(0, relation_target),
            TokenKind::Redefines => tail.redefines.insert(0, relation_target),
            _ => {}
        }

        let end = self.finish_usage("declaration", tail.had_body)?;
        let keyword = implicit_usage_keyword(&modifiers);

        Ok(Declaration::GenericUsage(GenericUsageDecl {
            keyword: keyword.to_string(),
            name,
            is_implicit_name: false,
            ty: tail.ty,
            reference_target: None,
            multiplicity: tail.multiplicity,
            expression: tail.expression,
            additional_types: tail.additional_types,
            specializes: tail.specializes,
            subsets: tail.subsets,
            redefines: tail.redefines,
            body_members: tail.body_members,
            docs,
            modifiers,
            span: merge_span(&start.span, &end.span),
        }))
    }

    fn parse_hashed_declaration(
        &mut self,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        self.expect(TokenKind::Hash, "expected `#`")?;
        match self.peek_kind().clone() {
            TokenKind::Identifier(value)
                if value == "derivation"
                    && matches!(self.next_kind(), Some(TokenKind::Identifier(next)) if next == "connection") =>
            {
                let mut hashed_modifiers = modifiers;
                hashed_modifiers.push("derivation".to_string());
                self.parse_composite_feature_declaration(
                    "derivation",
                    "connection",
                    "connection",
                    docs,
                    hashed_modifiers,
                )
            }
            TokenKind::Identifier(value)
                if value == "causation"
                    && matches!(self.next_kind(), Some(TokenKind::Identifier(next)) if next == "connect") =>
            {
                self.parse_composite_feature_declaration(
                    "causation",
                    "connect",
                    "connect",
                    docs,
                    modifiers,
                )
            }
            TokenKind::Identifier(value) if matches!(self.next_kind(), Some(TokenKind::Hash)) => {
                let mut hashed_modifiers = modifiers;
                hashed_modifiers.push(value);
                self.advance();
                while matches!(self.peek_kind(), TokenKind::Hash)
                    && matches!(self.next_kind(), Some(TokenKind::Identifier(_)))
                {
                    self.expect(TokenKind::Hash, "expected `#`")?;
                    let modifier = self.expect_identifier("expected metadata annotation name")?;
                    hashed_modifiers.push(modifier);
                }
                match self.peek_kind().clone() {
                    TokenKind::Identifier(next) if self.should_parse_as_feature_keyword(&next) => {
                        self.parse_feature_declaration(&next, docs, hashed_modifiers)
                    }
                    TokenKind::Part => {
                        self.parse_feature_declaration("part", docs, hashed_modifiers)
                    }
                    _ => self.parse_implicit_usage(docs, hashed_modifiers),
                }
            }
            TokenKind::Identifier(value)
                if !matches!(value.as_str(), "cause" | "effect")
                    && (matches!(
                        self.next_kind(),
                        Some(TokenKind::Package | TokenKind::Import | TokenKind::Part)
                    ) || matches!(
                        self.next_kind(),
                        Some(TokenKind::Identifier(next)) if is_feature_keyword(next)
                    )) =>
            {
                let mut hashed_modifiers = modifiers;
                hashed_modifiers.push(value);
                self.advance();
                match self.peek_kind().clone() {
                    TokenKind::Package => Ok(Declaration::Package(
                        self.parse_package(docs, hashed_modifiers)?,
                    )),
                    TokenKind::Import => Ok(Declaration::Import(
                        self.parse_import(docs, hashed_modifiers)?,
                    )),
                    TokenKind::Part => {
                        self.parse_feature_declaration("part", docs, hashed_modifiers)
                    }
                    TokenKind::Identifier(next) => {
                        self.parse_feature_declaration(&next, docs, hashed_modifiers)
                    }
                    _ => unreachable!(),
                }
            }
            TokenKind::Identifier(value) => self.parse_feature_declaration(&value, docs, modifiers),
            _ => Err(self.error_here("expected hashed declaration keyword after `#`")),
        }
    }

    fn parse_composite_feature_declaration(
        &mut self,
        first_keyword: &str,
        second_keyword: &str,
        canonical_keyword: &str,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        let start =
            self.expect_identifier_named(first_keyword, &format!("expected `{first_keyword}`"))?;
        self.expect_identifier_named(
            second_keyword,
            &format!("expected `{second_keyword}` after `{first_keyword}`"),
        )?;
        let is_definition = matches!(self.peek_kind(), TokenKind::Def);
        if is_definition {
            self.advance();
            return self.parse_definition_after_keyword(canonical_keyword, start, docs, modifiers);
        }
        self.parse_usage_after_keyword(canonical_keyword, start, docs, modifiers)
    }

    fn parse_modifier_only_declaration(
        &mut self,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        let start = self.current().span.clone();
        let mut tail = self.parse_usage_tail(&[])?;
        let keyword = implicit_usage_keyword(&modifiers);
        let name = if matches!(self.peek_kind(), TokenKind::Semicolon) {
            modifiers
                .last()
                .cloned()
                .unwrap_or_else(|| keyword.to_string())
        } else {
            tail.derived_name(keyword)
        };
        let end = self.finish_usage("modifier-prefixed feature", tail.had_body)?;
        let has_type = tail.ty.is_some();
        let reference_target = infer_reference_target(keyword, &name, has_type, &mut tail);

        Ok(Declaration::GenericUsage(GenericUsageDecl {
            keyword: keyword.to_string(),
            name,
            is_implicit_name: true,
            ty: tail.ty,
            reference_target,
            multiplicity: tail.multiplicity,
            expression: tail.expression,
            additional_types: tail.additional_types,
            specializes: tail.specializes,
            subsets: tail.subsets,
            redefines: tail.redefines,
            body_members: tail.body_members,
            docs,
            modifiers,
            span: merge_span(&start, &end.span),
        }))
    }

    fn starts_control_flow_reference(&self, modifiers: &[String]) -> bool {
        if !modifiers
            .iter()
            .any(|modifier| modifier == "first" || modifier == "then")
        {
            return false;
        }

        match (self.peek_kind(), self.next_kind()) {
            (TokenKind::Identifier(_), Some(TokenKind::Semicolon)) => true,
            (TokenKind::Identifier(_), Some(TokenKind::Identifier(next))) => next == "then",
            _ => false,
        }
    }

    fn modifier_led_definition_keyword(&self, modifiers: &[String]) -> Option<String> {
        if !matches!(self.peek_kind(), TokenKind::Def) {
            return None;
        }

        modifiers
            .last()
            .filter(|modifier| matches!(modifier.as_str(), "individual"))
            .cloned()
    }

    fn parse_control_flow_reference(
        &mut self,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        let start = self.current().clone();
        let name = self.expect_identifier("expected control-flow target")?;

        while !matches!(
            self.peek_kind(),
            TokenKind::Semicolon | TokenKind::LBrace | TokenKind::RBrace | TokenKind::Eof
        ) {
            self.advance();
        }

        let mut body_closed = false;
        if matches!(self.peek_kind(), TokenKind::LBrace) {
            self.consume_opaque_block_with_open()?;
            body_closed = true;
        }

        let end = self.finish_usage("control-flow reference", body_closed)?;
        Ok(Declaration::GenericUsage(GenericUsageDecl {
            keyword: "succession".to_string(),
            name,
            is_implicit_name: false,
            ty: None,
            reference_target: None,
            multiplicity: None,
            expression: None,
            additional_types: Vec::new(),
            specializes: Vec::new(),
            subsets: Vec::new(),
            redefines: Vec::new(),
            body_members: Vec::new(),
            docs,
            modifiers,
            span: merge_span(&start.span, &end.span),
        }))
    }

    fn parse_package(
        &mut self,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<PackageDecl, Diagnostic> {
        let start = self.expect(TokenKind::Package, "expected `package`")?;
        let name = self.parse_qualified_name()?;
        if matches!(self.peek_kind(), TokenKind::Semicolon) {
            let end = self.expect(TokenKind::Semicolon, "expected `;` after package name")?;
            return Ok(PackageDecl {
                name,
                members: Vec::new(),
                imports: Vec::new(),
                definitions: Vec::new(),
                docs,
                modifiers,
                span: merge_span(&start.span, &end.span),
            });
        }

        self.expect(TokenKind::LBrace, "expected `{` after package name")?;

        let mut members = Vec::new();
        let mut imports = Vec::new();
        let mut definitions = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            self.collect_docs();
            let declaration = match self.parse_declaration().and_then(|declaration| {
                declaration.ok_or_else(|| self.error_here("expected declaration inside package"))
            }) {
                Ok(declaration) => declaration,
                Err(diagnostic) if self.recover => {
                    self.diagnostics.push(diagnostic);
                    self.recover_declaration();
                    continue;
                }
                Err(diagnostic) => return Err(diagnostic),
            };
            append_package_member(&mut members, &mut imports, &mut definitions, declaration);
            if matches!(self.peek_kind(), TokenKind::Eof) {
                break;
            }
        }
        let end = self.expect(TokenKind::RBrace, "expected `}` to close package")?;

        Ok(PackageDecl {
            name,
            members,
            imports,
            definitions,
            docs,
            modifiers,
            span: merge_span(&start.span, &end.span),
        })
    }

    fn parse_import(
        &mut self,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<ImportDecl, Diagnostic> {
        let start = self.expect(TokenKind::Import, "expected `import`")?;
        if matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "all") {
            self.expect_identifier_named("all", "expected `all` after `import`")?;
        }
        let path = self.parse_import_path()?;
        self.consume_suffix_adornments()?;
        let end = if matches!(self.peek_kind(), TokenKind::LBrace) {
            self.consume_opaque_block_with_open()?
        } else {
            self.expect(TokenKind::Semicolon, "expected `;` after import")?
        };

        Ok(ImportDecl {
            path,
            docs,
            modifiers,
            span: merge_span(&start.span, &end.span),
        })
    }

    fn parse_annotation_usage(
        &mut self,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        let start = self.expect(TokenKind::At, "expected `@`")?;
        let name = self.expect_identifier("expected annotation name")?;
        let reference_target = if matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "about")
        {
            self.expect_identifier_named("about", "expected `about` after annotation name")?;
            Some(self.parse_qualified_name()?)
        } else {
            None
        };
        let end = match self.peek_kind() {
            TokenKind::Semicolon => {
                self.expect(TokenKind::Semicolon, "expected `;` after annotation")?
            }
            TokenKind::LBrace => self.consume_opaque_block_with_open()?,
            _ => return Err(self.error_here("expected `;` or body after annotation")),
        };

        Ok(Declaration::GenericUsage(GenericUsageDecl {
            keyword: "metadata".to_string(),
            name,
            is_implicit_name: false,
            ty: None,
            reference_target,
            multiplicity: None,
            expression: None,
            additional_types: Vec::new(),
            specializes: Vec::new(),
            subsets: Vec::new(),
            redefines: Vec::new(),
            body_members: Vec::new(),
            docs,
            modifiers,
            span: merge_span(&start.span, &end.span),
        }))
    }

    fn parse_feature_declaration(
        &mut self,
        keyword: &str,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        let start = self.expect_keyword(keyword)?;
        if keyword == "end" {
            return self.parse_end_feature_declaration(start, docs, modifiers);
        }
        if keyword == "include"
            && matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "use")
            && matches!(self.next_kind(), Some(TokenKind::Identifier(next)) if next == "case")
        {
            self.expect_identifier_named("use", "expected `use` after `include`")?;
            self.expect_identifier_named("case", "expected `case` after `include use`")?;
            return self.parse_usage_after_keyword("use-case", start, docs, modifiers);
        }
        let is_definition = matches!(self.peek_kind(), TokenKind::Def);
        if is_definition {
            self.advance();
            return self.parse_definition_after_keyword(keyword, start, docs, modifiers);
        }
        self.parse_usage_after_keyword(keyword, start, docs, modifiers)
    }

    fn parse_end_feature_declaration(
        &mut self,
        start: Token,
        docs: Vec<String>,
        mut modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        modifiers.push("end".to_string());
        self.consume_suffix_adornments()?;

        match self.peek_kind() {
            TokenKind::Part => {
                self.advance();
                self.parse_usage_after_keyword("part", start, docs, modifiers)
            }
            TokenKind::Identifier(value) if value == "item" => {
                self.advance();
                self.parse_usage_after_keyword("item", start, docs, modifiers)
            }
            TokenKind::Identifier(value) if value == "port" => {
                self.advance();
                self.parse_usage_after_keyword("port", start, docs, modifiers)
            }
            _ => self.parse_usage_after_keyword("end", start, docs, modifiers),
        }
    }

    fn parse_definition_after_keyword(
        &mut self,
        keyword: &str,
        start: Token,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        self.consume_angle_adornments()?;
        let name = self.expect_identifier(&format!("expected {keyword} definition name"))?;
        self.consume_suffix_adornments()?;
        let mut specializes = Vec::new();

        if matches!(self.peek_kind(), TokenKind::Specializes) {
            self.advance();
            specializes.push(self.parse_qualified_name()?);
            while matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
                specializes.push(self.parse_qualified_name()?);
            }
        }

        let mut members = Vec::new();
        let mut part_members = Vec::new();
        let end = match self.peek_kind() {
            TokenKind::Semicolon => self.expect(TokenKind::Semicolon, "expected `;`")?,
            TokenKind::LBrace => {
                self.advance();
                let (body_members, end) = self.parse_declaration_block_contents_after_open()?;
                for declaration in body_members {
                    if let Declaration::PartUsage(part_usage) = &declaration {
                        part_members.push(part_usage.clone());
                    }
                    members.push(declaration);
                }
                end
            }
            _ => return Err(self.error_here("expected `;` or `{` after part definition")),
        };

        let span = merge_span(&start.span, &end.span);
        Ok(match keyword {
            "part" => Declaration::PartDefinition(PartDefinitionDecl {
                name,
                specializes,
                members,
                part_members,
                docs,
                modifiers,
                span,
            }),
            _ => Declaration::GenericDefinition(GenericDefinitionDecl {
                keyword: keyword.to_string(),
                name,
                specializes,
                members,
                docs,
                modifiers,
                span,
            }),
        })
    }

    fn parse_usage_after_keyword(
        &mut self,
        keyword: &str,
        start: Token,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        self.consume_angle_adornments()?;
        if keyword == "comment" {
            return self.parse_comment_usage_after_keyword(start, docs, modifiers);
        }
        if keyword == "rep" {
            return self.parse_textual_representation_after_keyword(start, docs, modifiers);
        }
        let mut effective_keyword = keyword.to_string();
        let mut synthetic_body_members = Vec::new();
        let mut force_implicit_name = false;
        let mut leading_specialization = None;

        let explicit_name = if keyword == "accept"
            && matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "at" || value == "when")
        {
            self.advance();
            while !matches!(
                self.peek_kind(),
                TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof
            ) {
                self.advance();
            }
            force_implicit_name = true;
            Some("AcceptActionUsage".to_string())
        } else if keyword == "accept"
            && matches!(self.peek_kind(), TokenKind::Identifier(_))
            && matches!(self.next_kind(), Some(TokenKind::Identifier(value)) if value == "after")
        {
            let payload_name = self.expect_identifier("expected accept payload name")?;
            synthetic_body_members.push(synthetic_reference_usage(
                &payload_name,
                None,
                None,
                &["payload", "in"],
                &start.span,
            ));
            self.expect_identifier_named("after", "expected `after` after accept payload")?;
            while !matches!(
                self.peek_kind(),
                TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof
            ) {
                self.advance();
            }
            force_implicit_name = true;
            Some("AcceptActionUsage".to_string())
        } else if keyword == "accept"
            && matches!(self.peek_kind(), TokenKind::Identifier(_))
            && matches!(self.next_kind(), Some(TokenKind::Colon))
        {
            let payload_name = self.expect_identifier("expected accept payload name")?;
            self.expect(TokenKind::Colon, "expected `:` after accept payload name")?;
            let payload_type = self.parse_qualified_name()?;
            synthetic_body_members.push(synthetic_reference_usage(
                &payload_name,
                Some(payload_type),
                None,
                &["payload", "in"],
                &start.span,
            ));
            force_implicit_name = true;
            Some("AcceptActionUsage".to_string())
        } else if keyword == "accept"
            && matches!(self.peek_kind(), TokenKind::Identifier(_))
        {
            let payload = self.parse_qualified_name()?;
            synthetic_body_members.push(synthetic_reference_usage(
                "payload",
                Some(payload),
                None,
                &["payload", "in"],
                &start.span,
            ));
            if matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "via") {
                self.expect_identifier_named("via", "expected `via` after accept payload")?;
                let _receiver = self.parse_qualified_name()?;
                synthetic_body_members.push(synthetic_reference_usage(
                    "receiver",
                    None,
                    None,
                    &["receiver", "in"],
                    &start.span,
                ));
            }
            force_implicit_name = true;
            Some("AcceptActionUsage".to_string())
        } else if keyword == "succession"
            && matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "flow")
        {
            self.expect_identifier_named("flow", "expected `flow` after `succession`")?;
            let explicit_flow_name =
                if matches!(self.peek_kind(), TokenKind::Identifier(_))
                    && matches!(self.next_kind(), Some(TokenKind::Identifier(value)) if value == "from")
                {
                    let name = self.expect_identifier("expected succession flow name")?;
                    self.expect_identifier_named("from", "expected `from` after succession flow name")?;
                    Some(name)
                } else {
                    None
                };
            if matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "from") {
                self.expect_identifier_named("from", "expected `from` before succession flow source")?;
            }
            let source = self.parse_qualified_name()?;
            self.expect_identifier_named("to", "expected `to` between succession flow ends")?;
            let target = self.parse_qualified_name()?;
            let source_name = source
                .segments
                .last()
                .cloned()
                .unwrap_or_else(|| "source".to_string());
            let target_name = target
                .segments
                .last()
                .cloned()
                .unwrap_or_else(|| "target".to_string());
            synthetic_body_members.push(synthetic_reference_usage(
                &source_name,
                None,
                Some(source),
                &["source-output"],
                &start.span,
            ));
            synthetic_body_members.push(synthetic_reference_usage(
                &target_name,
                None,
                None,
                &["target-input", "in"],
                &start.span,
            ));
            force_implicit_name = true;
            explicit_flow_name.or_else(|| Some("SuccessionFlowUsage".to_string()))
        } else if keyword == "perform"
            && matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "action")
        {
            self.expect_identifier_named("action", "expected `action` after `perform`")?;
            effective_keyword = "action".to_string();
            if matches!(
                self.peek_kind(),
                TokenKind::Specializes
                    | TokenKind::Redefines
                    | TokenKind::Colon
                    | TokenKind::LBrace
            ) {
                None
            } else {
                Some(self.expect_identifier("expected perform action name")?)
            }
        } else if keyword == "perform"
            && matches!(self.peek_kind(), TokenKind::Identifier(_))
            && matches!(self.next_kind(), Some(TokenKind::Dot))
        {
            let qualified = self.parse_qualified_name()?;
            leading_specialization = Some(qualified.clone());
            Some(
                qualified
                    .segments
                    .last()
                    .cloned()
                    .unwrap_or_else(|| "perform".to_string()),
            )
        } else if keyword == "satisfy"
            && matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "requirement")
        {
            self.expect_identifier_named("requirement", "expected `requirement` after `satisfy`")?;
            Some(self.expect_identifier("expected satisfy requirement name")?)
        } else if keyword == "exhibit"
            && matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "state")
        {
            self.expect_identifier_named("state", "expected `state` after `exhibit`")?;
            Some(self.expect_identifier("expected exhibit state name")?)
        } else if matches!(
            self.peek_kind(),
            TokenKind::LBrace
                | TokenKind::Semicolon
                | TokenKind::Colon
                | TokenKind::Specializes
                | TokenKind::Redefines
                | TokenKind::Equals
        ) || matches!(
            self.peek_kind(),
            TokenKind::Identifier(value) if value == "subsets" || value == "redefines"
        ) || (keyword == "connect"
            && matches!(
                self.peek_kind(),
                TokenKind::LParen | TokenKind::LBracket | TokenKind::Identifier(_)
            ))
            || (keyword == "connection" && matches!(self.peek_kind(), TokenKind::Colon))
        {
            None
        } else {
            Some(self.expect_identifier(&format!("expected {keyword} declaration name"))?)
        };
        if keyword == "action"
            && explicit_name.is_some()
            && matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "accept")
        {
            self.expect_identifier_named("accept", "expected `accept` after action name")?;
            effective_keyword = "accept".to_string();
            let payload = self.parse_qualified_name()?;
            synthetic_body_members.push(synthetic_reference_usage(
                "payload",
                Some(payload),
                None,
                &["payload", "in"],
                &start.span,
            ));
        }
        if keyword == "action"
            && explicit_name.is_some()
            && matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "send")
        {
            self.expect_identifier_named("send", "expected `send` after action name")?;
            effective_keyword = "send".to_string();
        }
        let is_implicit_name = explicit_name.is_none() || force_implicit_name;
        let mut tail = if keyword == "connect" {
            UsageTail {
                ty: None,
                multiplicity: None,
                expression: None,
                additional_types: Vec::new(),
                specializes: Vec::new(),
                subsets: Vec::new(),
                redefines: Vec::new(),
                body_members: Vec::new(),
                had_body: false,
            }
        } else {
            self.parse_usage_tail(if matches!(keyword, "connection" | "interface") {
                &["connect"]
            } else {
                &[]
            })?
        };
        if let Some(target) = leading_specialization {
            tail.specializes.insert(0, target);
        }
        if !synthetic_body_members.is_empty() {
            tail.body_members.extend(synthetic_body_members);
        }
        self.parse_connection_end_members(keyword, &mut tail)?;
        let name = explicit_name.unwrap_or_else(|| tail.derived_name(&effective_keyword));
        let end = self.finish_usage(&effective_keyword, tail.had_body)?;
        let span = merge_span(&start.span, &end.span);
        let has_type = tail.ty.is_some();
        let reference_target =
            infer_reference_target(&effective_keyword, &name, has_type, &mut tail).or_else(|| {
                if effective_keyword == "require" && tail.ty.is_none() && name != "constraint" {
                    Some(QualifiedName {
                        segments: vec![name.clone()],
                        span: span.clone(),
                    })
                } else {
                    None
                }
            });

        Ok(match effective_keyword.as_str() {
            "part" => Declaration::PartUsage(PartUsageDecl {
                name,
                is_implicit_name,
                ty: tail.ty,
                multiplicity: tail.multiplicity,
                expression: tail.expression.clone(),
                additional_types: tail.additional_types,
                specializes: tail.specializes,
                subsets: tail.subsets,
                redefines: tail.redefines,
                body_members: tail.body_members,
                docs,
                modifiers,
                span,
            }),
            _ => Declaration::GenericUsage(GenericUsageDecl {
                keyword: effective_keyword,
                name,
                is_implicit_name,
                ty: tail.ty,
                reference_target,
                multiplicity: tail.multiplicity,
                expression: tail.expression,
                additional_types: tail.additional_types,
                specializes: tail.specializes,
                subsets: tail.subsets,
                redefines: tail.redefines,
                body_members: tail.body_members,
                docs,
                modifiers,
                span,
            }),
        })
    }

    fn parse_comment_usage_after_keyword(
        &mut self,
        start: Token,
        mut docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        let mut end = start.clone();
        let explicit_name = if matches!(self.peek_kind(), TokenKind::Identifier(value) if value != "about")
        {
            let token = self.expect_identifier_token("expected comment declaration name")?;
            end = token.clone();
            match token.kind {
                TokenKind::Identifier(value) => Some(value),
                _ => unreachable!(),
            }
        } else {
            None
        };

        if matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "about") {
            end = self
                .expect_identifier_named("about", "expected `about` after comment name")?
                .clone();
            if matches!(self.peek_kind(), TokenKind::Identifier(_)) {
                let target = self.parse_qualified_name()?;
                end = Token {
                    kind: TokenKind::Identifier(
                        target.segments.last().cloned().unwrap_or_default(),
                    ),
                    span: target.span,
                };
            }
        }

        if matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "locale") {
            end = self
                .expect_identifier_named("locale", "expected `locale` after comment target")?
                .clone();
            if matches!(self.peek_kind(), TokenKind::String(_)) {
                end = self.expect_string_literal("expected locale string after `locale`")?;
            }
        }

        while let TokenKind::Doc(text) = self.peek_kind().clone() {
            docs.push(text);
            end = self.current().clone();
            self.advance();
        }

        let mut body_members = Vec::new();
        if matches!(self.peek_kind(), TokenKind::LBrace) {
            self.advance();
            let (members, block_end) = self.parse_declaration_block_contents_after_open()?;
            body_members = members;
            end = block_end;
        } else if matches!(self.peek_kind(), TokenKind::Semicolon) {
            end = self.expect(TokenKind::Semicolon, "expected `;`")?;
        } else if !self.comment_usage_can_end_here() {
            return Err(self.error_here("expected `;`, body, or documentation after comment"));
        }

        let is_implicit_name = explicit_name.is_none();

        Ok(Declaration::GenericUsage(GenericUsageDecl {
            keyword: "comment".to_string(),
            name: explicit_name.unwrap_or_else(|| "comment".to_string()),
            is_implicit_name,
            ty: None,
            reference_target: None,
            multiplicity: None,
            expression: None,
            additional_types: Vec::new(),
            specializes: Vec::new(),
            subsets: Vec::new(),
            redefines: Vec::new(),
            body_members,
            docs,
            modifiers,
            span: merge_span(&start.span, &end.span),
        }))
    }

    fn comment_usage_can_end_here(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof)
            || self.block_starts_with_declaration()
    }

    fn parse_textual_representation_after_keyword(
        &mut self,
        start: Token,
        mut docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        let explicit_name = if matches!(self.peek_kind(), TokenKind::Identifier(value) if value != "language")
        {
            Some(self.expect_identifier("expected textual representation name")?)
        } else {
            None
        };
        let mut end = self.tokens[self.index.saturating_sub(1)].clone();

        if matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "language") {
            end = self.expect_identifier_named(
                "language",
                "expected `language` in textual representation",
            )?;
            if matches!(self.peek_kind(), TokenKind::String(_)) {
                end = self.expect_string_literal("expected language string")?;
            }
        }

        while let TokenKind::Doc(text) = self.peek_kind().clone() {
            docs.push(text);
            end = self.current().clone();
            self.advance();
        }

        if matches!(self.peek_kind(), TokenKind::Semicolon) {
            end = self.expect(TokenKind::Semicolon, "expected `;`")?;
        } else if !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof)
            && !self.block_starts_with_declaration()
        {
            return Err(self.error_here("expected textual representation body"));
        }

        let is_implicit_name = explicit_name.is_none();

        Ok(Declaration::GenericUsage(GenericUsageDecl {
            keyword: "rep".to_string(),
            name: explicit_name.unwrap_or_else(|| "rep".to_string()),
            is_implicit_name,
            ty: None,
            reference_target: None,
            multiplicity: None,
            expression: None,
            additional_types: Vec::new(),
            specializes: Vec::new(),
            subsets: Vec::new(),
            redefines: Vec::new(),
            body_members: Vec::new(),
            docs,
            modifiers,
            span: merge_span(&start.span, &end.span),
        }))
    }

    fn parse_implicit_usage(
        &mut self,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<Declaration, Diagnostic> {
        let start = self.current().clone();
        let name = self.expect_identifier("expected declaration name")?;
        let tail = self.parse_usage_tail(&[])?;
        let end = self.finish_usage("declaration", tail.had_body)?;
        let keyword = implicit_usage_keyword(&modifiers);

        Ok(Declaration::GenericUsage(GenericUsageDecl {
            keyword: keyword.to_string(),
            name,
            is_implicit_name: false,
            ty: tail.ty,
            reference_target: None,
            multiplicity: tail.multiplicity,
            expression: tail.expression,
            additional_types: tail.additional_types,
            specializes: tail.specializes,
            subsets: tail.subsets,
            redefines: tail.redefines,
            body_members: tail.body_members,
            docs,
            modifiers,
            span: merge_span(&start.span, &end.span),
        }))
    }

    fn parse_alias(
        &mut self,
        docs: Vec<String>,
        modifiers: Vec<String>,
    ) -> Result<AliasDecl, Diagnostic> {
        let start = self.expect_identifier_named("alias", "expected `alias`")?;
        let name = self.expect_identifier("expected alias name")?;
        self.expect_identifier_named("for", "expected `for` after alias name")?;
        let target = self.parse_qualified_name()?;
        let end = match self.peek_kind() {
            TokenKind::Semicolon => {
                self.expect(TokenKind::Semicolon, "expected `;` after alias")?
            }
            TokenKind::LBrace => self.consume_opaque_block_with_open()?,
            _ => return Err(self.error_here("expected `;` or body after alias")),
        };

        Ok(AliasDecl {
            name,
            target,
            docs,
            modifiers,
            span: merge_span(&start.span, &end.span),
        })
    }

    fn parse_import_path(&mut self) -> Result<QualifiedName, Diagnostic> {
        self.parse_name_path(true)
    }

    fn parse_qualified_name(&mut self) -> Result<QualifiedName, Diagnostic> {
        self.parse_name_path(false)
    }

    fn parse_name_path(&mut self, allow_wildcards: bool) -> Result<QualifiedName, Diagnostic> {
        let first = self.expect_path_segment("expected identifier", allow_wildcards)?;
        let mut segments = vec![segment_text(&first.kind)];
        let mut span = first.span.clone();

        while match (self.peek_kind(), self.next_kind()) {
            (TokenKind::ScopeSep | TokenKind::Dot, Some(TokenKind::Identifier(_))) => true,
            (
                TokenKind::ScopeSep | TokenKind::Dot,
                Some(TokenKind::Star | TokenKind::DoubleStar),
            ) => allow_wildcards,
            _ => false,
        } {
            self.advance();
            let next =
                self.expect_path_segment("expected identifier after `::`", allow_wildcards)?;
            segments.push(segment_text(&next.kind));
            span = merge_span(&span, &next.span);
        }

        Ok(QualifiedName { segments, span })
    }

    fn parse_usage_tail(&mut self, stop_keywords: &[&str]) -> Result<UsageTail, Diagnostic> {
        let mut ty = None;
        let mut multiplicity = None;
        let mut expression = None;
        let mut additional_types = Vec::new();
        let mut specializes = Vec::new();
        let mut subsets = Vec::new();
        let mut redefines = Vec::new();
        let mut body_members = Vec::new();
        let mut had_body = false;

        if let Some(parsed) = self.consume_suffix_adornments()? {
            multiplicity = Some(parsed);
        }

        loop {
            match self.peek_kind() {
                TokenKind::Colon => {
                    self.advance();
                    let mut conjugated = self.consume_optional_type_prefix();
                    if matches!(self.peek_kind(), TokenKind::Identifier(_)) {
                        let mut first = self.parse_qualified_name()?;
                        if conjugated {
                            first.segments[0] = format!("~{}", first.segments[0]);
                        }
                        if ty.is_none() {
                            ty = Some(first);
                        } else {
                            additional_types.push(first);
                        }
                        if let Some(parsed) = self.consume_suffix_adornments()? {
                            multiplicity = Some(parsed);
                        }
                        while matches!(self.peek_kind(), TokenKind::Comma) {
                            self.advance();
                            conjugated = self.consume_optional_type_prefix();
                            if matches!(self.peek_kind(), TokenKind::Identifier(_)) {
                                let mut additional = self.parse_qualified_name()?;
                                if conjugated {
                                    additional.segments[0] = format!("~{}", additional.segments[0]);
                                }
                                additional_types.push(additional);
                                self.consume_suffix_adornments()?;
                            } else {
                                break;
                            }
                        }
                    }
                }
                TokenKind::Specializes => {
                    self.advance();
                    specializes.push(self.parse_qualified_name()?);
                    self.consume_suffix_adornments()?;
                    while matches!(self.peek_kind(), TokenKind::Comma) {
                        self.advance();
                        specializes.push(self.parse_qualified_name()?);
                        self.consume_suffix_adornments()?;
                    }
                }
                TokenKind::Redefines => {
                    self.advance();
                    redefines.push(self.parse_qualified_name()?);
                    self.consume_suffix_adornments()?;
                    while matches!(self.peek_kind(), TokenKind::Comma) {
                        self.advance();
                        redefines.push(self.parse_qualified_name()?);
                        self.consume_suffix_adornments()?;
                    }
                }
                TokenKind::Identifier(value) if value == "subsets" || value == "redefines" => {
                    let keyword = self.expect_identifier("expected relation keyword")?;
                    let refs = self.parse_reference_list()?;
                    if keyword == "subsets" {
                        subsets.extend(refs);
                    } else {
                        redefines.extend(refs);
                    }
                }
                TokenKind::Equals => {
                    self.advance();
                    expression = Some(self.parse_expression()?);
                }
                TokenKind::LBrace => {
                    had_body = true;
                    body_members = self.parse_declaration_block()?;
                    break;
                }
                TokenKind::Semicolon | TokenKind::RBrace | TokenKind::Eof => break,
                TokenKind::Identifier(value) if stop_keywords.contains(&value.as_str()) => break,
                _ => {
                    self.advance();
                }
            }
        }

        Ok(UsageTail {
            ty,
            multiplicity,
            expression,
            additional_types,
            specializes,
            subsets,
            redefines,
            body_members,
            had_body,
        })
    }

    fn parse_expression(&mut self) -> Result<Expr, Diagnostic> {
        self.parse_or_expression()
    }

    fn parse_or_expression(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_and_expression()?;
        while matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "or") {
            self.expect_identifier_token("expected `or`")?;
            let right = self.parse_and_expression()?;
            let span = merge_span(&expr_span(&expr), &expr_span(&right));
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::Or,
                span,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_and_expression(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_equality_expression()?;
        while matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "and") {
            self.expect_identifier_token("expected `and`")?;
            let right = self.parse_equality_expression()?;
            let span = merge_span(&expr_span(&expr), &expr_span(&right));
            expr = Expr::Binary {
                left: Box::new(expr),
                op: BinaryOp::And,
                span,
                right: Box::new(right),
            };
        }
        Ok(expr)
    }

    fn parse_equality_expression(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_comparison_expression()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::DoubleEquals => BinaryOp::Equal,
                TokenKind::BangEquals => BinaryOp::NotEqual,
                _ => break,
            };
            self.advance();
            let right = self.parse_comparison_expression()?;
            let span = merge_span(&expr_span(&expr), &expr_span(&right));
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_comparison_expression(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_additive_expression()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::LAngle => BinaryOp::Less,
                TokenKind::LessEqual => BinaryOp::LessEqual,
                TokenKind::RAngle => BinaryOp::Greater,
                TokenKind::GreaterEqual => BinaryOp::GreaterEqual,
                _ => break,
            };
            self.advance();
            let right = self.parse_additive_expression()?;
            let span = merge_span(&expr_span(&expr), &expr_span(&right));
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_additive_expression(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_multiplicative_expression()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Plus => BinaryOp::Add,
                TokenKind::Minus => BinaryOp::Subtract,
                _ => break,
            };
            self.advance();
            let right = self.parse_multiplicative_expression()?;
            let span = merge_span(&expr_span(&expr), &expr_span(&right));
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_multiplicative_expression(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_power_expression()?;
        loop {
            let op = match self.peek_kind() {
                TokenKind::Star => BinaryOp::Multiply,
                TokenKind::Slash => BinaryOp::Divide,
                _ => break,
            };
            self.advance();
            let right = self.parse_power_expression()?;
            let span = merge_span(&expr_span(&expr), &expr_span(&right));
            expr = Expr::Binary {
                left: Box::new(expr),
                op,
                right: Box::new(right),
                span,
            };
        }
        Ok(expr)
    }

    fn parse_power_expression(&mut self) -> Result<Expr, Diagnostic> {
        let expr = self.parse_unary_expression()?;
        match self.peek_kind() {
            TokenKind::Caret | TokenKind::DoubleStar => {
                self.advance();
                let right = self.parse_power_expression()?;
                let span = merge_span(&expr_span(&expr), &expr_span(&right));
                Ok(Expr::Binary {
                    left: Box::new(expr),
                    op: BinaryOp::Power,
                    right: Box::new(right),
                    span,
                })
            }
            _ => Ok(expr),
        }
    }

    fn parse_unary_expression(&mut self) -> Result<Expr, Diagnostic> {
        match self.peek_kind().clone() {
            TokenKind::Minus => {
                let token = self.current().clone();
                self.advance();
                let expr = self.parse_unary_expression()?;
                let span = merge_span(&token.span, &expr_span(&expr));
                Ok(Expr::Unary {
                    op: UnaryOp::Negate,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::Bang => {
                let token = self.current().clone();
                self.advance();
                let expr = self.parse_unary_expression()?;
                let span = merge_span(&token.span, &expr_span(&expr));
                Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                    span,
                })
            }
            TokenKind::Identifier(value) if value == "not" => {
                let token = self.current().clone();
                self.advance();
                let expr = self.parse_unary_expression()?;
                let span = merge_span(&token.span, &expr_span(&expr));
                Ok(Expr::Unary {
                    op: UnaryOp::Not,
                    expr: Box::new(expr),
                    span,
                })
            }
            _ => self.parse_expression_postfix(),
        }
    }

    fn parse_expression_postfix(&mut self) -> Result<Expr, Diagnostic> {
        let mut expr = self.parse_expression_primary()?;

        loop {
            match self.peek_kind() {
                TokenKind::Dot => {
                    self.advance();
                    if matches!(self.peek_kind(), TokenKind::Question) {
                        self.advance();
                        if matches!(self.peek_kind(), TokenKind::LBrace) {
                            self.consume_opaque_block_with_open()?;
                            continue;
                        }
                        return Err(self.error_here("expected `{` after filter operator"));
                    }
                    let segment = self.expect_identifier("expected identifier after `.`")?;
                    let segment_span = self.tokens[self.index - 1].span.clone();
                    let span = merge_span(&expr_span(&expr), &segment_span);
                    expr = Expr::Path {
                        root: Box::new(expr),
                        segment,
                        span,
                    };
                }
                TokenKind::LParen => {
                    let function = callable_expr_name(&expr)
                        .ok_or_else(|| self.error_here("expected callable expression"))?;
                    let start_span = expr_span(&expr);
                    self.expect(TokenKind::LParen, "expected `(` after function name")?;
                    let mut args = Vec::new();
                    if !matches!(self.peek_kind(), TokenKind::RParen) {
                        loop {
                            args.push(self.parse_call_argument_expression()?);
                            if matches!(self.peek_kind(), TokenKind::Comma) {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                    }
                    let end = self.expect(TokenKind::RParen, "expected `)` after expression")?;
                    expr = Expr::Call {
                        function,
                        args,
                        span: merge_span(&start_span, &end.span),
                    };
                }
                TokenKind::LBracket => {
                    self.consume_balanced(TokenKind::LBracket, TokenKind::RBracket)?;
                }
                TokenKind::Identifier(value) if value == "as" => {
                    self.expect_identifier_named("as", "expected `as` in cast expression")?;
                    if matches!(self.peek_kind(), TokenKind::Identifier(_)) {
                        let ty = self.parse_qualified_name()?;
                        let span = merge_span(&expr_span(&expr), &ty.span);
                        expr = Expr::Call {
                            function: format!("as {}", ty.as_dot_string()),
                            args: vec![expr],
                            span,
                        };
                    } else {
                        return Err(self.error_here("expected type name after `as`"));
                    }
                }
                _ => break,
            }
        }

        Ok(expr)
    }

    fn parse_expression_primary(&mut self) -> Result<Expr, Diagnostic> {
        match self.peek_kind().clone() {
            TokenKind::Identifier(value) if value == "new" => {
                let start = self.expect_identifier_named("new", "expected `new`")?;
                let constructor = self.parse_qualified_name()?;
                let function = format!("new {}", constructor.as_dot_string());
                let mut args = Vec::new();
                let end = if matches!(self.peek_kind(), TokenKind::LParen) {
                    self.expect(TokenKind::LParen, "expected `(` after constructor name")?;
                    if !matches!(self.peek_kind(), TokenKind::RParen) {
                        loop {
                            args.push(self.parse_call_argument_expression()?);
                            if matches!(self.peek_kind(), TokenKind::Comma) {
                                self.advance();
                            } else {
                                break;
                            }
                        }
                    }
                    self.expect(TokenKind::RParen, "expected `)` after expression")?
                } else {
                    self.tokens[self.index - 1].clone()
                };
                Ok(Expr::Call {
                    function,
                    args,
                    span: merge_span(&start.span, &end.span),
                })
            }
            TokenKind::Identifier(value) if value == "self" => {
                let token = self.expect_identifier_token("expected `self`")?;
                Ok(Expr::SelfRef(token.span))
            }
            TokenKind::Identifier(value) if value == "true" || value == "false" => {
                self.advance();
                Ok(Expr::Literal(LiteralExpr::Boolean(value == "true")))
            }
            TokenKind::Identifier(_) => {
                let name = self.parse_qualified_name()?;
                Ok(Expr::Name(name))
            }
            TokenKind::Number(value) => {
                let token = self.current().clone();
                self.advance();
                if value.contains('.') {
                    Ok(Expr::Literal(LiteralExpr::Real(value)))
                } else {
                    let value = value.parse::<i64>().map_err(|_| {
                        Diagnostic::new("invalid integer literal", Some(token.span.clone()))
                    })?;
                    Ok(Expr::Literal(LiteralExpr::Integer(value)))
                }
            }
            TokenKind::String(value) => {
                self.advance();
                Ok(Expr::Literal(LiteralExpr::String(value)))
            }
            TokenKind::LParen => {
                let start = self.current().clone();
                self.advance();
                if matches!(self.peek_kind(), TokenKind::RParen) {
                    let end = self.expect(TokenKind::RParen, "expected `)` after expression")?;
                    return Ok(Expr::Tuple {
                        items: Vec::new(),
                        span: merge_span(&start.span, &end.span),
                    });
                }

                let first = self.parse_expression()?;
                if matches!(self.peek_kind(), TokenKind::Comma) {
                    let mut items = vec![first];
                    while matches!(self.peek_kind(), TokenKind::Comma) {
                        self.advance();
                        if matches!(self.peek_kind(), TokenKind::RParen) {
                            break;
                        }
                        items.push(self.parse_expression()?);
                    }
                    let end = self.expect(TokenKind::RParen, "expected `)` after expression")?;
                    Ok(Expr::Tuple {
                        items,
                        span: merge_span(&start.span, &end.span),
                    })
                } else {
                    self.expect(TokenKind::RParen, "expected `)` after expression")?;
                    Ok(first)
                }
            }
            _ => Err(self.error_here("expected expression")),
        }
    }

    fn parse_call_argument_expression(&mut self) -> Result<Expr, Diagnostic> {
        if matches!(self.peek_kind(), TokenKind::Identifier(_))
            && matches!(self.next_kind(), Some(TokenKind::Equals))
        {
            self.expect_identifier_token("expected argument name")?;
            self.expect(TokenKind::Equals, "expected `=` after argument name")?;
        }
        self.parse_expression()
    }

    fn parse_connection_end_members(
        &mut self,
        keyword: &str,
        tail: &mut UsageTail,
    ) -> Result<(), Diagnostic> {
        let starts_named_connect = matches!(keyword, "connection" | "interface")
            && matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "connect");
        if starts_named_connect {
            self.expect_identifier_named("connect", "expected `connect`")?;
            if matches!(self.peek_kind(), TokenKind::LParen) {
                self.consume_balanced(TokenKind::LParen, TokenKind::RParen)?;
                if matches!(self.peek_kind(), TokenKind::LBrace) {
                    tail.had_body = true;
                    tail.body_members.extend(self.parse_declaration_block()?);
                }
                return Ok(());
            }
            let has_named_ends = self.starts_named_connection_end_member();
            tail.body_members = self.parse_connection_end_member_pair(has_named_ends)?;
            if matches!(self.peek_kind(), TokenKind::LBrace) {
                tail.had_body = true;
                tail.body_members.extend(self.parse_declaration_block()?);
            }
        }

        let starts_anonymous_connect = keyword == "connect"
            && matches!(
                self.peek_kind(),
                TokenKind::LParen | TokenKind::LBracket | TokenKind::Identifier(_)
            );
        if starts_anonymous_connect {
            if matches!(self.peek_kind(), TokenKind::LParen) {
                self.consume_balanced(TokenKind::LParen, TokenKind::RParen)?;
                if matches!(self.peek_kind(), TokenKind::LBrace) {
                    tail.had_body = true;
                    tail.body_members.extend(self.parse_declaration_block()?);
                }
                return Ok(());
            }
            tail.body_members = self.parse_connection_end_member_pair(false)?;
            if matches!(self.peek_kind(), TokenKind::LBrace) {
                tail.had_body = true;
                tail.body_members.extend(self.parse_declaration_block()?);
            }
        }

        Ok(())
    }

    fn parse_connection_end_member_pair(
        &mut self,
        named_ends: bool,
    ) -> Result<Vec<Declaration>, Diagnostic> {
        let source = self.parse_connection_end_member("source", named_ends)?;
        self.expect_identifier_named("to", "expected `to` between connection ends")?;
        let target = self.parse_connection_end_member("target", named_ends)?;
        Ok(vec![source, target])
    }

    fn parse_connection_end_member(
        &mut self,
        fallback_name: &str,
        named_end: bool,
    ) -> Result<Declaration, Diagnostic> {
        self.consume_suffix_adornments()?;
        let start = self.current().clone();
        let name = if named_end {
            self.expect_identifier("expected connection end name")?
        } else {
            fallback_name.to_string()
        };

        let reference_target = if named_end && self.consume_connection_end_reference_arrow() {
            self.parse_qualified_name()?
        } else if matches!(self.peek_kind(), TokenKind::Identifier(value) if value == "references")
        {
            self.expect_identifier_named(
                "references",
                "expected `references` after connection end name",
            )?;
            self.parse_qualified_name()?
        } else {
            self.parse_qualified_name()?
        };
        let span = merge_span(&start.span, &reference_target.span);

        Ok(Declaration::GenericUsage(GenericUsageDecl {
            keyword: "reference".to_string(),
            name,
            is_implicit_name: false,
            ty: None,
            reference_target: Some(reference_target),
            multiplicity: None,
            expression: None,
            additional_types: Vec::new(),
            specializes: Vec::new(),
            subsets: Vec::new(),
            redefines: Vec::new(),
            body_members: Vec::new(),
            docs: Vec::new(),
            modifiers: vec!["end".to_string(), format!("end-{fallback_name}")],
            span,
        }))
    }

    fn finish_usage(&mut self, label: &str, body_closed: bool) -> Result<Token, Diagnostic> {
        match self.peek_kind() {
            TokenKind::Semicolon => self.expect(TokenKind::Semicolon, "expected `;`"),
            _ if body_closed => Ok(self.current().clone()),
            _ => Err(self.error_here(&format!("expected `;` or body terminator after {label}"))),
        }
    }

    fn parse_declaration_block(&mut self) -> Result<Vec<Declaration>, Diagnostic> {
        self.expect(TokenKind::LBrace, "expected `{`")?;
        let (members, _) = self.parse_declaration_block_contents_after_open()?;
        Ok(members)
    }

    fn parse_declaration_block_contents_after_open(
        &mut self,
    ) -> Result<(Vec<Declaration>, Token), Diagnostic> {
        if !self.block_starts_with_declaration() {
            let end = self.consume_opaque_block_contents()?;
            return Ok((Vec::new(), end));
        }

        let mut members = Vec::new();

        while !matches!(self.peek_kind(), TokenKind::RBrace | TokenKind::Eof) {
            self.collect_docs();
            if !self.block_starts_with_declaration() {
                self.consume_opaque_statement_in_block()?;
                continue;
            }
            let declaration = match self.parse_declaration().and_then(|declaration| {
                declaration.ok_or_else(|| self.error_here("expected declaration inside body"))
            }) {
                Ok(declaration) => declaration,
                Err(diagnostic) if self.recover => {
                    self.diagnostics.push(diagnostic);
                    self.recover_declaration();
                    continue;
                }
                Err(diagnostic) => return Err(diagnostic),
            };
            members.push(declaration);
        }

        let end = self.expect(TokenKind::RBrace, "expected `}` to close body")?;
        Ok((members, end))
    }

    fn block_starts_with_declaration(&self) -> bool {
        let mut index = self.index;

        while matches!(
            self.tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Doc(_))
        ) {
            index += 1;
        }

        while matches!(
            self.tokens.get(index).map(|token| &token.kind),
            Some(TokenKind::Identifier(value)) if is_declaration_modifier(value)
        ) {
            index += 1;
        }

        match self.tokens.get(index).map(|token| &token.kind) {
            Some(
                TokenKind::Package
                | TokenKind::Import
                | TokenKind::Part
                | TokenKind::Specializes
                | TokenKind::Redefines,
            ) => true,
            Some(TokenKind::Hash) => matches!(
                self.tokens.get(index + 1).map(|token| &token.kind),
                Some(TokenKind::Identifier(_))
            ),
            Some(TokenKind::Identifier(value)) => {
                if matches!(value.as_str(), "if" | "else" | "new") {
                    return false;
                }
                let next_kind = self.tokens.get(index + 1).map(|token| &token.kind);
                matches!(
                    next_kind,
                    Some(
                        TokenKind::Def
                            | TokenKind::Identifier(_)
                            | TokenKind::Colon
                            | TokenKind::Specializes
                            | TokenKind::Redefines
                            | TokenKind::Equals
                            | TokenKind::LBrace
                            | TokenKind::Semicolon
                    )
                ) || matches!(next_kind, Some(TokenKind::LBracket) if value == "connect" || value == "end")
                    || matches!(next_kind, Some(TokenKind::Doc(_)) if value == "comment")
            }
            _ => false,
        }
    }

    fn consume_opaque_block_contents(&mut self) -> Result<Token, Diagnostic> {
        let mut depth = 1;

        while depth > 0 {
            let token = self.current().clone();
            match token.kind {
                TokenKind::LBrace => {
                    self.advance();
                    depth += 1;
                }
                TokenKind::RBrace => {
                    self.advance();
                    depth -= 1;
                    if depth == 0 {
                        return Ok(token);
                    }
                }
                TokenKind::Eof => {
                    return Err(self.error_here("unterminated body block"));
                }
                _ => self.advance(),
            }
        }

        Err(self.error_here("unterminated body block"))
    }

    fn consume_opaque_block_with_open(&mut self) -> Result<Token, Diagnostic> {
        self.expect(TokenKind::LBrace, "expected `{`")?;
        self.consume_opaque_block_contents()
    }

    fn consume_opaque_statement_in_block(&mut self) -> Result<(), Diagnostic> {
        let mut paren_depth = 0usize;
        let mut bracket_depth = 0usize;
        let mut angle_depth = 0usize;

        while !matches!(self.peek_kind(), TokenKind::Eof) {
            match self.peek_kind() {
                TokenKind::LParen => {
                    paren_depth += 1;
                    self.advance();
                }
                TokenKind::RParen => {
                    paren_depth = paren_depth.saturating_sub(1);
                    self.advance();
                }
                TokenKind::LBracket => {
                    bracket_depth += 1;
                    self.advance();
                }
                TokenKind::RBracket => {
                    bracket_depth = bracket_depth.saturating_sub(1);
                    self.advance();
                }
                TokenKind::LAngle => {
                    angle_depth += 1;
                    self.advance();
                }
                TokenKind::RAngle => {
                    angle_depth = angle_depth.saturating_sub(1);
                    self.advance();
                }
                TokenKind::LBrace => {
                    self.consume_opaque_block_with_open()?;
                    break;
                }
                TokenKind::Semicolon
                    if paren_depth == 0 && bracket_depth == 0 && angle_depth == 0 =>
                {
                    self.advance();
                    break;
                }
                TokenKind::RBrace if paren_depth == 0 && bracket_depth == 0 => {
                    break;
                }
                _ => self.advance(),
            }
        }

        Ok(())
    }

    fn recover_declaration(&mut self) {
        let start_index = self.index;
        self.pending_docs.clear();
        while !matches!(self.peek_kind(), TokenKind::Eof | TokenKind::RBrace) {
            match self.peek_kind() {
                TokenKind::LBrace => {
                    let _ = self.consume_opaque_block_with_open();
                    return;
                }
                TokenKind::Semicolon => {
                    self.advance();
                    return;
                }
                _ => self.advance(),
            }
        }
        if self.index == start_index && !self.at_end() {
            self.advance();
        }
    }

    fn consume_angle_adornments(&mut self) -> Result<(), Diagnostic> {
        while matches!(self.peek_kind(), TokenKind::LAngle) {
            self.consume_balanced(TokenKind::LAngle, TokenKind::RAngle)?;
        }
        Ok(())
    }

    fn consume_suffix_adornments(&mut self) -> Result<Option<MultiplicityRange>, Diagnostic> {
        let mut multiplicity = None;
        while matches!(self.peek_kind(), TokenKind::LBracket | TokenKind::LAngle) {
            match self.peek_kind() {
                TokenKind::LBracket => {
                    let parsed = self.parse_multiplicity_range()?;
                    multiplicity.get_or_insert(parsed);
                }
                TokenKind::LAngle => self.consume_balanced(TokenKind::LAngle, TokenKind::RAngle)?,
                _ => unreachable!(),
            }
        }
        Ok(multiplicity)
    }

    fn parse_multiplicity_range(&mut self) -> Result<MultiplicityRange, Diagnostic> {
        let start = self.expect(TokenKind::LBracket, "expected `[` before multiplicity")?;
        let mut parts = Vec::new();
        while !matches!(self.peek_kind(), TokenKind::RBracket | TokenKind::Eof) {
            let token = self.current().clone();
            parts.push(multiplicity_token_text(&token.kind));
            self.advance();
        }
        let end = self.expect(TokenKind::RBracket, "expected `]` after multiplicity")?;
        let raw = normalize_multiplicity_raw(&parts.join(""));
        let (lower, upper) = multiplicity_bounds(&raw);
        Ok(MultiplicityRange {
            lower,
            upper,
            raw,
            span: merge_span(&start.span, &end.span),
        })
    }

    fn consume_balanced(&mut self, open: TokenKind, close: TokenKind) -> Result<(), Diagnostic> {
        self.expect(open.clone(), "expected opening delimiter")?;
        let mut depth = 1;
        while depth > 0 {
            match self.peek_kind() {
                kind if std::mem::discriminant(kind) == std::mem::discriminant(&open) => {
                    self.advance();
                    depth += 1;
                }
                kind if std::mem::discriminant(kind) == std::mem::discriminant(&close) => {
                    self.advance();
                    depth -= 1;
                }
                TokenKind::Eof => {
                    return Err(self.error_here("unterminated delimited syntax"));
                }
                _ => self.advance(),
            }
        }
        Ok(())
    }

    fn consume_optional_type_prefix(&mut self) -> bool {
        let mut consumed = false;
        while matches!(self.peek_kind(), TokenKind::Tilde) {
            self.advance();
            consumed = true;
        }
        consumed
    }

    fn parse_reference_list(&mut self) -> Result<Vec<QualifiedName>, Diagnostic> {
        let mut references = Vec::new();
        if matches!(self.peek_kind(), TokenKind::Identifier(_)) {
            references.push(self.parse_qualified_name()?);
            self.consume_suffix_adornments()?;
            while matches!(self.peek_kind(), TokenKind::Comma) {
                self.advance();
                if matches!(self.peek_kind(), TokenKind::Identifier(_)) {
                    references.push(self.parse_qualified_name()?);
                    self.consume_suffix_adornments()?;
                } else {
                    break;
                }
            }
        }
        Ok(references)
    }

    fn collect_docs(&mut self) {
        while let TokenKind::Doc(text) = self.peek_kind().clone() {
            self.pending_docs.push(text);
            self.advance();
        }
    }

    fn starts_connection_end_reference_arrow(&self, offset: usize) -> bool {
        match self.tokens.get(self.index + offset).map(|token| &token.kind) {
            Some(TokenKind::Specializes) => true,
            Some(TokenKind::ScopeSep) => matches!(
                self.tokens.get(self.index + offset + 1).map(|token| &token.kind),
                Some(TokenKind::RAngle)
            ),
            _ => false,
        }
    }

    fn starts_named_connection_end_member(&self) -> bool {
        let mut offset = 0;
        if matches!(self.peek_kind(), TokenKind::LBracket) {
            let mut depth = 0usize;
            while let Some(token) = self.tokens.get(self.index + offset) {
                match token.kind {
                    TokenKind::LBracket => depth += 1,
                    TokenKind::RBracket => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            offset += 1;
                            break;
                        }
                    }
                    TokenKind::Eof => return false,
                    _ => {}
                }
                offset += 1;
            }
        }
        if !matches!(
            self.tokens.get(self.index + offset).map(|token| &token.kind),
            Some(TokenKind::Identifier(_))
        ) {
            return false;
        }
        matches!(
            self.tokens.get(self.index + offset + 1).map(|token| &token.kind),
            Some(TokenKind::Identifier(value)) if value == "references"
        ) || self.starts_connection_end_reference_arrow(offset + 1)
    }

    fn consume_connection_end_reference_arrow(&mut self) -> bool {
        if matches!(self.peek_kind(), TokenKind::Specializes) {
            self.advance();
            return true;
        }
        if matches!(self.peek_kind(), TokenKind::ScopeSep)
            && matches!(self.next_kind(), Some(TokenKind::RAngle))
        {
            self.advance();
            self.advance();
            return true;
        }
        false
    }

    fn should_parse_as_feature_keyword(&self, keyword: &str) -> bool {
        match self.next_kind() {
            Some(TokenKind::Def | TokenKind::Identifier(_) | TokenKind::LAngle) => true,
            Some(TokenKind::Doc(_)) => keyword == "comment",
            Some(TokenKind::Colon) => {
                matches!(keyword, "connection") || is_feature_keyword(keyword)
            }
            Some(TokenKind::Specializes | TokenKind::Redefines) => is_feature_keyword(keyword),
            Some(TokenKind::LBracket) => matches!(keyword, "connect" | "end"),
            Some(TokenKind::Part) => matches!(keyword, "end"),
            _ => false,
        }
    }

    fn consume_declaration_modifiers(&mut self) -> Vec<String> {
        let mut modifiers = Vec::new();
        while matches!(
            self.peek_kind(),
            TokenKind::Identifier(value) if is_declaration_modifier(value)
        ) {
            if let TokenKind::Identifier(value) = self.current().kind.clone() {
                modifiers.push(value);
                self.advance();
            }
        }
        modifiers
    }

    fn expect_identifier(&mut self, message: &str) -> Result<String, Diagnostic> {
        let token = self.expect_identifier_token(message)?;
        match token.kind {
            TokenKind::Identifier(value) => Ok(value),
            _ => unreachable!(),
        }
    }

    fn expect_path_segment(
        &mut self,
        message: &str,
        allow_wildcards: bool,
    ) -> Result<Token, Diagnostic> {
        let token = self.current().clone();
        match &token.kind {
            TokenKind::Identifier(_) => {
                self.advance();
                Ok(token)
            }
            TokenKind::Star | TokenKind::DoubleStar if allow_wildcards => {
                self.advance();
                Ok(token)
            }
            _ => Err(Diagnostic::new(message, Some(token.span))),
        }
    }

    fn expect_keyword(&mut self, keyword: &str) -> Result<Token, Diagnostic> {
        match keyword {
            "part" => self.expect(TokenKind::Part, "expected `part`"),
            _ => self.expect_identifier_named(keyword, &format!("expected `{keyword}`")),
        }
    }

    fn expect_identifier_named(
        &mut self,
        expected: &str,
        message: &str,
    ) -> Result<Token, Diagnostic> {
        let token = self.expect_identifier_token(message)?;
        match &token.kind {
            TokenKind::Identifier(value) if value == expected => Ok(token),
            _ => Err(Diagnostic::new(message, Some(token.span))),
        }
    }

    fn expect_identifier_token(&mut self, message: &str) -> Result<Token, Diagnostic> {
        let token = self.current().clone();
        match &token.kind {
            TokenKind::Identifier(_) => {
                self.advance();
                Ok(token)
            }
            _ => Err(Diagnostic::new(message, Some(token.span))),
        }
    }

    fn expect_string_literal(&mut self, message: &str) -> Result<Token, Diagnostic> {
        let token = self.current().clone();
        match &token.kind {
            TokenKind::String(_) => {
                self.advance();
                Ok(token)
            }
            _ => Err(Diagnostic::new(message, Some(token.span))),
        }
    }

    fn expect(&mut self, expected: TokenKind, message: &str) -> Result<Token, Diagnostic> {
        let token = self.current().clone();
        if std::mem::discriminant(&token.kind) == std::mem::discriminant(&expected) {
            self.advance();
            Ok(token)
        } else {
            Err(Diagnostic::new(message, Some(token.span)))
        }
    }

    fn error_here(&self, message: &str) -> Diagnostic {
        Diagnostic::new(message, Some(self.current().span.clone()))
    }

    fn peek_kind(&self) -> &TokenKind {
        &self.current().kind
    }

    fn next_kind(&self) -> Option<&TokenKind> {
        self.tokens.get(self.index + 1).map(|token| &token.kind)
    }

    fn current(&self) -> &Token {
        &self.tokens[self.index]
    }

    fn advance(&mut self) {
        if !self.at_end() {
            self.index += 1;
        }
    }

    fn at_end(&self) -> bool {
        matches!(self.current().kind, TokenKind::Eof)
    }
}

fn merge_span(start: &SourceSpan, end: &SourceSpan) -> SourceSpan {
    SourceSpan {
        start_line: start.start_line,
        start_col: start.start_col,
        end_line: end.end_line,
        end_col: end.end_col,
    }
}

fn expr_span(expr: &Expr) -> SourceSpan {
    match expr {
        Expr::Literal(_) => SourceSpan {
            start_line: 0,
            start_col: 0,
            end_line: 0,
            end_col: 0,
        },
        Expr::Name(name) => name.span.clone(),
        Expr::SelfRef(span) => span.clone(),
        Expr::Tuple { span, .. }
        | Expr::Unary { span, .. }
        | Expr::Binary { span, .. }
        | Expr::Path { span, .. }
        | Expr::Call { span, .. } => span.clone(),
    }
}

fn segment_text(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Identifier(value) => value.clone(),
        TokenKind::Star => "*".to_string(),
        TokenKind::DoubleStar => "**".to_string(),
        _ => unreachable!(),
    }
}

fn is_declaration_modifier(value: &str) -> bool {
    matches!(
        value,
        "public"
            | "private"
            | "protected"
            | "library"
            | "entry"
            | "exit"
            | "abstract"
            | "do"
            | "ref"
            | "in"
            | "out"
            | "first"
            | "derived"
            | "readonly"
            | "then"
            | "individual"
            | "variation"
            | "variant"
            | "constant"
    )
}

fn is_feature_keyword(value: &str) -> bool {
    matches!(
        value,
        "accept"
            | "action"
            | "allocation"
            | "analysis"
            | "assert"
            | "assume"
            | "attribute"
            | "calc"
            | "comment"
            | "concern"
            | "connect"
            | "connection"
            | "constraint"
            | "dependency"
            | "effect"
            | "exhibit"
            | "flow"
            | "include"
            | "individual"
            | "interface"
            | "item"
            | "metadata"
            | "occurrence"
            | "objective"
            | "perform"
            | "port"
            | "ref"
            | "reference"
            | "render"
            | "require"
            | "requirement"
            | "satisfy"
            | "state"
            | "subject"
            | "transition"
            | "use"
            | "verification"
            | "verify"
            | "view"
            | "viewpoint"
    )
}

fn append_module_member(module: &mut SysmlModule, declaration: Declaration) {
    match &declaration {
        Declaration::Import(import_decl) => module.imports.push(import_decl.clone()),
        Declaration::PartDefinition(definition) => module.definitions.push(definition.clone()),
        _ => {}
    }
    module.members.push(declaration);
}

struct UsageTail {
    ty: Option<QualifiedName>,
    multiplicity: Option<MultiplicityRange>,
    expression: Option<Expr>,
    additional_types: Vec<QualifiedName>,
    specializes: Vec<QualifiedName>,
    subsets: Vec<QualifiedName>,
    redefines: Vec<QualifiedName>,
    body_members: Vec<Declaration>,
    had_body: bool,
}

impl UsageTail {
    fn derived_name(&self, keyword: &str) -> String {
        self.specializes
            .first()
            .or(self.redefines.first())
            .or(self.subsets.first())
            .or(self.ty.as_ref())
            .and_then(|name| name.segments.last())
            .cloned()
            .unwrap_or_else(|| keyword.to_string())
    }
}

fn multiplicity_token_text(kind: &TokenKind) -> String {
    match kind {
        TokenKind::Number(value) | TokenKind::Identifier(value) | TokenKind::String(value) => {
            value.clone()
        }
        TokenKind::Star => "*".to_string(),
        TokenKind::Dot => ".".to_string(),
        TokenKind::Comma => ",".to_string(),
        TokenKind::Colon => ":".to_string(),
        TokenKind::Minus => "-".to_string(),
        TokenKind::Plus => "+".to_string(),
        _ => String::new(),
    }
}

fn normalize_multiplicity_raw(raw: &str) -> String {
    raw.replace(". .", "..")
        .replace(" ", "")
        .replace("...", "..")
}

fn multiplicity_bounds(raw: &str) -> (String, String) {
    if let Some((lower, upper)) = raw.split_once("..") {
        return (lower.to_string(), upper.to_string());
    }
    (raw.to_string(), raw.to_string())
}

fn synthetic_reference_usage(
    name: &str,
    ty: Option<QualifiedName>,
    reference_target: Option<QualifiedName>,
    modifiers: &[&str],
    span: &SourceSpan,
) -> Declaration {
    Declaration::GenericUsage(GenericUsageDecl {
        keyword: "reference".to_string(),
        name: name.to_string(),
        is_implicit_name: false,
        ty,
        reference_target,
        multiplicity: None,
        expression: None,
        additional_types: Vec::new(),
        specializes: Vec::new(),
        subsets: Vec::new(),
        redefines: Vec::new(),
        body_members: Vec::new(),
        docs: Vec::new(),
        modifiers: modifiers
            .iter()
            .map(|modifier| (*modifier).to_string())
            .collect(),
        span: span.clone(),
    })
}

fn append_package_member(
    members: &mut Vec<Declaration>,
    imports: &mut Vec<ImportDecl>,
    definitions: &mut Vec<PartDefinitionDecl>,
    declaration: Declaration,
) {
    match &declaration {
        Declaration::Import(import_decl) => imports.push(import_decl.clone()),
        Declaration::PartDefinition(definition) => definitions.push(definition.clone()),
        _ => {}
    }
    members.push(declaration);
}

fn implicit_usage_keyword(modifiers: &[String]) -> &'static str {
    if modifiers.iter().any(|modifier| modifier == "ref") {
        "reference"
    } else {
        "feature"
    }
}

fn infer_reference_target(
    keyword: &str,
    name: &str,
    has_type: bool,
    tail: &mut UsageTail,
) -> Option<QualifiedName> {
    if keyword == "reference" && !has_type && name == "ref" && tail.redefines.len() == 1 {
        return Some(tail.redefines.remove(0));
    }
    None
}

fn callable_expr_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Name(name) => Some(name.as_dot_string()),
        Expr::Path { root, segment, .. } => {
            Some(format!("{}.{}", callable_expr_name(root)?, segment))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::{
        SemanticCompileStatus, compile_sysml_module_with_context,
        compile_sysml_module_with_context_report, compile_sysml_text_with_context_report,
        load_sysml_document, parse_sysml, parse_sysml_recovering,
    };
    use crate::frontend::ast::{Declaration, Expr};
    use crate::frontend::resolver::{resolve_module, resolve_module_with_context};
    use crate::frontend::transpile::{MappingBundle, transpile_module};
    use crate::ir::{KirDocument, load_model_stack};
    use crate::runtime::Runtime;

    fn write_sample_model() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("mercurio-sysml-{unique}.sysml"));
        std::fs::write(
            &path,
            "package Demo2 {\n  part def Engine;\n  part def Vehicle {\n    part engine2: Engine;\n  }\n}\n",
        )
        .unwrap();
        path
    }

    #[test]
    fn parses_minimal_vehicle_model() {
        let module = parse_sysml(
            "package Demo2 {\n  part def Engine;\n  part def Vehicle {\n    part engine2: Engine;\n  }\n}\n",
        )
        .unwrap();
        assert!(module.package.is_some());
        assert_eq!(module.definitions.len(), 0);
        let package = module.package.unwrap();
        assert_eq!(package.name.as_dot_string(), "Demo2");
        assert_eq!(package.definitions.len(), 2);
    }

    #[test]
    fn recovering_parser_consumes_unmatched_closing_brace() {
        let report = parse_sysml_recovering("package Demo { } }").unwrap();

        assert!(report.module.package.is_some());
        assert_eq!(report.diagnostics.len(), 1);
        assert!(
            report.diagnostics[0]
                .message
                .contains("expected a declaration")
        );
    }

    #[test]
    fn transpiles_minimal_vehicle_model_to_expected_kir() {
        let path = write_sample_model();
        let actual = load_sysml_document(&path).unwrap();

        assert!(
            actual
                .elements
                .iter()
                .any(|element| element.id == "pkg.Demo2")
        );
        assert!(
            actual
                .elements
                .iter()
                .any(|element| element.id == "type.Demo2.Engine")
        );
        assert!(
            actual
                .elements
                .iter()
                .any(|element| element.id == "type.Demo2.Vehicle")
        );

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn stack_loader_accepts_sysml_files() {
        let path = write_sample_model();
        let document = load_model_stack(&path).unwrap();
        assert!(
            document
                .elements
                .iter()
                .any(|element| element.id == "type.Demo2.Vehicle")
        );
        assert!(
            document
                .elements
                .iter()
                .any(|element| element.id == "Base::Anything")
        );
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn runtime_can_query_parsed_l2_types() {
        let path = write_sample_model();
        let document = load_model_stack(&path).unwrap();
        let runtime = Runtime::from_document(document).unwrap();
        let features = runtime.get_features("type.Demo2.Vehicle").unwrap();

        assert!(
            features
                .value
                .contains(&"feature.Demo2.Vehicle.engine2".to_string())
        );

        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn rejects_unresolved_qualified_name_without_leaf_fallback() {
        let module =
            parse_sysml("package Demo { part def Vehicle specializes Wrong::PartDefinition; }")
                .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let error = resolve_module(&module, &stdlib, &mappings).unwrap_err();

        assert!(
            error
                .message
                .contains("unresolved specialization `Wrong::PartDefinition`")
        );
    }

    #[test]
    fn colliding_import_aliases_are_ambiguous_not_fatal() {
        let module = parse_sysml(
            "package Demo { import First::Thing; import Second::Thing; part thing : Thing; }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["First::Thing", "Second::Thing"]);
        let mappings = MappingBundle::load().unwrap();
        let error = resolve_module(&module, &stdlib, &mappings).unwrap_err();

        assert!(error.message.contains("unresolved type `Thing`"));
    }

    #[test]
    fn rejects_duplicate_emitted_kir_ids_before_write() {
        let module = parse_sysml(
            "package Demo { part def Engine; part def Vehicle { part engine: Engine; part engine: Engine; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let error = transpile_module(&resolved, "inline.sysml", &mappings).unwrap_err();

        assert!(error.message.contains("duplicate emitted KIR id"));
    }

    #[test]
    fn named_assert_constraint_usages_emit_distinct_ids() {
        let module = parse_sysml(
            "package Demo { part def Vehicle { assert constraint massBalance { totalMass == dryMass + fuelMass } assert constraint maxMassCheck { totalMass <= maxMass } } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(
            kir.elements
                .iter()
                .any(|element| element.id == "assert.Demo.Vehicle.massBalance")
        );
        assert!(
            kir.elements
                .iter()
                .any(|element| element.id == "assert.Demo.Vehicle.maxMassCheck")
        );
    }

    #[test]
    fn anonymous_assert_constraint_usages_are_source_disambiguated() {
        let module = parse_sysml(
            "package Demo { part def Vehicle { assert constraint { totalMass == dryMass + fuelMass } assert constraint { totalMass <= maxMass } } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();
        let assert_ids = kir
            .elements
            .iter()
            .filter(|element| element.id.starts_with("assert.Demo.Vehicle."))
            .map(|element| element.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(assert_ids.len(), 2);
        assert_ne!(assert_ids[0], assert_ids[1]);
    }

    #[test]
    fn parses_visibility_wildcard_imports_and_quoted_package_names() {
        let module = parse_sysml(
            "package 'Package Example' { public import ScalarValues::*; private part def Automobile; }",
        )
        .unwrap();

        let package = module.package.unwrap();
        assert_eq!(package.name.as_dot_string(), "Package Example");
        assert_eq!(package.imports[0].path.as_colon_string(), "ScalarValues::*");
        assert_eq!(package.definitions[0].name, "Automobile");
    }

    #[test]
    fn parses_specialization_shorthand_and_ref_part_members() {
        let module =
            parse_sysml("package Demo { part def Engine :> Vehicle { ref part driver: Person; } }")
                .unwrap();

        let definition = &module.package.unwrap().definitions[0];
        assert_eq!(definition.specializes[0].as_colon_string(), "Vehicle");
        assert_eq!(definition.part_members[0].name, "driver");
        assert_eq!(
            definition.part_members[0]
                .ty
                .as_ref()
                .unwrap()
                .as_colon_string(),
            "Person"
        );
    }

    #[test]
    fn parses_generic_item_attribute_alias_and_nested_package_declarations() {
        let module = parse_sysml(
            "package Demo { item def A; attribute def Status; alias Car for A; package Inner { part def Wheel; } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        assert_eq!(package.members.len(), 4);
        assert!(matches!(
            &package.members[0],
            Declaration::GenericDefinition(definition) if definition.keyword == "item" && definition.name == "A"
        ));
        assert!(matches!(
            &package.members[1],
            Declaration::GenericDefinition(definition) if definition.keyword == "attribute" && definition.name == "Status"
        ));
        assert!(matches!(
            &package.members[2],
            Declaration::Alias(alias) if alias.name == "Car" && alias.target.as_colon_string() == "A"
        ));
        assert!(matches!(
            &package.members[3],
            Declaration::Package(inner) if inner.name.as_dot_string() == "Inner"
        ));
    }

    #[test]
    fn resolver_supports_item_definitions_after_parse() {
        let module = parse_sysml("package Demo { item def A; }").unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        assert_eq!(resolved.definitions.len(), 1);
        assert_eq!(resolved.definitions[0].construct, "ItemDefinition");
        assert_eq!(resolved.definitions[0].qualified_name, "Demo.A");
    }

    #[test]
    fn transpiles_feature_definitions_with_kerml_mapping() {
        let module = parse_sysml("package Demo { feature def SemanticThing; }").unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "demo.kerml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| {
            element.id == "type.Demo.SemanticThing" && element.kind == "KerML::Core::Type"
        }));
    }

    #[test]
    fn transpiles_custom_profile_definition_as_classifier() {
        let module = parse_sysml("package Demo { service def APISService; }").unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| {
            element.id == "type.Demo.APISService" && element.kind == "KerML::Core::Type"
        }));
    }

    #[test]
    fn resolver_supports_root_relative_wildcard_imports_into_nested_packages() {
        let module = parse_sysml(
            "package ImportTest { package Pkg1 { import Pkg2::Pkg21::*; part p11: Pkg211::P211; } package Pkg2 { package Pkg21 { package Pkg211 { part def P211; } } } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        assert!(
            resolved
                .usages
                .iter()
                .any(|usage| usage.declared_name == "p11"
                    && usage.type_ref.as_deref() == Some("type.ImportTest.Pkg2.Pkg21.Pkg211.P211"))
        );
    }

    #[test]
    fn resolver_allows_import_fallback_without_relaxing_general_type_lookup() {
        let module = parse_sysml("package Demo { import ISQ::TorqueValue; part def Vehicle { part torque: TorqueValue; } }").unwrap();
        let stdlib = fake_stdlib(["ISQ", "ISQMechanics::TorqueValue"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        assert!(
            resolved
                .imports
                .iter()
                .any(|import| import.target_id == "ISQMechanics::TorqueValue")
        );
        assert!(
            resolved
                .definitions
                .iter()
                .flat_map(|definition| definition.members.iter())
                .any(|usage| usage.declared_name == "torque"
                    && usage.type_ref.as_deref() == Some("ISQMechanics::TorqueValue"))
        );
    }

    #[test]
    fn transpiles_feature_attribute_and_port_constructs_with_seed_mappings() {
        let module = parse_sysml(
            "package Demo { item def A; attribute def Status { attribute gear: Integer; } port def P { item payload: A; } part def Vehicle { attribute status: Status; port conn: P; sample: A; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["Integer", "SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(
            kir.elements
                .iter()
                .any(|element| element.id == "type.Demo.Status"
                    && element.kind == "SysML::AttributeDefinition")
        );
        assert!(
            kir.elements
                .iter()
                .any(|element| element.id == "type.Demo.P"
                    && element.kind == "SysML::PortDefinition")
        );
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.Vehicle.status" && element.kind == "SysML::AttributeUsage"
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.Vehicle.conn" && element.kind == "SysML::PortUsage"
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.Vehicle.sample" && element.kind == "KerML::Core::Feature"
        }));
    }

    #[test]
    fn parses_redefinition_shorthand_and_initializer_tail() {
        let module = parse_sysml(
            "package Demo { part def TrafficLightGo specializes TrafficLight { attribute redefines currentColor = TrafficLightColor::green; } part def BigVehicle :> Vehicle { part bigEng : BigEngine :>> eng; } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        assert_eq!(package.definitions.len(), 2);
        assert!(!package.definitions[0].members.is_empty());
        assert!(matches!(
            &package.definitions[1].members[0],
            Declaration::PartUsage(usage) if usage.name == "bigEng"
                && usage.redefines.len() == 1
                && usage.redefines[0].as_dot_string() == "eng"
        ));
    }

    #[test]
    fn parses_relation_led_members_inside_definition_bodies() {
        let module = parse_sysml(
            "package Demo { attribute def NominalScenario { :>> samples : TimeStateRecord; n : Natural; } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        assert_eq!(package.members.len(), 1);
        assert!(matches!(
            &package.members[0],
            Declaration::GenericDefinition(definition)
                if matches!(
                    &definition.members[0],
                    Declaration::GenericUsage(usage)
                        if usage.name == "samples"
                            && usage.redefines.len() == 1
                            && usage.redefines[0].as_dot_string() == "samples"
                            && usage.ty.as_ref().map(|name| name.as_dot_string()).as_deref()
                                == Some("TimeStateRecord")
                )
        ));
    }

    #[test]
    fn definition_bodies_allow_declarations_followed_by_opaque_statements() {
        let module = parse_sysml(
            "package Demo { calc def TotalPressure { in P_static; in rho; in V; 1/2 * rho * V^2 + P_static } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        assert!(matches!(
            &package.members[0],
            Declaration::GenericDefinition(definition)
                if definition.members.len() == 3
        ));
    }

    #[test]
    fn parses_and_transpiles_enumeration_definition_surface_syntax() {
        let module = parse_sysml(
            "package Demo { enum def TrafficLightColor { enum green; enum yellow; enum red; } part def TrafficLight { attribute currentColor : TrafficLightColor; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| {
            element.id == "type.Demo.TrafficLightColor"
                && element.properties.get("is_abstract") == Some(&serde_json::Value::Bool(true))
                && element.properties.get("metatype")
                    == Some(&serde_json::Value::String(
                        "SysML::Systems::EnumerationDefinition".to_string(),
                    ))
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.TrafficLightColor.green"
                && element.properties.get("metatype")
                    == Some(&serde_json::Value::String(
                        "SysML::Systems::EnumerationUsage".to_string(),
                    ))
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.TrafficLight.currentColor"
                && element.properties.get("type")
                    == Some(&serde_json::Value::String(
                        "type.Demo.TrafficLightColor".to_string(),
                    ))
        }));
    }

    #[test]
    fn partial_compile_recovers_invalid_interface_end_and_preserves_valid_sibling_end() {
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let report = compile_sysml_text_with_context_report(
            "package Demo { port def FuelInPort; interface def FuelInterface { end supplierPort : MissingFuelOutPort; end consumerPort : FuelInPort; } }",
            "inline.sysml",
            &[],
            &stdlib,
        );

        assert_eq!(report.status, SemanticCompileStatus::Partial);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("unresolved type `MissingFuelOutPort`")
        }));
        let kir = report.document.unwrap();
        assert!(kir.elements.iter().any(|element| {
            element
                .id
                .starts_with("feature.Demo.FuelInterface.consumerPort")
                && element.kind == "KerML::Core::Feature"
                && element.properties.get("type")
                    == Some(&serde_json::Value::String(
                        "type.Demo.FuelInPort".to_string(),
                    ))
        }));
    }

    #[test]
    fn transpiles_connection_definition_end_parts_and_excludes_connection_usages_from_owned_members()
     {
        let module = parse_sysml(
            "package Demo { part def TireBead; part def Rim; connection def PressureSeat { end [1] part bead : TireBead; end [1] part mountingRim : Rim; } part wheel { connection : PressureSeat; connect bead to mountingRim; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| {
            element.id == "type.Demo.PressureSeat"
                && element.properties.get("features")
                    == Some(&serde_json::json!([
                        "feature.Demo.PressureSeat.bead",
                        "feature.Demo.PressureSeat.mountingRim"
                    ]))
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.PressureSeat.bead"
                && element.kind == "SysML::PartUsage"
                && element.properties.get("is_end") == Some(&serde_json::Value::Bool(true))
                && element.properties.get("specializes").is_some()
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.PressureSeat.mountingRim"
                && element.kind == "SysML::PartUsage"
                && element.properties.get("is_end") == Some(&serde_json::Value::Bool(true))
                && element.properties.get("specializes").is_some()
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.wheel"
                && element.properties.get("features").is_none()
                && element.properties.get("members").is_none()
        }));
        assert!(
            kir.elements
                .iter()
                .any(|element| element.id.contains("PressureSeat"))
        );
    }

    #[test]
    fn parses_typed_connection_usage_end_references_as_body_members() {
        let module = parse_sysml(
            "package Demo { part def TireBead; part def Rim; connection def PressureSeat { end [1] part bead : TireBead; end [1] part mountingRim : Rim; } part wheel { connection : PressureSeat connect bead references local.bead to mountingRim references local.rim; } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        let wheel = package
            .members
            .iter()
            .find_map(|member| match member {
                Declaration::PartUsage(usage) if usage.name == "wheel" => Some(usage),
                _ => None,
            })
            .unwrap();
        assert_eq!(wheel.name, "wheel");
    }

    #[test]
    fn parses_anonymous_connect_usage_end_references_as_body_members() {
        let module = parse_sysml(
            "package Demo { part def Joint; part def Hole; part assembly { part joints : Joint; part holes : Hole; connect [0..1] joints to [1] holes; } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        let assembly = package
            .members
            .iter()
            .find_map(|member| match member {
                Declaration::PartUsage(usage) if usage.name == "assembly" => Some(usage),
                _ => None,
            })
            .unwrap();
        let connection = assembly
            .body_members
            .iter()
            .find_map(|member| match member {
                Declaration::GenericUsage(usage) if usage.keyword == "connect" => Some(usage),
                _ => None,
            })
            .unwrap();

        assert_eq!(connection.body_members.len(), 2);
        assert!(matches!(
            &connection.body_members[0],
            Declaration::GenericUsage(usage)
                if usage.keyword == "reference"
                    && usage.name == "source"
                    && usage.reference_target.as_ref().map(|target| target.as_dot_string())
                        == Some("joints".to_string())
        ));
        assert!(matches!(
            &connection.body_members[1],
            Declaration::GenericUsage(usage)
                if usage.keyword == "reference"
                    && usage.name == "target"
                    && usage.reference_target.as_ref().map(|target| target.as_dot_string())
                        == Some("holes".to_string())
        ));
    }

    #[test]
    fn transpiles_package_and_usage_metatype_anchors() {
        let module = parse_sysml(
            "package Demo { import SysML::*; item def Payload; part def Vehicle { part engine: Engine; attribute status: Integer; item cargo: Payload; } part def Engine; }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["Integer", "SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| element.id == "pkg.Demo"));
        assert!(kir.elements.iter().any(|element| {
            element.kind == "SysML::Import"
                && element.properties.get("imports").is_some()
                && element.properties.get("metatype")
                    == Some(&serde_json::Value::String("SysML::Import".to_string()))
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.Vehicle.engine"
                && element.kind == "SysML::PartUsage"
                && element.properties.get("metatype")
                    == Some(&serde_json::Value::String(
                        "SysML::Systems::PartUsage".to_string(),
                    ))
                && element.properties.get("specializes").is_some()
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.Vehicle.status"
                && element.kind == "SysML::AttributeUsage"
                && element.properties.get("metatype")
                    == Some(&serde_json::Value::String(
                        "SysML::Systems::AttributeUsage".to_string(),
                    ))
                && element.properties.get("specializes").is_some()
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.Vehicle.cargo"
                && element.kind == "SysML::ItemUsage"
                && element.properties.get("metatype")
                    == Some(&serde_json::Value::String(
                        "SysML::Systems::ItemUsage".to_string(),
                    ))
                && element.properties.get("specializes").is_some()
        }));
    }

    #[test]
    fn transpiles_family_subset_relations_as_subsets_and_specializes() {
        let module = parse_sysml(
            "package Demo { port def FuelPort; part def Engine; part p: Engine; part def Vehicle { part engine: Engine; part fuelTank; port fuelPort: FuelPort; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.p"
                && element.kind == "SysML::PartUsage"
                && element.properties.get("subsetted_features")
                    == Some(&serde_json::json!(["Parts::parts"]))
                && element.properties.get("specializes")
                    == Some(&serde_json::json!(["type.Demo.Engine", "Parts::parts"]))
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.Vehicle.engine"
                && element.kind == "SysML::PartUsage"
                && element.properties.get("subsetted_features")
                    == Some(&serde_json::json!(["Items::Item::subparts"]))
                && element.properties.get("specializes")
                    == Some(&serde_json::json!([
                        "type.Demo.Engine",
                        "Items::Item::subparts"
                    ]))
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.Vehicle.fuelTank"
                && element.kind == "SysML::PartUsage"
                && element.properties.get("type") == Some(&serde_json::json!("Parts::Part"))
                && element.properties.get("subsetted_features")
                    == Some(&serde_json::json!(["Items::Item::subparts"]))
                && element.properties.get("specializes")
                    == Some(&serde_json::json!(["Items::Item::subparts"]))
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.Vehicle.fuelPort"
                && element.kind == "SysML::PortUsage"
                && element.properties.get("subsetted_features")
                    == Some(&serde_json::json!(["Parts::Part::ownedPorts"]))
                && element.properties.get("specializes")
                    == Some(&serde_json::json!([
                        "type.Demo.FuelPort",
                        "Parts::Part::ownedPorts"
                    ]))
        }));
    }

    #[test]
    fn transpiles_nested_item_usages_as_subitems() {
        let module =
            parse_sysml(
                "package Demo { item def A { item b; } part def Vehicle { part fuelTank { item fuel; } } }",
            )
                .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::ItemDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.A.b"
                && element.kind == "SysML::ItemUsage"
                && element.properties.get("subsetted_features")
                    == Some(&serde_json::json!(["Items::Item::subitems"]))
                && element.properties.get("specializes")
                    == Some(&serde_json::json!(["Items::Item::subitems"]))
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.Vehicle.fuelTank.fuel"
                && element.kind == "SysML::ItemUsage"
                && element.properties.get("subsetted_features")
                    == Some(&serde_json::json!(["Items::Item::subitems"]))
                && element.properties.get("specializes")
                    == Some(&serde_json::json!(["Items::Item::subitems"]))
        }));
    }

    #[test]
    fn transpiles_part_usage_inside_item_definition_with_part_base_type() {
        let module =
            parse_sysml("package Demo { item def A; item def B { abstract part a: A; } }").unwrap();
        let stdlib = fake_stdlib(["A", "SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.B.a"
                && element.kind == "SysML::PartUsage"
                && element.properties.get("type")
                    == Some(&serde_json::json!(["type.Demo.A", "Parts::Part"]))
                && element.properties.get("definition")
                    == Some(&serde_json::json!(["type.Demo.A", "Parts::Part"]))
        }));
    }

    #[test]
    fn parses_implicit_ref_usage_as_reference_usage() {
        let module =
            parse_sysml("package Demo { part def C { private in ref y: A, B; } }").unwrap();

        let package = module.package.unwrap();
        let definition = package
            .members
            .iter()
            .find_map(|member| match member {
                Declaration::PartDefinition(definition) if definition.name == "C" => {
                    Some(definition)
                }
                _ => None,
            })
            .unwrap();

        assert!(matches!(
            &definition.members[0],
            Declaration::GenericUsage(usage)
                if usage.keyword == "reference"
                    && usage.name == "y"
                    && usage.ty.as_ref().map(|target| target.as_dot_string()) == Some("A".to_string())
                    && usage.additional_types.iter().map(|target| target.as_dot_string()).collect::<Vec<_>>() == vec!["B".to_string()]
        ));
    }

    #[test]
    fn parses_modifier_only_ref_redefinition_as_reference_target() {
        let module = parse_sysml(
            "package Demo { part system; part satisfactionContext { ref :>> system; } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        let context = package
            .members
            .iter()
            .find_map(|member| match member {
                Declaration::PartUsage(usage) if usage.name == "satisfactionContext" => Some(usage),
                _ => None,
            })
            .unwrap();

        assert!(matches!(
            &context.body_members[0],
            Declaration::GenericUsage(usage)
                if usage.keyword == "reference"
                    && usage.name == "ref"
                    && usage.reference_target.as_ref().map(|target| target.as_dot_string())
                        == Some("system".to_string())
                    && usage.redefines.is_empty()
        ));
    }

    #[test]
    fn parses_action_accept_shorthand_as_accept_usage_with_payload_member() {
        let module = parse_sysml(
            "package Demo { item def S; part def B { action a1 { action aa accept S; } } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        let definition = package
            .members
            .iter()
            .find_map(|member| match member {
                Declaration::PartDefinition(definition) if definition.name == "B" => {
                    Some(definition)
                }
                _ => None,
            })
            .unwrap();
        let action = definition
            .members
            .iter()
            .find_map(|member| match member {
                Declaration::GenericUsage(usage)
                    if usage.keyword == "action" && usage.name == "a1" =>
                {
                    Some(usage)
                }
                _ => None,
            })
            .unwrap();
        let nested_accept = action
            .body_members
            .iter()
            .find_map(|member| match member {
                Declaration::GenericUsage(usage)
                    if usage.keyword == "accept" && usage.name == "aa" =>
                {
                    Some(usage)
                }
                _ => None,
            })
            .unwrap();

        assert!(matches!(
            &nested_accept.body_members[0],
            Declaration::GenericUsage(usage)
                if usage.keyword == "reference"
                    && usage.name == "payload"
                    && usage.ty.as_ref().map(|target| target.as_dot_string()) == Some("S".to_string())
        ));
    }

    #[test]
    fn resolves_nested_package_inside_part_definition_body() {
        let module = parse_sysml("package Demo { part def B { package P { } } }").unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        assert!(
            resolved
                .packages
                .iter()
                .any(|package| package.qualified_name == "Demo.B.P")
        );
    }

    #[test]
    fn transpiles_nested_package_inside_part_definition_with_definition_owner() {
        let module = parse_sysml("package Demo { part def B { package P { } } }").unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| {
            element.id == "pkg.Demo.B.P"
                && element.properties.get("owner") == Some(&serde_json::json!("type.Demo.B"))
        }));
    }

    #[test]
    fn transpiles_nested_port_usages_with_subport_family_defaults() {
        let module =
            parse_sysml("package Demo { port def C { port c1 : C; ref port c2 : C; } }").unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.C.c1"
                && element.properties.get("subsetted_features")
                    == Some(&serde_json::json!(["Ports::Port::subports"]))
        }));
        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.C.c2"
                && element.properties.get("subsetted_features")
                    == Some(&serde_json::json!(["Ports::ports"]))
        }));
    }

    #[test]
    fn transpiles_reference_usage_families_from_explicit_types() {
        let module = parse_sysml(
            "package Demo { item def A; item def B; part def C { in ref y: A, B; ref z: Integer; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["Integer", "SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        let y = kir
            .elements
            .iter()
            .find(|element| element.id == "reference.Demo.C.y.1")
            .unwrap();
        assert_eq!(
            y.properties.get("type"),
            Some(&serde_json::json!(["type.Demo.A", "type.Demo.B"]))
        );
        assert_eq!(
            y.properties.get("definition"),
            Some(&serde_json::json!(["type.Demo.A", "type.Demo.B"]))
        );
        assert_eq!(
            y.properties.get("subsetted_features"),
            Some(&serde_json::json!(["Objects::objects"]))
        );
        assert_eq!(
            y.properties.get("specializes"),
            Some(&serde_json::json!([
                "type.Demo.A",
                "type.Demo.B",
                "Objects::objects"
            ]))
        );
        assert_eq!(
            y.properties.get("direction"),
            Some(&serde_json::json!("in"))
        );

        let z = kir
            .elements
            .iter()
            .find(|element| element.id == "reference.Demo.C.z.1")
            .unwrap();
        assert_eq!(
            z.properties.get("type"),
            Some(&serde_json::json!("Integer"))
        );
        assert_eq!(
            z.properties.get("subsetted_features"),
            Some(&serde_json::json!(["Base::dataValues"]))
        );
        assert_eq!(
            z.properties.get("specializes"),
            Some(&serde_json::json!(["Integer", "Base::dataValues"]))
        );
    }

    #[test]
    fn transpiles_synthetic_reference_usage_roles_for_accept_and_succession() {
        let module = parse_sysml(
            "package Demo { item def S; port def P; part def B { port x: P; succession flow x to a1.aa.receiver; action a1 { accept S via x; action aa accept S; } } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        let succession_source = kir
            .elements
            .iter()
            .find(|element| element.id == "reference.Demo.B.SuccessionFlowUsage.x.1")
            .unwrap();
        assert_eq!(
            succession_source.properties.get("type"),
            Some(&serde_json::json!("type.Demo.P"))
        );
        assert_eq!(
            succession_source.properties.get("redefined_features"),
            Some(&serde_json::json!(["x", "Transfers::sourceOutput"]))
        );
        assert_eq!(
            succession_source.properties.get("specializes"),
            Some(&serde_json::json!(["x", "Transfers::sourceOutput"]))
        );
        assert!(
            !succession_source
                .properties
                .contains_key("subsetted_features")
        );

        let succession_target = kir
            .elements
            .iter()
            .find(|element| element.id == "reference.Demo.B.SuccessionFlowUsage.receiver.1")
            .unwrap();
        assert_eq!(
            succession_target.properties.get("type"),
            Some(&serde_json::json!("Occurrences::Occurrence"))
        );
        assert_eq!(
            succession_target.properties.get("redefined_features"),
            Some(&serde_json::json!(["receiver", "Transfers::targetInput"]))
        );
        assert_eq!(
            succession_target.properties.get("direction"),
            Some(&serde_json::json!("in"))
        );

        let payload = kir
            .elements
            .iter()
            .find(|element| element.id == "reference.Demo.B.a1.AcceptActionUsage.payload.1")
            .unwrap();
        assert_eq!(
            payload.properties.get("type"),
            Some(&serde_json::json!("type.Demo.S"))
        );
        assert_eq!(
            payload.properties.get("subsetted_features"),
            Some(&serde_json::json!(["Objects::objects"]))
        );
        assert_eq!(
            payload.properties.get("redefined_features"),
            Some(&serde_json::json!(["payload"]))
        );
        assert_eq!(
            payload.properties.get("direction"),
            Some(&serde_json::json!("in"))
        );

        let receiver = kir
            .elements
            .iter()
            .find(|element| element.id == "reference.Demo.B.a1.AcceptActionUsage.receiver.1")
            .unwrap();
        assert_eq!(
            receiver.properties.get("type"),
            Some(&serde_json::json!("Occurrences::Occurrence"))
        );
        assert_eq!(
            receiver.properties.get("redefined_features"),
            Some(&serde_json::json!(["receiver"]))
        );
        assert_eq!(
            receiver.properties.get("direction"),
            Some(&serde_json::json!("in"))
        );
    }

    #[test]
    fn resolves_self_specializing_part_usage() {
        let module = parse_sysml("package Demo { part p4 :> p4; }").unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(kir.elements.iter().any(|element| {
            element.id == "feature.Demo.p4"
                && element.kind == "SysML::PartUsage"
                && element.properties.get("specializes")
                    == Some(&serde_json::json!(["feature.Demo.p4", "Parts::parts"]))
                && element.properties.get("subsetted_features")
                    == Some(&serde_json::json!(["feature.Demo.p4", "Parts::parts"]))
        }));
    }

    #[test]
    fn transpiles_loop_and_send_usages_from_behavioral_examples() {
        let module = parse_sysml(
            "package Demo { item def Signal; part controller; loop action charging { } until charging; send new Signal() to controller; }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        assert!(
            kir.elements
                .iter()
                .any(|element| element.id == "loop.Demo.action"
                    && element.kind == "KerML::Core::Feature")
        );
        assert!(
            kir.elements
                .iter()
                .any(|element| element.id == "until.Demo.charging"
                    && element.kind == "KerML::Core::Feature")
        );
        assert!(
            kir.elements
                .iter()
                .any(|element| element.id == "send.Demo.new"
                    && element.kind == "KerML::Core::Feature")
        );
    }

    #[test]
    fn parses_anonymous_objective_and_opaque_constraint_blocks() {
        let module = parse_sysml(
            "package Demo { requirement def Need { subject vehicle; objective { doc /* hi */ } require constraint { vehicle.mass > 0[kg] } } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        let definition = match &package.members[0] {
            Declaration::GenericDefinition(definition) => definition,
            other => panic!("expected generic definition, got {other:?}"),
        };

        assert_eq!(definition.keyword, "requirement");
        assert!(matches!(
            &definition.members[0],
            Declaration::GenericUsage(usage) if usage.keyword == "subject" && usage.name == "vehicle"
        ));
        assert!(definition.members.len() >= 2);
    }

    #[test]
    fn parses_composite_use_case_definition_keyword() {
        let module = parse_sysml(
            "package Demo { use case def 'Provide Transportation' { subject vehicle; actor driver; } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        let definition = match &package.members[0] {
            Declaration::GenericDefinition(definition) => definition,
            other => panic!("expected generic definition, got {other:?}"),
        };

        assert_eq!(definition.keyword, "use-case");
        assert_eq!(definition.name, "Provide Transportation");
    }

    #[test]
    fn parses_multiple_root_packages_and_root_imports() {
        let module = parse_sysml(
            "package P1 { part def A; } package P2 { private import P1::*; part a : A; } private import P2::*; package P3 { part b subsets a; }",
        )
        .unwrap();

        assert_eq!(module.members.len(), 4);
        assert!(matches!(module.members[0], Declaration::Package(_)));
        assert!(matches!(module.members[1], Declaration::Package(_)));
        assert!(matches!(module.members[2], Declaration::Import(_)));
        assert!(matches!(module.members[3], Declaration::Package(_)));
    }

    #[test]
    fn parses_library_package_modifier() {
        let module = parse_sysml("library package Profile { port def P; }").unwrap();

        let package = match &module.members[0] {
            Declaration::Package(package) => package,
            other => panic!("expected package, got {other:?}"),
        };
        assert!(package.modifiers.contains(&"library".to_string()));
    }

    #[test]
    fn parses_empty_package_declaration_with_semicolon() {
        let module =
            parse_sysml("package DependencyTest { package 'Application Layer'; }").unwrap();
        let package = module.package.unwrap();
        assert!(matches!(
            &package.members[0],
            Declaration::Package(nested) if nested.name.as_dot_string() == "Application Layer"
                && nested.members.is_empty()
        ));
    }

    #[test]
    fn parses_expression_unit_suffixes_and_cast_postfix() {
        let module = parse_sysml(
            "package Demo { attribute length = new Cuboid(4800 [mm], 1840 [mm]); attribute local = new Translation((3800, 825, 40)[datum]); attribute masses = (vehicles as VehiclePart).m; attribute named = F(q = 1, p = a); }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert_eq!(package.members.len(), 4);
    }

    #[test]
    fn parses_expression_filter_postfix() {
        let module = parse_sysml(
            "package Demo { attribute total = mass + sum(subcomponents.totalMass.?{ in p :> ISQ::mass; p > minMass }); }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert_eq!(package.members.len(), 1);
    }

    #[test]
    fn parses_connection_usage_connecting_features_without_named_ends() {
        let module = parse_sysml(
            "package Demo { item def Customer { item cart; item products; connection ps : ProductSelection connect [1] cart to [1] products { :>> info = details; } } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        let definition = match &package.members[0] {
            Declaration::GenericDefinition(definition) => definition,
            other => panic!("expected item definition, got {other:?}"),
        };
        assert!(matches!(
            &definition.members[2],
            Declaration::GenericUsage(usage) if usage.keyword == "connection" && usage.body_members.len() >= 2
        ));
    }

    #[test]
    fn parses_interface_connect_named_ends_with_specialization_arrow() {
        let module = parse_sysml(
            "package Demo { interface drive connect transDrive ::> transmission.drive to axleDrive ::> axle.drive { flow f from transDrive.torque to axleDrive.torque; } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert!(matches!(
            &package.members[0],
            Declaration::GenericUsage(usage)
                if usage.keyword == "interface" && usage.body_members.len() == 3
        ));
    }

    #[test]
    fn parses_nary_connection_tuple_connect_form() {
        let module = parse_sysml(
            "package Demo { part d1; part d2; part d3; connection bus : C connect (d1, d2, d3); }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert_eq!(package.members.len(), 4);
    }

    #[test]
    fn parses_interface_tuple_connect_form_with_body() {
        let module = parse_sysml(
            "package Demo { interface i : Interfaces::Interface connect (a ::> A.p, b ::> B.p) { flow f from a.out to b.in; } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert!(matches!(
            &package.members[0],
            Declaration::GenericUsage(usage)
                if usage.keyword == "interface" && usage.body_members.len() == 1
        ));
    }

    #[test]
    fn parses_named_succession_flow_with_from_clause() {
        let module = parse_sysml(
            "package Demo { action illuminate { action send { out x; } action receive { in x; } succession flow xFlow from send.x to receive.x; } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        let action = match &package.members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected action usage, got {other:?}"),
        };
        assert!(matches!(
            &action.body_members[2],
            Declaration::GenericUsage(usage)
                if usage.keyword == "succession" && usage.name == "xFlow"
        ));
    }

    #[test]
    fn parses_constraint_body_with_opaque_if_else_expression() {
        let module = parse_sysml(
            "package Demo { part p { assert constraint { if flag? p istype A else p istype B } } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        let part = match &package.members[0] {
            Declaration::PartUsage(usage) => usage,
            other => panic!("expected part usage, got {other:?}"),
        };
        let constraint = match &part.body_members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected constraint usage, got {other:?}"),
        };
        assert_eq!(constraint.keyword, "assert");
        assert!(constraint.body_members.is_empty());
    }

    #[test]
    fn parses_accept_payload_name_and_type() {
        let module =
            parse_sysml("package Demo { state s { accept rs:ResultGiveItems then Wait; } }")
                .unwrap();
        let package = module.package.unwrap();
        let state = match &package.members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected state usage, got {other:?}"),
        };
        let accept = match &state.body_members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected accept usage, got {other:?}"),
        };
        assert_eq!(accept.keyword, "accept");
        assert!(matches!(
            &accept.body_members[0],
            Declaration::GenericUsage(payload)
                if payload.name == "rs"
                    && payload.ty.as_ref().map(|ty| ty.as_dot_string()).as_deref()
                        == Some("ResultGiveItems")
        ));
    }

    #[test]
    fn parses_accept_after_when_and_at_forms_without_payload_type_lookup() {
        let module = parse_sysml(
            "package Demo { action a { accept sig after 10[SI::s]; accept when b.f; accept at new Time::Iso8601DateTime(\"2022-01-30T01:00:00Z\"); } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        let action = match &package.members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected action usage, got {other:?}"),
        };

        assert_eq!(action.body_members.len(), 3);
        assert!(action.body_members.iter().all(|member| matches!(
            member,
            Declaration::GenericUsage(usage) if usage.keyword == "accept"
        )));
    }

    #[test]
    fn parses_named_action_send_as_send_usage() {
        let module =
            parse_sysml("package Demo { action a { action snd send { in :>> payload = s; } } }")
                .unwrap();
        let package = module.package.unwrap();
        let action = match &package.members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected action usage, got {other:?}"),
        };

        assert!(matches!(
            &action.body_members[0],
            Declaration::GenericUsage(usage) if usage.keyword == "send" && usage.name == "snd"
        ));
    }

    #[test]
    fn resolves_send_payload_redefinition_against_send_action() {
        let module =
            parse_sysml("package Demo { action a { in s; action snd send { in :>> payload = s; } } }")
                .unwrap();
        let stdlib = fake_stdlib(["Actions::SendAction", "Actions::SendAction::payload"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let snd = find_resolved_usage(&resolved.usages, "Demo.a")
            .and_then(|action| action.members.iter().find(|member| member.declared_name == "snd"))
            .unwrap();
        let payload = snd
            .members
            .iter()
            .find(|member| member.declared_name == "in")
            .unwrap();
        assert_eq!(
            payload.redefined_features,
            vec!["feature.Actions::SendAction::payload".to_string()]
        );
    }

    #[test]
    fn analysis_case_definition_inherits_case_result_feature() {
        let module = parse_sysml(
            "package Demo { analysis def AnalysisCase { objective obj { subject = result; } } }",
        )
        .unwrap();
        let stdlib = fake_stdlib([
            "AnalysisCases::AnalysisCase",
            "Cases::Case",
            "Cases::Case::result",
            "AnalysisCases::AnalysisCase::result",
            "OtherFunctions::f::result",
        ]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let objective = resolved
            .definitions
            .iter()
            .find(|definition| definition.qualified_name == "Demo.AnalysisCase")
            .and_then(|definition| {
                definition
                    .members
                    .iter()
                    .find(|member| member.declared_name == "obj")
            })
            .unwrap();
        let subject = objective
            .members
            .iter()
            .find(|member| member.declared_name == "subject")
            .unwrap();
        assert_eq!(
            subject.expression,
            Some(crate::frontend::resolver::ResolvedExpr::FeaturePath {
                segments: vec![crate::frontend::resolver::ResolvedPathSegment {
                    name: "result".to_string(),
                    feature_id: "feature.AnalysisCases::AnalysisCase::result".to_string(),
                }]
            })
        );
    }

    #[test]
    fn expression_path_resolves_instance_member_then_member_type_feature() {
        let module = parse_sysml(
            "package Demo { part def VehiclePart { attribute m; } part def Vehicle; part vehicle : Vehicle { part eng : VehiclePart; } calc ms { in partMasses = (vehicle.eng.m); } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let calc = find_resolved_usage(&resolved.usages, "Demo.ms").unwrap();
        let input = calc
            .members
            .iter()
            .find(|member| member.declared_name == "partMasses")
            .unwrap();
        assert!(input.expression.is_some());
    }

    #[test]
    fn parses_anonymous_nary_connect_tuple_form() {
        let module = parse_sysml(
            "package Demo { part d1; part d2; part d3; #multicausation connect (cause1 ::> d1, cause2 ::> d2, effect1 ::> d3); }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert_eq!(package.members.len(), 4);
    }

    #[test]
    fn parses_hashed_cause_and_effect_usages() {
        let module =
            parse_sysml("package Demo { part a; #cause causeA ::> a; #effect effectA ::> a; }")
                .unwrap();
        let package = module.package.unwrap();
        assert_eq!(package.members.len(), 3);
    }

    #[test]
    fn parses_profiled_arbitrary_keyword_usage_name_before_specialization() {
        let module =
            parse_sysml("package Demo { #profiled concreteThing :> Base { #nested child; } }")
                .unwrap();
        let package = module.package.unwrap();
        let usage = match &package.members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected profiled usage, got {other:?}"),
        };

        assert_eq!(usage.keyword, "profiled");
        assert_eq!(usage.name, "concreteThing");
        assert_eq!(usage.specializes[0].as_dot_string(), "Base");
        assert!(matches!(
            &usage.body_members[0],
            Declaration::GenericUsage(child)
                if child.keyword == "nested" && child.name == "child"
        ));
    }

    #[test]
    fn parses_stacked_hashed_metadata_before_declaration() {
        let module = parse_sysml(
            "package Demo { private ref #Classified #Security z1; ref z { #Security #Classified metadata Classified { level = secret; } } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert_eq!(package.members.len(), 2);
    }

    #[test]
    fn parses_perform_action_with_anonymous_redefinition() {
        let module = parse_sysml(
            "package Demo { part vehicle { perform action :>> providePowerFamily { action :>> generateTorque = generateTorque::generateTorque4Cyl; } } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert_eq!(package.members.len(), 1);
    }

    #[test]
    fn parses_textual_representation_with_language_body() {
        let module = parse_sysml(
            "package Demo { item def C { assert constraint x { rep inOCL language \"ocl\" /* self.x > 0 */ } } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert!(matches!(
            &package.members[0],
            Declaration::GenericDefinition(_)
        ));
    }

    #[test]
    fn treats_new_constructor_statement_as_opaque_body_statement() {
        let module = parse_sysml(
            "package Demo { calc def F { calc :>> getOutput { in state; new CartOutput(state.velocity) } } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert!(matches!(
            &package.members[0],
            Declaration::GenericDefinition(_)
        ));
    }

    #[test]
    fn parses_opaque_constraint_expression_with_less_than() {
        let module = parse_sysml(
            "package Demo { constraint massLimitation { mass : MassValue; massLimit : MassValue; mass < massLimit } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        assert_eq!(package.members.len(), 1);
    }

    #[test]
    fn parses_comment_usages_with_about_and_trailing_docs() {
        let module = parse_sysml(
            "package Comments { comment cmt /* Named */ comment cmt_cmt about cmt /* About comment */ comment about C /* About part */ part def C { comment /* Inline */ comment about Comments locale \"en_US\" /* About package */ } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        let package_comments = package
            .members
            .iter()
            .filter(|member| matches!(member, Declaration::GenericUsage(usage) if usage.keyword == "comment"))
            .count();
        assert_eq!(package_comments, 3);

        let definition = match package
            .members
            .iter()
            .find(|member| matches!(member, Declaration::PartDefinition(definition) if definition.name == "C"))
            .unwrap()
        {
            Declaration::PartDefinition(definition) => definition,
            other => panic!("expected part definition, got {other:?}"),
        };
        let body_comments = definition
            .members
            .iter()
            .filter(|member| matches!(member, Declaration::GenericUsage(usage) if usage.keyword == "comment"))
            .count();
        assert_eq!(body_comments, 1);
    }

    #[test]
    fn parses_anonymous_redefinition_usage() {
        let module = parse_sysml(
            "package Demo { part def Logical { part component; } part l : Logical { part :>> component; } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        let usage = package
            .members
            .iter()
            .find_map(|member| match member {
                Declaration::PartUsage(usage) if usage.name == "l" => Some(usage),
                _ => None,
            })
            .unwrap();
        let nested = match &usage.body_members[0] {
            Declaration::PartUsage(usage) => usage,
            other => panic!("expected nested part usage, got {other:?}"),
        };
        assert_eq!(nested.name, "component");
        assert!(nested.is_implicit_name);
        assert_eq!(nested.redefines[0].as_colon_string(), "component");
    }

    #[test]
    fn resolves_anonymous_redefinition_usage_against_owner_type() {
        let module = parse_sysml(
            "package Demo { part def Logical { part component; } part l : Logical { part :>> component; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let usage = find_resolved_usage(&resolved.usages, "Demo.l.component").unwrap();
        assert_eq!(
            usage.redefined_features,
            vec!["feature.Demo.Logical.component".to_string()]
        );
    }

    #[test]
    fn resolves_root_wildcard_imported_usage() {
        let module = parse_sysml(
            "package P1 { part def A; } package P2 { private import P1::*; part a : A; } private import P2::*; package P3 { part b subsets a; }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let package = resolved
            .packages
            .iter()
            .find(|package| package.qualified_name == "P3")
            .unwrap();
        assert_eq!(package.declared_name, "P3");
        let b = find_resolved_usage(&resolved.usages, "P3.b").unwrap();
        assert_eq!(b.subsetted_features, vec!["feature.P2.a".to_string()]);
    }

    #[test]
    fn resolves_wildcard_imported_profiled_usage_in_expression() {
        let producer = parse_sysml("package Producer { #profiled produced; }").unwrap();
        let consumer =
            parse_sysml("package Consumer { private import Producer::*; part copy = produced; }")
                .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module_with_context(
            &consumer,
            &[producer, consumer.clone()],
            &stdlib,
            &mappings,
        )
        .unwrap();

        let copy = find_resolved_usage(&resolved.usages, "Consumer.copy").unwrap();
        assert_eq!(
            copy.expression,
            Some(crate::frontend::resolver::ResolvedExpr::FeaturePath {
                segments: vec![crate::frontend::resolver::ResolvedPathSegment {
                    name: "produced".to_string(),
                    feature_id: "feature.Producer.produced".to_string(),
                }]
            })
        );
    }

    #[test]
    fn redefinition_suffix_lookup_ignores_current_feature() {
        let module = parse_sysml(
            "package Demo { part def Base { attribute label; } part other { attribute label; } part def Derived { attribute :>> label = \"x\"; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let label = resolved
            .definitions
            .iter()
            .find(|definition| definition.qualified_name == "Demo.Derived")
            .and_then(|definition| {
                definition
                    .members
                    .iter()
                    .find(|member| member.qualified_name == "Demo.Derived.label")
            })
            .unwrap();
        assert_eq!(
            label.redefined_features,
            vec!["feature.Demo.Base.label".to_string()]
        );
    }

    #[test]
    fn resolves_conjugated_type_reference_by_unconjugated_name() {
        let module = parse_sysml(
            "package Demo { port def ServiceDiscoveryDD; part consumer { port serviceDiscovery : ~ServiceDiscoveryDD; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let service_discovery = find_resolved_usage(&resolved.usages, "Demo.consumer")
            .and_then(|consumer| {
                consumer
                    .members
                    .iter()
                    .find(|member| member.declared_name == "serviceDiscovery")
            })
            .unwrap();
        assert_eq!(
            service_discovery.type_ref.as_deref(),
            Some("type.Demo.ServiceDiscoveryDD")
        );
    }

    #[test]
    fn resolves_relative_qualified_type_in_owner_package() {
        let module =
            parse_sysml("package Demo { package P1 { part def A; } part x : P1::A; }").unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let x = find_resolved_usage(&resolved.usages, "Demo.x").unwrap();
        assert_eq!(x.type_ref.as_deref(), Some("type.Demo.P1.A"));
    }

    #[test]
    fn resolves_imported_alias_name_as_type() {
        let module = parse_sysml(
            "package AliasImport { package Definitions { part def Vehicle; alias Car for Vehicle; } package Usages { private import Definitions::Car; part vehicle : Car; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let vehicle = find_resolved_usage(&resolved.usages, "AliasImport.Usages.vehicle").unwrap();
        assert_eq!(
            vehicle.type_ref.as_deref(),
            Some("type.AliasImport.Definitions.Vehicle")
        );
    }

    #[test]
    fn resolves_stdlib_breadth_alias_import() {
        let module = parse_sysml(
            "package AliasTest { private import ISQSpaceTime::breadth; attribute b :> breadth; }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["ISQSpaceTime::width"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let b = find_resolved_usage(&resolved.usages, "AliasTest.b").unwrap();
        assert_eq!(b.specializes, vec!["ISQSpaceTime::width".to_string()]);
    }

    #[test]
    fn resolves_nested_feature_alias_for_redefinition() {
        let module = parse_sysml(
            "package AliasTest { part def P1 { port porig1; alias po1 for porig1; } part p1 : P1 { port po1 :>> po1; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let p1 = find_resolved_usage(&resolved.usages, "AliasTest.p1").unwrap();
        let po1 = p1
            .members
            .iter()
            .find(|member| member.declared_name == "po1")
            .unwrap();
        assert_eq!(
            po1.redefined_features,
            vec!["feature.AliasTest.P1.porig1".to_string()]
        );
    }

    #[test]
    fn resolves_simple_type_against_owner_package_before_global_duplicates() {
        let module = parse_sysml(
            "package P1 { part def A; part x : A; } package P2 { part def A; part x : A; }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let p1_x = find_resolved_usage(&resolved.usages, "P1.x").unwrap();
        let p2_x = find_resolved_usage(&resolved.usages, "P2.x").unwrap();
        assert_eq!(p1_x.type_ref.as_deref(), Some("type.P1.A"));
        assert_eq!(p2_x.type_ref.as_deref(), Some("type.P2.A"));
    }

    #[test]
    fn resolves_qualified_name_through_nested_public_import() {
        let module = parse_sysml(
            "package Demo { package P1 { part def A; } package P2 { package P2a { public import P1::*; } part x : P2a::A; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let x = find_resolved_usage(&resolved.usages, "Demo.P2.x").unwrap();
        assert_eq!(x.type_ref.as_deref(), Some("type.Demo.P1.A"));
    }

    #[test]
    fn resolves_anonymous_redefinition_against_typed_owner_usage() {
        let module = parse_sysml(
            "package Demo { connection def C { item info; } item def Customer { connection c : C { :>> info; } } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let info = resolved
            .definitions
            .iter()
            .find_map(|definition| find_resolved_usage(&definition.members, "Demo.Customer.c.info"))
            .unwrap();
        assert_eq!(
            info.redefined_features,
            vec!["feature.Demo.C.info".to_string()]
        );
    }

    #[test]
    fn resolves_nested_redefinition_against_specialized_owner_usage() {
        let module = parse_sysml(
            "package Demo { part car { part engine; } part c :> car { part redefines engine; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let engine = find_resolved_usage(&resolved.usages, "Demo.c.engine").unwrap();
        assert_eq!(
            engine.redefined_features,
            vec!["feature.Demo.car.engine".to_string()]
        );
    }

    #[test]
    fn resolves_nested_redefinition_against_redefined_owner_usage() {
        let module = parse_sysml(
            "package Demo { part packet { attribute data { attribute field; } } part packet3 { attribute data redefines packet::data { attribute redefines field; } } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();

        let field = find_resolved_usage(&resolved.usages, "Demo.packet3.data.field").unwrap();
        assert_eq!(
            field.redefined_features,
            vec!["feature.Demo.packet.data.field".to_string()]
        );
    }

    #[test]
    fn injects_default_library_specialization_for_plain_part_definitions() {
        let module = parse_sysml("package Demo { part def Vehicle; }").unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        let vehicle = kir
            .elements
            .iter()
            .find(|element| element.id == "type.Demo.Vehicle")
            .unwrap();
        assert_eq!(
            vehicle.properties.get("specializes"),
            Some(&serde_json::Value::Array(vec![serde_json::Value::String(
                "Parts::Part".to_string()
            )]))
        );
    }

    #[test]
    fn compiles_cross_file_specialization_with_wildcard_import_context() {
        let picture_taking = parse_sysml(
            "package PictureTaking {
                part def Exposure;
                action def Focus { out xrsl: Exposure; }
                action def Shoot { in xsf: Exposure; }
                action takePicture {
                    action focus: Focus[1];
                    flow of Exposure from focus.xrsl to shoot.xsf;
                    action shoot: Shoot[1];
                }
            }",
        )
        .unwrap();
        let camera = parse_sysml(
            "part def Camera {
                private import PictureTaking::*;
                perform action takePicture[*] :> PictureTaking::takePicture;
                part focusingSubsystem {
                    perform takePicture.focus;
                }
                part imagingSubsystem {
                    perform takePicture.shoot;
                }
            }",
        )
        .unwrap();
        let stdlib = KirDocument::from_path(&crate::paths::default_stdlib_path()).unwrap();

        let document = compile_sysml_module_with_context(
            &camera,
            "Camera.sysml",
            &[picture_taking, camera.clone()],
            &stdlib,
        )
        .unwrap();

        assert!(
            document
                .elements
                .iter()
                .any(|element| element.id == "type.Camera")
        );
        assert!(
            document
                .elements
                .iter()
                .any(|element| element.id.contains("takePicture"))
        );
    }

    #[test]
    fn parses_usage_expression_into_ast() {
        let module = parse_sysml(
            "package Demo { part vehicle { derived attribute totalMass = sum(self.parts.mass); } }",
        )
        .unwrap();

        let package = module.package.unwrap();
        let vehicle = package
            .members
            .iter()
            .find_map(|member| match member {
                Declaration::PartUsage(usage) if usage.name == "vehicle" => Some(usage),
                _ => None,
            })
            .unwrap();
        let total_mass = vehicle
            .body_members
            .iter()
            .find_map(|member| match member {
                Declaration::GenericUsage(usage) if usage.name == "totalMass" => Some(usage),
                _ => None,
            })
            .unwrap();

        assert!(matches!(
            total_mass.expression.as_ref(),
            Some(Expr::Call { function, args, .. })
                if function == "sum"
                    && matches!(args.first(), Some(Expr::Path { .. }))
        ));
    }

    #[test]
    fn transpiles_expression_ir_for_derived_attribute_usage() {
        let module = parse_sysml(
            "package Demo { attribute def Mass; part def Engine { attribute mass: Mass; } part vehicle { part parts: Engine; derived attribute totalMass = sum(self.parts.mass); } }",
        )
        .unwrap();
        let stdlib = KirDocument::from_path(&crate::paths::default_stdlib_path()).unwrap();
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        let total_mass = kir
            .elements
            .iter()
            .find(|element| element.id == "feature.Demo.vehicle.totalMass")
            .unwrap();
        assert_eq!(
            total_mass.properties.get("is_derived"),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            total_mass.properties.get("expression_ir"),
            Some(&serde_json::json!({
                "kind": "call",
                "function": "sum",
                "args": [{
                    "kind": "path",
                    "root": "self",
                    "segments": [
                        {"name": "parts", "feature": "feature.Demo.vehicle.parts"},
                        {"name": "mass", "feature": "feature.Demo.Engine.mass"}
                    ]
                }]
            }))
        );
    }

    #[test]
    fn initialized_attribute_usage_is_marked_derived_when_expression_is_present() {
        let module = parse_sysml(
            "package Demo { attribute def Mass; part vehicle { attribute totalMass : Mass = 42; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["Mass", "SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        let total_mass = kir
            .elements
            .iter()
            .find(|element| element.id == "feature.Demo.vehicle.totalMass")
            .unwrap();
        assert_eq!(
            total_mass.properties.get("is_derived"),
            Some(&serde_json::Value::Bool(true))
        );
        assert!(total_mass.properties.contains_key("expression_ir"));
    }

    #[test]
    fn redefining_attribute_with_expression_is_not_marked_derived() {
        let module = parse_sysml(
            "package Demo { enum def Color { enum green; } part def TrafficLight { attribute currentColor : Color; } part def TrafficLightGo specializes TrafficLight { attribute redefines currentColor = Color::green; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();

        let current_color = kir
            .elements
            .iter()
            .find(|element| element.id == "feature.Demo.TrafficLightGo.currentColor")
            .unwrap();
        assert_eq!(
            current_color.properties.get("is_derived"),
            Some(&serde_json::Value::Bool(false))
        );
        assert!(current_color.properties.contains_key("expression_ir"));
    }

    #[test]
    fn resolver_rejects_unresolved_expression_path_segments() {
        let module = parse_sysml(
            "package Demo { attribute def Mass; part def Engine { attribute mass: Mass; } part vehicle { part parts: Engine; derived attribute totalMass = sum(self.parts.missing); } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["Mass", "SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let error = resolve_module(&module, &stdlib, &mappings).unwrap_err();

        assert!(
            error
                .message
                .contains("unresolved expression path segment `missing`")
        );
    }

    #[test]
    fn partial_compile_skips_invalid_usage_and_preserves_valid_siblings() {
        let module = parse_sysml(
            "package Demo { part def Good; part vehicle { part good: Good; part bad: Missing; } }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);

        let report = compile_sysml_module_with_context_report(
            &module,
            "inline.sysml",
            &[module.clone()],
            &stdlib,
        );

        assert_eq!(report.status, SemanticCompileStatus::Partial);
        assert!(
            report
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("unresolved type `Missing`"))
        );
        let document = report.document.unwrap();
        assert!(
            document
                .elements
                .iter()
                .any(|element| element.id == "feature.Demo.vehicle.good")
        );
        assert!(
            !document
                .elements
                .iter()
                .any(|element| element.id == "feature.Demo.vehicle.bad")
        );
    }

    #[test]
    fn partial_compile_recovers_parse_errors_and_preserves_valid_siblings() {
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let report = compile_sysml_text_with_context_report(
            "package Demo { part def Good; #servicedd :>> serviceDiscovery:ServiceDiscoveryDD { #idd serviceDiscovery_HTTP; } part def AlsoGood; }",
            "inline.sysml",
            &[],
            &stdlib,
        );

        assert_eq!(report.status, SemanticCompileStatus::Partial);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("unresolved type `ServiceDiscoveryDD`")
        }));
        let document = report.document.unwrap();
        assert!(
            document
                .elements
                .iter()
                .any(|element| element.id == "type.Demo.Good")
        );
        assert!(
            document
                .elements
                .iter()
                .any(|element| element.id == "type.Demo.AlsoGood")
        );
    }

    #[test]
    fn semantic_compile_resolves_same_package_type_references() {
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let report = compile_sysml_text_with_context_report(
            "package UavModel { part def BatteryPack; part flightBattery: BatteryPack; }",
            "inline.sysml",
            &[],
            &stdlib,
        );

        assert_eq!(report.status, SemanticCompileStatus::Ok);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn semantic_compile_allows_usage_typed_part_collections() {
        let stdlib = KirDocument::from_path(&crate::paths::default_stdlib_path()).unwrap();
        let report = compile_sysml_text_with_context_report(
            "package ConstraintSmokeTest {
                constraint def MassBalance {
                    in totalMass;
                    in dryMass;
                    in fuelMass;
                    in payloadMass;

                    totalMass == dryMass + fuelMass + payloadMass
                }

                constraint def MaxMassCheck {
                    in totalMass;
                    in maxMass;

                    totalMass <= maxMass
                }

                part testVehicle {
                    attribute dryMass = 900;
                    attribute fuelMass = 120;
                    attribute payloadMass = 180;
                    attribute maxMass = 1250;
                    attribute totalMass = dryMass + fuelMass + payloadMass;
                    attribute grossWeight = totalMass * 9.81;

                    assert constraint massBalance : MassBalance {
                        in totalMass = totalMass;
                        in dryMass = dryMass;
                        in fuelMass = fuelMass;
                        in payloadMass = payloadMass;
                    }

                    assert constraint maxMassCheck : MaxMassCheck {
                        in totalMass = totalMass;
                        in maxMass = maxMass;
                    }
                }

                part compositeVehicle {
                    part components : testVehicle[*];
                    attribute totalMass default 100 + sum(components.totalMass);
                }
            }",
            "inline.sysml",
            &[],
            &stdlib,
        );

        assert_eq!(report.status, SemanticCompileStatus::Ok);
        assert!(report.diagnostics.is_empty());
        let document = report.document.unwrap();
        assert!(document.elements.iter().any(|element| {
            element.id == "feature.ConstraintSmokeTest.compositeVehicle.components"
        }));
    }

    #[test]
    fn semantic_compile_resolves_wildcard_imported_package_types() {
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let report = compile_sysml_text_with_context_report(
            "package UavLibrary { part def BatteryPack; part def FlightComputer; } package UavSystem { import UavLibrary::*; part flightBattery: BatteryPack; part controller: FlightComputer; }",
            "inline.sysml",
            &[],
            &stdlib,
        );

        assert_eq!(report.status, SemanticCompileStatus::Ok);
        assert!(report.diagnostics.is_empty());
    }

    #[test]
    fn semantic_compile_does_not_resolve_sibling_package_types_without_import() {
        let stdlib = fake_stdlib(["SysML::Systems::PartDefinition"]);
        let report = compile_sysml_text_with_context_report(
            "package UavLibrary2 { part def BatteryPack; part def FlightComputer; } package UavSystem { import UavLibrary::*; part flightBattery: BatteryPack; part controller: FlightComputer; }",
            "inline.sysml",
            &[],
            &stdlib,
        );

        assert_ne!(report.status, SemanticCompileStatus::Ok);
        assert!(report.diagnostics.iter().any(|diagnostic| {
            diagnostic
                .message
                .contains("unresolved import `UavLibrary::*`")
                || diagnostic.message.contains("unresolved type `BatteryPack`")
                || diagnostic
                    .message
                    .contains("unresolved type `FlightComputer`")
        }));
    }

    #[test]
    fn fixture_sysml_expression_compiles_and_evaluates_end_to_end() {
        let document = load_model_stack(&crate::paths::repo_path(
            "fixtures/l2/expression_fixture.sysml",
        ))
        .unwrap();
        let runtime = Runtime::from_document(document).unwrap();
        let context = crate::runtime::ExecutionContext::default();

        let result = runtime
            .evaluate(
                "feature.ExprDemo.Vehicle.arithmeticCheck",
                "type.ExprDemo.Vehicle",
                &context,
            )
            .unwrap();
        assert_eq!(result.value, serde_json::Value::Bool(true));
    }

    #[test]
    fn evaluates_derived_sum_over_typed_part_default_attributes() {
        let module = parse_sysml(
            "package EvalDemo {
                attribute def Mass;
                part def Engine {
                    attribute mass : Mass = 4.0;
                }
                part def Vehicle {
                    part leftEngine : Engine;
                    part rightEngine : Engine;
                    derived attribute totalMass = sum(self.leftEngine.mass) + sum(self.rightEngine.mass);
                }
            }",
        )
        .unwrap();
        let stdlib = fake_stdlib(["Mass", "SysML::Systems::PartDefinition"]);
        let mappings = MappingBundle::load().unwrap();
        let resolved = resolve_module(&module, &stdlib, &mappings).unwrap();
        let kir = transpile_module(&resolved, "inline.sysml", &mappings).unwrap();
        let runtime = Runtime::from_document(kir).unwrap();

        let result = runtime
            .evaluate(
                "feature.EvalDemo.Vehicle.totalMass",
                "type.EvalDemo.Vehicle",
                &crate::runtime::ExecutionContext::default(),
            )
            .unwrap();

        assert_eq!(result.value, serde_json::Value::from(8.0));
    }

    #[test]
    fn parses_event_occurrence_as_occurrence_usage_with_declared_name() {
        let module =
            parse_sysml("package Demo { part def Sequence { event occurrence publish; } }")
                .unwrap();
        let package = module.package.unwrap();
        let definition = match &package.members[0] {
            Declaration::PartDefinition(definition) => definition,
            other => panic!("expected part definition, got {other:?}"),
        };
        let usage = match &definition.members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected occurrence usage, got {other:?}"),
        };

        assert_eq!(usage.keyword, "occurrence");
        assert_eq!(usage.name, "publish");
        assert!(!usage.is_implicit_name);
    }

    #[test]
    fn parses_include_use_case_as_use_case_usage() {
        let module = parse_sysml(
            "package Demo { use case def UC1; use case def Main { include use case uc1 : UC1; } }",
        )
        .unwrap();
        let package = module.package.unwrap();
        let definition = match &package.members[1] {
            Declaration::GenericDefinition(definition) => definition,
            other => panic!("expected use case definition, got {other:?}"),
        };
        let usage = match &definition.members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected use case usage, got {other:?}"),
        };

        assert_eq!(usage.keyword, "use-case");
        assert_eq!(usage.name, "uc1");
        assert!(!usage.is_implicit_name);
    }

    #[test]
    fn parses_keyword_usage_with_anonymous_redefinition_tail() {
        let module =
            parse_sysml("package Demo { action def Behavior { calc :>> getDerivative; } }")
                .unwrap();
        let package = module.package.unwrap();
        let definition = match &package.members[0] {
            Declaration::GenericDefinition(definition) => definition,
            other => panic!("expected action definition, got {other:?}"),
        };
        let usage = match &definition.members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected calculation usage, got {other:?}"),
        };

        assert_eq!(usage.keyword, "calc");
        assert_eq!(usage.name, "getDerivative");
        assert!(usage.is_implicit_name);
    }

    #[test]
    fn parses_end_item_usage_with_declared_name() {
        let module =
            parse_sysml("package Demo { connection def C { end [0..1] item cart: Cart[1]; } }")
                .unwrap();
        let package = module.package.unwrap();
        let definition = match &package.members[0] {
            Declaration::GenericDefinition(definition) => definition,
            other => panic!("expected connection definition, got {other:?}"),
        };
        let usage = match &definition.members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected item usage, got {other:?}"),
        };

        assert_eq!(usage.keyword, "item");
        assert_eq!(usage.name, "cart");
        assert!(usage.modifiers.contains(&"end".to_string()));
    }

    #[test]
    fn parses_end_port_usage_with_declared_name() {
        let module = parse_sysml("package Demo { connection def C { end port p1: P; } }").unwrap();
        let package = module.package.unwrap();
        let definition = match &package.members[0] {
            Declaration::GenericDefinition(definition) => definition,
            other => panic!("expected connection definition, got {other:?}"),
        };
        let usage = match &definition.members[0] {
            Declaration::GenericUsage(usage) => usage,
            other => panic!("expected port usage, got {other:?}"),
        };

        assert_eq!(usage.keyword, "port");
        assert_eq!(usage.name, "p1");
        assert!(usage.modifiers.contains(&"end".to_string()));
    }

    fn fake_stdlib<const N: usize>(ids: [&str; N]) -> KirDocument {
        let default_ids = [
            "Actions::Action",
            "Base::DataValue",
            "BinaryConnection",
            "Items::Item",
            "Parts::Part",
            "Ports::Port",
        ];
        KirDocument {
            metadata: BTreeMap::new(),
            elements: default_ids
                .into_iter()
                .chain(ids)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|id| crate::ir::KirElement {
                    id: id.to_string(),
                    kind: id.to_string(),
                    layer: 1,
                    properties: BTreeMap::new(),
                })
                .collect(),
        }
    }

    fn find_resolved_usage<'a>(
        usages: &'a [crate::frontend::resolver::ResolvedUsage],
        qualified_name: &str,
    ) -> Option<&'a crate::frontend::resolver::ResolvedUsage> {
        usages.iter().find_map(|usage| {
            if usage.qualified_name == qualified_name {
                Some(usage)
            } else {
                find_resolved_usage(&usage.members, qualified_name)
            }
        })
    }
}
