package dev.mercurio.pilot;

import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;
import java.lang.reflect.Method;
import java.time.Instant;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.Collection;
import java.util.Comparator;
import java.util.Deque;
import java.util.IdentityHashMap;
import java.util.LinkedHashMap;
import java.util.LinkedHashSet;
import java.util.List;
import java.util.Locale;
import java.util.Map;
import java.util.Objects;
import java.util.Set;
import java.util.TreeSet;
import java.util.stream.Collectors;

import org.eclipse.emf.common.util.TreeIterator;
import org.eclipse.emf.ecore.EClass;
import org.eclipse.emf.ecore.EReference;
import org.eclipse.emf.ecore.EObject;
import org.eclipse.emf.ecore.resource.Resource;
import org.eclipse.emf.ecore.resource.Resource.Diagnostic;
import org.eclipse.emf.ecore.resource.ResourceSet;
import org.eclipse.xtext.EcoreUtil2;
import org.eclipse.xtext.nodemodel.ICompositeNode;
import org.eclipse.xtext.nodemodel.util.NodeModelUtils;
import org.omg.sysml.interactive.SysMLInteractive;
import org.omg.sysml.lang.sysml.Documentation;
import org.omg.sysml.lang.sysml.Element;
import org.omg.sysml.lang.sysml.Feature;
import org.omg.sysml.lang.sysml.FeatureTyping;
import org.omg.sysml.lang.sysml.Namespace;
import org.omg.sysml.lang.sysml.Relationship;
import org.omg.sysml.lang.sysml.Specialization;
import org.omg.sysml.lang.sysml.Type;
import org.omg.sysml.util.ElementUtil;

import com.google.gson.GsonBuilder;
import com.google.gson.Gson;

public final class PilotModelExporter {
    private static final String KERNEL_LIBRARIES = "Kernel Libraries";
    private static final String SYSTEMS_LIBRARY = "Systems Library";
    private static final String DOMAIN_LIBRARIES = "Domain Libraries";
    private static final Gson JSON = new GsonBuilder().disableHtmlEscaping().setPrettyPrinting().create();

    private PilotModelExporter() {
    }

    public static void main(String[] args) throws Exception {
        if (args.length >= 1 && "--syntax".equals(args[0])) {
            exportSyntax(args);
            return;
        }
        if (args.length >= 1 && "--diagnostics".equals(args[0])) {
            exportDiagnostics(args);
            return;
        }
        if (args.length >= 1 && "--diagnostics-batch".equals(args[0])) {
            exportDiagnosticsBatch(args);
            return;
        }
        if (args.length >= 1 && "--batch-spec".equals(args[0])) {
            exportBatch(args);
            return;
        }

        if (args.length < 3) {
            System.err.println(
                "Usage: PilotModelExporter <library-root> <output-json> <model-file> [support-file ...]\n"
                + "   or: PilotModelExporter --syntax <library-root> <output-json> <model-file> [support-file ...]\n"
                + "   or: PilotModelExporter --diagnostics <library-root> <output-json> <model-file> [support-file ...]\n"
                + "   or: PilotModelExporter --diagnostics-batch <library-root> <spec-json> <output-json>\n"
                + "   or: PilotModelExporter --batch-spec <library-root> <spec-json> <output-json>"
            );
            System.exit(2);
        }

        Path libraryRoot = Paths.get(args[0]).toAbsolutePath().normalize();
        Path outputPath = Paths.get(args[1]).toAbsolutePath().normalize();
        List<Path> inputFiles = new ArrayList<>();
        for (int i = 2; i < args.length; i += 1) {
            inputFiles.add(Paths.get(args[i]).toAbsolutePath().normalize());
        }

        System.setProperty("org.eclipse.emf.common.util.ReferenceClearingQueue", "false");

        SysMLInteractive interactive = SysMLInteractive.getInstance();
        interactive.getLibraryIndexCache().setIndexDisabled(true);
        interactive.loadLibrary(libraryRoot.toString());
        interactive.setVerbose(false);

        List<Resource> inputResources = new ArrayList<>();
        for (Path inputFile : inputFiles) {
            Resource resource = interactive.readResource(inputFile.toString());
            interactive.addInputResource(resource);
            inputResources.add(resource);
        }

        ResourceSet resourceSet = interactive.getResourceSet();
        resourceSet.getResources().forEach(resource -> EcoreUtil2.resolveLazyCrossReferences(resource, null));
        interactive.resolveAllInputResources();
        ElementUtil.transformAll(resourceSet, true);
        resourceSet.getResources().forEach(resource -> EcoreUtil2.resolveLazyCrossReferences(resource, null));

        ExportDocument document = exportDocument(libraryRoot, inputResources, resourceSet);
        if (outputPath.getParent() != null) {
            Files.createDirectories(outputPath.getParent());
        }
        Files.writeString(
            outputPath,
            JSON.toJson(document),
            StandardCharsets.UTF_8
        );
    }

    private static void exportSyntax(String[] args) throws Exception {
        if (args.length < 4) {
            System.err.println(
                "Usage: PilotModelExporter --syntax <library-root> <output-json> <model-file> [support-file ...]"
            );
            System.exit(2);
        }

        Path libraryRoot = Paths.get(args[1]).toAbsolutePath().normalize();
        Path outputPath = Paths.get(args[2]).toAbsolutePath().normalize();
        List<Path> inputFiles = new ArrayList<>();
        for (int i = 3; i < args.length; i += 1) {
            inputFiles.add(Paths.get(args[i]).toAbsolutePath().normalize());
        }

        System.setProperty("org.eclipse.emf.common.util.ReferenceClearingQueue", "false");

        SysMLInteractive interactive = SysMLInteractive.getInstance();
        interactive.getLibraryIndexCache().setIndexDisabled(true);
        interactive.loadLibrary(libraryRoot.toString());
        interactive.setVerbose(false);

        List<Resource> inputResources = new ArrayList<>();
        for (Path inputFile : inputFiles) {
            Resource resource = interactive.readResource(inputFile.toString());
            interactive.addInputResource(resource);
            inputResources.add(resource);
        }

        SyntaxSnapshotDocument snapshot = exportSyntaxDocument(libraryRoot, inputResources);
        writeJson(outputPath, snapshot);
    }

