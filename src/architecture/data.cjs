// Checker-backed recognition of the data boundary: Fastify route registration,
// Zod request schemas, Prisma client operations and field-level value origins.
// Runs over an in-memory retained snapshot. Nothing here publishes knowledge;
// the Rust boundary validates and constructs every canonical record.
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
if (!files.has(project)) throw new Error('Project configuration was not captured');
const config = ts.readConfigFile(project, system.readFile);
if (config.error) throw new Error(ts.flattenDiagnosticMessageText(config.error.messageText, '\n'));
const parsed = ts.parseJsonConfigFileContent(config.config, system, path.dirname(project), undefined, project);
if (parsed.errors.length) throw new Error(parsed.errors.map(d => ts.flattenDiagnosticMessageText(d.messageText, '\n')).join('\n'));
if (parsed.projectReferences?.length) throw new Error('Project references are outside this producer scope');
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
const sourceFiles = program.getSourceFiles()
  .filter(s => files.has(s.fileName) && isCode(s.fileName) && !s.isDeclarationFile);
if (sourceFiles.length > 4096) throw new Error('Project file limit exceeded (4096)');
const byte = (sf, p) => Buffer.byteLength(sf.text.slice(0, p), 'utf8');
const span = n => ({ path: relative(n.getSourceFile().fileName),
  start: byte(n.getSourceFile(), n.getStart()), end: byte(n.getSourceFile(), n.end) });
const textOf = n => n.getText();

// Same function identity formula as the TS/JS structure producer, so data
// facts attach to the existing compiler-verified symbol entities.
const functionKeys = new Map();
for (const sf of sourceFiles) {
  for (const n of sf.statements) {
    if (ts.isFunctionDeclaration(n) && n.name && n.body) {
      functionKeys.set(n, `${relative(sf.fileName)}::${n.name.text}#${byte(sf, n.getStart())}`);
    }
  }
}
const resolved = node => {
  let s = checker.getSymbolAtLocation(node);
  if (s && s.flags & ts.SymbolFlags.Alias) s = checker.getAliasedSymbol(s);
  return s;
};
// The explicit import an identifier binds to: {module, name} or null.
const importOf = identifier => {
  const s = checker.getSymbolAtLocation(identifier);
  for (const d of s?.declarations || []) {
    let n = d;
    let imported = null;
    if (ts.isImportSpecifier(n)) imported = (n.propertyName || n.name).text;
    else if (ts.isImportClause(n)) imported = 'default';
    else if (ts.isNamespaceImport(n)) imported = '*';
    else continue;
    while (n && !ts.isImportDeclaration(n)) n = n.parent;
    if (n && ts.isStringLiteral(n.moduleSpecifier)) return { module: n.moduleSpecifier.text, name: imported };
  }
  return null;
};
const unwrap = e => {
  while (e && (ts.isParenthesizedExpression(e) || ts.isAwaitExpression(e))) e = e.expression;
  return e;
};
const propertyName = p => {
  if (!p.name) return null;
  if (ts.isIdentifier(p.name) || ts.isStringLiteral(p.name)) return p.name.text;
  return null;
};

// Mutated bindings cannot carry a declared DTO identity.
const mutated = new Set();
const assignmentTarget = n => {
  const s = resolved(n);
  if (s) mutated.add(s);
  ts.forEachChild(n, assignmentTarget);
};
const mutations = n => {
  if (ts.isBinaryExpression(n) && n.operatorToken.kind >= ts.SyntaxKind.FirstAssignment
      && n.operatorToken.kind <= ts.SyntaxKind.LastAssignment) assignmentTarget(n.left);
  else if ((ts.isPrefixUnaryExpression(n) || ts.isPostfixUnaryExpression(n))
      && [ts.SyntaxKind.PlusPlusToken, ts.SyntaxKind.MinusMinusToken].includes(n.operator)) assignmentTarget(n.operand);
  ts.forEachChild(n, mutations);
};
for (const sf of sourceFiles) mutations(sf);

const unresolved = [];
const unknown = (n, predicate, reason, caller, detail = {}) => {
  unresolved.push({ ...span(n), predicate, reason, caller, ...detail });
};

