// Compiler-owned resolution over an in-memory retained snapshot. Repository
// scripts, plugins, live files, package downloads and emit are never invoked.
'use strict';
const fs = require('node:fs');
const path = require('node:path').posix;
const crypto = require('node:crypto');
const compilerPath = process.argv[2];
const ts = require(compilerPath);
const request = JSON.parse(fs.readFileSync(0, 'utf8'));
const root = '/briefcase-input';
const files = new Map(Object.entries(request.files).map(([p, s]) => [path.join(root, p), s]));
const relative = p => path.relative(root, p);
const isCode = p => /\.(?:[cm]?[jt]sx?)$/.test(p);
const directories = new Set([root]);
for (const p of files.keys()) {
  for (let d = path.dirname(p); d.startsWith(root); d = path.dirname(d)) {
    directories.add(d);
    if (d === root) break;
  }
}
const system = {
  useCaseSensitiveFileNames: true,
  fileExists: p => files.has(path.normalize(p)),
  readFile: p => files.get(path.normalize(p)),
  directoryExists: p => directories.has(path.normalize(p)),
  getDirectories: p => [...directories].filter(d => path.dirname(d) === p),
  readDirectory: (p, extensions, excludes, includes, depth) => ts.matchFiles(
    p, extensions, excludes, includes, true, root, depth,
    d => ({ files: [...files.keys()].filter(f => path.dirname(f) === d).map(f => path.basename(f)),
      directories: [...directories].filter(f => path.dirname(f) === d).map(f => path.basename(f)) }),
    p => p),
  realpath: p => path.normalize(p),
};
const project = path.join(root, request.project);
const compilerDigest = () => 'sha256:' + crypto.createHash('sha256').update(fs.readFileSync(compilerPath)).digest('hex');
const configurationError = (reason, message) => {
  process.stdout.write(JSON.stringify({ schema_version: '1.0', compiler_digest: compilerDigest(),
    configuration: { reason, message } }));
  process.exit(0);
};
if (!files.has(project)) configurationError('unavailable_project', 'Project configuration was not captured');
const config = ts.readConfigFile(project, system.readFile);
if (config.error) configurationError('invalid_config', ts.flattenDiagnosticMessageText(config.error.messageText, '\n'));
const parsed = ts.parseJsonConfigFileContent(config.config, system, path.dirname(project), undefined, project);
// Project references need a dedicated multi-project mapping; never silently
// resolve one project as if it covered its referenced builds.
if (parsed.projectReferences?.length) configurationError('unsupported_references',
  'Project references are outside this producer scope; select a config covering the intended sources without project references');
if (parsed.errors.length) configurationError(
  parsed.errors.every(d => d.code === 18002 || d.code === 18003) ? 'no_sources' : 'invalid_config',
  parsed.errors.map(d => ts.flattenDiagnosticMessageText(d.messageText, '\n')).join('\n'));
if (!parsed.fileNames.some(p => files.has(p) && isCode(p))) configurationError('no_sources',
  'Project contains no captured TypeScript/JavaScript source');
if (request.mode === 'validate') {
  process.stdout.write(JSON.stringify({ schema_version: '1.0', compiler_digest: compilerDigest(), valid: true }));
  process.exit(0);
}
const options = { ...parsed.options, noEmit: true, noLib: true, incremental: false, composite: false };
const host = {
  ...system,
  getSourceFile: (p, languageVersion) => {
    const source = system.readFile(p);
    return source === undefined ? undefined : ts.createSourceFile(p, source, languageVersion, true);
  },
  getDefaultLibFileName: () => '/unavailable/lib.d.ts',
  writeFile: () => { throw new Error('Compiler emit is disabled'); },
  getCurrentDirectory: () => root,
  getCanonicalFileName: p => p,
  getNewLine: () => '\n',
  useCaseSensitiveFileNames: () => true,
};
const program = ts.createProgram(parsed.fileNames, options, host);
const checker = program.getTypeChecker();
const sourceFiles = program.getSourceFiles().filter(s => files.has(s.fileName) && isCode(s.fileName));
if (!sourceFiles.length) throw new Error('Project contains no captured TypeScript/JavaScript source');
if (sourceFiles.length > 4096) throw new Error('Project file limit exceeded (4096)');
const byte = (sf, p) => Buffer.byteLength(sf.text.slice(0, p), 'utf8');
const span = n => ({ path: relative(n.getSourceFile().fileName),
  start: byte(n.getSourceFile(), n.getStart()), end: byte(n.getSourceFile(), n.end) });