    private static void exportDiagnostics(String[] args) throws Exception {
        if (args.length < 4) {
            System.err.println(
                "Usage: PilotModelExporter --diagnostics <library-root> <output-json> <model-file> [support-file ...]"
            );
            System.exit(2);
        }

        Path libraryRoot = Paths.get(args[1]).toAbsolutePath().normalize();
        Path outputPath = Paths.get(args[2]).toAbsolutePath().normalize();
        List<Path> inputFiles = new ArrayList<>();
        for (int i = 3; i < args.length; i += 1) {
            inputFiles.add(Paths.get(args[i]).toAbsolutePath().normalize());
        }

        DiagnosticRunDocument document = collectDiagnostics(libraryRoot, inputFiles);
        writeJson(outputPath, document);
    }

    private static void exportDiagnosticsBatch(String[] args) throws Exception {
        if (args.length != 4) {
            System.err.println(
                "Usage: PilotModelExporter --diagnostics-batch <library-root> <spec-json> <output-json>"
            );
            System.exit(2);
        }

        Path libraryRoot = Paths.get(args[1]).toAbsolutePath().normalize();
        Path specPath = Paths.get(args[2]).toAbsolutePath().normalize();
        Path outputPath = Paths.get(args[3]).toAbsolutePath().normalize();
        BatchSpec spec = new Gson().fromJson(Files.readString(specPath, StandardCharsets.UTF_8), BatchSpec.class);
        if (spec == null || spec.cases == null) {
            throw new IllegalArgumentException("batch spec must contain cases");
        }

        BatchDiagnosticsDocument document = new BatchDiagnosticsDocument();
        BatchDiagnosticsMetadata metadata = new BatchDiagnosticsMetadata();
        metadata.library_root = libraryRoot.toString();
        metadata.exported_at_utc = Instant.now().toString();
        metadata.pilot_version = pilotVersion();
        metadata.case_count = spec.cases.size();
        document.metadata = metadata;
        document.cases = new ArrayList<>();

        System.setProperty("org.eclipse.emf.common.util.ReferenceClearingQueue", "false");

        Instant setupStart = Instant.now();
        SysMLInteractive interactive = SysMLInteractive.getInstance();
        interactive.getLibraryIndexCache().setIndexDisabled(true);
        interactive.setVerbose(false);

        Instant loadLibraryStart = Instant.now();
        interactive.loadLibrary(libraryRoot.toString());
        long loadLibraryMs = elapsedMillis(loadLibraryStart, Instant.now());

        Map<Path, Resource> resourcesByPath = new LinkedHashMap<>();
        Instant loadInputsStart = Instant.now();
        for (BatchSpecCase batchCase : spec.cases) {
            for (String inputFile : batchCase.input_files) {
                Path normalizedPath = Paths.get(inputFile).toAbsolutePath().normalize();
                if (resourcesByPath.containsKey(normalizedPath)) {
                    continue;
                }
                Resource resource = interactive.readResource(normalizedPath.toString());
                interactive.addInputResource(resource);
                resourcesByPath.put(normalizedPath, resource);
            }
        }
        long loadInputsMs = elapsedMillis(loadInputsStart, Instant.now());

        String failureStage = null;
        String exceptionType = null;
        String exceptionMessage = null;
        Instant resolveStart = Instant.now();
        try {
            ResourceSet resourceSet = interactive.getResourceSet();
            resourceSet.getResources().forEach(resource -> EcoreUtil2.resolveLazyCrossReferences(resource, null));
            interactive.resolveAllInputResources();
            ElementUtil.transformAll(resourceSet, true);
            resourceSet.getResources().forEach(resource -> EcoreUtil2.resolveLazyCrossReferences(resource, null));
        } catch (Exception ex) {
            failureStage = "resolve_transform";
            exceptionType = ex.getClass().getName();
            exceptionMessage = clean(ex.getMessage()) == null ? ex.toString() : clean(ex.getMessage());
        }
        long resolveMs = elapsedMillis(resolveStart, Instant.now());
        long setupTotalMs = elapsedMillis(setupStart, Instant.now());

        for (BatchSpecCase batchCase : spec.cases) {
            List<Path> inputFiles = batchCase.input_files.stream()
                .map(path -> Paths.get(path).toAbsolutePath().normalize())
                .toList();
            Path targetFile = inputFiles.isEmpty() ? null : inputFiles.get(inputFiles.size() - 1);
            document.cases.add(new BatchDiagnosticsCase(
                batchCase.relative_path,
                collectDiagnosticsFromLoadedResources(
                    libraryRoot,
                    inputFiles,
                    targetFile,
                    resourcesByPath,
                    setupTotalMs,
                    List.of(
                        new DiagnosticPhaseTiming("load_library", loadLibraryMs),
                        new DiagnosticPhaseTiming("load_unique_inputs", loadInputsMs),
                        new DiagnosticPhaseTiming("resolve_transform", resolveMs)
                    ),
                    failureStage,
                    exceptionType,
                    exceptionMessage
                )
            ));
        }

        writeJson(outputPath, document);
    }