// --- Zod request schemas -------------------------------------------------
const dtos = [];
const dtoByDeclaration = new Map();
const isZod = identifier => {
  const i = importOf(identifier);
  return i && i.module === 'zod' && (i.name === 'z' || i.name === '*');
};
for (const sf of sourceFiles) {
  for (const statement of sf.statements) {
    if (!ts.isVariableStatement(statement)
        || !(statement.declarationList.flags & ts.NodeFlags.Const)) continue;
    for (const d of statement.declarationList.declarations) {
      const init = d.initializer;
      if (!ts.isIdentifier(d.name) || !init || !ts.isCallExpression(init)) continue;
      const callee = init.expression;
      if (!ts.isPropertyAccessExpression(callee) || callee.name.text !== 'object'
          || !ts.isIdentifier(callee.expression) || !isZod(callee.expression)) continue;
      const key = `${relative(sf.fileName)}::${d.name.text}`;
      const dto = { key, name: d.name.text, ...span(d), fields: [] };
      const shape = init.arguments[0];
      if (init.arguments.length !== 1 || !shape || !ts.isObjectLiteralExpression(shape)) {
        unknown(init, 'TRANSFORMS', 'dynamic_property', null, { dto: key });
        continue;
      }
      for (const p of shape.properties) {
        const name = ts.isPropertyAssignment(p) ? propertyName(p) : null;
        if (!name) {
          unknown(p, 'TRANSFORMS', ts.isSpreadAssignment(p) ? 'spread_object' : 'dynamic_property', null, { dto: key });
          continue;
        }
        dto.fields.push({ name, key: `${key}.${name}`, ...span(p), type_text: textOf(p.initializer) });
      }
      dtos.push(dto);
      dtoByDeclaration.set(d, dto);
    }
  }
}
// `typeof X` where X is a recognized Zod object schema.
const dtoOfQuery = node => {
  if (!node || !ts.isTypeQueryNode(node) || !ts.isIdentifier(node.exprName)) return null;
  const s = resolved(node.exprName);
  for (const d of s?.declarations || []) if (dtoByDeclaration.has(d)) return dtoByDeclaration.get(d);
  return null;
};
// `z.infer<typeof X>` directly or through a type alias.
const dtoOfType = (type, depth = 0) => {
  if (!type || depth > 4 || !ts.isTypeReferenceNode(type)) return null;
  const name = type.typeName;
  if (ts.isQualifiedName(name) && name.right.text === 'infer' && ts.isIdentifier(name.left)
      && isZod(name.left) && type.typeArguments?.length === 1) return dtoOfQuery(type.typeArguments[0]);
  const s = resolved(name);
  for (const d of s?.declarations || []) {
    if (ts.isTypeAliasDeclaration(d)) {
      const dto = dtoOfType(d.type, depth + 1);
      if (dto) return dto;
    }
  }
  return null;
};
// Variables and parameters bound to a DTO: parameter type annotations and
// `const v = X.parse(...)`. Bindings never cross unsupported assignments.
const bindings = new Map();
const bindingRecords = [];
const bindingDeclarations = new Map();
const bind = (declaration, dto, kind, functionKey) => {
  const s = checker.getSymbolAtLocation(declaration.name);
  if (!s || mutated.has(s)) return;
  bindings.set(s, dto);
  const record = { ...span(declaration), dto: dto.key, kind, function: functionKey, variable: declaration.name.text };
  bindingRecords.push(record);
  bindingDeclarations.set(s, record);
};
const parseTarget = init => {
  init = unwrap(init);
  if (!init || !ts.isCallExpression(init) || !ts.isPropertyAccessExpression(init.expression)
      || init.expression.name.text !== 'parse' || !ts.isIdentifier(init.expression.expression)) return null;
  const s = resolved(init.expression.expression);
  for (const d of s?.declarations || []) if (dtoByDeclaration.has(d)) return dtoByDeclaration.get(d);
  return null;
};
for (const [fn, key] of functionKeys) {
  for (const p of fn.parameters) {
    if (!ts.isIdentifier(p.name)) continue;
    const dto = dtoOfType(p.type);
    if (dto) bind(p, dto, 'parameter_type', key);
  }
  const walk = n => {
    if (ts.isFunctionLike(n) && n !== fn) return;
    if (ts.isVariableDeclaration(n) && ts.isIdentifier(n.name)
        && n.parent && n.parent.flags & ts.NodeFlags.Const) {
      const dto = parseTarget(n.initializer);
      if (dto) bind(n, dto, 'schema_parse', key);
    }
    ts.forEachChild(n, walk);
  };
  walk(fn.body);
}
const boundDto = identifier => {
  const s = checker.getSymbolAtLocation(identifier);
  return s ? bindings.get(s) || null : null;
};
const bindingOf = identifier => bindingDeclarations.get(checker.getSymbolAtLocation(identifier)) || null;
// `v.field` where v is DTO-bound and field is a declared schema property.
const dtoField = e => {
  e = unwrap(e);
  if (!e || !ts.isPropertyAccessExpression(e) || !ts.isIdentifier(e.expression)) return null;
  const dto = boundDto(e.expression);
  const field = dto?.fields.find(f => f.name === e.name.text);
  return field ? { dto, field, binding: bindingOf(e.expression) } : null;
};
// Does the expression read any variable or parameter (possible request data)?
const readsData = e => {
  let found = false;
  const visit = n => {
    if (found) return;
    if (ts.isIdentifier(n) && !(n.parent && ts.isPropertyAccessExpression(n.parent) && n.parent.name === n)) {
      const s = resolved(n);
      if ((s?.declarations || []).some(d => ts.isParameter(d) || ts.isVariableDeclaration(d)
          || ts.isBindingElement(d))) found = true;
    }
    ts.forEachChild(n, visit);
  };
  visit(e);
  return found;
};
const mentionedFields = e => {
  const out = new Set();
  const visit = n => {
    if (ts.isIdentifier(n)) {
      const dto = boundDto(n);
      if (dto) {
        const parent = n.parent;
        if (parent && ts.isPropertyAccessExpression(parent) && parent.expression === n) {
          const f = dto.fields.find(f => f.name === parent.name.text);
          if (f) out.add(f.key);
        } else dto.fields.forEach(f => out.add(f.key));
      }
    }
    ts.forEachChild(n, visit);
  };
  visit(e);
  return [...out];
};
const globalConversion = identifier => {
  if (!['String', 'Number', 'Boolean'].includes(identifier.text)) return false;
  const s = checker.getSymbolAtLocation(identifier);
  return (s?.declarations || []).every(d => d.getSourceFile().isDeclarationFile);
};
// Deterministic value-origin classes. Anything else is an explicit boundary.
const classify = (value, target) => {
  const e = unwrap(value);
  if (ts.isStringLiteral(e) || ts.isNumericLiteral(e) || ts.isNoSubstitutionTemplateLiteral(e)
      || e.kind === ts.SyntaxKind.TrueKeyword || e.kind === ts.SyntaxKind.FalseKeyword
      || e.kind === ts.SyntaxKind.NullKeyword) return { origin: 'literal' };
  const direct = dtoField(e);
  if (direct) {
    return { origin: 'dto_field', source: direct,
      transformation: direct.field.name === target ? { kind: 'identity' }
        : { kind: 'rename', parameters: `${direct.field.name} -> ${target}` } };
  }
  if (ts.isBinaryExpression(e) && [ts.SyntaxKind.AsteriskToken, ts.SyntaxKind.SlashToken].includes(e.operatorToken.kind)) {
    const operator = e.operatorToken.getText();
    const pairs = [[e.left, e.right], [e.right, e.left]];
    for (const [field, constant] of operator === '/' ? [pairs[0]] : pairs) {
      const f = dtoField(field);
      const c = unwrap(constant);
      if (f && ts.isNumericLiteral(c)) {
        return { origin: 'dto_field', source: f,
          transformation: { kind: 'constant_scaling', parameters: `${operator} ${c.text}` } };
      }
    }
  }
  if (ts.isCallExpression(e) && e.arguments.length === 0 && ts.isPropertyAccessExpression(e.expression)
      && ['trim', 'toLowerCase', 'toUpperCase'].includes(e.expression.name.text)) {
    const f = dtoField(e.expression.expression);
    if (f) return { origin: 'dto_field', source: f,
      transformation: { kind: 'string_normalization', parameters: `${e.expression.name.text}()` } };
  }
  if ((ts.isCallExpression(e) && ts.isIdentifier(e.expression) && globalConversion(e.expression)
       || ts.isNewExpression(e) && ts.isIdentifier(e.expression) && e.expression.text === 'Date')
      && e.arguments?.length === 1) {
    const f = dtoField(e.arguments[0]);
    if (f) return { origin: 'dto_field', source: f,
      transformation: { kind: 'conversion', parameters: `${e.expression.text}(...)` } };
  }
  if (!readsData(e)) return { origin: 'generated' };
  return { origin: 'boundary', reason: ts.isConditionalExpression(e) ? 'conditional_mapping' : 'unsupported_transformation',
    candidates: mentionedFields(e) };
};

