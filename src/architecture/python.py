"""CPython syntax and lexical binding analysis over retained inputs only.

This adapter never imports, evaluates or executes repository code. Rust owns
canonical records. Unstable bindings and dynamic execution stay unresolved.
"""

import _imp
import ast
import hashlib
import json
import os
import symtable
import sys
import sysconfig
from pathlib import PurePosixPath

MAX_FILES = 4096
MAX_DEFINITIONS = 4096
MAX_OCCURRENCES = 16384
EXTENSIONS = {".py", ".pyi", ".pyw"}


def digest_file(path):
    with open(path, "rb") as source:
        return "sha256:" + hashlib.file_digest(source, "sha256").hexdigest()


def provider(roots, syntax_only):
    if sys.implementation.name != "cpython" or sys.version_info < (3, 11):
        raise ValueError("Python analysis requires CPython 3.11 or newer")
    library = os.path.join(
        sysconfig.get_config_var("LIBDIR") or "",
        sysconfig.get_config_var("LDLIBRARY") or "",
    )
    return {
        "name": "cpython-ast-symtable",
        "version": sys.version,
        "executable_digest": digest_file(sys.executable),
        "compiler_library_digest": (digest_file(library) if os.path.isfile(library) else None),
        "ast_digest": digest_file(ast.__file__),
        "symtable_digest": digest_file(symtable.__file__),
        "adapter_digest": digest_file(__file__),
        "mapping": "retained-python-bindings-v1",
        "source_roots": roots,
        "syntax_only": syntax_only,
        "isolated": bool(
            sys.flags.isolated and sys.flags.no_site and sys.flags.dont_write_bytecode
        ),
        "module_resolution": "retained source roots only; no installed packages or import hooks",
        "stub_resolution": ".py implementations before paired .pyi; .pyw is script-only",
        "executes_source": False,
    }


def source_roots(files, configured):
    roots = configured or (["src", "."] if any(p.startswith("src/") for p in files) else ["."])
    if not roots or len(roots) > 64 or len(set(roots)) != len(roots):
        raise ValueError("Python source roots must be unique (1..64)")
    for root in roots:
        path = PurePosixPath(root)
        if (
            not root
            or path.is_absolute()
            or ".." in path.parts
            or str(path) != root
            or "\\" in root
        ):
            raise ValueError("Python source roots must be normalized relative paths")
    return sorted(roots, key=lambda root: (-len(PurePosixPath(root).parts), root))


class Scope:
    def __init__(self, module, table, parent=None, kind="module", caller=None, qualifiers=()):
        self.module = module
        self.table = table
        self.parent = parent
        self.kind = kind
        self.caller = caller
        self.qualifiers = qualifiers
        self.bindings = {}
        self.loaded = set()
        self.opaque = False
        self.children = {}
        if table is not None:
            for child in table.get_children():
                key = (str(child.get_type()), child.get_name(), child.get_lineno())
                self.children.setdefault(key, []).append(child)

    def root(self):
        scope = self
        while scope.parent is not None:
            scope = scope.parent
        return scope