    private static void exportBatch(String[] args) throws Exception {
        if (args.length != 4) {
            System.err.println(
                "Usage: PilotModelExporter --batch-spec <library-root> <spec-json> <output-json>"
            );
            System.exit(2);
        }

        Path libraryRoot = Paths.get(args[1]).toAbsolutePath().normalize();
        Path specPath = Paths.get(args[2]).toAbsolutePath().normalize();
        Path outputPath = Paths.get(args[3]).toAbsolutePath().normalize();
        BatchSpec spec = new Gson().fromJson(Files.readString(specPath, StandardCharsets.UTF_8), BatchSpec.class);
        if (spec == null || spec.cases == null) {
            throw new IllegalArgumentException("batch spec must contain cases");
        }

        System.setProperty("org.eclipse.emf.common.util.ReferenceClearingQueue", "false");

        Instant setupStart = Instant.now();
        SysMLInteractive interactive = SysMLInteractive.getInstance();
        interactive.getLibraryIndexCache().setIndexDisabled(true);

        Instant loadLibraryStart = Instant.now();
        interactive.loadLibrary(libraryRoot.toString());
        long loadLibraryMs = elapsedMillis(loadLibraryStart, Instant.now());
        interactive.setVerbose(false);

        Map<Path, Resource> resourcesByPath = new LinkedHashMap<>();
        Instant loadInputsStart = Instant.now();
        for (BatchSpecCase batchCase : spec.cases) {
            for (String inputFile : batchCase.input_files) {
                Path normalizedPath = Paths.get(inputFile).toAbsolutePath().normalize();
                if (resourcesByPath.containsKey(normalizedPath)) {
                    continue;
                }
                Resource resource = interactive.readResource(normalizedPath.toString());
                interactive.addInputResource(resource);
                resourcesByPath.put(normalizedPath, resource);
            }
        }
        long loadInputsMs = elapsedMillis(loadInputsStart, Instant.now());

        Instant resolveStart = Instant.now();
        ResourceSet resourceSet = interactive.getResourceSet();
        resourceSet.getResources().forEach(resource -> EcoreUtil2.resolveLazyCrossReferences(resource, null));
        interactive.resolveAllInputResources();
        ElementUtil.transformAll(resourceSet, true);
        resourceSet.getResources().forEach(resource -> EcoreUtil2.resolveLazyCrossReferences(resource, null));
        long resolveInputsMs = elapsedMillis(resolveStart, Instant.now());
        long setupTotalMs = elapsedMillis(setupStart, Instant.now());

        List<BatchExportCase> exportCases = new ArrayList<>();
        for (BatchSpecCase batchCase : spec.cases) {
            List<Resource> inputResources = new ArrayList<>();
            for (String inputFile : batchCase.input_files) {
                Path normalizedPath = Paths.get(inputFile).toAbsolutePath().normalize();
                Resource resource = resourcesByPath.get(normalizedPath);
                if (resource == null) {
                    throw new IllegalStateException("missing loaded resource for " + normalizedPath);
                }
                inputResources.add(resource);
            }

            Instant caseExportStart = Instant.now();
            ExportDocument document = exportDocument(libraryRoot, inputResources, resourceSet);
            long exportMs = elapsedMillis(caseExportStart, Instant.now());
            exportCases.add(new BatchExportCase(batchCase.relative_path, exportMs, document));
        }

        BatchExportMetadata metadata = new BatchExportMetadata();
        metadata.library_root = libraryRoot.toString();
        metadata.exported_at_utc = Instant.now().toString();
        metadata.pilot_version = pilotVersion();
        metadata.case_count = exportCases.size();
        metadata.unique_input_file_count = resourcesByPath.size();
        metadata.setup_timings = new BatchSetupTimings(
            setupTotalMs,
            List.of(
                new BatchPhaseTiming("load_library", loadLibraryMs),
                new BatchPhaseTiming("load_unique_inputs", loadInputsMs),
                new BatchPhaseTiming("resolve_and_transform", resolveInputsMs)
            )
        );

        BatchExportDocument document = new BatchExportDocument();
        document.metadata = metadata;
        document.cases = exportCases;

        if (outputPath.getParent() != null) {
            Files.createDirectories(outputPath.getParent());
        }
        Files.writeString(
            outputPath,
            JSON.toJson(document),
            StandardCharsets.UTF_8
        );
    }

    private static DiagnosticRunDocument collectDiagnostics(Path libraryRoot, List<Path> inputFiles) {
        Instant totalStart = Instant.now();
        Path repoRoot = libraryRoot.getParent();

        DiagnosticRunDocument document = new DiagnosticRunDocument();
        DiagnosticRunMetadata metadata = new DiagnosticRunMetadata();
        metadata.library_root = libraryRoot.toString();
        metadata.input_files = inputFiles.stream().map(Path::toString).toList();
        metadata.exported_at_utc = Instant.now().toString();
        metadata.pilot_version = pilotVersion();
        document.metadata = metadata;
        document.status = "ok";
        document.diagnostics = new ArrayList<>();
        document.timings = new DiagnosticTimings();
        document.timings.phases = new ArrayList<>();

        Set<CompileDiagnosticKey> seenDiagnostics = new LinkedHashSet<>();
        String currentStage = "initialize";
        try {
            System.setProperty("org.eclipse.emf.common.util.ReferenceClearingQueue", "false");

            SysMLInteractive interactive = SysMLInteractive.getInstance();
            interactive.getLibraryIndexCache().setIndexDisabled(true);
            interactive.setVerbose(false);

            Instant phaseStart = Instant.now();
            currentStage = "load_library";
            interactive.loadLibrary(libraryRoot.toString());
            document.timings.phases.add(new DiagnosticPhaseTiming(
                currentStage,
                elapsedMillis(phaseStart, Instant.now())
            ));

            List<Resource> inputResources = new ArrayList<>();
            for (Path inputFile : inputFiles) {
                phaseStart = Instant.now();
                currentStage = "load_input";
                Resource resource = interactive.readResource(inputFile.toString());
                interactive.addInputResource(resource);
                inputResources.add(resource);
                collectResourceDiagnostics(
                    resource,
                    repoRoot,
                    currentStage,
                    document.diagnostics,
                    seenDiagnostics
                );
                document.timings.phases.add(new DiagnosticPhaseTiming(
                    currentStage,
                    elapsedMillis(phaseStart, Instant.now())
                ));
            }

            phaseStart = Instant.now();
            currentStage = "resolve_transform";
            ResourceSet resourceSet = interactive.getResourceSet();
            resourceSet.getResources().forEach(resource -> EcoreUtil2.resolveLazyCrossReferences(resource, null));
            interactive.resolveAllInputResources();
            ElementUtil.transformAll(resourceSet, true);
            resourceSet.getResources().forEach(resource -> EcoreUtil2.resolveLazyCrossReferences(resource, null));
            for (Resource resource : inputResources) {
                collectResourceDiagnostics(
                    resource,
                    repoRoot,
                    currentStage,
                    document.diagnostics,
                    seenDiagnostics
                );
            }
            document.timings.phases.add(new DiagnosticPhaseTiming(
                currentStage,
                elapsedMillis(phaseStart, Instant.now())
            ));
        } catch (Exception ex) {
            document.status = "error";
            document.failure_stage = currentStage;
            document.exception_type = ex.getClass().getName();
            document.exception_message = clean(ex.getMessage()) == null ? ex.toString() : clean(ex.getMessage());
            CompileDiagnosticDocument diagnostic = new CompileDiagnosticDocument(
                currentStage,
                null,
                null,
                null,
                document.exception_message,
                "exception"
            );
            if (seenDiagnostics.add(CompileDiagnosticKey.from(diagnostic))) {
                document.diagnostics.add(diagnostic);
            }
        }

        if (!document.diagnostics.isEmpty()) {
            document.status = "error";
            if (document.failure_stage == null) {
                document.failure_stage = firstFailureStage(document.diagnostics);
            }
        }

        document.timings.total_ms = elapsedMillis(totalStart, Instant.now());
        return document;
    }