// --- Prisma client operations -------------------------------------------
const isPrismaClient = identifier => {
  const s = resolved(identifier);
  return (s?.declarations || []).some(d => {
    if (!ts.isVariableDeclaration(d) || !(d.parent.flags & ts.NodeFlags.Const)) return false;
    const init = d.initializer && unwrap(d.initializer);
    if (!init || !ts.isNewExpression(init) || !ts.isIdentifier(init.expression)) return false;
    const clientSymbol = checker.getSymbolAtLocation(init.expression);
    const i = importOfDeclared(clientSymbol);
    return i && i.module === '@prisma/client' && i.name === 'PrismaClient';
  });
};
function importOfDeclared(symbol) {
  for (const d of symbol?.declarations || []) {
    if (!ts.isImportSpecifier(d)) continue;
    let n = d;
    while (n && !ts.isImportDeclaration(n)) n = n.parent;
    if (n && ts.isStringLiteral(n.moduleSpecifier)) return { module: n.moduleSpecifier.text, name: (d.propertyName || d.name).text };
  }
  return null;
}
const WRITE_OPS = new Set(['create', 'update']);
const READ_OPS = new Set(['findUnique', 'findFirst', 'findMany']);
const operations = [];
const objectProperty = (object, name) => object.properties.find(p => propertyName(p) === name);
const prismaOperation = (call, caller) => {
  const callee = call.expression;
  // prisma.$queryRaw..., prisma.$executeRaw...: raw SQL never yields column lineage.
  if (ts.isPropertyAccessExpression(callee) && ts.isIdentifier(callee.expression)
      && callee.name.text.startsWith('$') && isPrismaClient(callee.expression)) {
    unknown(call, 'READS', 'raw_sql', caller, { operation: callee.name.text });
    return true;
  }
  if (!ts.isPropertyAccessExpression(callee)) return false;
  const delegate = callee.expression;
  if (ts.isElementAccessExpression(delegate) && ts.isIdentifier(delegate.expression) && isPrismaClient(delegate.expression)) {
    unknown(call, WRITE_OPS.has(callee.name.text) ? 'WRITES' : 'READS', 'dynamic_model', caller, { operation: callee.name.text });
    return true;
  }
  if (!ts.isPropertyAccessExpression(delegate) || !ts.isIdentifier(delegate.expression)
      || !isPrismaClient(delegate.expression)) return false;
  const op = callee.name.text;
  const model = delegate.name.text;
  if (!caller) {
    unknown(call, WRITE_OPS.has(op) ? 'WRITES' : 'READS', 'unsupported_context', null, { operation: op, model });
    return true;
  }
  if (!WRITE_OPS.has(op) && !READ_OPS.has(op)) {
    unknown(call, 'WRITES', 'unsupported_operation', caller, { operation: op, model });
    return true;
  }
  const record = { ...span(call), function: caller, model_accessor: model, operation: op,
    kind: WRITE_OPS.has(op) ? 'write' : 'read', assignments: [], selected: null, where: [] };
  const arg = call.arguments[0] && unwrap(call.arguments[0]);
  if (call.arguments.length > 1 || (arg && !ts.isObjectLiteralExpression(arg)) || (!arg && op !== 'findMany')) {
    unknown(call, record.kind === 'write' ? 'WRITES' : 'READS', 'dynamic_property', caller, { operation: op, model });
    return true;
  }
  const where = arg && objectProperty(arg, 'where');
  if (where && ts.isPropertyAssignment(where) && ts.isObjectLiteralExpression(unwrap(where.initializer))) {
    for (const p of unwrap(where.initializer).properties) {
      const name = propertyName(p);
      if (name) record.where.push({ field: name, ...span(p) });
    }
  }
  if (record.kind === 'write') {
    const data = objectProperty(arg, 'data');
    if (!data || !ts.isPropertyAssignment(data) || !ts.isObjectLiteralExpression(unwrap(data.initializer))) {
      unknown(call, 'WRITES', 'dynamic_property', caller, { operation: op, model });
      return true;
    }
    record.data = span(data);
    for (const p of unwrap(data.initializer).properties) {
      if (ts.isSpreadAssignment(p)) {
        unknown(p, 'WRITES', 'spread_object', caller, { operation: op, model, candidates: mentionedFields(p.expression) });
        record.partial = true;
        continue;
      }
      const name = propertyName(p);
      if (!name || !(ts.isPropertyAssignment(p) || ts.isShorthandPropertyAssignment(p))) {
        unknown(p, 'WRITES', 'dynamic_property', caller, { operation: op, model,
          candidates: ts.isPropertyAssignment(p) ? mentionedFields(p.initializer) : [] });
        record.partial = true;
        continue;
      }
      const value = ts.isPropertyAssignment(p) ? p.initializer : p.name;
      const c = classify(value, name);
      const assignment = { field: name, ...span(p), value_text: textOf(value), origin: c.origin };
      if (c.origin === 'dto_field') {
        assignment.source_field = c.source.field.key;
        assignment.dto = c.source.dto.key;
        assignment.transformation = c.transformation;
        assignment.binding = c.source.binding;
      } else if (c.origin === 'boundary') {
        assignment.reason = c.reason;
        unknown(p, 'TRANSFORMS', c.reason, caller, { operation: op, model, field: name, candidates: c.candidates });
      }
      record.assignments.push(assignment);
    }
  } else {
    const select = arg && objectProperty(arg, 'select');
    if (select) {
      const value = ts.isPropertyAssignment(select) && unwrap(select.initializer);
      if (!value || !ts.isObjectLiteralExpression(value)) {
        unknown(call, 'READS', 'dynamic_property', caller, { operation: op, model });
        return true;
      }
      record.selected = [];
      for (const p of value.properties) {
        const name = propertyName(p);
        if (name && ts.isPropertyAssignment(p) && p.initializer.kind === ts.SyntaxKind.TrueKeyword) {
          record.selected.push({ field: name, ...span(p) });
        } else {
          unknown(p, 'READS', 'dynamic_property', caller, { operation: op, model });
          record.partial = true;
        }
      }
    }
    if (arg && objectProperty(arg, 'include')) {
      unknown(call, 'READS', 'unsupported_operation', caller, { operation: op, model, detail: 'include' });
      record.partial = true;
    }
  }
  operations.push(record);
  return true;
};