class Analysis:
    def __init__(self, request):
        self.files = request["files"]
        self.roots = source_roots(self.files, request.get("source_roots", []))
        self.syntax_only = request.get("syntax_only", False)
        self.modules = {}
        self.packages = {}
        self.index = {}
        self.namespace_index = {}
        self.definitions = []
        self.definition_nodes = {}
        self.assignment_values = {}
        self.import_nodes = []
        self.callsites = []
        self.references = []
        self.attribute_writes = []
        self.escapes = []
        self.dynamic_imports = []
        self.unconditional_imports = set()
        self.unsafe_functions = set()
        self.project_dynamic = False
        self.imports = []
        self.calls = []
        self.resolved_references = []
        self.unresolved = []
        self.diagnostics = []
        self.omitted = []

    def span(self, module, node):
        lines = module["lines"]
        start = lines[node.lineno - 1] + node.col_offset
        end = lines[node.end_lineno - 1] + node.end_col_offset
        return {"path": module["path"], "start": start, "end": end}

    def unknown(self, module, node, predicate, reason, caller=None, candidates=(), detail=None):
        self.unresolved.append(
            {
                **self.span(module, node),
                "predicate": predicate,
                "reason": reason,
                "status": "ambiguous" if candidates else "unresolved",
                "caller": caller,
                "candidate_keys": sorted(set(candidates)),
                "detail": detail,
            }
        )

    def load(self):
        paths = sorted(p for p in self.files if PurePosixPath(p).suffix.lower() in EXTENSIONS)
        if not paths or len(paths) > MAX_FILES:
            raise ValueError("Python analysis needs 1..4096 retained Python files")
        for path in paths:
            code = self.files[path]
            parse_source = code[1:] if code.startswith("\ufeff") else code
            offsets = [3 if code.startswith("\ufeff") else 0]
            for line in parse_source.encode("utf-8").splitlines(keepends=True):
                offsets.append(offsets[-1] + len(line))
            root = next((r for r in self.roots if r == "." or path.startswith(r + "/")), None)
            relative = path if root == "." else path[len(root) + 1 :] if root is not None else path
            parts = list(PurePosixPath(relative).with_suffix("").parts)
            package = parts[-1] == "__init__"
            if package:
                parts.pop()
            name = (
                ".".join(parts)
                if parts and all(p.isidentifier() for p in parts) and root is not None
                else None
            )
            module = {
                "path": path,
                "source": code,
                "lines": offsets,
                "name": name,
                "package_name": (name if package else name.rpartition(".")[0] if name else ""),
                "package": None,
                "stub": path.lower().endswith(".pyi"),
                "script": path.lower().endswith(".pyw"),
                "is_package": package,
                "configured": root is not None,
                "parsed": False,
                "tree": None,
                "scope": None,
            }
            if module["script"]:
                module["package_name"] = ""
            self.modules[path] = module
            if root is None:
                self.omitted.append(path)
            if name and not module["script"]:
                self.index.setdefault(name, []).append(path)
            # Identifier directories describe regular and PEP 420 namespace packages.
            directory_parts = list(PurePosixPath(relative).parent.parts)
            for index in range(1, len(directory_parts) + 1):
                prefix = directory_parts[:index]
                if not all(part.isidentifier() for part in prefix) or root is None:
                    break
                directory = "/".join(([root] if root != "." else []) + prefix)
                key = "package::" + directory
                parent = (
                    "package::" + "/".join(([root] if root != "." else []) + prefix[:-1])
                    if index > 1
                    else None
                )
                entry = self.packages.setdefault(
                    key,
                    {"key": key, "parent": parent, "path": directory, "source": path},
                )
                # A nonempty member supplies a valid source span, even with an empty __init__.
                if not self.files[entry["source"]] and code:
                    entry["source"] = path
                self.namespace_index.setdefault(".".join(prefix), set()).add(key)
                module["package"] = key
            try:
                tree = ast.parse(parse_source, filename=path, type_comments=True)
                # Compilation validates control-flow/scope constraints without executing bytecode.
                compile(tree, path, "exec", dont_inherit=True)
                table = symtable.symtable(parse_source, path, "exec")
                module.update(parsed=True, tree=tree, scope=Scope(module, table))
                for node in tree.body:
                    self.collect(node, module["scope"], True)
            except (SyntaxError, ValueError, RecursionError) as error:
                self.diagnostics.append(
                    {
                        "path": path,
                        "line": getattr(error, "lineno", None),
                        "message": str(error),
                    }
                )
                # Discard partial facts from a failed file, not valid facts from other files.
                self.definitions = [d for d in self.definitions if d["path"] != path]
                self.definition_nodes = {
                    node: d for node, d in self.definition_nodes.items() if d["path"] != path
                }
                self.assignment_values = {
                    key: value
                    for key, value in self.assignment_values.items()
                    if value[0].module != module
                }
                self.unresolved = [u for u in self.unresolved if u["path"] != path]
                self.import_nodes = [(s, n) for s, n in self.import_nodes if s.module != module]
                self.callsites = [(s, n) for s, n in self.callsites if s.module != module]
                self.references = [(s, n) for s, n in self.references if s.module != module]
                self.attribute_writes = [
                    (s, n) for s, n in self.attribute_writes if s.module != module
                ]
                self.escapes = [(s, n) for s, n in self.escapes if s.module != module]
                self.dynamic_imports = [
                    (s, n) for s, n in self.dynamic_imports if s.module != module
                ]
                module.update(parsed=False, tree=None, scope=None)
                if code:
                    for predicate in ("IMPORTS", "CALLS_STATIC", "REFERENCES"):
                        self.unresolved.append(
                            {
                                "path": path,
                                "start": 0,
                                "end": len(code.encode("utf-8")),
                                "predicate": predicate,
                                "reason": "extraction_failure",
                                "status": "unsupported",
                                "caller": None,
                                "candidate_keys": [],
                                "detail": str(error),
                            }
                        )
        if len(self.definitions) > MAX_DEFINITIONS:
            raise ValueError("Python definition limit exceeded (4096)")

    def child_table(self, scope, node, kind, name):
        if scope.table is None:
            return None
        candidates = scope.children.get((kind, name, node.lineno), ())
        return candidates[0] if len(candidates) == 1 else None

    def owner(self, scope, name):
        if scope.kind == "module":
            return scope
        if scope.table is None:
            return None
        try:
            symbol = scope.table.lookup(name)
        except KeyError:
            return None
        if symbol.is_global():
            return scope.root()
        if symbol.is_free() or symbol.is_nonlocal():
            parent = scope.parent
            while parent is not None:
                if parent.kind != "class" and parent.table is not None:
                    try:
                        binding = parent.table.lookup(name)
                        if binding.is_local() or binding.is_parameter():
                            return parent
                    except KeyError:
                        pass
                parent = parent.parent
            return None
        return scope if symbol.is_local() or symbol.is_parameter() else None

    def bind(self, scope, name, value):
        owner = self.owner(scope, name)
        if owner is not None:
            owner.bindings.setdefault(name, []).append(value)

    def definition(self, scope, node, name, kind, callable=False):
        if len(self.definitions) >= MAX_DEFINITIONS:
            raise OverflowError("Python definition limit exceeded (4096)")
        span = self.span(scope.module, node)
        decorators = getattr(node, "decorator_list", [])
        if decorators:
            span["start"] = self.span(scope.module, decorators[0])["start"] - 1
        key = f'{scope.module["path"]}::{"::".join((*scope.qualifiers, name))}#{span["start"]}'
        definition = {
            **span,
            "key": key,
            "name": name,
            "declaration_kind": kind,
            "top_level": scope.kind == "module",
            "callable": callable,
        }
        self.definitions.append(definition)
        self.definition_nodes[id(node)] = definition
        return ("function" if callable else "symbol", key, scope.module["path"])

    def collect(self, node, scope, direct=False):
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            function = not isinstance(node, ast.ClassDef)
            callable = (
                function
                and direct
                and scope.kind in {"module", "function"}
                and scope.table is not None
                and not scope.module["stub"]
                and scope.module["configured"]
                and not node.decorator_list
            )
            value = self.definition(
                scope, node, node.name, "function" if function else "class", callable
            )
            self.bind(scope, node.name, ("definition", value, not direct))
            table = self.child_table(scope, node, "function" if function else "class", node.name)
            child = Scope(
                scope.module,
                table,
                scope,
                "function" if function else "class",
                value[1] if callable else None,
                (*scope.qualifiers, node.name),
            )
            # Defaults, decorators and annotations do not inherit the defined body's caller.
            for field, value_node in ast.iter_fields(node):
                if field == "body":
                    continue
                if isinstance(value_node, ast.AST):
                    target_scope = (
                        Scope(scope.module, None, scope, "annotation")
                        if field in {"returns", "type_params"}
                        else scope
                    )
                    self.collect(value_node, target_scope)
                elif isinstance(value_node, list):
                    for item in value_node:
                        if isinstance(item, ast.AST):
                            target_scope = (
                                Scope(scope.module, None, scope, "annotation")
                                if field == "type_params"
                                else scope
                            )
                            self.collect(item, target_scope)
            for decorator in node.decorator_list if not scope.module["stub"] else []:
                self.unknown(
                    scope.module,
                    decorator,
                    "CALLS_STATIC",
                    "dynamic_property",
                    scope.caller,
                    detail="Decorator application may replace the function or class binding",
                )
            if function:
                for argument in ast.walk(node.args):
                    if isinstance(argument, ast.arg):
                        self.bind(child, argument.arg, ("write", None, False))
            for statement in node.body:
                self.collect(statement, child, True)
            return
        if isinstance(node, ast.arg) and node.annotation is not None:
            self.collect(node.annotation, Scope(scope.module, None, scope, "annotation"))
            return
        if isinstance(node, ast.Lambda):
            child = Scope(
                scope.module,
                self.child_table(scope, node, "function", "lambda"),
                scope,
                "lambda",
            )
            self.collect(node.args, scope)
            for argument in ast.walk(node.args):
                if isinstance(argument, ast.arg):
                    self.bind(child, argument.arg, ("write", None, False))
            self.collect(node.body, child)
            return
        if isinstance(node, (ast.ListComp, ast.SetComp, ast.DictComp, ast.GeneratorExp)):
            child = Scope(scope.module, None, scope, "comprehension")
            for item in ast.walk(node):
                if isinstance(item, ast.NamedExpr) and isinstance(item.target, ast.Name):
                    self.bind(scope, item.target.id, ("write", None, True))
            for item in ast.iter_child_nodes(node):
                self.collect(item, child)
            return
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            if isinstance(node, ast.ImportFrom) and node.module == "__future__" and not node.level:
                return  # A compiler directive, not a captured module dependency.
            self.import_nodes.append((scope, node))
            if direct:
                self.unconditional_imports.add(id(node))
            for alias in node.names:
                if alias.name == "*":
                    scope.opaque = True
                else:
                    name = alias.asname or (
                        alias.name.split(".")[0] if isinstance(node, ast.Import) else alias.name
                    )
                    self.bind(scope, name, ("import", (node, alias), not direct))
            return
        if isinstance(node, (ast.Assign, ast.AnnAssign)):
            targets = node.targets if isinstance(node, ast.Assign) else [node.target]
            for target in targets:
                for item in ast.walk(target):
                    if isinstance(item, ast.Name) and isinstance(item.ctx, ast.Store):
                        value = self.definition(scope, item, item.id, "variable")
                        if (
                            isinstance(node, ast.AnnAssign)
                            and node.value is None
                            and not scope.module["stub"]
                        ):
                            self.bind(scope, item.id, ("write", None, True))
                        if item is target and node.value is not None:
                            self.assignment_values[value[1]] = (scope, node.value)
            if isinstance(node, ast.AnnAssign):
                self.collect(node.target, scope, direct)
                self.collect(node.annotation, Scope(scope.module, None, scope, "annotation"))
                if node.value is not None:
                    self.collect(node.value, scope, direct)
                return
        if (
            hasattr(ast, "TypeAlias")
            and isinstance(node, ast.TypeAlias)
            and isinstance(node.name, ast.Name)
        ):
            self.definition(scope, node, node.name.id, "type_alias")
            self.definition_nodes[id(node.name)] = self.definition_nodes[id(node)]
            self.collect(node.name, scope, direct)
            annotation = Scope(scope.module, None, scope, "annotation")
            for item in node.type_params:
                self.collect(item, annotation)
            self.collect(node.value, annotation)
            return
        if isinstance(node, ast.ExceptHandler) and node.name:
            self.bind(scope, node.name, ("write", None, True))
        if isinstance(node, (ast.MatchAs, ast.MatchStar)) and node.name:
            self.bind(scope, node.name, ("write", None, True))
        if isinstance(node, ast.MatchMapping) and node.rest:
            self.bind(scope, node.rest, ("write", None, True))
        if isinstance(node, ast.Name):
            if isinstance(node.ctx, (ast.Store, ast.Del)):
                definition = self.definition_nodes.get(id(node))
                value = ("symbol", definition["key"], scope.module["path"]) if definition else None
                self.bind(
                    scope,
                    node.id,
                    ("definition" if value else "write", value, not direct),
                )
            elif isinstance(node.ctx, ast.Load):
                self.references.append((scope, node))
                if node.id in {
                    "exec",
                    "eval",
                    "globals",
                    "locals",
                    "vars",
                    "setattr",
                    "delattr",
                    "getattr",
                    "__import__",
                }:
                    self.escapes.append((scope, node))
        if isinstance(node, ast.Attribute):
            if isinstance(node.ctx, (ast.Store, ast.Del)):
                self.attribute_writes.append((scope, node))
            elif isinstance(node.ctx, ast.Load):
                self.references.append((scope, node))
                if node.attr in {
                    "exec",
                    "eval",
                    "globals",
                    "locals",
                    "vars",
                    "setattr",
                    "delattr",
                    "getattr",
                    "__import__",
                }:
                    self.escapes.append((scope, node))
                if node.attr in {"import_module", "reload"}:
                    self.dynamic_imports.append((scope, node))
                if node.attr in {
                    "__dict__",
                    "__globals__",
                    "__builtins__",
                    "__code__",
                    "__closure__",
                    "__class__",
                    "__path__",
                }:
                    self.project_dynamic = True
        if isinstance(node, ast.Call):
            self.callsites.append((scope, node))
            if isinstance(node.func, (ast.Name, ast.Attribute)) and (
                getattr(node.func, "id", None) or getattr(node.func, "attr", None)
            ) in {
                "exec",
                "eval",
                "globals",
                "locals",
                "vars",
                "setattr",
                "delattr",
                "getattr",
                "__import__",
            }:
                self.escapes.append((scope, node))
            if isinstance(node.func, ast.Attribute) and node.func.attr in {
                "import_module",
                "reload",
            }:
                self.dynamic_imports.append((scope, node))
            if (
                isinstance(node.func, ast.Attribute)
                and isinstance(node.func.value, ast.Attribute)
                and node.func.value.attr
                in {"path", "meta_path", "path_hooks", "modules", "__path__"}
            ):
                self.project_dynamic = True
                self.unknown(
                    scope.module,
                    node,
                    "IMPORTS",
                    "dynamic_property",
                    scope.caller,
                    detail="Import-system mutation is outside retained source resolution",
                )
        conditional = isinstance(
            node,
            (
                ast.If,
                ast.For,
                ast.AsyncFor,
                ast.While,
                ast.With,
                ast.AsyncWith,
                ast.Try,
                ast.Match,
            ),
        ) or (hasattr(ast, "TryStar") and isinstance(node, ast.TryStar))
        for child in ast.iter_child_nodes(node):
            self.collect(child, scope, direct and not conditional)

    def module_value(self, name):
        # Competing roots and non-package parents cannot establish a unique
        # local import environment, including regular/namespace shadowing.
        pieces = name.split(".")
        if pieces[0] in sys.builtin_module_names or _imp.is_frozen(pieces[0]):
            return None, ()  # Builtin/frozen modules cannot be shadowed by local files.
        for i in range(1, len(pieces) + 1):
            prefix = ".".join(pieces[:i])
            packages = self.namespace_index.get(prefix, ())
            parents = self.index.get(prefix, []) if i < len(pieces) else []
            selected_parents = [p for p in parents if not self.modules[p]["stub"]] or parents
            if selected_parents and any(not self.modules[p]["parsed"] for p in selected_parents):
                return None, ()
            if len(packages) > 1 or any(not self.modules[p]["is_package"] for p in parents):
                return None, tuple(
                    sorted(set(packages) | set(parents) | set(self.index.get(name, [])))
                )
        paths = self.index.get(name, [])
        implementations = [p for p in paths if not self.modules[p]["stub"]]
        # Paired stubs cannot replace runtime implementations. Competing roots
        # or module/package definitions stay ambiguous rather than guessing sys.path.
        selected = implementations or paths
        if len({str(PurePosixPath(p).with_suffix("")) for p in paths}) > 1:
            return None, tuple(paths)
        if len(selected) == 1:
            if not self.modules[selected[0]]["parsed"]:
                return None, ()
            return ("module", name, selected[0]), ()
        if selected:
            return None, tuple(selected)
        packages = sorted(self.namespace_index.get(name, ()))
        if len(packages) == 1:
            return ("module", name, packages[0]), ()
        return None, tuple(packages)

    def absolute_from(self, module, node):
        if not node.level:
            return node.module or ""
        package = module["package_name"].split(".") if module["package_name"] else []
        if len(package) < node.level:
            return None
        prefix = package[: len(package) - node.level + 1]
        return ".".join(prefix + ((node.module or "").split(".") if node.module else []))

    def name_value(self, scope, name, seen=frozenset()):
        owner = self.owner(scope, name)
        if owner is None or owner.opaque:
            return None
        token = (id(owner), name)
        if token in seen or len(seen) >= 128:
            return None
        values = []
        bindings = owner.bindings.get(name, [])
        for kind, value, conditional in bindings:
            if conditional or kind == "write":
                return None
            if kind == "definition":
                values.append(value)
            else:
                values.append(self.import_value(owner, *value, seen | {token}))
        # Repeated identical imports are stable; a definition plus another
        # binding, or two declarations, is not an immutable function binding.
        if len(bindings) > 1 and any(b[0] != "import" for b in bindings):
            return None
        if not values or any(v is None for v in values) or len(set(values)) != 1:
            return None
        return values[0]

    def import_value(self, scope, node, alias, seen):
        if isinstance(node, ast.Import):
            imported, _ = self.module_value(alias.name)
            if imported is None:
                return None
            if alias.asname:
                return imported
            return self.module_value(alias.name.split(".")[0])[0]
        name = self.absolute_from(scope.module, node)
        base = self.module_value(name)[0] if name else None
        return self.export_value(base, alias.name, scope, seen, True) if base else None

    def export_value(self, base, name, scope, seen, allow_submodule=False):
        if base is None or base[0] != "module":
            return None
        target = self.modules.get(base[2])
        if target and not target["is_package"]:
            allow_submodule = False
        if target and target["scope"]:
            exports = target["scope"]
            if name in exports.bindings or exports.opaque:
                return self.name_value(exports, name, seen)
        child_name = base[1] + "." + name
        current = scope
        loaded = False
        while current is not None:
            loaded = loaded or child_name in current.loaded
            current = current.parent
        is_package = target is None or target["is_package"]
        return (
            self.module_value(child_name)[0] if is_package and (allow_submodule or loaded) else None
        )

    def expression_value(self, scope, expression):
        if isinstance(expression, ast.Name):
            return self.name_value(scope, expression.id)
        if isinstance(expression, ast.Attribute):
            return self.export_value(
                self.expression_value(scope, expression.value),
                expression.attr,
                scope,
                frozenset(),
            )
        return None

    def resolve_imports(self):
        for scope, node in self.import_nodes:
            if not scope.module["configured"] or self.syntax_only:
                self.unknown(
                    scope.module,
                    node,
                    "IMPORTS",
                    "unsupported_context",
                    scope.caller,
                    detail="Semantic resolution is disabled or the file is outside configured source roots",
                )
                continue
            for alias in node.names:
                if alias.name == "*":
                    self.unknown(
                        scope.module,
                        node,
                        "IMPORTS",
                        "unsupported_generated_source",
                        scope.caller,
                        detail="Wildcard exports are not expanded",
                    )
                    continue
                name = (
                    alias.name
                    if isinstance(node, ast.Import)
                    else self.absolute_from(scope.module, node)
                )
                base, candidates = self.module_value(name) if name else (None, ())
                if base is None:
                    self.unknown(
                        scope.module,
                        node,
                        "IMPORTS",
                        "missing_dependency",
                        scope.caller,
                        candidates,
                        detail=f"Module {name!r} is absent or ambiguous in the retained source roots",
                    )
                    continue
                self.imports.append(
                    {
                        **self.span(scope.module, node),
                        "target": base[2],
                        "specifier": name,
                    }
                )
                if isinstance(node, ast.Import) and id(node) in self.unconditional_imports:
                    for i in range(1, len(name.split(".")) + 1):
                        scope.loaded.add(".".join(name.split(".")[:i]))
                if isinstance(node, ast.ImportFrom):
                    value = self.export_value(base, alias.name, scope, frozenset(), True)
                    if value is None:
                        self.unknown(
                            scope.module,
                            node,
                            "IMPORTS",
                            "missing_dependency",
                            scope.caller,
                            detail=f"Imported member {alias.name!r} has no unique retained binding",
                        )
                    elif value[0] == "module" and value[2] != base[2]:
                        if id(node) in self.unconditional_imports:
                            scope.loaded.add(value[1])
                        self.imports.append(
                            {
                                **self.span(scope.module, node),
                                "target": value[2],
                                "specifier": name + "." + alias.name,
                            }
                        )
                # Importing a child also initializes its regular parent packages.
                for i in range(1, len(name.split("."))):
                    parent, ambiguous = self.module_value(".".join(name.split(".")[:i]))
                    if parent and parent[2] in self.modules:
                        self.imports.append(
                            {
                                **self.span(scope.module, node),
                                "target": parent[2],
                                "specifier": parent[1],
                            }
                        )
                    elif ambiguous:
                        self.unknown(
                            scope.module,
                            node,
                            "IMPORTS",
                            "missing_dependency",
                            scope.caller,
                            ambiguous,
                            detail="A parent package has competing retained definitions",
                        )

    def mutation_values(self, scope, expression, seen=frozenset()):
        """Follow assignment aliases only for invalidation, never call promotion."""
        if len(seen) >= 128:
            self.project_dynamic = True
            return set()
        value = self.expression_value(scope, expression)
        if value is not None and value[0] != "symbol":
            return {value}
        if isinstance(expression, ast.Subscript):
            return self.mutation_values(scope, expression.value, seen)
        if isinstance(expression, (ast.Tuple, ast.List, ast.Set, ast.Dict)):
            values = set()
            for item in ast.iter_child_nodes(expression):
                values.update(self.mutation_values(scope, item, seen))
            return values
        candidates = []
        if isinstance(expression, ast.Name):
            owner = self.owner(scope, expression.id)
            if owner:
                candidates = []
                for kind, binding, _ in owner.bindings.get(expression.id, ()):
                    if kind == "import":
                        imported = self.import_value(owner, *binding, frozenset())
                        if imported:
                            candidates.append(imported)
                    elif kind == "definition" and binding and binding[1] in self.assignment_values:
                        candidates.append(binding)
        elif value and value[1] in self.assignment_values:
            candidates = [value]
        values = set()
        for candidate in candidates:
            if candidate[0] != "symbol":
                values.add(candidate)
            elif candidate[1] not in seen:
                owner, assigned = self.assignment_values[candidate[1]]
                values.update(self.mutation_values(owner, assigned, seen | {candidate[1]}))
        return values

    def mutations(self):
        for scope, node in self.import_nodes:
            if not isinstance(node, ast.ImportFrom):
                continue
            for alias in node.names:
                reflection = node.module == "builtins" and alias.name in {
                    "exec",
                    "eval",
                    "globals",
                    "locals",
                    "vars",
                    "setattr",
                    "delattr",
                    "getattr",
                    "__import__",
                }
                loader = node.module == "importlib" and alias.name in {"import_module", "reload"}
                import_system = node.module == "sys" and alias.name in {
                    "path",
                    "meta_path",
                    "path_hooks",
                    "modules",
                }
                if (reflection or loader or import_system) and self.import_value(
                    scope, node, alias, frozenset()
                ) is None:
                    self.project_dynamic = True
                    if loader or import_system or alias.name == "__import__":
                        self.unknown(
                            scope.module,
                            node,
                            "IMPORTS",
                            "dynamic_property",
                            scope.caller,
                            detail="Imported reflective/import-system aliases are outside retained binding guarantees",
                        )
        for scope, node in self.attribute_writes:
            self.unknown(
                scope.module,
                node,
                "CALLS_STATIC",
                "dynamic_property",
                scope.caller,
                detail="Attribute mutation and descriptor execution are outside exact direct-call scope",
            )
            for base in self.mutation_values(scope, node.value):
                if base[0] == "module" and base[2] in self.modules:
                    target = self.modules[base[2]]["scope"]
                    if target:
                        target.bindings.setdefault(node.attr, []).append(("write", None, True))
                elif base[0] == "function":
                    self.unsafe_functions.add(base[1])
            # Import-system changes invalidate source-root assumptions.
            if node.attr in {"path", "meta_path", "path_hooks", "modules", "__path__"}:
                self.project_dynamic = True
                self.unknown(
                    scope.module,
                    node,
                    "IMPORTS",
                    "dynamic_property",
                    scope.caller,
                    detail="Import-system mutation is outside retained source resolution",
                )
        for scope, node in self.dynamic_imports:
            expression = node.func if isinstance(node, ast.Call) else node
            value = self.expression_value(scope, expression)
            if value and value[0] == "function":
                continue
            self.project_dynamic = True
            self.unknown(
                scope.module,
                node,
                "IMPORTS",
                "dynamic_property",
                scope.caller,
                detail="Dynamic import loaders are not executed",
            )
        for scope, node in self.escapes:
            # A uniquely declared function named eval/exec/etc is not a builtin escape.
            expression = node.func if isinstance(node, ast.Call) else node
            value = self.expression_value(scope, expression)
            if value and value[0] == "function":
                continue
            self.project_dynamic = True
            if (
                getattr(expression, "id", None) or getattr(expression, "attr", None)
            ) == "__import__":
                self.unknown(
                    scope.module,
                    node,
                    "IMPORTS",
                    "dynamic_property",
                    scope.caller,
                    detail="Dynamic import loaders are not executed",
                )
        if self.project_dynamic:
            self.diagnostics.append(
                {
                    "path": None,
                    "line": None,
                    "message": "Dynamic execution, reflective mutation or import hooks prevent exact call/binding guarantees",
                }
            )

    def resolve_execution(self):
        for scope, node in self.callsites:
            if scope.module["stub"]:
                continue
            target = self.expression_value(scope, node.func)
            if (
                not self.syntax_only
                and not self.project_dynamic
                and scope.caller
                and target
                and target[0] == "function"
                and target[1] not in self.unsafe_functions
                and scope.caller not in self.unsafe_functions
                and not scope.module["stub"]
            ):
                self.calls.append(
                    {
                        **self.span(scope.module, node),
                        "caller": scope.caller,
                        "target": target[1],
                    }
                )
            else:
                self.unknown(
                    scope.module,
                    node,
                    "CALLS_STATIC",
                    "dynamic_property" if scope.caller else "unsupported_context",
                    scope.caller,
                    detail="A stable direct free-function binding and a supported caller are required",
                )
        for scope, node in self.references:
            target = self.expression_value(scope, node)
            if not self.syntax_only and not self.project_dynamic and target:
                self.resolved_references.append(
                    {
                        **self.span(scope.module, node),
                        "caller": scope.caller,
                        "target": target[2] if target[0] == "module" else target[1],
                    }
                )
            else:
                self.unknown(
                    scope.module,
                    node,
                    "REFERENCES",
                    "dynamic_property",
                    scope.caller,
                    detail="No stable retained lexical or imported binding",
                )

    def output(self):
        self.load()
        if len(self.import_nodes) + len(self.callsites) + len(self.references) > MAX_OCCURRENCES:
            raise ValueError("Python source occurrence limit exceeded (16384)")
        # Populate module namespace visibility before resolving cross-file re-exports.
        for scope, node in self.import_nodes:
            if id(node) not in self.unconditional_imports:
                continue
            for alias in node.names:
                if isinstance(node, ast.Import):
                    name = alias.name if self.module_value(alias.name)[0] else None
                else:
                    value = (
                        self.import_value(scope, node, alias, frozenset())
                        if alias.name != "*"
                        else None
                    )
                    name = value[1] if value and value[0] == "module" else None
                if name:
                    scope.loaded.update(
                        ".".join(name.split(".")[:i]) for i in range(1, len(name.split(".")) + 1)
                    )
        self.mutations()
        self.resolve_imports()
        self.resolve_execution()

        def unique(rows):
            return sorted(
                {json.dumps(row, sort_keys=True): row for row in rows}.values(),
                key=lambda row: json.dumps(row, sort_keys=True),
            )

        imports, calls, references, unresolved = map(
            unique,
            (self.imports, self.calls, self.resolved_references, self.unresolved),
        )
        if len(imports) + len(calls) + len(references) + len(unresolved) > MAX_OCCURRENCES:
            raise ValueError("Python source occurrence limit exceeded (16384)")
        return {
            "schema_version": "1.0",
            "provider": provider(self.roots, self.syntax_only),
            "modules": [
                {
                    k: m[k]
                    for k in (
                        "path",
                        "name",
                        "package",
                        "stub",
                        "script",
                        "configured",
                        "parsed",
                    )
                }
                for m in self.modules.values()
            ],
            "packages": sorted(self.packages.values(), key=lambda p: p["key"]),
            "definitions": sorted(
                self.definitions, key=lambda d: (d["path"], d["start"], d["key"])
            ),
            "imports": imports,
            "calls": calls,
            "references": references,
            "unresolved": unresolved,
            "diagnostics": self.diagnostics,
            "omitted": self.omitted,
        }


def main():
    request = json.load(sys.stdin)
    if request.get("schema_version") != "1.0":
        raise ValueError("Unsupported Python adapter request")
    if request.get("identity_only"):
        result = {
            "schema_version": "1.0",
            "provider": provider(
                source_roots({}, request["source_roots"]) if request.get("source_roots") else [],
                request.get("syntax_only", False),
            ),
        }
    else:
        result = Analysis(request).output()
    json.dump(result, sys.stdout, sort_keys=True, ensure_ascii=True)


if __name__ == "__main__":
    main()