    private static DiagnosticRunDocument collectDiagnosticsFromLoadedResources(
        Path libraryRoot,
        List<Path> inputFiles,
        Path targetFile,
        Map<Path, Resource> resourcesByPath,
        long totalMs,
        List<DiagnosticPhaseTiming> phases,
        String failureStage,
        String exceptionType,
        String exceptionMessage
    ) {
        Path repoRoot = libraryRoot.getParent();

        DiagnosticRunDocument document = new DiagnosticRunDocument();
        DiagnosticRunMetadata metadata = new DiagnosticRunMetadata();
        metadata.library_root = libraryRoot.toString();
        metadata.input_files = inputFiles.stream().map(Path::toString).toList();
        metadata.exported_at_utc = Instant.now().toString();
        metadata.pilot_version = pilotVersion();
        document.metadata = metadata;
        document.status = "ok";
        document.diagnostics = new ArrayList<>();
        document.timings = new DiagnosticTimings();
        document.timings.total_ms = totalMs;
        document.timings.phases = phases;

        Set<CompileDiagnosticKey> seenDiagnostics = new LinkedHashSet<>();
        Resource targetResource = targetFile == null ? null : resourcesByPath.get(targetFile);
        collectResourceDiagnostics(
            targetResource,
            repoRoot,
            "resolve_transform",
            document.diagnostics,
            seenDiagnostics
        );

        if (exceptionMessage != null) {
            document.status = "error";
            document.failure_stage = failureStage;
            document.exception_type = exceptionType;
            document.exception_message = exceptionMessage;
            CompileDiagnosticDocument diagnostic = new CompileDiagnosticDocument(
                failureStage,
                targetFile == null ? null : normalizeRelativePath(repoRoot.relativize(targetFile)),
                null,
                null,
                exceptionMessage,
                "exception"
            );
            if (seenDiagnostics.add(CompileDiagnosticKey.from(diagnostic))) {
                document.diagnostics.add(diagnostic);
            }
        }

        if (!document.diagnostics.isEmpty()) {
            document.status = "error";
            if (document.failure_stage == null) {
                document.failure_stage = firstFailureStage(document.diagnostics);
            }
        }

        return document;
    }

    private static void collectResourceDiagnostics(
        Resource resource,
        Path repoRoot,
        String stage,
        List<CompileDiagnosticDocument> diagnostics,
        Set<CompileDiagnosticKey> seenDiagnostics
    ) {
        if (resource == null) {
            return;
        }
        for (Diagnostic diagnostic : resource.getErrors()) {
            CompileDiagnosticDocument entry = new CompileDiagnosticDocument(
                stage,
                diagnosticPath(repoRoot, resource),
                diagnostic.getLine() > 0 ? diagnostic.getLine() : null,
                diagnostic.getColumn() > 0 ? diagnostic.getColumn() : null,
                clean(diagnostic.getMessage()),
                "error"
            );
            if (seenDiagnostics.add(CompileDiagnosticKey.from(entry))) {
                diagnostics.add(entry);
            }
        }
    }

    private static String diagnosticPath(Path repoRoot, Resource resource) {
        Path resourcePath = resourcePath(resource);
        if (resourcePath == null) {
            return null;
        }
        if (repoRoot != null && resourcePath.startsWith(repoRoot)) {
            return normalizeRelativePath(repoRoot.relativize(resourcePath));
        }
        return normalizeRelativePath(resourcePath);
    }

    private static String firstFailureStage(List<CompileDiagnosticDocument> diagnostics) {
        return diagnostics.isEmpty() ? null : diagnostics.get(0).stage;
    }

    private static void writeJson(Path outputPath, Object document) throws Exception {
        if (outputPath.getParent() != null) {
            Files.createDirectories(outputPath.getParent());
        }
        Files.writeString(outputPath, JSON.toJson(document), StandardCharsets.UTF_8);
    }

    private static ExportDocument exportDocument(
        Path libraryRoot,
        List<Resource> inputResources,
        ResourceSet resourceSet
    ) {
        Path repoRoot = libraryRoot.getParent();
        Map<String, Element> allByQualifiedName = new LinkedHashMap<>();
        Map<Element, String> elementIds = new IdentityHashMap<>();
        Set<String> usedIds = new LinkedHashSet<>();
        Set<RelationshipKey> allRelationships = new LinkedHashSet<>();
        Set<String> inputFiles = new TreeSet<>();

        for (Resource resource : resourceSet.getResources()) {
            TreeIterator<EObject> iterator = resource.getAllContents();
            while (iterator.hasNext()) {
                EObject object = iterator.next();
                if (!(object instanceof Element element)) {
                    continue;
                }

                String qualifiedName = identifierOf(element, repoRoot, libraryRoot, elementIds, usedIds);
                if (qualifiedName == null) {
                    continue;
                }

                allByQualifiedName.putIfAbsent(qualifiedName, element);
            }
        }

        for (Resource resource : resourceSet.getResources()) {
            TreeIterator<EObject> iterator = resource.getAllContents();
            while (iterator.hasNext()) {
                EObject object = iterator.next();
                if (!(object instanceof Element element)) {
                    continue;
                }

                String qualifiedName = elementIds.get(element);
                if (qualifiedName == null) {
                    continue;
                }

                collectRelationships(element, qualifiedName, elementIds, allRelationships);
            }
        }

        Set<String> seeds = new LinkedHashSet<>();
        for (Resource resource : inputResources) {
            Path resourcePath = resourcePath(resource);
            if (resourcePath == null) {
                continue;
            }
            inputFiles.add(normalizeRelativePath(repoRoot.relativize(resourcePath)));

            TreeIterator<EObject> iterator = resource.getAllContents();
            while (iterator.hasNext()) {
                EObject object = iterator.next();
                if (!(object instanceof Element element)) {
                    continue;
                }

                String qualifiedName = elementIds.get(element);
                if (qualifiedName != null) {
                    seeds.add(qualifiedName);
                }
            }
        }

        Set<String> includedIds = collectClosure(seeds, allRelationships);
        List<ExportElement> exportElements = includedIds.stream()
            .map(allByQualifiedName::get)
            .filter(Objects::nonNull)
            .map(element -> toExportElement(element, repoRoot, libraryRoot, elementIds))
            .sorted(Comparator.comparing(element -> element.qualified_name))
            .toList();

        Set<String> exportedIds = exportElements.stream()
            .map(element -> element.qualified_name)
            .collect(Collectors.toCollection(LinkedHashSet::new));
        List<ExportRelationship> exportRelationships = allRelationships.stream()
            .filter(relationship -> exportedIds.contains(relationship.source))
            .filter(relationship -> exportedIds.contains(relationship.target))
            .map(relationship -> new ExportRelationship(relationship.source, relationship.relation, relationship.target))
            .sorted(
                Comparator.comparing((ExportRelationship relationship) -> relationship.source)
                    .thenComparing(relationship -> relationship.relation)
                    .thenComparing(relationship -> relationship.target)
            )
            .toList();

        ExportMetadata metadata = new ExportMetadata();
        metadata.element_count = exportElements.size();
        metadata.relationship_count = exportRelationships.size();
        metadata.library_root = libraryRoot.toString();
        metadata.input_files = new ArrayList<>(inputFiles);
        metadata.exported_at_utc = Instant.now().toString();
        metadata.pilot_version = pilotVersion();

        ExportDocument document = new ExportDocument();
        document.metadata = metadata;
        document.elements = exportElements;
        document.relationships = exportRelationships;
        return document;
    }