// --- Routes ----------------------------------------------------------------
const routes = [];
const HTTP = new Set(['get', 'post', 'put', 'patch', 'delete']);
const isFastifyInstance = identifier => {
  const s = resolved(identifier);
  return (s?.declarations || []).some(d => {
    if (ts.isParameter(d) && d.type && ts.isTypeReferenceNode(d.type) && ts.isIdentifier(d.type.typeName)) {
      const i = importOf(d.type.typeName);
      return i && i.module === 'fastify' && i.name === 'FastifyInstance';
    }
    if (ts.isVariableDeclaration(d) && d.initializer && ts.isCallExpression(unwrap(d.initializer))) {
      const callee = unwrap(d.initializer).expression;
      const i = ts.isIdentifier(callee) && importOf(callee);
      return i && i.module === 'fastify' && i.name === 'default';
    }
    return false;
  });
};
const route = (call, caller) => {
  const callee = call.expression;
  if (!ts.isPropertyAccessExpression(callee) || !HTTP.has(callee.name.text)
      || !ts.isIdentifier(callee.expression) || !isFastifyInstance(callee.expression)) return false;
  const [pathArg, handler] = call.arguments;
  const target = handler && ts.isIdentifier(handler) && resolved(handler);
  const declaration = (target?.declarations || []).find(d => functionKeys.has(d));
  if (call.arguments.length !== 2 || !pathArg || !ts.isStringLiteral(pathArg) || !declaration
      || mutated.has(target)) {
    unknown(call, 'ROUTES_TO', 'dynamic_property', caller);
    return true;
  }
  routes.push({ ...span(call), method: callee.name.text.toUpperCase(), route: pathArg.text,
    handler: functionKeys.get(declaration), registrar: caller });
  return true;
};

