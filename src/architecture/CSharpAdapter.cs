// Retained-source Roslyn adapter. No MSBuild, repository assemblies, generators,
// analyzers, packaging, compiler emit or source execution is used.
using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Text;
using System.Text.Json;
using Microsoft.CodeAnalysis;
using Microsoft.CodeAnalysis.CSharp;
using Microsoft.CodeAnalysis.CSharp.Syntax;
using Microsoft.CodeAnalysis.Operations;
using Microsoft.CodeAnalysis.Text;

internal sealed class CSharpAdapter
{
    private const int MaxFiles = 4096, MaxDefinitions = 4096, MaxOccurrences = 16384;
    private readonly SortedDictionary<string, string> files = new(StringComparer.Ordinal);
    private readonly Dictionary<string, List<Dictionary<string, object>>> rows = new();
    private readonly Dictionary<SyntaxTree, string> paths = new();
    private readonly Dictionary<SyntaxTree, int[]> offsets = new();
    private readonly Dictionary<ISymbol, List<Definition>> definitions = new(SymbolEqualityComparer.Default);
    private readonly Dictionary<SyntaxNode, Definition> syntax = new();
    private readonly HashSet<SyntaxTree> failed = new();
    private readonly HashSet<string> seen = new(StringComparer.Ordinal);
    private readonly SortedSet<string> omitted = new(StringComparer.Ordinal);
    private readonly SortedSet<string> namespaces = new(StringComparer.Ordinal);
    private string[] roots = Array.Empty<string>(), defines = Array.Empty<string>();
    private string version = "";
    private bool syntaxOnly, identityOnly, unsupportedProject;
    private int occurrences;
    private CSharpParseOptions parse = new();
    private CSharpCompilation compilation;

