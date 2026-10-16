// JDK compiler analysis over in-memory retained source. No repository classes,
// annotation processors, compiler plugins, packaging scripts or emit are run.
import com.sun.source.tree.*;
import com.sun.source.util.*;
import java.io.*;
import java.net.URI;
import java.nio.charset.StandardCharsets;
import java.nio.file.Path;
import java.util.*;
import javax.lang.model.element.*;
import javax.lang.model.type.TypeKind;
import javax.tools.*;

public final class JavaAdapter {
    private static final int MAX_FILES = 4096;
    private static final int MAX_DEFINITIONS = 4096;
    private static final int MAX_OCCURRENCES = 16384;
    private static final long MAX_INPUT_BYTES = 272L * 1024 * 1024;
    private final Map<String, String> files = new TreeMap<>();
    private final List<String> roots = new ArrayList<>();
    private final Map<String, List<Map<String, Object>>> rows = new LinkedHashMap<>();
    private final List<String> omitted = new ArrayList<>();
    private final Map<String, Source> sources = new TreeMap<>();
    private final Map<Tree, Definition> definitions = new IdentityHashMap<>();
    private final Map<Element, Definition> elements = new IdentityHashMap<>();
    private final Map<String, List<Definition>> types = new TreeMap<>();
    private final Set<String> errors = new TreeSet<>();
    private final Map<CompilationUnitTree, Source> units = new IdentityHashMap<>();
    private final Map<Source, List<Definition>> bySource = new IdentityHashMap<>();
    private int release;
    private boolean syntaxOnly;
    private boolean identityOnly;
    private boolean unsupportedProject;
    private int occurrences;
    private long inputBytes;
    private Trees trees;
    private SourcePositions positions;