// --- DTO escapes ---------------------------------------------------------
// A whole DTO value passed to an unanalyzed callee is a lineage boundary.
const escape = (call, caller) => {
  const callee = unwrap(call.expression);
  let target = null;
  if (ts.isIdentifier(callee)) {
    const s = resolved(callee);
    target = (s?.declarations || []).find(d => functionKeys.has(d)) || null;
  }
  call.arguments.forEach((argument, index) => {
    const a = unwrap(argument);
    if (!ts.isIdentifier(a)) return;
    const dto = boundDto(a);
    if (!dto) return;
    const parameter = target?.parameters[index];
    if (parameter && dtoOfType(parameter.type) === dto) return;
    unknown(call, 'WRITES', 'opaque_wrapper', caller, { candidates: dto.fields.map(f => f.key), dto: dto.key });
  });
};

const visit = (n, caller = null) => {
  if (ts.isFunctionLike(n)) caller = functionKeys.get(n) || null;
  if (ts.isClassDeclaration(n) || ts.isClassExpression(n)) caller = null;
  if (ts.isCallExpression(n)) {
    if (!prismaOperation(n, caller) && !route(n, caller)) escape(n, caller);
  }
  if (ts.isTaggedTemplateExpression(n) && ts.isPropertyAccessExpression(n.tag)
      && ts.isIdentifier(n.tag.expression) && isPrismaClient(n.tag.expression)) {
    unknown(n, 'READS', 'raw_sql', caller, { operation: n.tag.name.text });
  }
  ts.forEachChild(n, child => visit(child, caller));
};
for (const sf of sourceFiles) visit(sf);
const diagnostics = [];
for (const sf of sourceFiles) {
  for (const d of [...program.getSyntacticDiagnostics(sf), ...program.getSemanticDiagnostics(sf)]) {
    if (d.category === ts.DiagnosticCategory.Error) {
      diagnostics.push({ path: relative(sf.fileName), code: d.code,
        start: d.start === undefined ? null : byte(sf, d.start),
        message: ts.flattenDiagnosticMessageText(d.messageText, '\n') });
    }
  }
}
if (operations.length + unresolved.length + routes.length > 16384) throw new Error('Data occurrence limit exceeded (16384)');
const provider = { name: 'briefcase-data-boundary', version: '1', typescript_version: ts.version,
  compiler_digest: 'sha256:' + crypto.createHash('sha256').update(fs.readFileSync(compilerPath)).digest('hex'),
  node_version: process.version, project: request.project,
  node_executable_digest: 'sha256:' + crypto.createHash('sha256').update(fs.readFileSync(process.execPath)).digest('hex'),
  recognizes: ['fastify-routes-v1', 'zod-object-dto-v1', 'prisma-client-operations-v1', 'field-value-origin-v1'] };
process.stdout.write(JSON.stringify({ schema_version: '1.0', provider, dtos, bindings: bindingRecords,
  routes, operations, unresolved, diagnostics }));