    private static SyntaxSnapshotDocument exportSyntaxDocument(
        Path libraryRoot,
        List<Resource> inputResources
    ) {
        Path repoRoot = libraryRoot.getParent();
        SyntaxSnapshotDocument document = new SyntaxSnapshotDocument();
        document.root_kind = "XtextResourceSet";
        document.nodes = new ArrayList<>();

        for (Resource resource : inputResources) {
            Path resourcePath = resourcePath(resource);
            if (resourcePath == null) {
                continue;
            }
            String relativePath = repoRoot == null
                ? normalizeRelativePath(resourcePath)
                : normalizeRelativePath(repoRoot.relativize(resourcePath));
            int[] rootIndex = new int[] {0};
            for (EObject object : resource.getContents()) {
                collectSyntaxNodes(object, relativePath, "", rootIndex[0], document.nodes);
                rootIndex[0] += 1;
            }
        }

        document.nodes.sort(
            Comparator.comparing((SyntaxNodeDocument node) -> node.span.start_line)
                .thenComparing(node -> node.span.start_col)
                .thenComparing(node -> node.path)
        );
        return document;
    }

    private static void collectSyntaxNodes(
        EObject object,
        String relativePath,
        String parentPath,
        int siblingIndex,
        List<SyntaxNodeDocument> nodes
    ) {
        String path = parentPath.isEmpty() ? Integer.toString(siblingIndex) : parentPath + "." + siblingIndex;
        ICompositeNode node = NodeModelUtils.findActualNodeFor(object);
        if (shouldIncludeSyntaxNode(object, node)) {
            nodes.add(toSyntaxNode(object, relativePath, path, node));
        }

        int childIndex = 0;
        for (EObject child : object.eContents()) {
            collectSyntaxNodes(child, relativePath, path, childIndex, nodes);
            childIndex += 1;
        }
    }

    private static boolean shouldIncludeSyntaxNode(EObject object, ICompositeNode node) {
        if (object == null || node == null) {
            return false;
        }

        String kind = object.eClass().getName();
        if (kind == null || kind.equals("Documentation")) {
            return false;
        }

        String family = syntaxFamily(kind);
        return !family.equals("other");
    }

    private static SyntaxNodeDocument toSyntaxNode(
        EObject object,
        String relativePath,
        String path,
        ICompositeNode node
    ) {
        String kind = object.eClass().getName();
        Map<String, List<String>> properties = new LinkedHashMap<>();
        String declaredName = firstNonNull(
            clean(stringValue(invokeMethod(object, "getDeclaredName"))),
            clean(stringValue(invokeMethod(object, "getName")))
        );
        String typeValue = clean(stringValue(invokeMethod(object, "getType")));
        if (typeValue != null) {
            properties.put("type", List.of(typeValue));
        }
        String targetValue = clean(stringValue(invokeMethod(object, "getImportedNamespace")));
        if (targetValue != null) {
            properties.put("path", List.of(targetValue));
        }
        String text = clean(node.getText());
        if (text != null) {
            String firstLine = text.lines().findFirst().orElse(text).trim();
            if (!firstLine.isEmpty()) {
                properties.put("text", List.of(firstLine));
            }
        }

        SyntaxNodeDocument export = new SyntaxNodeDocument();
        export.path = path;
        export.family = syntaxFamily(kind);
        export.kind = kind;
        export.keyword = syntaxKeyword(kind);
        export.declared_name = declaredName;
        export.source_file = relativePath;
        export.span = new SyntaxSpanDocument(
            node.getStartLine(),
            columnAt(node, node.getOffset()),
            node.getEndLine(),
            columnAt(node, node.getOffset() + node.getLength())
        );
        export.properties = properties;
        return export;
    }

    private static String syntaxFamily(String kind) {
        if (kind.contains("Package")) {
            return "package";
        }
        if (kind.contains("Import")) {
            return "import";
        }
        if (kind.contains("Alias")) {
            return "alias";
        }
        if (kind.endsWith("Definition")) {
            return "definition";
        }
        if (kind.endsWith("Usage")) {
            return "usage";
        }
        return "other";
    }

    private static String syntaxKeyword(String kind) {
        if (kind.contains("Package")) {
            return "package";
        }
        if (kind.contains("Import")) {
            return "import";
        }
        if (kind.contains("Alias")) {
            return "alias";
        }
        String normalized = kind
            .replace("Definition", "")
            .replace("Usage", "")
            .replace("Membership", "")
            .replace("Feature", "")
            .replace("Occurrence", "occurrence");
        if (normalized.equals("UseCase")) {
            return "use-case";
        }
        return normalized.toLowerCase(Locale.ROOT);
    }

    private static int columnAt(ICompositeNode node, int absoluteOffset) {
        String text = node.getRootNode().getText();
        int boundedOffset = Math.max(0, Math.min(absoluteOffset, text.length()));
        int lastBreak = -1;
        for (int i = boundedOffset - 1; i >= 0; i -= 1) {
            char current = text.charAt(i);
            if (current == '\n' || current == '\r') {
                lastBreak = i;
                break;
            }
        }
        return boundedOffset - lastBreak;
    }

    private static Set<String> collectClosure(Set<String> seeds, Set<RelationshipKey> relationships) {
        Map<String, List<RelationshipKey>> bySource = new LinkedHashMap<>();
        for (RelationshipKey relationship : relationships) {
            bySource.computeIfAbsent(relationship.source, ignored -> new ArrayList<>()).add(relationship);
        }

        Set<String> included = new LinkedHashSet<>(seeds);
        Deque<String> queue = new ArrayDeque<>(seeds);

        while (!queue.isEmpty()) {
            String current = queue.removeFirst();
            for (RelationshipKey relationship : bySource.getOrDefault(current, List.of())) {
                if (included.add(relationship.target)) {
                    queue.addLast(relationship.target);
                }
            }
        }

        return included;
    }