    private static final class Source extends SimpleJavaFileObject {
        final String path;
        final String text;
        final int[] bytes;
        Source(String path, String text) throws Exception {
            super(new URI("briefcase", null, "/" + path, null), Kind.SOURCE);
            this.path = path;
            this.text = text;
            bytes = new int[text.length() + 1];
            int offset = 0;
            for (int i = 0; i < text.length();) {
                int code = text.codePointAt(i);
                int chars = Character.charCount(code);
                bytes[i] = offset;
                if (chars == 2) bytes[i + 1] = offset;
                offset += new String(Character.toChars(code)).getBytes(StandardCharsets.UTF_8).length;
                i += chars;
                bytes[i] = offset;
            }
        }
        @Override public CharSequence getCharContent(boolean ignore) { return text; }
    }
    private static final class Definition {
        final Tree node;
        final Source source;
        final String key;
        final String qualified;
        final boolean callable;
        Element element;
        Definition(Tree node, Source source, String key, String qualified, boolean callable) {
            this.node = node; this.source = source; this.key = key;
            this.qualified = qualified; this.callable = callable;
        }
    }
    private static Map<String, Object> row(Object... entries) {
        Map<String, Object> value = new TreeMap<>();
        for (int i = 0; i < entries.length; i += 2) value.put((String) entries[i], entries[i + 1]);
        return value;
    }
    private void add(String family, Map<String, Object> value) {
        if (family.equals("definitions") && rows.get(family).size() >= MAX_DEFINITIONS)
            throw new IllegalArgumentException("Java definition limit exceeded (4096)");
        if (List.of("imports", "references", "calls", "unresolved").contains(family)
                && ++occurrences > MAX_OCCURRENCES)
            throw new IllegalArgumentException("Java occurrence limit exceeded (16384)");
        rows.get(family).add(value);
    }
    private String string(DataInputStream input) throws IOException {
        int length = input.readInt();
        inputBytes += length;
        if (length < 0 || length > 10 * 1024 * 1024 || inputBytes > MAX_INPUT_BYTES)
            throw new IllegalArgumentException("Java input byte quota exceeded");
        byte[] bytes = input.readNBytes(length);
        if (bytes.length != length) throw new EOFException("Incomplete Java adapter input");
        return new String(bytes, StandardCharsets.UTF_8);
    }
    private static boolean within(String path, String root) {
        return root.equals(".") || path.startsWith(root + "/");
    }
    private void read() throws Exception {
        DataInputStream input = new DataInputStream(System.in);
        if (input.readInt() != 1) throw new IllegalArgumentException("Java input protocol version");
        int count = input.readInt();
        if (count < 0 || count > 65536) throw new IllegalArgumentException("Java input file quota exceeded");
        for (int i = 0; i < count; i++) {
            String path = string(input); String text = string(input);
            if (files.put(path, text) != null) throw new IllegalArgumentException("Duplicate Java input path");
        }
        int rootCount = input.readInt();
        if (rootCount < 0 || rootCount > 64) throw new IllegalArgumentException("Java source root quota exceeded");
        for (int i = 0; i < rootCount; i++) roots.add(string(input));
        release = input.readInt(); syntaxOnly = input.readBoolean(); identityOnly = input.readBoolean();
        if (release == 0) release = Runtime.version().feature();
        if (Runtime.version().feature() < 17 || release < 8 || release > Runtime.version().feature())
            throw new IllegalArgumentException("Java requires JDK 17+ and a supported --java-release");
        if (roots.isEmpty()) {
            for (String root : List.of("src/main/java", "src/test/java"))
                if (files.keySet().stream().anyMatch(p -> p.startsWith(root + "/"))) roots.add(root);
            if (roots.isEmpty()) roots.add(".");
        }
        Collections.sort(roots);
        Set<String> unique = new HashSet<>();
        for (String root : roots) {
            if (root.isEmpty() || root.startsWith("/") || root.contains("\\")
                    || (!root.equals(".") && !Path.of(root).normalize().toString().equals(root))
                    || root.equals("..") || root.startsWith("../") || !unique.add(root))
                throw new IllegalArgumentException("Java roots must be unique normalized relative paths");
        }
    }
    private Map<String, Object> span(CompilationUnitTree unit, Tree node) {
        long start = positions.getStartPosition(unit, node);
        long end = positions.getEndPosition(unit, node);
        Source source = units.get(unit);
        if (source == null || start < 0 || start >= end || end > source.text.length()) return null;
        return row("path", source.path, "start", source.bytes[(int) start], "end", source.bytes[(int) end]);
    }
    private void diagnostic(Source source, String message) {
        add("diagnostics", row("path", source == null ? null : source.path, "message", message));
    }
    private void unknown(CompilationUnitTree unit, Tree node, String predicate, String reason,
                         String caller, Collection<String> candidates) {
        Map<String, Object> value = span(unit, node);
        if (value == null) return;
        value.putAll(row("predicate", predicate, "reason", reason, "caller", caller,
                "status", candidates.isEmpty() ? "unresolved" : "ambiguous",
                "candidate_keys", new TreeSet<>(candidates)));
        add("unresolved", value);
    }
    private void site(CompilationUnitTree unit, Tree node, String family, String target, String caller) {
        Map<String, Object> value = span(unit, node);
        if (value == null) return;
        value.putAll(row("target", target, "caller", caller)); add(family, value);
    }
    private void declare(CompilationUnitTree unit, Tree node, String name, String qualified, boolean callable) {
        Map<String, Object> value = span(unit, node);
        if (value == null || name.isEmpty()) return;
        Source source = units.get(unit);
        String key = source.path + "::" + name + "#" + value.get("start");
        Definition def = new Definition(node, source, key, qualified, callable);
        definitions.put(node, def);
        bySource.computeIfAbsent(source, s -> new ArrayList<>()).add(def);
        value.putAll(row("key", key, "callable", callable)); add("definitions", value);
        if (node instanceof ClassTree && !qualified.isEmpty())
            types.computeIfAbsent(qualified, k -> new ArrayList<>()).add(def);
    }
    private void declarations(CompilationUnitTree unit) {
        new TreePathScanner<Void, String>() {
            @Override public Void visitClass(ClassTree node, String prefix) {
                String simple = node.getSimpleName().toString();
                if (simple.isEmpty()) return super.visitClass(node, prefix + "::anonymous");
                String name = prefix.isEmpty() ? simple : prefix + "::" + simple;
                String pkg = unit.getPackageName() == null ? "" : unit.getPackageName().toString();
                String qualified = (pkg.isEmpty() ? "" : pkg + ".") + name.replace("::", ".");
                declare(unit, node, name, qualified, false);
                return super.visitClass(node, name);
            }
            @Override public Void visitMethod(MethodTree node, String prefix) {
                String name = prefix + "::" + node.getName();
                declare(unit, node, name, "", node.getBody() != null
                        && !node.getModifiers().getFlags().contains(Modifier.NATIVE));
                return super.visitMethod(node, name);
            }
            @Override public Void visitTypeParameter(TypeParameterTree node, String prefix) {
                declare(unit, node, prefix + "::" + node.getName(), "", false);
                return super.visitTypeParameter(node, prefix);
            }
            @Override public Void visitVariable(VariableTree node, String prefix) {
                declare(unit, node, prefix + "::" + node.getName(), "", false);
                return super.visitVariable(node, prefix);
            }
            @Override public Void visitLambdaExpression(LambdaExpressionTree node, String prefix) {
                return super.visitLambdaExpression(node, prefix + "::lambda@" + positions.getStartPosition(unit, node));
            }
        }.scan(unit, "");
    }
    private boolean good(Definition def) {
        if (unsupportedProject || def == null || errors.contains(def.source.path)) return false;
        for (Element owner = def.element; owner != null; owner = owner.getEnclosingElement()) {
            Definition declaration = elements.get(owner);
            if (declaration != null && declaration.node instanceof ClassTree
                    && types.getOrDefault(declaration.qualified, List.of()).size() != 1) return false;
        }
        return true;
    }
    private boolean exact(ExecutableElement method, MethodInvocationTree call) {
        if (method.getKind() == ElementKind.CONSTRUCTOR) return true;
        Set<Modifier> modifiers = method.getModifiers();
        if (modifiers.contains(Modifier.STATIC) || modifiers.contains(Modifier.PRIVATE)
                || modifiers.contains(Modifier.FINAL)) return true;
        Element owner = method.getEnclosingElement();
        if (owner.getModifiers().contains(Modifier.FINAL)) return true;
        if (call.getMethodSelect() instanceof MemberSelectTree selected) {
            String receiver = selected.getExpression().toString();
            if (receiver.equals("super") || receiver.endsWith(".super")) return true;
        }
        return false;
    }
    private Definition target(Element element) {
        Definition def = elements.get(element);
        if (def != null && def.node instanceof ClassTree
                && types.getOrDefault(def.qualified, List.of()).size() != 1) return null;
        return def;
    }
    private void imports(CompilationUnitTree unit) {
        for (ImportTree imported : unit.getImports()) {
            String name = imported.getQualifiedIdentifier().toString();
            if (name.endsWith(".*")) {
                unknown(unit, imported, "IMPORTS", "unsupported_operation", null, List.of());
                continue;
            }
            Element element = syntaxOnly ? null : trees.getElement(TreePath.getPath(unit, imported.getQualifiedIdentifier()));
            Definition target = target(element);
            List<String> candidates = new ArrayList<>();
            if (imported.isStatic()) {
                int dot = name.lastIndexOf('.');
                if (dot > 0) {
                    List<Definition> owners = types.getOrDefault(name.substring(0, dot), List.of());
                    for (Definition owner : owners) {
                        for (Definition def : bySource.getOrDefault(owner.source, List.of()))
                            if (def.element != null && def.element.getEnclosingElement() == owner.element
                                    && def.element.getSimpleName().contentEquals(name.substring(dot + 1))) candidates.add(def.key);
                    }
                    // An overloaded static import names a member set. It does not
                    // select a callable. Dependency points at a unique owner module.
                    if (owners.size() == 1 && !syntaxOnly && good(owners.get(0)) && !errors.contains(units.get(unit).path)) {
                        site(unit, imported, "imports", owners.get(0).source.path, null);
                        continue;
                    }
                }
            } else {
                for (Definition def : types.getOrDefault(name, List.of())) candidates.add(def.key);
            }
            if (!syntaxOnly && good(target) && !errors.contains(units.get(unit).path)
                    && (candidates.isEmpty() || candidates.size() == 1)) {
                site(unit, imported, "imports", target.source.path, null);
            } else unknown(unit, imported, "IMPORTS", "missing_dependency", null,
                    candidates.size() > 1 ? candidates : List.of());
        }
    }
    private void referencesAndCalls(CompilationUnitTree unit) {
        new TreePathScanner<Void, String>() {
            @Override public Void visitImport(ImportTree node, String caller) { return null; }
            @Override public Void visitClass(ClassTree node, String caller) { return super.visitClass(node, null); }
            @Override public Void visitMethod(MethodTree node, String caller) {
                Definition def = definitions.get(node);
                // Parameter types, annotations and default values are syntax,
                // not execution inside a method body.
                scan(node.getModifiers(), null);
                scan(node.getTypeParameters(), null);
                scan(node.getReturnType(), null);
                scan(node.getParameters(), null);
                scan(node.getThrows(), null);
                scan(node.getDefaultValue(), null);
                if (node.getBody() != null) scan(node.getBody(), def != null && def.callable ? def.key : null);
                return null;
            }
            @Override public Void visitLambdaExpression(LambdaExpressionTree node, String caller) {
                unknown(unit, node, "CALLS_STATIC", "dynamic_property", caller, List.of());
                return super.visitLambdaExpression(node, null);
            }
            @Override public Void visitMemberReference(MemberReferenceTree node, String caller) {
                unknown(unit, node, "CALLS_STATIC", "dynamic_property", caller, List.of());
                return super.visitMemberReference(node, caller);
            }
            @Override public Void visitMethodInvocation(MethodInvocationTree node, String caller) {
                Element element = syntaxOnly ? null : trees.getElement(getCurrentPath());
                Definition def = target(element);
                if (element instanceof ExecutableElement method && caller != null
                        && good(def) && def.callable && !errors.contains(units.get(unit).path)
                        && exact(method, node)) {
                    site(unit, node, "calls", def.key, caller);
                } else unknown(unit, node, "CALLS_STATIC",
                        unsupportedProject ? "unsupported_context" : element instanceof ExecutableElement && def != null ? "trait_dispatch" : "missing_dependency",
                        caller, List.of());
                return super.visitMethodInvocation(node, caller);
            }
            @Override public Void visitNewClass(NewClassTree node, String caller) {
                Element element = syntaxOnly ? null : trees.getElement(getCurrentPath());
                Definition def = target(element);
                if (caller != null && good(def) && def.callable && node.getClassBody() == null
                        && !errors.contains(units.get(unit).path)) site(unit, node, "calls", def.key, caller);
                else unknown(unit, node, "CALLS_STATIC", "missing_dependency", caller, List.of());
                return super.visitNewClass(node, caller);
            }
            private void reference(Tree node, String caller) {
                if (syntaxOnly) return;
                Element element = trees.getElement(getCurrentPath());
                if (element == null || element.getKind() == ElementKind.PACKAGE
                        || element.getKind() == ElementKind.MODULE) return;
                Definition def = target(element);
                if (good(def) && !errors.contains(units.get(unit).path)) {
                    site(unit, node, "references", def.key, caller);
                    // Java can depend on another compilation unit without an
                    // import declaration (same-package or qualified use).
                    if (!def.source.path.equals(units.get(unit).path))
                        site(unit, node, "imports", def.source.path, caller);
                }
                else if (element.asType().getKind() != TypeKind.VOID) {
                    unknown(unit, node, "REFERENCES", "missing_dependency", caller, List.of());
                    if (element.getKind().isClass() || element.getKind().isInterface()
                            || element instanceof ExecutableElement || element.getKind() == ElementKind.FIELD)
                        unknown(unit, node, "IMPORTS", "missing_dependency", caller, List.of());
                }
            }
            @Override public Void visitIdentifier(IdentifierTree node, String caller) {
                if (!Set.of("this", "super").contains(node.getName().toString())) reference(node, caller);
                return super.visitIdentifier(node, caller);
            }
            @Override public Void visitMemberSelect(MemberSelectTree node, String caller) {
                reference(node, caller);
                return super.visitMemberSelect(node, caller);
            }
        }.scan(unit, null);
    }
    private void analyze() throws Exception {
        int count = 0;
        for (Map.Entry<String, String> file : files.entrySet()) {
            String path = file.getKey();
            if (!path.endsWith(".java")) continue;
            if (++count > MAX_FILES) throw new IllegalArgumentException("Java file limit exceeded (4096)");
            Source source = new Source(path, file.getValue());
            add("modules", row("path", path, "package", null, "parsed", false));
            if (roots.stream().noneMatch(r -> within(path, r))) { omitted.add(path); continue; }
            // JPMS requires a dedicated multi-module compilation environment.
            // Preserve descriptors rather than merging their namespaces.
            if (path.endsWith("module-info.java")) {
                unsupportedProject = true;
                omitted.add(path); diagnostic(source, "Java module descriptors are outside this retained single-source-universe producer");
                continue;
            }
            sources.put(path, source);
        }
        if (count == 0) throw new IllegalArgumentException("Java analysis needs retained .java files");
        if (sources.isEmpty()) return;
        JavaCompiler compiler = ToolProvider.getSystemJavaCompiler();
        if (compiler == null) throw new IllegalArgumentException("A full JDK with jdk.compiler is required");
        DiagnosticCollector<JavaFileObject> diagnostics = new DiagnosticCollector<>();
        try (StandardJavaFileManager standard = compiler.getStandardFileManager(diagnostics, Locale.ROOT, StandardCharsets.UTF_8)) {
            for (StandardLocation location : List.of(StandardLocation.CLASS_PATH, StandardLocation.SOURCE_PATH,
                    StandardLocation.ANNOTATION_PROCESSOR_PATH, StandardLocation.MODULE_PATH,
                    StandardLocation.UPGRADE_MODULE_PATH)) standard.setLocationFromPaths(location, List.of());
            JavaFileManager manager = new ForwardingJavaFileManager<StandardJavaFileManager>(standard) {
                @Override public JavaFileObject getJavaFileForOutput(Location l, String n, JavaFileObject.Kind k, FileObject s) {
                    throw new IllegalStateException("Repository compiler emit is disabled");
                }
                @Override public FileObject getFileForOutput(Location l, String p, String n, FileObject s) {
                    throw new IllegalStateException("Repository compiler emit is disabled");
                }
            };
            List<String> options = List.of("-proc:none", "-implicit:none", "-encoding", "UTF-8",
                    "--release", Integer.toString(release), "-Xmaxerrs", "4096", "-Xmaxwarns", "4096");
            JavacTask task = (JavacTask) compiler.getTask(null, manager, diagnostics, options, null, sources.values());
            trees = Trees.instance(task); positions = trees.getSourcePositions();
            List<CompilationUnitTree> parsed = new ArrayList<>();
            for (CompilationUnitTree unit : task.parse()) {
                Source source = sources.values().stream().filter(s -> s.toUri().equals(unit.getSourceFile().toUri())).findFirst().orElseThrow();
                units.put(unit, source); parsed.add(unit);
                for (Map<String, Object> module : rows.get("modules")) if (module.get("path").equals(source.path)) module.put("parsed", true);
            }
            for (Diagnostic<? extends JavaFileObject> diagnostic : diagnostics.getDiagnostics()) {
                if (diagnostic.getKind() == Diagnostic.Kind.ERROR && diagnostic.getSource() != null)
                    errors.add(diagnostic.getSource().toUri().getPath().substring(1));
            }
            for (Map<String, Object> module : rows.get("modules"))
                if (errors.contains((String) module.get("path"))) module.put("parsed", false);
            for (CompilationUnitTree unit : parsed) if (!errors.contains(units.get(unit).path)) declarations(unit);
            if (!syntaxOnly) {
                task.analyze();
                for (CompilationUnitTree unit : parsed) for (Definition def : bySource.getOrDefault(units.get(unit), List.of())) {
                    def.element = trees.getElement(TreePath.getPath(unit, def.node));
                    if (def.element != null) elements.put(def.element, def);
                }
            }
            for (Diagnostic<? extends JavaFileObject> d : diagnostics.getDiagnostics()) {
                String path = d.getSource() == null ? null : d.getSource().toUri().getPath().substring(1);
                if (d.getKind() == Diagnostic.Kind.ERROR && path != null) errors.add(path);
                add("diagnostics", row("path", path, "code", d.getCode(), "message", d.getMessage(Locale.ROOT)));
            }
            for (String failed : errors) {
                Source source = sources.get(failed);
                if (source == null || source.text.isEmpty()) continue;
                for (String predicate : List.of("IMPORTS", "REFERENCES", "CALLS_STATIC"))
                    add("unresolved", row("path", failed, "start", 0, "end", source.bytes[source.text.length()],
                        "predicate", predicate, "reason", "extraction_failure", "status", "unsupported", "candidate_keys", List.of()));
            }
            Set<String> packages = new TreeSet<>();
            for (CompilationUnitTree unit : parsed) {
                Source source = units.get(unit);
                String pkg = unit.getPackageName() == null ? "" : unit.getPackageName().toString();
                String key = "package::" + (pkg.isEmpty() ? "<default>" : pkg);
                if (packages.add(key)) add("packages", row("key", key, "parent", null, "source", source.path));
                for (Map<String, Object> module : rows.get("modules")) if (module.get("path").equals(source.path)) module.put("package", key);
                imports(unit); referencesAndCalls(unit);
            }
        }
    }
    private Map<String, Object> output() {
        Map<String, Object> result = row("schema_version", "1.0", "provider", row(
                "name", "jdk-compiler-api", "version", Runtime.version().toString(), "release", release,
                "source_roots", roots, "syntax_only", syntaxOnly, "executes_source", false, "emit", false,
                "annotation_processing", false, "mapping", "retained-java-compiler-elements-v1",
                "module_resolution", "retained sources and trusted JDK platform types only; no repository binaries"),
                "omitted", omitted);
        for (Map.Entry<String, List<Map<String, Object>>> family : rows.entrySet()) {
            family.getValue().sort(Comparator.comparing(JavaAdapter::json));
            result.put(family.getKey(), family.getValue());
        }
        return result;
    }
    private static String json(Object value) {
        if (value == null) return "null";
        if (value instanceof String text) {
            StringBuilder out = new StringBuilder("\"");
            for (int i = 0; i < text.length(); i++) {
                char c = text.charAt(i);
                if (c == '\\' || c == '"') out.append('\\').append(c);
                else if (c < 32) out.append(String.format(Locale.ROOT, "\\u%04x", (int) c));
                else out.append(c);
            }
            return out.append('"').toString();
        }
        if (value instanceof Boolean || value instanceof Number) return value.toString();
        if (value instanceof Map<?, ?> map) {
            StringJoiner out = new StringJoiner(",", "{", "}");
            for (Map.Entry<?, ?> e : map.entrySet()) out.add(json(e.getKey().toString()) + ":" + json(e.getValue()));
            return out.toString();
        }
        if (value instanceof Collection<?> list) {
            StringJoiner out = new StringJoiner(",", "[", "]"); for (Object item : list) out.add(json(item)); return out.toString();
        }
        throw new IllegalArgumentException("Unsupported Java adapter output type");
    }
    public static void main(String[] args) throws Exception {
        JavaAdapter adapter = new JavaAdapter();
        for (String family : List.of("packages", "groups", "modules", "definitions", "imports", "references", "calls", "unresolved", "diagnostics"))
            adapter.rows.put(family, new ArrayList<>());
        adapter.read(); if (!adapter.identityOnly) adapter.analyze();
        byte[] bytes = json(adapter.output()).getBytes(StandardCharsets.UTF_8);
        if (bytes.length > 16 * 1024 * 1024) throw new IllegalArgumentException("Java output byte quota exceeded");
        System.out.write(bytes);
    }
}