    private sealed class Definition
    {
        public SyntaxNode Node;
        public ISymbol Symbol;
        public string Key;
        public bool Callable;
    }
    private static Dictionary<string, object> Row(params object[] values)
    {
        var row = new Dictionary<string, object>();
        for (int i = 0; i < values.Length; i += 2) row.Add((string)values[i], values[i + 1]);
        return row;
    }
    private void Add(string family, Dictionary<string, object> row)
    {
        var key = family + JsonSerializer.Serialize(row);
        if (!seen.Add(key)) return;
        if (family == "definitions" && rows[family].Count >= MaxDefinitions)
            throw new InvalidDataException("C# declaration quota exceeded (4096)");
        if (new[] { "imports", "references", "calls", "unresolved" }.Contains(family) && ++occurrences > MaxOccurrences)
            throw new InvalidDataException("C# occurrence quota exceeded (16384)");
        rows[family].Add(row);
    }
    private static bool Within(string path, string root) => root == "." || path.StartsWith(root + "/", StringComparison.Ordinal);
    private static bool SafeRoot(string root) => root == "." || (!string.IsNullOrEmpty(root) && !root.StartsWith("/")
        && !root.Contains('\\') && !root.Contains(':') && root.Split('/').All(p => p.Length > 0 && p != "." && p != ".."));
    private void Read()
    {
        using var memory = new MemoryStream();
        byte[] chunk = new byte[65536];
        using var input = Console.OpenStandardInput();
        for (int n; (n = input.Read(chunk, 0, chunk.Length)) != 0;)
        {
            if (memory.Length + n > 272L * 1024 * 1024) throw new InvalidDataException("C# input byte quota exceeded");
            memory.Write(chunk, 0, n);
        }
        using var document = JsonDocument.Parse(memory.ToArray());
        var json = document.RootElement;
        foreach (var file in json.GetProperty("files").EnumerateObject()) files.Add(file.Name, file.Value.GetString());
        roots = json.GetProperty("source_roots").EnumerateArray().Select(x => x.GetString()).OrderBy(x => x, StringComparer.Ordinal).ToArray();
        if (roots.Length == 0) roots = new[] { "." };
        if (roots.Length > 64 || roots.Distinct().Count() != roots.Length || roots.Any(r => !SafeRoot(r)))
            throw new InvalidDataException("C# roots must be unique normalized relative paths (at most 64)");
        defines = json.GetProperty("defines").EnumerateArray().Select(x => x.GetString()).OrderBy(x => x, StringComparer.Ordinal).ToArray();
        if (defines.Length > 64 || defines.Distinct().Count() != defines.Length || defines.Any(d => !SyntaxFacts.IsValidIdentifier(d)))
            throw new InvalidDataException("C# defines must be unique identifiers (at most 64)");
        version = json.GetProperty("language_version").GetString();
        var language = LanguageVersion.Default;
        if (version != null && (!LanguageVersionFacts.TryParse(version, out language) || !version.All(c => char.IsDigit(c) || c == '.')))
            throw new InvalidDataException("C# language version must be an installed non-preview numeric version");
        parse = new CSharpParseOptions(language, DocumentationMode.Parse, SourceCodeKind.Regular, defines);
        version = parse.LanguageVersion.ToDisplayString();
        syntaxOnly = json.GetProperty("syntax_only").GetBoolean();
        identityOnly = json.GetProperty("identity_only").GetBoolean();
    }
    private Dictionary<string, object> Span(SyntaxNode node)
    {
        if (!paths.TryGetValue(node.SyntaxTree, out var path) || node.Span.Length == 0) return null;
        var bytes = offsets[node.SyntaxTree];
        return Row("path", path, "start", bytes[node.Span.Start], "end", bytes[node.Span.End]);
    }
    private void Unknown(SyntaxNode node, string predicate, string reason, string caller = null, IEnumerable<Definition> candidates = null)
    {
        var row = Span(node);
        if (row == null) return;
        string[] keys = (candidates ?? Enumerable.Empty<Definition>()).Select(d => d.Key).Distinct().OrderBy(x => x, StringComparer.Ordinal).ToArray();
        foreach (var pair in Row("predicate", predicate, "reason", reason, "caller", caller,
            "status", keys.Length > 1 ? "ambiguous" : "unresolved", "candidate_keys", keys)) row.Add(pair.Key, pair.Value);
        Add("unresolved", row);
    }
    private void Site(SyntaxNode node, string family, string target, string caller = null)
    {
        var row = Span(node);
        if (row == null) return;
        row.Add("target", target); row.Add("caller", caller); Add(family, row);
    }
    private static ISymbol Normal(ISymbol symbol)
    {
        if (symbol is IMethodSymbol method)
        {
            method = (method.ReducedFrom ?? method).OriginalDefinition;
            return method.PartialImplementationPart ?? method;
        }
        return symbol?.OriginalDefinition;
    }
    private List<Definition> Candidates(ISymbol symbol)
    {
        symbol = Normal(symbol);
        return symbol != null && definitions.TryGetValue(symbol, out var found) ? found : new();
    }
    private Definition Target(ISymbol symbol, bool callable = false)
    {
        var candidates = Candidates(symbol).Where(d => !failed.Contains(d.Node.SyntaxTree) && (!callable || d.Callable)).ToArray();
        return !unsupportedProject && candidates.Length == 1 ? candidates[0] : null;
    }
    private string Caller(SyntaxNode node)
    {
        foreach (var ancestor in node.Ancestors())
        {
            if (ancestor is AnonymousFunctionExpressionSyntax || ancestor is TypeDeclarationSyntax) return null;
            if (syntax.TryGetValue(ancestor, out var declaration) && declaration.Callable)
            {
                bool body = ancestor switch {
                    BaseMethodDeclarationSyntax method => method.Body?.Span.Contains(node.Span) == true || method.ExpressionBody?.Span.Contains(node.Span) == true
                        || method is ConstructorDeclarationSyntax ctor && ctor.Initializer?.Span.Contains(node.Span) == true,
                    LocalFunctionStatementSyntax local => local.Body?.Span.Contains(node.Span) == true || local.ExpressionBody?.Span.Contains(node.Span) == true,
                    AccessorDeclarationSyntax accessor => accessor.Body?.Span.Contains(node.Span) == true || accessor.ExpressionBody?.Span.Contains(node.Span) == true,
                    _ => false
                };
                return body ? declaration.Key : null;
            }
        }
        return null;
    }
    private static string Name(ISymbol symbol)
    {
        var names = new List<string>();
        for (var current = symbol; current != null && !(current is INamespaceSymbol) && current.Kind != SymbolKind.Assembly; current = current.ContainingSymbol)
            names.Add(current.Name);
        names.Reverse();
        var ns = symbol.ContainingNamespace;
        string prefix = ns == null || ns.IsGlobalNamespace ? "" : ns.ToDisplayString().Replace(".", "::") + "::";
        return prefix + string.Join("::", names);
    }
    private void Declare(SyntaxNode node, ISymbol symbol, bool callable)
    {
        if (symbol == null || symbol.IsImplicitlyDeclared || node.Span.Length == 0) return;
        var row = Span(node);
        var declaration = new Definition { Node = node, Symbol = Normal(symbol), Callable = callable && !syntaxOnly,
            Key = paths[node.SyntaxTree] + "::" + Name(symbol) + "#" + row["start"] };
        syntax.Add(node, declaration);
        if (!definitions.TryGetValue(declaration.Symbol, out var list)) definitions.Add(declaration.Symbol, list = new());
        list.Add(declaration);
        row.Add("key", declaration.Key); row.Add("callable", declaration.Callable); Add("definitions", row);
    }
    private static bool HasBody(SyntaxNode node) => node switch
    {
        BaseMethodDeclarationSyntax method => method.Body != null || method.ExpressionBody != null,
        LocalFunctionStatementSyntax local => local.Body != null || local.ExpressionBody != null,
        AccessorDeclarationSyntax accessor => accessor.Body != null || accessor.ExpressionBody != null,
        _ => false
    };
    private void Declarations(SyntaxTree tree, SemanticModel model)
    {
        foreach (var node in tree.GetRoot().DescendantNodes())
        {
            if (node is BaseTypeDeclarationSyntax || node is DelegateDeclarationSyntax || node is BaseMethodDeclarationSyntax
                || node is LocalFunctionStatementSyntax || node is AccessorDeclarationSyntax || node is VariableDeclaratorSyntax
                || node is ParameterSyntax || node is TypeParameterSyntax || node is PropertyDeclarationSyntax
                || node is IndexerDeclarationSyntax || node is EnumMemberDeclarationSyntax || node is EventDeclarationSyntax)
                Declare(node, model.GetDeclaredSymbol(node), HasBody(node));
        }
    }
    private bool Exact(IMethodSymbol method, SyntaxNode node)
    {
        if (method == null || method.IsAbstract || method.IsExtern || method.ContainingType?.TypeKind == TypeKind.Interface) return false;
        if (method.IsStatic || method.MethodKind == MethodKind.Constructor || method.MethodKind == MethodKind.LocalFunction
            || method.DeclaredAccessibility == Accessibility.Private || method.IsSealed || method.ContainingType?.IsSealed == true
            || (!method.IsVirtual && !method.IsOverride)) return true;
        return node is InvocationExpressionSyntax call && call.Expression is MemberAccessExpressionSyntax member && member.Expression is BaseExpressionSyntax;
    }
    private void Call(SyntaxNode node, IMethodSymbol method)
    {
        string caller = Caller(node);
        var target = Target(method, true);
        if (caller != null && !syntaxOnly && !failed.Contains(node.SyntaxTree) && target != null && Exact(method, node))
            Site(node, "calls", target.Key, caller);
        else Unknown(node, "CALLS_STATIC", method?.MethodKind == MethodKind.DelegateInvoke ? "dynamic_property"
            : method?.ContainingType?.TypeKind == TypeKind.Interface || method?.IsVirtual == true || method?.IsOverride == true ? "trait_dispatch"
            : caller == null ? "unsupported_context" : "missing_dependency", caller, Candidates(method));
    }
    private void Reference(SyntaxNode node, SemanticModel model)
    {
        if (syntaxOnly) return;
        var info = model.GetSymbolInfo(node);
        if (info.Symbol is INamespaceSymbol ns)
        {
            if (!ns.IsGlobalNamespace && namespaces.Contains(ns.ToDisplayString())) Site(node, "references", "package::" + ns.ToDisplayString());
            return;
        }
        var target = Target(info.Symbol);
        if (!failed.Contains(node.SyntaxTree) && target != null)
        {
            Site(node, "references", target.Key, Caller(node));
            if (target.Node.SyntaxTree != node.SyntaxTree) Site(node, "imports", paths[target.Node.SyntaxTree], Caller(node));
        }
        else if (info.Symbol != null || info.CandidateSymbols.Length != 0)
        {
            var candidates = info.CandidateSymbols.SelectMany(Candidates).Concat(Candidates(info.Symbol));
            Unknown(node, "REFERENCES", "missing_dependency", Caller(node), candidates);
            if (info.Symbol is INamedTypeSymbol || info.Symbol is IMethodSymbol || info.Symbol is IFieldSymbol)
                Unknown(node, "IMPORTS", "missing_dependency", Caller(node), candidates);
        }
    }
    private void Imports(SyntaxTree tree, SemanticModel model)
    {
        foreach (var directive in tree.GetRoot().DescendantNodes().OfType<UsingDirectiveSyntax>())
        {
            var symbol = directive.Name == null ? null : model.GetSymbolInfo(directive.Name).Symbol;
            if (!syntaxOnly && !failed.Contains(tree) && symbol is INamespaceSymbol ns && namespaces.Contains(ns.ToDisplayString()))
            {
                Site(directive, "imports", "package::" + ns.ToDisplayString());
                if (ns.ConstituentNamespaces.Any(part => part.ContainingAssembly != null && !SymbolEqualityComparer.Default.Equals(part.ContainingAssembly, compilation.Assembly)))
                    Unknown(directive, "IMPORTS", "missing_dependency");
            }
            else if (!syntaxOnly && !failed.Contains(tree) && Target(symbol) is Definition target)
                Site(directive, "imports", paths[target.Node.SyntaxTree]);
            else Unknown(directive, "IMPORTS", "missing_dependency", null, Candidates(symbol));
        }
        foreach (var directive in tree.GetRoot().DescendantNodes().OfType<ExternAliasDirectiveSyntax>())
            Unknown(directive, "IMPORTS", "unsupported_context");
    }
    private sealed class Boundaries : OperationWalker
    {
        private readonly CSharpAdapter adapter;
        internal Boundaries(CSharpAdapter adapter) { this.adapter = adapter; }
        public override void Visit(IOperation operation)
        {
            if (operation == null) return;
            bool boundary = operation is IConversionOperation conversion && conversion.OperatorMethod != null
                || operation is IBinaryOperation binary && binary.OperatorMethod != null
                || operation is IUnaryOperation unary && unary.OperatorMethod != null
                || operation is IIncrementOrDecrementOperation increment && increment.OperatorMethod != null
                || operation is ICompoundAssignmentOperation assignment && assignment.OperatorMethod != null
                || operation is IPropertyReferenceOperation || operation is IEventReferenceOperation
                || operation is IDelegateCreationOperation || operation is IFunctionPointerInvocationOperation
                || operation is IDynamicInvocationOperation || operation is IDynamicObjectCreationOperation
                || operation is IAwaitOperation || operation is IUsingOperation || operation is ILockOperation
                || operation is IForEachLoopOperation || operation.Kind.ToString().Contains("InterpolatedStringHandler")
                || operation.IsImplicit && (operation is IInvocationOperation || operation is IObjectCreationOperation);
            if (boundary) adapter.Unknown(operation.Syntax, "CALLS_STATIC", "unsupported_operation", adapter.Caller(operation.Syntax));
            base.Visit(operation);
        }
    }
    private void Occurrences(SyntaxTree tree, SemanticModel model)
    {
        Imports(tree, model);
        var roots = new HashSet<IOperation>();
        var boundaries = new Boundaries(this);
        foreach (var node in tree.GetRoot().DescendantNodes())
        {
            if (node.Ancestors().Any(a => a is UsingDirectiveSyntax)) continue;
            var operation = model.GetOperation(node);
            if (operation != null && operation.Parent == null && roots.Add(operation)) boundaries.Visit(operation);
            if (node is SimpleNameSyntax) Reference(node, model);
            if (node is InvocationExpressionSyntax invocation)
            {
                if (invocation.Expression is IdentifierNameSyntax identifier && identifier.Identifier.Text == "nameof"
                    && operation is INameOfOperation) continue;
                Call(node, (operation as IInvocationOperation)?.TargetMethod);
            }
            else if (node is BaseObjectCreationExpressionSyntax)
                Call(node, (operation as IObjectCreationOperation)?.Constructor);
            else if (node is ConstructorInitializerSyntax)
                Call(node, model.GetSymbolInfo(node).Symbol as IMethodSymbol);
            else if (node is AnonymousFunctionExpressionSyntax || node is AwaitExpressionSyntax || node is CommonForEachStatementSyntax
                || node is UsingStatementSyntax || node is LockStatementSyntax || node is QueryExpressionSyntax
                || node is LocalDeclarationStatementSyntax local && local.UsingKeyword != default
                || node is ImplicitArrayCreationExpressionSyntax || node is CollectionExpressionSyntax)
                Unknown(node, "CALLS_STATIC", "unsupported_operation", Caller(node));
            else if (operation is IPropertyReferenceOperation || operation is IEventReferenceOperation)
                Unknown(node, "CALLS_STATIC", "unsupported_operation", Caller(node));
            else if (operation is IBinaryOperation binary && binary.OperatorMethod != null
                || operation is IUnaryOperation unary && unary.OperatorMethod != null
                || operation is IConversionOperation conversion && conversion.OperatorMethod != null
                || operation is ICompoundAssignmentOperation assignment && assignment.OperatorMethod != null)
                Unknown(node, "CALLS_STATIC", "unsupported_operation", Caller(node));
            if (node is ConstructorDeclarationSyntax constructor && constructor.Initializer == null
                || node is RecordDeclarationSyntax || node is ClassDeclarationSyntax cls && cls.ParameterList != null)
                Unknown(node, "CALLS_STATIC", "unsupported_generated_source", syntax.TryGetValue(node, out var d) && d.Callable ? d.Key : null);
        }
    }
    private void Analyze(string references)
    {
        var trees = new List<SyntaxTree>();
        int projects = files.Keys.Count(p => p.EndsWith(".csproj", StringComparison.Ordinal) && roots.Any(r => Within(p, r)));
        unsupportedProject = projects > 1;
        if (unsupportedProject) Add("diagnostics", Row("path", null, "message", "Multiple retained C# projects require separate source universes; project references are not evaluated"));
        int count = 0;
        foreach (var file in files)
        {
            if (!file.Key.EndsWith(".cs", StringComparison.Ordinal)) continue;
            if (++count > MaxFiles) throw new InvalidDataException("C# file quota exceeded (4096)");
            bool selected = roots.Any(r => Within(file.Key, r)) && !file.Key.Split('/').Any(p => p == "obj" || p == "bin")
                && !file.Key.EndsWith(".g.cs", StringComparison.Ordinal) && !file.Key.EndsWith(".generated.cs", StringComparison.Ordinal);
            Add("modules", Row("path", file.Key, "package", null, "parsed", false));
            if (!selected) { omitted.Add(file.Key); continue; }
            var tree = CSharpSyntaxTree.ParseText(SourceText.From(file.Value, Encoding.UTF8), parse, file.Key);
            paths.Add(tree, file.Key);
            int[] bytes = new int[file.Value.Length + 1];
            int offset = 0;
            for (int i = 0; i < file.Value.Length;)
            {
                int length = char.IsHighSurrogate(file.Value[i]) && i + 1 < file.Value.Length && char.IsLowSurrogate(file.Value[i + 1]) ? 2 : 1;
                bytes[i] = offset; if (length == 2) bytes[i + 1] = offset;
                offset += Encoding.UTF8.GetByteCount(file.Value.AsSpan(i, length)); i += length; bytes[i] = offset;
            }
            offsets.Add(tree, bytes); trees.Add(tree);
        }
        if (count == 0) throw new InvalidDataException("C# analysis needs retained .cs source");
        var metadata = Directory.EnumerateFiles(references, "*.dll").OrderBy(p => p, StringComparer.Ordinal).Select(p => MetadataReference.CreateFromFile(p));
        compilation = CSharpCompilation.Create("TraceRetained", trees, metadata,
            new CSharpCompilationOptions(OutputKind.DynamicallyLinkedLibrary, allowUnsafe: true, deterministic: true,
                metadataReferenceResolver: null, sourceReferenceResolver: null, xmlReferenceResolver: null));
        foreach (var diagnostic in syntaxOnly ? trees.SelectMany(t => t.GetDiagnostics()) : compilation.GetDiagnostics())
        {
            if (diagnostic.Severity == DiagnosticSeverity.Hidden) continue;
            var tree = diagnostic.Location.SourceTree;
            if (diagnostic.Severity == DiagnosticSeverity.Error && tree != null) failed.Add(tree);
            Add("diagnostics", Row("path", tree == null ? null : paths[tree], "code", diagnostic.Id,
                "message", diagnostic.GetMessage(System.Globalization.CultureInfo.InvariantCulture)));
        }
        foreach (var tree in trees)
        {
            var model = compilation.GetSemanticModel(tree, ignoreAccessibility: false);
            Declarations(tree, model);
            var ns = tree.GetRoot().DescendantNodes().OfType<BaseNamespaceDeclarationSyntax>().Select(n => model.GetDeclaredSymbol(n))
                .OfType<INamespaceSymbol>().Select(n => n.ToDisplayString()).Distinct();
            foreach (var name in ns)
            {
                if (namespaces.Add(name)) Add("packages", Row("key", "package::" + name, "parent", null, "source", paths[tree]));
            }
            rows["modules"].Single(m => (string)m["path"] == paths[tree])["parsed"] = !tree.GetDiagnostics().Any(d => d.Severity == DiagnosticSeverity.Error);
            if (tree.GetRoot().DescendantTrivia(descendIntoTrivia: true).Any(t => t.IsKind(SyntaxKind.DisabledTextTrivia)))
                Add("diagnostics", Row("path", paths[tree], "message", "Inactive preprocessor source is retained text, not resolved syntax or execution"));
        }
        foreach (var tree in trees)
        {
            Occurrences(tree, compilation.GetSemanticModel(tree));
            if (failed.Contains(tree) && tree.GetRoot().Span.Length > 0)
                foreach (var predicate in new[] { "IMPORTS", "REFERENCES", "CALLS_STATIC" }) Unknown(tree.GetRoot(), predicate, "extraction_failure");
        }
    }
    private object Output() => new Dictionary<string, object>(rows.ToDictionary(r => r.Key, r => (object)r.Value.OrderBy(x => JsonSerializer.Serialize(x), StringComparer.Ordinal).ToArray()))
    {
        ["schema_version"] = "1.0", ["omitted"] = omitted,
        ["provider"] = Row("name", "roslyn", "version", typeof(CSharpCompilation).Assembly.GetName().Version.ToString(),
            "language_version", version, "source_roots", roots, "defines", defines, "syntax_only", syntaxOnly,
            "executes_source", false, "emit", false, "generators", false, "mapping", "retained-csharp-roslyn-symbols-v1")
    };
    public static void Main(string[] args)
    {
        if (args.Length != 1) throw new InvalidDataException("C# adapter needs the trusted platform reference directory");
        var adapter = new CSharpAdapter();
        foreach (var name in new[] { "packages", "groups", "modules", "definitions", "imports", "references", "calls", "unresolved", "diagnostics" })
            adapter.rows.Add(name, new());
        adapter.Read();
        if (!adapter.identityOnly) adapter.Analyze(args[0]);
        byte[] output = JsonSerializer.SerializeToUtf8Bytes(adapter.Output());
        if (output.Length > 16 * 1024 * 1024) throw new InvalidDataException("C# output byte quota exceeded");
        Console.OpenStandardOutput().Write(output);
    }
}