    private static ExportElement toExportElement(
        Element element,
        Path repoRoot,
        Path libraryRoot,
        Map<Element, String> elementIds
    ) {
        Path resourcePath = resourcePath(element.eResource());
        String relativePath = resourcePath == null ? null : normalizeRelativePath(repoRoot.relativize(resourcePath));
        String libraryGroup = resourcePath != null && resourcePath.startsWith(libraryRoot)
            ? libraryGroup(normalizeRelativePath(libraryRoot.relativize(resourcePath)))
            : "Input Model";

        ExportElement export = new ExportElement();
        export.qualified_name = elementIds.get(element);
        export.kind = element.eClass().getName();
        export.library_group = libraryGroup;
        export.source = relativePath == null ? null : new ExportSource(relativePath, startLineOf(element), endLineOf(element));
        export.documentation = documentationOf(element);
        export.properties = propertiesOf(element);
        return export;
    }

    private static List<ExportDocumentationBlock> documentationOf(Element element) {
        List<ExportDocumentationBlock> docs = new ArrayList<>();
        for (Documentation documentation : element.getDocumentation()) {
            String body = clean(documentation.getBody());
            if (body != null) {
                docs.add(new ExportDocumentationBlock("comment", body));
            }
        }
        docs.sort(Comparator.comparing(block -> block.text));
        return docs;
    }

    private static Map<String, Object> propertiesOf(Element element) {
        Map<String, Object> properties = new LinkedHashMap<>();
        putIfPresent(properties, "declared_name", clean(element.getDeclaredName()));
        putIfPresent(properties, "declared_short_name", clean(element.getDeclaredShortName()));
        putIfPresent(properties, "name", clean(element.getName()));
        putIfPresent(properties, "short_name", clean(element.getShortName()));
        properties.put("is_library_element", element.isLibraryElement());
        properties.put(
            "metatype_specialization_chain",
            element.eClass().getEAllSuperTypes().stream().map(EClass::getName).toList()
        );

        if (element instanceof Feature feature) {
            properties.put("is_abstract", feature.isAbstract());
            properties.put("is_derived", feature.isDerived());
            properties.put("is_end", feature.isEnd());
            properties.put("is_ordered", feature.isOrdered());
            properties.put("is_unique", feature.isUnique());
            properties.put("is_variable", feature.isVariable());
            if (feature.getDirection() != null) {
                properties.put("direction", feature.getDirection().toString().toLowerCase(Locale.ROOT));
            }
        } else if (element instanceof Type type) {
            properties.put("is_abstract", type.isAbstract());
        } else if (element instanceof Relationship relationship) {
            properties.put("is_implied", relationship.isImplied());
        }

        properties.values().removeIf(Objects::isNull);
        return properties;
    }

    private static void collectRelationships(
        Element element,
        String sourceQualifiedName,
        Map<Element, String> elementIds,
        Set<RelationshipKey> relationships
    ) {
        addRelationship(relationships, sourceQualifiedName, "owner", identifierOf(element.getOwner(), elementIds));

        if (element instanceof Namespace namespace) {
            addRelationships(relationships, sourceQualifiedName, "members", namespace.getOwnedMember(), elementIds);
        }

        if (element instanceof Type type) {
            for (Specialization specialization : type.getOwnedSpecialization()) {
                addRelationship(
                    relationships,
                    sourceQualifiedName,
                    "specializes",
                    identifierOf(specialization.getGeneral(), elementIds)
                );
            }
            addRelationships(relationships, sourceQualifiedName, "features", type.getOwnedFeature(), elementIds);
        }

        if (element instanceof Feature feature) {
            addRelationships(relationships, sourceQualifiedName, "type", feature.getType(), elementIds);
            addRelationships(
                relationships,
                sourceQualifiedName,
                "featuring_type",
                feature.getFeaturingType(),
                elementIds
            );
            addRelationships(
                relationships,
                sourceQualifiedName,
                "chaining_feature",
                feature.getChainingFeature(),
                elementIds
            );
        }

        collectReflectiveRelationships(element, sourceQualifiedName, elementIds, relationships);
    }

    private static void collectReflectiveRelationships(
        Element element,
        String sourceQualifiedName,
        Map<Element, String> elementIds,
        Set<RelationshipKey> relationships
    ) {
        if (element instanceof Specialization specialization) {
            addRelationship(
                relationships,
                sourceQualifiedName,
                "general",
                identifierOf(specialization.getGeneral(), elementIds)
            );
        }

        collectDerivedMethodRelationships(element, sourceQualifiedName, elementIds, relationships);

        for (EReference reference : element.eClass().getEAllReferences()) {
            if (reference.isContainer() || !shouldCollectReflectiveReference(reference.getName())) {
                continue;
            }

            Object raw = element.eGet(reference, false);
            if (raw instanceof Element target) {
                addRelationship(
                    relationships,
                    sourceQualifiedName,
                    normalizeReferenceName(reference.getName()),
                    identifierOf(target, elementIds)
                );
            } else if (raw instanceof Collection<?> targets) {
                for (Object target : targets) {
                    if (target instanceof Element typedTarget) {
                        addRelationship(
                            relationships,
                            sourceQualifiedName,
                            normalizeReferenceName(reference.getName()),
                            identifierOf(typedTarget, elementIds)
                        );
                    }
                }
            }
        }
    }

    private static void collectDerivedMethodRelationships(
        Element element,
        String sourceQualifiedName,
        Map<Element, String> elementIds,
        Set<RelationshipKey> relationships
    ) {
        if (element instanceof FeatureTyping featureTyping) {
            addRelationship(
                relationships,
                sourceQualifiedName,
                "type",
                identifierOf(featureTyping.getType(), elementIds)
            );
        }
        addDerivedMethodRelationship(relationships, sourceQualifiedName, "definition", invokeMethod(element, "getDefinition"), elementIds);
        addDerivedMethodRelationship(
            relationships,
            sourceQualifiedName,
            "owning_feature_membership",
            invokeMethod(element, "getOwningFeatureMembership"),
            elementIds
        );
        addDerivedMethodRelationship(
            relationships,
            sourceQualifiedName,
            "owned_typing",
            invokeMethod(element, "getOwnedTyping"),
            elementIds
        );
        addDerivedMethodRelationship(
            relationships,
            sourceQualifiedName,
            "owned_subsetting",
            invokeMethod(element, "getOwnedSubsetting"),
            elementIds
        );
        addDerivedMethodRelationship(
            relationships,
            sourceQualifiedName,
            "owned_redefinition",
            invokeMethod(element, "getOwnedRedefinition"),
            elementIds
        );
    }

