// Workspace mode: every retained tsconfig.json is a partition over the same
// in-memory snapshot. Resolution is module-level only (no checker): compiler
// module resolution, then retained workspace package manifests, then explicit
// relative paths. Named imports follow syntactic re-export chains to the
// declaring module. Calls are classified, never resolved. Output is bounded by
// whole files; anything not analyzed is listed, never implied covered.
'use strict';
const path = require('node:path').posix;

const MAX_FILES = 20000;
const MAX_DEFINITIONS = 131072;
const MAX_IMPORTS = 131072;
const MAX_CHAIN = 8;
const CONDITIONS = ['types', 'import', 'module', 'default', 'require', 'node'];
const EXTENSIONS = ['.ts', '.tsx', '.mts', '.cts', '.d.ts', '.js', '.jsx', '.mjs', '.cjs'];

module.exports = ({ ts, request, files, system, root, relative, isCode, compilerDigest, configurationError }) => {
  const retained = [...files.keys()];
  const configs = request.project === 'workspace'
    ? retained.filter(p => path.basename(p) === 'tsconfig.json').map(relative).sort()
    : request.project.split(',').map(s => s.trim()).filter(Boolean);
  if (!configs.length) configurationError('unavailable_project', 'No retained tsconfig.json partition');

  // Partitions: parse each config independently; failures are per partition.
  const partitions = [];
  for (const config of configs) {
    const file = path.join(root, config);
    const entry = { config, status: 'analyzed', root_files: 0, references: 0 };
    if (!files.has(file)) { partitions.push({ ...entry, status: 'failed', reason: 'unavailable_project' }); continue; }
    const read = ts.readConfigFile(file, system.readFile);
    if (read.error) {
      partitions.push({ ...entry, status: 'failed', reason: 'invalid_config',
        message: ts.flattenDiagnosticMessageText(read.error.messageText, '\n') });
      continue;
    }
    let parsed = ts.parseJsonConfigFileContent(read.config, system, path.dirname(file), undefined, file);
    let fatal = parsed.errors.filter(d => d.code !== 18002 && d.code !== 18003);
    // A package-provided base config (expo/..., astro/...) lives outside the
    // retained snapshot. Parse the local config alone and say so.
    const external = [].concat(read.config.extends || []).filter(e => typeof e === 'string' && !e.startsWith('.') && !e.startsWith('/'));
    if (fatal.length && external.length && fatal.every(d => d.code === 6053)) {
      const { extends: _, ...local } = read.config;
      parsed = ts.parseJsonConfigFileContent(local, system, path.dirname(file), undefined, file);
      fatal = parsed.errors.filter(d => d.code !== 18002 && d.code !== 18003);
      entry.degraded = 'external_extends_unavailable';
      entry.message = `extends ${external.join(', ')} is outside the retained snapshot; local options only`;
    }
    const names = parsed.fileNames.filter(p => files.has(p) && isCode(p));
    entry.references = parsed.projectReferences?.length || 0;
    if (fatal.length) {
      partitions.push({ ...entry, status: 'failed', reason: 'invalid_config',
        message: fatal.map(d => ts.flattenDiagnosticMessageText(d.messageText, '\n')).join('\n').slice(0, 512) });
      continue;
    }
    if (!names.length) { partitions.push({ ...entry, status: 'failed', reason: 'no_sources' }); continue; }
    const options = { ...parsed.options, noEmit: true, noLib: true, allowImportingTsExtensions: true };
    partitions.push({ ...entry, dir: path.dirname(file), names, options,
      cache: ts.createModuleResolutionCache(root, p => p, options) });
  }
  const analyzable = partitions.filter(p => p.status === 'analyzed');
  if (!analyzable.length) configurationError('no_sources',
    partitions.map(p => `${p.config}: ${p.reason}${p.message ? ` (${p.message})` : ''}`).join('\n').slice(0, 2048));
  if (request.mode === 'validate') {
    process.stdout.write(JSON.stringify({ schema_version: '1.0', compiler_digest: compilerDigest(), valid: true }));
    return;
  }

  // Ownership: the deepest partition directory containing the file wins, so a
  // file listed by several configs is analyzed exactly once.
  const owner = new Map();
  for (const part of analyzable) {
    for (const name of part.names) {
      const current = owner.get(name);
      const inside = p => name.startsWith(p.dir + '/');
      if (!current || (inside(part) && (!inside(current) || part.dir.length > current.dir.length))) owner.set(name, part);
    }
  }
  for (const part of analyzable) part.root_files = [...owner.values()].filter(p => p === part).length;

  // Workspace manifests: package name -> directory and exports map.
  const packages = [];
  for (const p of retained.filter(p => path.basename(p) === 'package.json')) {
    try {
      const manifest = JSON.parse(files.get(p));
      if (typeof manifest.name === 'string') packages.push({ name: manifest.name, dir: path.dirname(p), manifest });
    } catch { /* an unparsable manifest simply resolves nothing */ }
  }
  packages.sort((a, b) => b.name.length - a.name.length);
  const codeFile = p => files.has(p) && isCode(p) ? p : null;
  const withExtensions = base => codeFile(base) || EXTENSIONS.map(e => codeFile(base + e)).find(Boolean)
    || EXTENSIONS.map(e => codeFile(path.join(base, 'index' + e))).find(Boolean) || null;
  const condition = value => {
    if (typeof value === 'string') return value;
    if (Array.isArray(value)) { for (const v of value) { const r = condition(v); if (r) return r; } return null; }
    if (value && typeof value === 'object') {
      for (const c of CONDITIONS) if (c in value) { const r = condition(value[c]); if (r) return r; }
    }
    return null;
  };
  const exportTarget = (exportsField, subpath) => {
    if (typeof exportsField === 'string' || Array.isArray(exportsField)) return subpath === '.' ? condition(exportsField) : null;
    if (!exportsField || typeof exportsField !== 'object') return null;
    const keys = Object.keys(exportsField);
    if (!keys.some(k => k.startsWith('.'))) return subpath === '.' ? condition(exportsField) : null;
    if (subpath in exportsField) return condition(exportsField[subpath]);
    for (const key of keys.filter(k => k.includes('*')).sort((a, b) => b.length - a.length)) {
      const [prefix, suffix] = key.split('*');
      if (subpath.startsWith(prefix) && subpath.endsWith(suffix) && subpath.length >= prefix.length + suffix.length) {
        const target = condition(exportsField[key]);
        if (target) return target.split('*').join(subpath.slice(prefix.length, subpath.length - suffix.length));
      }
    }
    return null;
  };
  const manifestResolve = specifier => {
    const pkg = packages.find(p => specifier === p.name || specifier.startsWith(p.name + '/'));
    if (!pkg) return null;
    const subpath = '.' + specifier.slice(pkg.name.length);
    const m = pkg.manifest;
    let target = m.exports !== undefined ? exportTarget(m.exports, subpath) : null;
    if (!target && m.exports === undefined) target = subpath === '.' ? (m.types || m.typings || m.module || m.main || 'index') : subpath;
    if (!target) return { pkg, target: null };
    return { pkg, target: withExtensions(path.join(pkg.dir, target)) };
  };
  const packageRoot = s => s.startsWith('@') ? s.split('/').slice(0, 2).join('/') : s.split('/')[0];

  const resolutions = new Map();
  const resolve = (specifier, from, part) => {
    const key = `${part ? part.config : ''}\0${path.dirname(from)}\0${specifier}`;
    if (resolutions.has(key)) return resolutions.get(key);
    let result;
    if (/\?[\w&=-]*$/.test(specifier) && specifier.includes('?')) {
      // Bundler asset queries (?url, ?raw, ?inline) name non-code assets.
      result = { status: 'bundler_asset' };
      resolutions.set(key, result);
      return result;
    }
    const compiled = part && ts.resolveModuleName(specifier, from, part.options, system, part.cache).resolvedModule;
    if (compiled && codeFile(compiled.resolvedFileName)) result = { target: compiled.resolvedFileName, method: 'compiler' };
    else if (specifier.startsWith('.') || specifier.startsWith('/')) {
      const base = path.normalize(path.join(path.dirname(from), specifier));
      const target = withExtensions(base);
      if (target) result = { target, method: 'relative' };
      else if (files.has(base)) result = { status: 'non_code', target: base };
      // Binary assets are excluded by capture; their absence is not a code gap.
      else if (/\.[a-z0-9]+$/i.test(base) && !isCode(base) && !/\.json$/i.test(base)) result = { status: 'non_code_unretained' };
      else result = { status: 'missing' };
    } else {
      const manifest = manifestResolve(specifier);
      if (manifest?.target) result = { target: manifest.target, method: 'workspace_manifest' };
      else if (manifest) result = { status: 'missing_workspace_export' };
      else if (compiled) result = { status: 'non_code', target: compiled.resolvedFileName };
      else result = { status: 'external', package: packageRoot(specifier) };
    }
    resolutions.set(key, result);
    return result;
  };

  // Deterministic analysis order with whole-file bounds.
  const candidates = retained.filter(isCode).sort();
  const analyzed = [];
  const omitted = [];
  for (const p of candidates) {
    if (!owner.has(p)) omitted.push({ path: relative(p), reason: 'outside_partitions' });
    else if (analyzed.length >= MAX_FILES) omitted.push({ path: relative(p), reason: 'file_bound' });
    else analyzed.push(p);
  }

  const byte = (sf, p) => Buffer.byteLength(sf.text.slice(0, p), 'utf8');
  const span = (sf, n) => ({ path: relative(sf.fileName), start: byte(sf, n.getStart(sf)), end: byte(sf, n.end) });
  const sources = new Map();
  const tables = new Map();
  const definitions = [];
  const diagnostics = [];
  const omittedByBound = [];
  const exported = n => (ts.getCombinedModifierFlags(n) & ts.ModifierFlags.Export) !== 0;

  for (const fileName of analyzed) {
    const sf = ts.createSourceFile(fileName, files.get(fileName), ts.ScriptTarget.Latest, true,
      /\.tsx$|\.jsx$/.test(fileName) ? ts.ScriptKind.TSX : undefined);
    const defs = [];
    const table = { local: new Set(), reexports: new Map(), stars: [], bindings: new Map() };
    for (const n of sf.statements) {
      const named = n.name && ts.isIdentifier(n.name) && (ts.isFunctionDeclaration(n) || ts.isClassDeclaration(n)
        || ts.isInterfaceDeclaration(n) || ts.isTypeAliasDeclaration(n) || ts.isEnumDeclaration(n) || ts.isModuleDeclaration(n));
      if (named) {
        defs.push({ ...span(sf, n), key: `${relative(fileName)}::${n.name.text}#${byte(sf, n.getStart(sf))}`,
          callable: ts.isFunctionDeclaration(n) && !!n.body && !sf.isDeclarationFile, kind: ts.SyntaxKind[n.kind] });
        if (exported(n)) table.local.add((ts.getCombinedModifierFlags(n) & ts.ModifierFlags.Default) ? 'default' : n.name.text);
      } else if (ts.isVariableStatement(n)) {
        for (const d of n.declarationList.declarations) {
          if (!ts.isIdentifier(d.name)) continue;
          defs.push({ ...span(sf, d), key: `${relative(fileName)}::${d.name.text}#${byte(sf, d.getStart(sf))}`,
            callable: false, kind: 'VariableDeclaration' });
          if (exported(n)) table.local.add(d.name.text);
        }
      } else if (ts.isExportAssignment(n)) table.local.add('default');
      else if (ts.isExportDeclaration(n) && !n.moduleSpecifier && n.exportClause && ts.isNamedExports(n.exportClause)) {
        for (const e of n.exportClause.elements) {
          table.reexports.set(e.name.text, { local: (e.propertyName || e.name).text });
        }
      }
    }
    if (definitions.length + defs.length > MAX_DEFINITIONS) { omittedByBound.push(fileName); continue; }
    definitions.push(...defs);
    sources.set(fileName, sf);
    tables.set(fileName, table);
    for (const d of sf.parseDiagnostics || []) {
      diagnostics.push({ path: relative(fileName), code: d.code, start: d.start === undefined ? null : byte(sf, d.start),
        message: ts.flattenDiagnosticMessageText(d.messageText, '\n') });
    }
  }
  for (const p of omittedByBound) omitted.push({ path: relative(p), reason: 'definition_bound' });

  // Module edges. Each occurrence keeps its statement span.
  const imports = [];
  const unresolved = [];
  const dict = () => Object.create(null);
  const counts = { by_method: dict(), by_status: dict(), external_packages: dict(), reexport_edges: 0 };
  const bump = (o, k, n = 1) => { o[k] = (o[k] || 0) + n; };
  const seen = new Set();
  let importBound = false;
  const edge = (sf, n, target, specifier, method) => {
    const s = span(sf, n);
    const key = `${s.path}\0${s.start}\0${target}`;
    if (seen.has(key) || target === sf.fileName) return;
    if (imports.length >= MAX_IMPORTS) { importBound = true; return; }
    seen.add(key);
    imports.push({ ...s, target: relative(target), specifier, method });
    bump(counts.by_method, method);
  };
  const moduleOf = new Map();
  const specifierOf = (sf, literal, n) => {
    const part = owner.get(sf.fileName);
    const r = resolve(literal.text, sf.fileName, part);
    if (r.target && !r.status) { moduleOf.set(n, r.target); return r; }
    bump(counts.by_status, r.status);
    if (r.status === 'external') bump(counts.external_packages, r.package);
    // Only retained-scope failures are actionable occurrences; external
    // packages are outside the retained snapshot by construction.
    if (r.status === 'missing' || r.status === 'missing_workspace_export') {
      unresolved.push({ ...span(sf, n), predicate: 'IMPORTS', reason: 'missing_dependency', caller: null, specifier: literal.text });
    }
    return r;
  };
  for (const [fileName, sf] of sources) {
    const table = tables.get(fileName);
    const visit = n => {
      let literal = null;
      if ((ts.isImportDeclaration(n) || ts.isExportDeclaration(n)) && n.moduleSpecifier) literal = n.moduleSpecifier;
      else if (ts.isImportEqualsDeclaration(n) && ts.isExternalModuleReference(n.moduleReference)) literal = n.moduleReference.expression;
      else if (ts.isCallExpression(n) && n.arguments.length === 1 && ts.isStringLiteralLike(n.arguments[0])
          && (n.expression.kind === ts.SyntaxKind.ImportKeyword || (ts.isIdentifier(n.expression) && n.expression.text === 'require'))) {
        literal = n.arguments[0];
      }
      if (literal && ts.isStringLiteralLike(literal)) {
        const r = specifierOf(sf, literal, n);
        if (r.target && !r.status) {
          edge(sf, n, r.target, literal.text, r.method);
          if (ts.isExportDeclaration(n) && n.parent === sf) {
            if (!n.exportClause) table.stars.push(r.target);
            else if (ts.isNamespaceExport(n.exportClause)) table.reexports.set(n.exportClause.name.text, { module: r.target });
            else for (const e of n.exportClause.elements) table.reexports.set(e.name.text, { target: r.target, name: (e.propertyName || e.name).text });
          }
          if (ts.isImportDeclaration(n) && n.importClause) {
            const c = n.importClause;
            if (c.name) table.bindings.set(c.name.text, { target: r.target, name: 'default' });
            if (c.namedBindings && ts.isNamedImports(c.namedBindings)) {
              for (const e of c.namedBindings.elements) table.bindings.set(e.name.text, { target: r.target, name: (e.propertyName || e.name).text });
            }
          }
        }
      }
      ts.forEachChild(n, visit);
    };
    visit(sf);
  }
  // `export { a }` of an imported binding is a re-export of that binding.
  for (const table of tables.values()) {
    for (const [name, r] of table.reexports) {
      if (r.local && !table.local.has(r.local) && table.bindings.has(r.local)) table.reexports.set(name, table.bindings.get(r.local));
      else if (r.local) { table.reexports.delete(name); table.local.add(name); }
    }
  }
  const declaring = (file, name, depth = 0, visited = new Set()) => {
    const table = tables.get(file);
    if (!table || depth > MAX_CHAIN || visited.has(file)) return null;
    visited.add(file);
    if (table.local.has(name)) return file;
    const r = table.reexports.get(name);
    if (r) return r.module || declaring(r.target, r.name, depth + 1, visited) || null;
    for (const star of table.stars) {
      const found = name !== 'default' && declaring(star, name, depth + 1, visited);
      if (found) return found;
    }
    return null;
  };
  // Named imports from a barrel also link the importer to the declaring module.
  for (const [fileName, sf] of sources) {
    for (const n of sf.statements) {
      const target = moduleOf.get(n);
      if (!target || !ts.isImportDeclaration(n) || !n.importClause) continue;
      const c = n.importClause;
      const names = [];
      if (c.name) names.push('default');
      if (c.namedBindings && ts.isNamedImports(c.namedBindings)) names.push(...c.namedBindings.elements.map(e => (e.propertyName || e.name).text));
      for (const name of names) {
        const found = declaring(target, name);
        if (found && found !== target) { const before = imports.length; edge(sf, n, found, n.moduleSpecifier.text, 'reexport'); counts.reexport_edges += imports.length - before; }
      }
    }
  }

  // Calls are classified syntactically and aggregated; no call is resolved.
  const constructs = Object.create(null);
  const record = (kind, sf, n) => {
    const entry = constructs[kind] || (constructs[kind] = { count: 0, examples: [] });
    entry.count += 1;
    if (entry.examples.length < 3) entry.examples.push(span(sf, n));
  };
  for (const [fileName, sf] of sources) {
    const table = tables.get(fileName);
    const externalBinding = new Set();
    for (const n of sf.statements) {
      if (!ts.isImportDeclaration(n) || !ts.isStringLiteralLike(n.moduleSpecifier) || moduleOf.has(n) || !n.importClause) continue;
      if (!/^effect(\/|$)|^@effect\//.test(n.moduleSpecifier.text)) continue;
      const c = n.importClause;
      if (c.name) externalBinding.add(c.name.text);
      if (c.namedBindings) {
        if (ts.isNamespaceImport(c.namedBindings)) externalBinding.add(c.namedBindings.name.text);
        else for (const e of c.namedBindings.elements) externalBinding.add(e.name.text);
      }
    }
    const rootName = e => { while (ts.isPropertyAccessExpression(e) || ts.isElementAccessExpression(e) || ts.isCallExpression(e)) e = e.expression; return ts.isIdentifier(e) ? e.text : null; };
    const visit = n => {
      if (ts.isCallExpression(n)) {
        const e = n.expression;
        if (e.kind === ts.SyntaxKind.ImportKeyword) record('dynamic_import', sf, n);
        else if (ts.isIdentifier(e) && e.text === 'require') record('require', sf, n);
        else if (ts.isPropertyAccessExpression(e) || ts.isElementAccessExpression(e)) {
          record(externalBinding.has(rootName(e)) ? 'effect_namespace_call' : 'method_or_property_call', sf, n);
        } else if (ts.isIdentifier(e)) {
          record(externalBinding.has(e.text) ? 'effect_function_call' : table.bindings.has(e.text) ? 'imported_function_call' : 'local_identifier_call', sf, n);
        } else record('computed_callee', sf, n);
      } else if (ts.isNewExpression(n)) record('constructor', sf, n);
      else if (ts.isTaggedTemplateExpression(n)) record('tagged_template', sf, n);
      else if (ts.isJsxOpeningElement(n) || ts.isJsxSelfClosingElement(n)) record('jsx_element', sf, n);
      else if (ts.isYieldExpression(n) && n.asteriskToken) record('effect_yield_star', sf, n);
      ts.forEachChild(n, visit);
    };
    visit(sf);
  }

  // Modules: analyzed files plus retained import targets outside them.
  const moduleSet = new Set(sources.keys());
  for (const i of imports) moduleSet.add(path.join(root, i.target));
  const omittedReasons = Object.create(null);
  for (const o of omitted) bump(omittedReasons, o.reason);
  const provider = { name: 'typescript-compiler-api', version: ts.version, compiler_digest: compilerDigest(),
    node_version: process.version, project: request.project,
    node_executable_digest: 'sha256:' + require('node:crypto').createHash('sha256').update(require('node:fs').readFileSync(process.execPath)).digest('hex'),
    mapping: 'retained-typescript-workspace-modules-v1',
    libraries: 'unavailable; no ambient/external target authority',
    module_resolution: 'per-partition compiler resolution, then retained workspace manifests (exports conditions types/import/module/default), then explicit relative paths; captured inputs only',
    emit: false };
  process.stdout.write(JSON.stringify({ schema_version: '1.0', provider, workspace: {
      partitions: partitions.map(({ config, status, reason, degraded, message, root_files, references }) => ({ config, status, reason, degraded, message, root_files, references })),
      bounds: { max_files: MAX_FILES, max_definitions: MAX_DEFINITIONS, max_imports: MAX_IMPORTS, import_bound_reached: importBound },
      bounded: importBound || omittedByBound.length > 0 || omitted.some(o => o.reason === 'file_bound'),
      import_resolution: counts, omitted_by_reason: omittedReasons, call_constructs: constructs,
      calls: 'not_resolved: workspace mode classifies call constructs and resolves none' },
    modules: [...moduleSet].sort().map(p => ({ path: relative(p), end: Buffer.byteLength(files.get(p)),
      analyzed: sources.has(p), language: /\.[cm]?jsx?$/.test(p) ? 'javascript' : 'typescript' })),
    definitions, imports, calls: [], unresolved, diagnostics, omitted: omitted.map(o => o.path) }));
};