const definitions = [];
const functionKeys = new Map();
const diagnostics = [];
const errors = new Map();
for (const sf of sourceFiles) {
  const ds = [...program.getSyntacticDiagnostics(sf), ...program.getSemanticDiagnostics(sf)];
  errors.set(sf.fileName, ds.filter(d => d.category === ts.DiagnosticCategory.Error));
  diagnostics.push(...ds.map(d => ({ path: relative(sf.fileName), code: d.code,
    start: d.start === undefined ? null : byte(sf, d.start), message: ts.flattenDiagnosticMessageText(d.messageText, '\n') })));
  for (const n of sf.statements) {
    if (n.name && ts.isIdentifier(n.name) && (ts.isFunctionDeclaration(n) || ts.isClassDeclaration(n)
        || ts.isInterfaceDeclaration(n) || ts.isTypeAliasDeclaration(n) || ts.isEnumDeclaration(n))) {
      const key = `${relative(sf.fileName)}::${n.name.text}#${byte(sf, n.getStart())}`;
      const callable = ts.isFunctionDeclaration(n) && !!n.body && !sf.isDeclarationFile;
      definitions.push({ ...span(n), key, callable });
      if (callable) functionKeys.set(n, key);
    }
  }
}
if (definitions.length > 4096) throw new Error('Definition limit exceeded (4096)');
const imports = [];
const calls = [];
const unresolved = [];
const mutated = new Set();
const assignmentTarget = n => {
  let s = checker.getSymbolAtLocation(n);
  if (s?.flags & ts.SymbolFlags.Alias) s = checker.getAliasedSymbol(s);
  if (s) mutated.add(s);
  // Destructuring and parenthesized assignments still replace their leaf
  // bindings. Recording all leaves is deliberately conservative.
  ts.forEachChild(n, assignmentTarget);
};
const mutations = n => {
  if (ts.isBinaryExpression(n) && n.operatorToken.kind >= ts.SyntaxKind.FirstAssignment
      && n.operatorToken.kind <= ts.SyntaxKind.LastAssignment) {
    assignmentTarget(n.left);
  } else if ((ts.isPrefixUnaryExpression(n) || ts.isPostfixUnaryExpression(n))
      && [ts.SyntaxKind.PlusPlusToken, ts.SyntaxKind.MinusMinusToken].includes(n.operator)) {
    assignmentTarget(n.operand);
  } else if ((ts.isForOfStatement(n) || ts.isForInStatement(n))
      && !ts.isVariableDeclarationList(n.initializer)) {
    assignmentTarget(n.initializer);
  }
  ts.forEachChild(n, mutations);
};
for (const sf of sourceFiles) mutations(sf);
const unknown = (n, predicate, reason, caller = null) => {
  unresolved.push({ ...span(n), predicate, reason, caller });
};
const hasError = n => (errors.get(n.getSourceFile().fileName) || []).some(d => d.start !== undefined
  && d.start < n.end && d.start + (d.length || 1) > n.getStart());
const literalModule = (n, literal) => {
  const sf = n.getSourceFile();
  if (!literal || !ts.isStringLiteralLike(literal)) {
    unknown(n, 'IMPORTS', 'dynamic_property'); return;
  }
  // Prefer the Program's module symbol (also honors NodeNext resolution mode).
  const symbol = checker.getSymbolAtLocation(literal);
  const candidates = new Set((symbol?.declarations || []).filter(ts.isSourceFile).map(s => s.fileName));
  if (!candidates.size) {
    const resolved = ts.resolveModuleName(literal.text, sf.fileName, options, system).resolvedModule;
    if (resolved) candidates.add(resolved.resolvedFileName);
  }
  const selected = [...candidates].filter(p => sourceFiles.some(s => s.fileName === p));
  if (selected.length === 1 && candidates.size === 1) {
    imports.push({ ...span(n), target: relative(selected[0]), specifier: literal.text });
  } else unknown(n, 'IMPORTS', 'missing_dependency');
};
const unshadowedRequire = e => ts.isIdentifier(e) && e.text === 'require'
  && !(checker.getSymbolAtLocation(e)?.declarations || []).some(d => files.has(d.getSourceFile().fileName));