    private static Object invokeMethod(Object target, String methodName) {
        try {
            Method method = target.getClass().getMethod(methodName);
            return method.invoke(target);
        } catch (ReflectiveOperationException ex) {
            return null;
        }
    }

    private static String stringValue(Object value) {
        return value == null ? null : value.toString();
    }

    private static String firstNonNull(String first, String second) {
        return first != null ? first : second;
    }

    private static void addDerivedMethodRelationship(
        Set<RelationshipKey> relationships,
        String sourceQualifiedName,
        String relation,
        Object rawValue,
        Map<Element, String> elementIds
    ) {
        if (rawValue instanceof Element target) {
            addRelationship(relationships, sourceQualifiedName, relation, identifierOf(target, elementIds));
        } else if (rawValue instanceof Collection<?> targets) {
            for (Object target : targets) {
                if (target instanceof Element typedTarget) {
                    addRelationship(
                        relationships,
                        sourceQualifiedName,
                        relation,
                        identifierOf(typedTarget, elementIds)
                    );
                }
            }
        }
    }

    private static boolean shouldCollectReflectiveReference(String referenceName) {
        return switch (referenceName) {
            case "general",
                "specific",
                "source",
                "target",
                "typedFeature",
                "redefinedFeature",
                "redefiningFeature",
                "subsettedFeature",
                "subsettingFeature",
                "owningDefinition",
                "owningFeature",
                "owningFeatureMembership",
                "owningMembership",
                "owningNamespace",
                "owningType",
                "membershipOwningNamespace",
                "featureOfType",
                "owningFeatureOfType" -> true;
            default -> false;
        };
    }

    private static void addRelationships(
        Set<RelationshipKey> relationships,
        String source,
        String relation,
        Collection<? extends Element> targets,
        Map<Element, String> elementIds
    ) {
        for (Element target : targets) {
            addRelationship(relationships, source, relation, identifierOf(target, elementIds));
        }
    }

    private static void addRelationship(
        Set<RelationshipKey> relationships,
        String source,
        String relation,
        String target
    ) {
        if (source == null || target == null || source.equals(target)) {
            return;
        }
        relationships.add(new RelationshipKey(source, relation, target));
    }

    private static String identifierOf(Element element, Map<Element, String> elementIds) {
        return element == null ? null : elementIds.get(element);
    }

    private static String identifierOf(
        Element element,
        Path repoRoot,
        Path libraryRoot,
        Map<Element, String> elementIds,
        Set<String> usedIds
    ) {
        if (element == null) {
            return null;
        }

        String existing = elementIds.get(element);
        if (existing != null) {
            return existing;
        }

        String qualifiedName = clean(element.getQualifiedName());
        if (qualifiedName != null) {
            elementIds.put(element, qualifiedName);
            usedIds.add(qualifiedName);
            return qualifiedName;
        }

        Path resourcePath = resourcePath(element.eResource());
        if (resourcePath == null || resourcePath.startsWith(libraryRoot)) {
            return null;
        }

        String relativePath = normalizeRelativePath(repoRoot.relativize(resourcePath));
        Integer startLine = startLineOf(element);
        String label = clean(element.getDeclaredName());
        if (label == null) {
            label = clean(element.getName());
        }
        if (label == null) {
            label = element.eClass().getName();
        }

        String baseId = String.format(
            "InputModel::%s::%s::%s",
            relativePath,
            startLine == null ? "?" : startLine.toString(),
            label
        );
        String candidate = baseId;
        int counter = 2;
        while (usedIds.contains(candidate)) {
            candidate = baseId + "#" + counter;
            counter += 1;
        }

        elementIds.put(element, candidate);
        usedIds.add(candidate);
        return candidate;
    }

    private static Integer startLineOf(Element element) {
        ICompositeNode node = NodeModelUtils.findActualNodeFor(element);
        return node == null ? null : node.getStartLine();
    }

    private static Integer endLineOf(Element element) {
        ICompositeNode node = NodeModelUtils.findActualNodeFor(element);
        return node == null ? null : node.getEndLine();
    }

    private static Path resourcePath(Resource resource) {
        if (resource == null || resource.getURI() == null || !resource.getURI().isFile()) {
            return null;
        }
        return Paths.get(resource.getURI().toFileString()).toAbsolutePath().normalize();
    }

    private static String normalizeRelativePath(Path path) {
        return path.toString().replace('\\', '/');
    }

    private static String normalizeReferenceName(String name) {
        StringBuilder normalized = new StringBuilder();
        for (int i = 0; i < name.length(); i += 1) {
            char current = name.charAt(i);
            if (Character.isUpperCase(current)) {
                if (normalized.length() > 0) {
                    normalized.append('_');
                }
                normalized.append(Character.toLowerCase(current));
            } else {
                normalized.append(current);
            }
        }
        return normalized.toString();
    }

    private static String libraryGroup(String relativePath) {
        if (relativePath.startsWith(KERNEL_LIBRARIES.replace('\\', '/'))) {
            return KERNEL_LIBRARIES;
        }
        if (relativePath.startsWith(SYSTEMS_LIBRARY.replace('\\', '/'))) {
            return SYSTEMS_LIBRARY;
        }
        if (relativePath.startsWith(DOMAIN_LIBRARIES.replace('\\', '/'))) {
            return DOMAIN_LIBRARIES;
        }
        return null;
    }

    private static void putIfPresent(Map<String, Object> properties, String key, String value) {
        if (value != null) {
            properties.put(key, value);
        }
    }

    private static String clean(String value) {
        if (value == null) {
            return null;
        }
        String normalized = value.replace("\r\n", "\n").trim();
        return normalized.isEmpty() ? null : normalized;
    }

    private static String pilotVersion() {
        Package pkg = SysMLInteractive.class.getPackage();
        if (pkg == null) {
            return null;
        }

        String implementationVersion = pkg.getImplementationVersion();
        if (implementationVersion != null && !implementationVersion.isBlank()) {
            return implementationVersion;
        }

        String specificationVersion = pkg.getSpecificationVersion();
        if (specificationVersion != null && !specificationVersion.isBlank()) {
            return specificationVersion;
        }

        return null;
    }

    private static long elapsedMillis(Instant start, Instant end) {
        return java.time.Duration.between(start, end).toMillis();
    }

    private static final class RelationshipKey {
        private final String source;
        private final String relation;
        private final String target;

        private RelationshipKey(String source, String relation, String target) {
            this.source = source;
            this.relation = relation;
            this.target = target;
        }

        @Override
        public boolean equals(Object other) {
            if (this == other) {
                return true;
            }
            if (!(other instanceof RelationshipKey key)) {
                return false;
            }
            return source.equals(key.source) && relation.equals(key.relation) && target.equals(key.target);
        }