const functionTarget = expression => {
  let selected = expression;
  if (ts.isPropertyAccessExpression(expression)) {
    // Only a namespace import's statically named property is supported.
    const namespace = checker.getSymbolAtLocation(expression.expression);
    if (!(namespace?.declarations || []).some(ts.isNamespaceImport)) return null;
    selected = expression.name;
  } else if (!ts.isIdentifier(expression)) return null;
  let symbol = checker.getSymbolAtLocation(selected);
  if (symbol?.flags & ts.SymbolFlags.Alias) symbol = checker.getAliasedSymbol(symbol);
  if (mutated.has(symbol)) return null;
  const declarations = symbol?.declarations || [];
  if (!declarations.length || declarations.some(d => !ts.isFunctionDeclaration(d))) return null;
  const implementations = declarations.filter(d => functionKeys.has(d));
  return implementations.length === 1 ? implementations[0] : null;
};
const visit = (n, caller = null) => {
  // Closures, callbacks, methods and initializers cannot acquire the enclosing
  // free function's identity just because their source occurs inside it.
  if (ts.isFunctionLike(n)) caller = functionKeys.get(n) || null;
  if (ts.isClassDeclaration(n) || ts.isClassExpression(n)) caller = null;
  if (ts.isImportDeclaration(n) || ts.isExportDeclaration(n)) {
    if (n.moduleSpecifier) literalModule(n, n.moduleSpecifier);
  } else if (ts.isImportEqualsDeclaration(n) && ts.isExternalModuleReference(n.moduleReference)) {
    literalModule(n, n.moduleReference.expression);
  }
  if (ts.isCallExpression(n)) {
    const e = n.expression;
    const loader = e.kind === ts.SyntaxKind.ImportKeyword || unshadowedRequire(e);
    if (loader) literalModule(n, n.arguments.length === 1 ? n.arguments[0] : null);
    if (e.kind === ts.SyntaxKind.ImportKeyword) {
      unknown(n, 'CALLS_STATIC', 'dynamic_property', caller);
    } else {
      const target = !loader && !n.questionDotToken && !e.questionDotToken && functionTarget(e);
      const signature = target && checker.getResolvedSignature(n);
      // A checker signature plus an immutable function declaration is required;
      // variables, parameters, object methods and textual names cannot suffice.
      if (caller && target && signature?.declaration && ts.isFunctionDeclaration(signature.declaration)
          && (checker.getSymbolAtLocation(target.name)?.declarations || []).includes(signature.declaration)
          && !hasError(n) && !hasError(target)) {
        calls.push({ ...span(n), caller, target: functionKeys.get(target) });
      } else unknown(n, 'CALLS_STATIC', loader ? 'missing_dependency' : 'dynamic_property', caller);
    }
  }
  if (ts.isNewExpression(n) || ts.isTaggedTemplateExpression(n)) {
    unknown(n, 'CALLS_STATIC', 'dynamic_property', caller);
  }
  if (ts.isJsxOpeningElement(n) || ts.isJsxSelfClosingElement(n)) {
    unknown(n, 'CALLS_STATIC', 'unsupported_generated_source', caller);
  }
  ts.forEachChild(n, child => visit(child, caller));
};
for (const sf of sourceFiles) visit(sf);
if (imports.length + calls.length + unresolved.length > 16384) throw new Error('Source occurrence limit exceeded (16384)');
const omitted = [...files.keys()].filter(p => isCode(p) && !sourceFiles.some(s => s.fileName === p)).map(relative);
const provider = { name: 'typescript-compiler-api', version: ts.version,
  compiler_digest: compilerDigest(),
  node_version: process.version, project: request.project, options: parsed.options,
  node_executable_digest: 'sha256:' + crypto.createHash('sha256').update(fs.readFileSync(process.execPath)).digest('hex'),
  mapping: 'retained-typescript-modules-functions-v1',
  libraries: 'unavailable; no ambient/external target authority',
  module_resolution: 'captured inputs only', emit: false };
// Compiler paths refer to the virtual snapshot, never a temporary/live root.
process.stdout.write(JSON.stringify({ schema_version: '1.0', provider,
  modules: sourceFiles.map(sf => ({ path: relative(sf.fileName), end: Buffer.byteLength(sf.text),
    language: /\.[cm]?jsx?$/.test(sf.fileName) ? 'javascript' : 'typescript' })),
  definitions, imports, calls, unresolved, diagnostics, omitted }));