        @Override
        public int hashCode() {
            return Objects.hash(source, relation, target);
        }
    }

    private static final class CompileDiagnosticKey {
        private final String stage;
        private final String file;
        private final Integer line;
        private final Integer column;
        private final String message;
        private final String severity;

        private CompileDiagnosticKey(
            String stage,
            String file,
            Integer line,
            Integer column,
            String message,
            String severity
        ) {
            this.stage = stage;
            this.file = file;
            this.line = line;
            this.column = column;
            this.message = message;
            this.severity = severity;
        }

        private static CompileDiagnosticKey from(CompileDiagnosticDocument diagnostic) {
            return new CompileDiagnosticKey(
                diagnostic.stage,
                diagnostic.file,
                diagnostic.line,
                diagnostic.column,
                diagnostic.message,
                diagnostic.severity
            );
        }

        @Override
        public boolean equals(Object other) {
            if (this == other) {
                return true;
            }
            if (!(other instanceof CompileDiagnosticKey key)) {
                return false;
            }
            return Objects.equals(stage, key.stage)
                && Objects.equals(file, key.file)
                && Objects.equals(line, key.line)
                && Objects.equals(column, key.column)
                && Objects.equals(message, key.message)
                && Objects.equals(severity, key.severity);
        }

        @Override
        public int hashCode() {
            return Objects.hash(stage, file, line, column, message, severity);
        }
    }

    private static final class ExportDocument {
        private ExportMetadata metadata;
        private List<ExportElement> elements;
        private List<ExportRelationship> relationships;
    }

    private static final class ExportMetadata {
        private int element_count;
        private int relationship_count;
        private String library_root;
        private List<String> input_files;
        private String exported_at_utc;
        private String pilot_version;
    }

    private static final class ExportElement {
        private String qualified_name;
        private String kind;
        private String library_group;
        private ExportSource source;
        private List<ExportDocumentationBlock> documentation;
        private Map<String, Object> properties;
    }

    private static final class ExportSource {
        private final String file;
        private final Integer start_line;
        private final Integer end_line;

        private ExportSource(String file, Integer startLine, Integer endLine) {
            this.file = file;
            this.start_line = startLine;
            this.end_line = endLine;
        }
    }

    private static final class ExportDocumentationBlock {
        private final String kind;
        private final String text;

        private ExportDocumentationBlock(String kind, String text) {
            this.kind = kind;
            this.text = text;
        }
    }

    private static final class ExportRelationship {
        private final String source;
        private final String relation;
        private final String target;

        private ExportRelationship(String source, String relation, String target) {
            this.source = source;
            this.relation = relation;
            this.target = target;
        }
    }

    private static final class BatchSpec {
        private List<BatchSpecCase> cases;
    }

    private static final class BatchSpecCase {
        private String relative_path;
        private List<String> input_files;
    }

    private static final class BatchExportDocument {
        private BatchExportMetadata metadata;
        private List<BatchExportCase> cases;
    }

    private static final class BatchExportMetadata {
        private String library_root;
        private String exported_at_utc;
        private String pilot_version;
        private int case_count;
        private int unique_input_file_count;
        private BatchSetupTimings setup_timings;
    }

    private static final class BatchSetupTimings {
        private final long total_ms;
        private final List<BatchPhaseTiming> phases;

        private BatchSetupTimings(long totalMs, List<BatchPhaseTiming> phases) {
            this.total_ms = totalMs;
            this.phases = phases;
        }
    }

    private static final class BatchPhaseTiming {
        private final String name;
        private final long duration_ms;

        private BatchPhaseTiming(String name, long durationMs) {
            this.name = name;
            this.duration_ms = durationMs;
        }
    }

    private static final class BatchExportCase {
        private final String relative_path;
        private final long export_ms;
        private final ExportDocument document;

        private BatchExportCase(String relativePath, long exportMs, ExportDocument document) {
            this.relative_path = relativePath;
            this.export_ms = exportMs;
            this.document = document;
        }
    }

    private static final class DiagnosticRunDocument {
        private DiagnosticRunMetadata metadata;
        private String status;
        private String failure_stage;
        private String exception_type;
        private String exception_message;
        private List<CompileDiagnosticDocument> diagnostics;
        private DiagnosticTimings timings;
    }

    private static final class DiagnosticRunMetadata {
        private String library_root;
        private List<String> input_files;
        private String exported_at_utc;
        private String pilot_version;
    }

    private static final class DiagnosticTimings {
        private long total_ms;
        private List<DiagnosticPhaseTiming> phases;
    }

    private static final class DiagnosticPhaseTiming {
        private final String name;
        private final long duration_ms;

        private DiagnosticPhaseTiming(String name, long durationMs) {
            this.name = name;
            this.duration_ms = durationMs;
        }
    }

    private static final class CompileDiagnosticDocument {
        private final String stage;
        private final String file;
        private final Integer line;
        private final Integer column;
        private final String message;
        private final String severity;

        private CompileDiagnosticDocument(
            String stage,
            String file,
            Integer line,
            Integer column,
            String message,
            String severity
        ) {
            this.stage = stage;
            this.file = file;
            this.line = line;
            this.column = column;
            this.message = message;
            this.severity = severity;
        }
    }

    private static final class BatchDiagnosticsDocument {
        private BatchDiagnosticsMetadata metadata;
        private List<BatchDiagnosticsCase> cases;
    }

    private static final class BatchDiagnosticsMetadata {
        private String library_root;
        private String exported_at_utc;
        private String pilot_version;
        private int case_count;
    }

    private static final class BatchDiagnosticsCase {
        private final String relative_path;
        private final DiagnosticRunDocument result;

        private BatchDiagnosticsCase(String relativePath, DiagnosticRunDocument result) {
            this.relative_path = relativePath;
            this.result = result;
        }
    }

    private static final class SyntaxSnapshotDocument {
        private String root_kind;
        private List<SyntaxNodeDocument> nodes;
    }

    private static final class SyntaxNodeDocument {
        private String path;
        private String family;
        private String kind;
        private String keyword;
        private String declared_name;
        private String source_file;
        private SyntaxSpanDocument span;
        private Map<String, List<String>> properties;
    }

    private static final class SyntaxSpanDocument {
        private final int start_line;
        private final int start_col;
        private final int end_line;
        private final int end_col;

        private SyntaxSpanDocument(int startLine, int startCol, int endLine, int endCol) {
            this.start_line = startLine;
            this.start_col = startCol;
            this.end_line = endLine;
            this.end_col = endCol;
        }
    }
}
