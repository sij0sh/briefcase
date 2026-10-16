//! Rust syntax boundaries plus native resolution of supported explicit imports.
//! Default scans do not emit calls. The opt-in execution producer verifies callsites independently.
use super::{
    analyzer::{self, Analyzer},
    capture::Capture,
    contracts::{add, rows, text},
    identity,
    ledger::Prepared,
    require, Result,
};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use tree_sitter::{Node, Parser};

#[derive(Clone)]
pub(crate) struct Occurrence {
    pub(crate) path: String,
    pub(crate) module: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) position: usize,
    pub(crate) kind: &'static str,
}
#[derive(Clone)]
pub(crate) struct Definition {
    pub(crate) path: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) name_start: usize,
    pub(crate) key: String,
    pub(crate) kind: &'static str,
    pub(crate) function: bool,
}
#[derive(Default)]
pub(crate) struct Syntax {
    pub(crate) definitions: Vec<Definition>,
    pub(crate) occurrences: Vec<Occurrence>,
    pub(crate) modules: BTreeMap<String, String>,
    pub(crate) diagnostics: Vec<String>,
    pub(crate) visited: BTreeSet<String>,
    pub(crate) unsupported: usize,
    pub(crate) parse_error_count: usize,
    pub(crate) inclusions: Vec<ModuleInclusion>,
    pub(crate) test_owners: BTreeMap<String,BTreeSet<String>>,
    pub(crate) test_manifests: BTreeMap<String,BTreeSet<String>>,
    contexts: BTreeSet<(String,String)>,
    active_paths: BTreeSet<String>,
    test_root: Option<String>,
    trees: BTreeMap<String,tree_sitter::Tree>,
}
#[derive(Clone)]
pub(crate) struct ModuleInclusion {
    pub(crate) from: String,
    pub(crate) to: String,
    pub(crate) module: String,
    pub(crate) start: usize,
    pub(crate) end: usize,
}
fn source<'a>(capture: &'a Capture, path: &str) -> Result<&'a str> {
    Ok(std::str::from_utf8(
        capture.files.get(path).ok_or("Missing source input")?,
    )?)
}
fn node_text<'a>(node: Node, code: &'a str) -> &'a str {
    &code[node.start_byte()..node.end_byte()]
}
fn module_dir(path: &str) -> PathBuf {
    let p = Path::new(path);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    if ["lib", "main", "mod"].contains(&stem) {
        p.parent().unwrap_or(Path::new("")).to_path_buf()
    } else {
        p.with_extension("")
    }
}
/// Literal #[path] modules are syntax declarations over retained bytes. The
/// caller supplies the source or inline directory; conditional/escaping paths
/// remain unknown.
fn literal_module_path(attribute: &str, base: &Path) -> Option<String> {
    let value = attribute.strip_prefix("#[")?.strip_suffix(']')?.trim();
    let value = value
        .strip_prefix("path")?
        .trim_start()
        .strip_prefix('=')?
        .trim();
    let relative: String = serde_json::from_str(value).ok()?;
    relative_source_path(base,&relative)
}
fn relative_source_path(base:&Path,relative:&str)->Option<String> {
    if relative.is_empty() || relative.contains(['\\', '\0']) {
        return None;
    }
    let relative = Path::new(relative);
    if relative.is_absolute() {
        return None;
    }
    let joined = base.join(relative);
    let mut parts = Vec::new();
    for part in joined.components() {
        match part {
            std::path::Component::Normal(p) => parts.push(p.to_str()?),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                parts.pop()?;
            }
            _ => return None,
        }
    }
    Some(parts.join("/"))
}

fn qualified(module: &str, name: &str) -> String {
    if module == "crate" {
        name.to_owned()
    } else {
        format!("{module}::{name}")
    }
}
fn visit_module(
    s: &mut Syntax,
    c: &Capture,
    path: &str,
    key: &str,
    parser: &mut Parser,
    root_directory: Option<PathBuf>,
) -> Result<()> {
    if !s.active_paths.insert(path.to_owned()) {
        s.unsupported+=1;s.diagnostics.push(format!("cyclic Rust module inclusion: {path}"));
        return Ok(());
    }
    if !s.contexts.insert((path.into(),key.into())) {s.active_paths.remove(path);return Ok(());}
    let first_visit=s.visited.insert(path.into());
    if let Some(root)=&s.test_root {s.test_owners.entry(path.into()).or_default().insert(root.clone());}
    let code = source(c, path)?;
    let tree=if let Some(tree)=s.trees.get(path) {tree.clone()}else{
        let tree=parser.parse(code,None).ok_or("Rust parse failed")?;
        s.trees.insert(path.into(),tree.clone());tree
    };
    s.modules.insert(key.to_owned(), path.to_owned());
    s.definitions.push(Definition {
        path: path.into(),
        start: 0,
        end: code.len(),
        name_start: 0,
        key: key.into(),
        kind: "module",
        function: false,
    });
    if first_visit && tree.root_node().has_error() {
        s.unsupported += 1;
        let errors=parse_errors(tree.root_node());
        s.parse_error_count+=errors.len();
        for (start,end) in errors.iter().take(8) {
            let line=1+code[..*start].bytes().filter(|b|*b==b'\n').count();
            s.diagnostics.push(format!("parse errors in {path}:{line} at bytes {start}..{end}"));
        }
        if errors.len()>8 {s.diagnostics.push(format!("{} additional parse boundaries in {path}",errors.len()-8));}
    }
    let result=walk(
        s,
        c,
        path,
        key,
        tree.root_node(),
        code,
        parser,
        root_directory.unwrap_or_else(||module_dir(path)),
    );
    s.active_paths.remove(path);
    result
}

/// Localize parser failures instead of treating a whole file as one failed
/// callsite. Valid sibling declarations/calls remain independently inspectable.
pub(crate) fn parse_errors(node:Node<'_>)->Vec<(usize,usize)> {
    fn collect(node:Node<'_>,out:&mut Vec<(usize,usize)>) {
        if node.is_error() || node.is_missing() {out.push((node.start_byte(),node.end_byte()));return;}
        if node.has_error() {let mut cursor=node.walk();for child in node.children(&mut cursor) {collect(child,out);}}
    }
    let mut errors=vec![];collect(node,&mut errors);errors
}
fn leaves(node: Node, out: &mut Vec<(usize, usize)>) {
    if node.kind() == "use_as_clause" {
        if let Some(path) = node
            .child_by_field_name("path")
            .or_else(|| node.named_child(0))
        {
            leaves(path, out);
        }
    } else if node.kind() == "scoped_identifier" || node.kind() == "scoped_type_identifier" {
        if let Some(name) = node.child_by_field_name("name") {
            out.push((name.start_byte(), name.end_byte()));
        }
    } else if node.kind() == "identifier"
        || node.kind() == "type_identifier"
        || node.kind() == "self"
        || node.kind() == "crate"
        || node.kind() == "super"
    {
        out.push((node.start_byte(), node.end_byte()));
    } else if node.kind() == "scoped_use_list" {
        if let Some(list) = node.child_by_field_name("list") {
            leaves(list, out);
        }
    } else if node.kind() != "use_wildcard" {
        let mut cursor = node.walk();
        for n in node.named_children(&mut cursor) {
            leaves(n, out);
        }
    }
}
#[allow(clippy::too_many_arguments)] // Traversal carries pinned source, lexical scope and parser state.
fn walk(
    s: &mut Syntax,
    c: &Capture,
    path: &str,
    module: &str,
    node: Node,
    code: &str,
    parser: &mut Parser,
    directory: PathBuf,
) -> Result<()> {
    let mut cursor = node.walk();
    let mut pending_cfg = false;
    let mut pending_path = None;
    for n in node.named_children(&mut cursor) {
        // Comments do not consume the attributes of the following item.
        if matches!(n.kind(), "line_comment" | "block_comment") {
            continue;
        }
        if pending_cfg && n.kind() != "attribute_item" && n.kind() != "inner_attribute_item" {
            s.occurrences.push(Occurrence {
                path: path.into(),
                module: module.into(),
                start: n.start_byte(),
                end: n.end_byte(),
                position: n.start_byte(),
                kind: "unsupported_generated_source",
            });
            pending_cfg = false;
            pending_path = None;
            continue;
        }
        let explicit_path = if n.kind() != "attribute_item" && n.kind() != "inner_attribute_item" {
            pending_path.take()
        } else {
            None
        };
        if explicit_path.is_some()
            && (n.kind() != "mod_item" || n.child_by_field_name("body").is_some())
        {
            s.unsupported += 1;
            s.occurrences.push(Occurrence {
                path: path.into(),
                module: module.into(),
                start: n.start_byte(),
                end: n.end_byte(),
                position: n.start_byte(),
                kind: "unsupported_generated_source",
            });
            continue;
        }
        match n.kind() {
            "mod_item" => {
                let Some(name) = n.child_by_field_name("name") else {
                    continue;
                };
                let key = qualified(module, node_text(name, code));
                if let Some(body) = n.child_by_field_name("body") {
                    s.modules.insert(key.clone(), path.into());
                    s.definitions.push(Definition {
                        path: path.into(),
                        start: n.start_byte(),
                        end: n.end_byte(),
                        name_start: name.start_byte(),
                        key: key.clone(),
                        kind: "module",
                        function: false,
                    });
                    walk(
                        s,
                        c,
                        path,
                        &key,
                        body,
                        code,
                        parser,
                        directory.join(node_text(name, code)),
                    )?;
                } else {
                    s.definitions.push(Definition {
                        path: path.into(),
                        start: n.start_byte(),
                        end: n.end_byte(),
                        name_start: name.start_byte(),
                        key: key.clone(),
                        kind: "module",
                        function: false,
                    });
                    let base = directory.join(node_text(name, code));
                    let paths = match explicit_path {
                        Some(path) => vec![PathBuf::from(path)],
                        None => vec![base.with_extension("rs"), base.join("mod.rs")],
                    };
                    let targets: Vec<_> = paths
                        .iter()
                        .filter_map(|p| p.to_str())
                        .filter(|p| c.files.contains_key(*p))
                        .collect();
                    if targets.len() == 1 {
                        let target=targets[0];
                        s.inclusions.push(ModuleInclusion {from:path.into(),to:target.into(),module:key.clone(),start:n.start_byte(),end:n.end_byte()});
                        visit_module(s, c, target, &key, parser, None)?;
                    } else {
                        s.unsupported += 1;
                        s.occurrences.push(Occurrence {
                            path: path.into(),
                            module: module.into(),
                            start: n.start_byte(),
                            end: n.end_byte(),
                            position: name.start_byte(),
                            kind: "unsupported_generated_source",
                        });
                    }
                }
            }
            "use_declaration" => {
                let full = node_text(n, code);
                let mut names = Vec::new();
                if full.contains('*') {
                    s.unsupported += 1;
                    s.occurrences.push(Occurrence {
                        path: path.into(),
                        module: module.into(),
                        start: n.start_byte(),
                        end: n.end_byte(),
                        position: n.start_byte(),
                        kind: "unsupported_generated_source",
                    });
                } else {
                    leaves(n, &mut names);
                    for (start, _) in names {
                        s.occurrences.push(Occurrence {
                            path: path.into(),
                            module: module.into(),
                            start: n.start_byte(),
                            end: n.end_byte(),
                            position: start,
                            kind: "import",
                        });
                    }
                }
            }
            "function_item" | "struct_item" | "enum_item" | "trait_item" | "type_item"
            | "const_item" | "static_item" => {
                if let Some(name) = n.child_by_field_name("name") {
                    let mut parents = Vec::new();
                    let mut ancestor = n.parent();
                    while let Some(parent) = ancestor {
                        if parent.kind() == "function_item" {
                            if let Some(pname) = parent.child_by_field_name("name") {
                                parents.push(node_text(pname, code));
                            }
                        }
                        ancestor = parent.parent();
                    }
                    parents.reverse();
                    parents.push(node_text(name, code));
                    let key = format!(
                        "{}#{path}:{}",
                        qualified(module, &parents.join("::")),
                        n.start_byte()
                    );
                    s.definitions.push(Definition {
                        path: path.into(),
                        start: n.start_byte(),
                        end: n.end_byte(),
                        name_start: name.start_byte(),
                        key,
                        kind: "symbol",
                        function: n.kind() == "function_item",
                    });
                }
                // Method/function-pointer dispatch remains visible, without invented exact targets.
                collect_dynamic(s, path, module, n, code);
                if n.kind() == "function_item" {
                    walk(s, c, path, module, n, code, parser, directory.clone())?;
                }
            }
            "macro_invocation" | "attribute_item" | "inner_attribute_item" => {
                let attr = node_text(n, code);
                if pending_path.is_none() {
                    let base=if node.kind()=="source_file" {Path::new(path).parent().unwrap_or(Path::new(""))}else{&directory};
                    if let Some(target) = literal_module_path(attr, base) {
                        if c.files.contains_key(&target) {
                            pending_path = Some(target);
                            continue;
                        }
                    }
                }
                if attr.contains("cfg") || attr.contains("path") {
                    pending_cfg = true;
                }
                if n.kind() == "macro_invocation"
                    || attr.contains("cfg")
                    || attr.contains("path")
                    || attr.contains("derive")
                {
                    s.unsupported += 1;
                    s.occurrences.push(Occurrence {
                        path: path.into(),
                        module: module.into(),
                        start: n.start_byte(),
                        end: n.end_byte(),
                        position: n.start_byte(),
                        kind: "unsupported_generated_source",
                    });
                }
            }
            "impl_item" => {
                s.unsupported += 1;
                collect_dynamic(s, path, module, n, code);
            }
            _ => {
                if n.kind() == "declaration_list" || n.kind() == "block" {
                    walk(s, c, path, module, n, code, parser, directory.clone())?;
                }
            }
        }
    }
    Ok(())
}
fn collect_dynamic(s: &mut Syntax, path: &str, module: &str, node: Node, code: &str) {
    let mut cursor = node.walk();
    for n in node.named_children(&mut cursor) {
        if n.kind() == "call_expression" {
            if let Some(f) = n.child_by_field_name("function") {
                if f.kind() == "field_expression"
                    || (f.kind() == "identifier"
                        && code.contains(&format!("{}: fn", node_text(f, code))))
                {
                    s.occurrences.push(Occurrence {
                        path: path.into(),
                        module: module.into(),
                        start: n.start_byte(),
                        end: n.end_byte(),
                        position: f.start_byte(),
                        kind: "trait_dispatch",
                    });
                }
            }
        }
        collect_dynamic(s, path, module, n, code);
    }
}
fn target_key(prefix:&str,kind:&str,name:&str)->String {
    if prefix=="crate" {format!("{kind}::{name}")}else{format!("{prefix}::{kind}::{name}")}
}
fn visit_root(s:&mut Syntax,c:&Capture,path:&str,key:&str,is_test:bool,parser:&mut Parser)->Result<()> {
    if !c.files.contains_key(path) {return Ok(());}
    s.test_root=is_test.then(||path.to_owned());
    let directory=Path::new(path).parent().unwrap_or(Path::new("")).to_path_buf();
    let result=visit_module(s,c,path,key,parser,Some(directory));
    s.test_root=None;
    result
}

/// Reuse the same Cargo/module parser for text-only task captures. No native
/// resolver or repository code executes while constructing these syntax links.
pub(crate) fn context_syntax(contents:&BTreeMap<String,String>)->Result<Syntax> {
    let mut diagnostics=vec![];
    let files=contents.iter().filter_map(|(path,content)| {
        if super::source::is_evaluation_artifact(path) {return None;}
        if path.ends_with("Cargo.toml") && content.parse::<toml_edit::DocumentMut>().is_err() {
            diagnostics.push(format!("Invalid Cargo manifest {path}; Rust module hints unavailable for this package"));return None;
        }
        Some((path.clone(),content.as_bytes().to_vec()))
    }).collect();
    let capture=Capture {root:PathBuf::from("."),files,diagnostics:vec![],exclusions:vec![],complete:true,
        inventory:Value::Null,captured_at_unix_ms:None};
    let mut syntax=match syntax(&capture) {
        Ok(syntax)=>syntax,
        Err(error)=>{diagnostics.push(format!("Rust module hints unavailable: {error}"));Syntax::default()},
    };
    syntax.unsupported+=diagnostics.len();syntax.diagnostics.extend(diagnostics);Ok(syntax)
}

pub(crate) fn syntax_diagnostics(s:&Syntax)->Value {
    json!({"retained_rust_files":s.visited.len(),"module_inclusions":s.inclusions.len(),
        "unsupported_occurrences":s.unsupported,
        "test_targets":s.test_manifests.len(),"parse_errors":s.parse_error_count,
        "unconfigured_files":s.diagnostics.iter().filter(|message|message.starts_with("Rust file outside")).count(),
        "diagnostics":s.diagnostics.iter().take(8).collect::<Vec<_>>(),"omitted_diagnostics":s.diagnostics.len().saturating_sub(8),
        "scope":"retained Cargo targets and literal module includes; conditions/macros remain explicitly unsupported"})
}

pub(crate) fn syntax(c: &Capture) -> Result<Syntax> {
    let mut p = Parser::new();
    p.set_language(&tree_sitter_rust::LANGUAGE.into())?;
    let mut s = Syntax::default();
    let manifests: Vec<_> = c.files.keys().filter(|path|path.ends_with("Cargo.toml")
        && !super::source::is_evaluation_artifact(path)).cloned().collect();
    for manifest in &manifests {
        let base=Path::new(manifest).parent().unwrap_or(Path::new(""));
        let cfg:toml_edit::DocumentMut=source(c,manifest)?.parse()?;
        let Some(package)=cfg.get("package").and_then(|p|p.get("name")).and_then(|v|v.as_str()) else {continue};
        let prefix=if manifests.len()>1 {package.to_owned()}else{"crate".into()};
        let lib=cfg.get("lib").and_then(|t|t.get("path")).and_then(|v|v.as_str()).unwrap_or("src/lib.rs");
        let Some(lib)=relative_source_path(base,lib) else {s.unsupported+=1;s.diagnostics.push(format!("Rust library path escapes capture in {manifest}"));continue;};
        if cfg.get("lib").is_some() || cfg["package"].get("autolib").and_then(|v|v.as_bool())!=Some(false) {
            visit_root(&mut s,c,&lib,&prefix,false,&mut p)?;
        }
        let old_auto=cfg["package"].get("edition").and_then(|v|v.as_str()).unwrap_or("2015")=="2015"
            && ["bin","test","example","bench"].iter().any(|table|cfg.get(table).is_some());
        for (kind,directory,flag) in [("bin","src/bin","autobins"),("test","tests","autotests"),
            ("example","examples","autoexamples"),("bench","benches","autobenches")] {
            let mut explicit=BTreeSet::new();
            if let Some(targets)=cfg.get(kind).and_then(|v|v.as_array_of_tables()) {
                for target in targets {
                    let name=target.get("name").and_then(|v|v.as_str()).unwrap_or(package);
                    let path=match target.get("path").and_then(|v|v.as_str()) {
                        Some(path)=>relative_source_path(base,path).map(PathBuf::from),
                        None=>{
                            let mut paths=vec![base.join(directory).join(format!("{name}.rs")),base.join(directory).join(name).join("main.rs")];
                            if kind=="bin" && name==package {paths.insert(0,base.join("src/main.rs"));}
                            paths.into_iter().find(|path|c.files.contains_key(path.to_string_lossy().as_ref()))
                        },
                    };
                    if let Some(path)=path {
                        let path=path.to_string_lossy().to_string();explicit.insert(path.clone());
                        if !c.files.contains_key(&path) {s.unsupported+=1;s.diagnostics.push(format!("Rust {kind} target {path} is not retained in {manifest}"));continue;}
                        visit_root(&mut s,c,&path,&target_key(&prefix,kind,name),kind=="test"||kind=="bench",&mut p)?;
                        if kind=="test"||kind=="bench" {s.test_manifests.entry(path).or_default().insert(manifest.clone());}
                    } else {s.unsupported+=1;s.diagnostics.push(format!("Rust {kind} target {name} has no retained source in {manifest}"));}
                }
            }
            let automatic=cfg["package"].get(flag).and_then(|v|v.as_bool()).unwrap_or(!old_auto);
            if !automatic {continue;}
            if kind=="bin" {
                let main=base.join("src/main.rs").to_string_lossy().to_string();
                if !explicit.contains(&main) {
                    let key=if !c.files.contains_key(&lib) && manifests.len()==1 {prefix.clone()}else{target_key(&prefix,kind,package)};
                    visit_root(&mut s,c,&main,&key,false,&mut p)?;
                }
            }
            let folder=format!("{}/",base.join(directory).to_string_lossy());
            let discovered:Vec<_>=c.files.keys().filter_map(|path| {
                let relative=path.strip_prefix(&folder)?;
                let name=if !relative.contains('/') {relative.strip_suffix(".rs")?}
                    else {let (name,rest)=relative.split_once('/')?;if rest!="main.rs" {return None;}name};
                (!explicit.contains(path)).then(||(path.clone(),name.to_owned()))
            }).collect();
            for (path,name) in discovered {
                visit_root(&mut s,c,&path,&target_key(&prefix,kind,&name),kind=="test"||kind=="bench",&mut p)?;
                if kind=="test"||kind=="bench" {s.test_manifests.entry(path).or_default().insert(manifest.clone());}
            }
        }
    }
    // An unconfigured Rust file is still retained and described, with explicit unsupported coverage.
    for path in c.files.keys().filter(|p| p.ends_with(".rs") && !super::source::is_evaluation_artifact(p)) {
        if !s.visited.contains(path) {
            s.unsupported += 1;
            s.diagnostics.push(format!(
                "Rust file outside supported Cargo module roots: {path}"
            ));
            visit_module(&mut s, c, path, &format!("unconfigured::{path}"), &mut p, None)?;
        }
    }
    s.occurrences
        .sort_by_key(|o| (o.path.clone(), o.start, o.position));
    s.occurrences.dedup_by(|a, b| {
        a.path == b.path && a.module == b.module && a.start == b.start && a.position == b.position && a.kind == b.kind
    });
    // Trees are shared only while discovering target contexts. Native work
    // needs the retained definitions, not the discovery cache's allocation.
    s.trees.clear();
    Ok(s)
}
pub(crate) fn entity(
    b: &mut Value,
    ctx: &Value,
    kind: &str,
    key: &str,
    provider: &str,
    quality: &str,
) -> Result<String> {
    entity_for_language(b, ctx, kind, key, provider, quality, "rust")
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn entity_for_language(
    b: &mut Value,
    ctx: &Value,
    kind: &str,
    key: &str,
    provider: &str,
    quality: &str,
    language: &str,
) -> Result<String> {
    let id = identity::id(
        "entity",
        &json!({"context":ctx,"kind":kind,"canonical_key":key,"identity_provider":provider}),
    )?;
    add(
        b,
        "entity_versions",
        json!({"id":id,"kind":kind,"language":if super::declarations::SEMANTIC_KINDS.contains(&kind) {"none"}else{language},"canonical_key":key,"context":ctx,"identity_provider":provider,"identity_quality":quality}),
    );
    Ok(id)
}
pub(crate) fn evidence(b: &mut Value, path: &str, start: usize, end: usize) -> Result<String> {
    let a = rows(b, "artifacts")
        .iter()
        .find(|a| a["path"] == path)
        .ok_or("Missing source artifact")?;
    let aid = text(a, "id");
    let locator = json!({"kind":"span","start":start,"end":end});
    let id = identity::id("evidence", &json!({"artifact_id":aid,"locator":locator}))?;
    add(
        b,
        "evidence_refs",
        json!({"id":id,"artifact_id":aid,"locator":locator}),
    );
    Ok(id)
}
#[allow(clippy::too_many_arguments)] // Class-specific fields are checked by the owned tagged union.
pub(crate) fn assertion(
    b: &mut Value,
    ctx: &Value,
    class: &str,
    subject: &str,
    predicate: &str,
    target: Value,
    origin: Value,
    support: Vec<String>,
    resolution: Option<Value>,
) -> Result<String> {
    let mut a = json!({"context":ctx,"class":class,"proposition":{"subject_id":subject,"predicate":predicate,"object":target},"origin":origin});
    if class == "evidence" {
        a["evidence_ref_ids"] = json!(support);
    } else if class == "derived" {
        a["supports"] = json!(support);
        a["support_class"] = json!("evidence_only");
    }
    if let Some(r) = resolution {
        a["resolution"] = r;
    }
    let id = identity::id("assertion", &a)?;
    a["id"] = json!(id);
    add(b, "assertions", a);
    Ok(id)
}
fn target(id: &str) -> Value {
    json!({"kind":"entity","entity_version_id":id})
}
pub(crate) fn byte_position(source: &str, p: &Value) -> Option<usize> {
    let line = p["line"].as_u64()? as usize;
    let column = p["character"].as_u64()? as usize;
    let mut offset = 0;
    for (i, s) in source.split_inclusive('\n').enumerate() {
        if i == line {
            let mut units = 0;
            for (j, ch) in s.char_indices() {
                if units == column {
                    return Some(offset + j);
                }
                units += ch.len_utf16();
            }
            return (units == column).then_some(offset + s.len());
        }
        offset += s.len();
    }
    None
}
fn resolved_module(s: &Syntax, c: &Capture, root: &Path, result: &Value) -> Result<Option<String>> {
    let candidates = if let Some(a) = result.as_array() {
        a.clone()
    } else if result.is_object() {
        vec![result.clone()]
    } else {
        vec![]
    };
    let mut keys = BTreeSet::new();
    for item in candidates {
        let uri = item["targetUri"]
            .as_str()
            .or_else(|| item["uri"].as_str())
            .ok_or("No native target URI")?;
        let path = analyzer::path_from_uri(uri)?;
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let path = relative.to_string_lossy().to_string();
        let Some(bytes) = c.files.get(&path) else {
            continue;
        };
        let code = std::str::from_utf8(bytes)?;
        let range = item
            .get("targetSelectionRange")
            .or_else(|| item.get("range"))
            .ok_or("No native range")?;
        let Some(position) = byte_position(code, &range["start"]) else {
            continue;
        };
        // Several Cargo targets may include the same physical source. A
        // location alone cannot choose between their distinct module keys.
        let definitions:Vec<_>=s.definitions.iter().filter(|d|d.path==path
            && (position==d.name_start||position==d.start)).collect();
        let smallest=definitions.iter().map(|d|d.end-d.start).min();
        for d in definitions.into_iter().filter(|d|Some(d.end-d.start)==smallest) {
            let key=if d.kind=="module" {d.key.clone()}else{
                s.modules.keys().filter(|module|d.key.starts_with(&format!("{module}::")))
                    .max_by_key(|module|module.len()).cloned().unwrap_or_else(||"crate".into())
            };
            keys.insert(key);
        }
    }
    Ok(if keys.len() == 1 {
        keys.into_iter().next()
    } else {
        None
    })
}
pub(crate) fn normalize_native(v: &mut Value, root: &Path) {
    match v {
        Value::String(s) => {
            let prefix = analyzer::uri(root);
            if let Some(rel) = s.strip_prefix(&prefix) {
                *s = format!("captured://{rel}");
            }
        }
        Value::Array(a) => {
            for v in a {
                normalize_native(v, root)
            }
        }
        Value::Object(o) => {
            for v in o.values_mut() {
                normalize_native(v, root)
            }
        }
        _ => {}
    }
}
#[derive(Clone)]
pub struct ScanOptions {
    pub analyzer: Option<PathBuf>,
    pub timeout: Duration,
    pub features: Vec<String>,
}
impl Default for ScanOptions {
    fn default() -> Self {
        Self {
            analyzer: Some("rust-analyzer".into()),
            timeout: Duration::from_secs(60),
            features: Vec::new(),
        }
    }
}
pub fn produce(
    c: Capture,
    repository: &str,
    generation: u64,
    options: &ScanOptions,
) -> Result<Prepared> {
    require(
        c.complete,
        "CAPTURE",
        &format!("Incomplete source capture: {}", c.diagnostics.join("; ")),
    )?;
    let started = Instant::now();
    let settings = json!({"features":options.features,"build_scripts":false,"procedural_macros":false,"fetch_dependencies":false});
    let (mut b, mut payloads, ctx) = c.seed(repository, &settings)?;
    let s = syntax(&c)?;
    let sandbox = c.materialize()?;
    let mut limitations = c.diagnostics.clone();
    limitations.extend(s.diagnostics.clone());
    let mut analyzer = if let Some(exe) = &options.analyzer {
        Some(Analyzer::start(
            exe,
            sandbox.path(),
            options.timeout.saturating_sub(started.elapsed()),
            &options.features,
        )?)
    } else {
        limitations.push("semantic resolver disabled; syntax-only capture".into());
        None
    };
    let producer = analyzer
        .as_ref()
        .map(|a| a.identity.clone())
        .unwrap_or(json!({"name":"rust-syntax","version":"1","settings":settings}));
    let provider = identity::id(
        "identity-provider",
        &json!({"mapping":"rust-module-source-key-v1","mapping_digest":identity::bytes_digest(include_bytes!("rust.rs")),"producer":producer}),
    )?;
    let mut config = json!({"producer_name":producer["name"],"implementation_digest":identity::bytes_digest(include_bytes!("rust.rs")),"settings_digest":identity::digest(&producer)?});
    let config_id = identity::id("analysis", &config)?;
    config["id"] = json!(config_id);
    add(&mut b, "analysis_configs", config);
    let mut entities = BTreeMap::new();
    for d in &s.definitions {
        let id = entity(&mut b, &ctx, d.kind, &d.key, &provider, "syntax")?;
        entities.insert((d.kind, d.key.clone()), id);
    }
    let mut local_roots: BTreeSet<String> =
        BTreeSet::from(["crate".into(), "self".into(), "super".into()]);
    for key in s.modules.keys() {
        local_roots.insert(key.split("::").next().unwrap().to_owned());
    }
    for path in c.files.keys().filter(|p| p.ends_with("Cargo.toml")) {
        let cfg: toml_edit::DocumentMut = source(&c, path)?.parse()?;
        if let Some(name) = cfg
            .get("package")
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
        {
            local_roots.insert(name.replace('-', "_"));
        }
        if let Some(name) = cfg
            .get("lib")
            .and_then(|t| t.get("name"))
            .and_then(|v| v.as_str())
        {
            local_roots.insert(name.to_owned());
        }
    }
    let mut native_witnesses = Vec::new();
    let mut resolved = Vec::new();
    let mut unresolved = Vec::new();
    let mut opened = BTreeSet::new();
    for o in &s.occurrences {
        if o.kind != "import" {
            unresolved.push((o.clone(), o.kind.to_owned()));
            continue;
        }
        let code = source(&c, &o.path)?;
        let import_text = &code[o.start..o.end];
        let root = import_text
            .split("use ")
            .nth(1)
            .unwrap_or("")
            .trim()
            .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
            .next()
            .unwrap_or("");
        if !local_roots.contains(root) {
            unresolved.push((o.clone(), "missing_dependency".into()));
            continue;
        }
        let result = if let Some(a) = analyzer.as_mut() {
            let code = source(&c, &o.path)?;
            let path = sandbox.path().join(&o.path);
            if opened.insert(o.path.clone()) {
                if let Err(e) = a.open(&path, code) {
                    limitations.push(e.to_string());
                }
            }
            a.definition(&path, code, o.position)
                .and_then(|v| resolved_module(&s, &c, sandbox.path(), &v))
        } else {
            Ok(None)
        };
        match result {
            Ok(Some(module)) if entities.contains_key(&("module", module.clone())) => {
                resolved.push((o.clone(), module))
            }
            Ok(_) => unresolved.push((o.clone(), "missing_dependency".into())),
            Err(e) => {
                limitations.push(e.to_string());
                unresolved.push((o.clone(), "extraction_failure".into()));
                if let Some(mut stopped) = analyzer.take() {
                    native_witnesses.append(&mut stopped.witnesses);
                }
            }
        }
    }
    let import_unknown = unresolved
        .iter()
        .filter(|(o, _)| o.kind == "import")
        .count();
    let import_complete = c.complete
        && s.unsupported == 0
        && import_unknown == 0
        && producer["name"] == "rust-analyzer-lsp";
    let coverage = json!({"attempted":s.visited.len(),"completed":s.visited.len(),"unsupported_count":s.unsupported,"unresolved_count":import_unknown,"resolution_complete":import_complete,"guarantees":[{"predicate":"CONTAINS","kind":"syntax_only"},{"predicate":"DECLARES","kind":"syntax_only"},{"predicate":"IMPORTS","kind":if options.analyzer.is_some()&&producer["name"]=="rust-analyzer-lsp" {"exact_supported"}else{"unknown"}}]});
    let run_id = identity::id(
        "run",
        &json!({"context":ctx,"analysis_config_id":config_id,"coverage":coverage}),
    )?;
    add(
        &mut b,
        "runs",
        json!({"id":run_id,"producer_kind":if producer["name"]=="rust-analyzer-lsp" {"semantic"}else{"syntax"},"analysis_config_id":config_id,"context":ctx,"status":if c.complete {"complete"}else{"partial"},"coverage":coverage}),
    );
    let repo = entity(&mut b, &ctx, "repository", repository, &provider, "syntax")?;
    for path in &s.visited {
        let code = source(&c, path)?;
        if code.is_empty() {
            continue;
        }
        let file = entity(&mut b, &ctx, "file", path, &provider, "syntax")?;
        let er = evidence(&mut b, path, 0, code.len())?;
        assertion(
            &mut b,
            &ctx,
            "evidence",
            &repo,
            "CONTAINS",
            target(&file),
            json!({"run_id":run_id}),
            vec![er.clone()],
            None,
        )?;
        for d in s.definitions.iter().filter(|d| d.path == *path) {
            let id = &entities[&(d.kind, d.key.clone())];
            let er = evidence(&mut b, path, d.start, d.end)?;
            let predicate = if d.kind == "module" {
                "CONTAINS"
            } else {
                "DECLARES"
            };
            assertion(
                &mut b,
                &ctx,
                "evidence",
                &file,
                predicate,
                target(id),
                json!({"run_id":run_id}),
                vec![er],
                None,
            )?;
        }
    }
    for inclusion in &s.inclusions {
        let from=entity(&mut b,&ctx,"file",&inclusion.from,&provider,"syntax")?;
        let to=entity(&mut b,&ctx,"file",&inclusion.to,&provider,"syntax")?;
        let witness=evidence(&mut b,&inclusion.from,inclusion.start,inclusion.end)?;
        assertion(&mut b,&ctx,"evidence",&from,"CONTAINS",target(&to),
            json!({"run_id":run_id}),vec![witness],None)?;
    }
    // Bind the actual module body, not an external mod declaration in its
    // parent. This locator is retained independently of the working tree.
    for (key, path) in &s.modules {
        if let Some(definition) = s
            .definitions
            .iter()
            .find(|d| d.kind == "module" && d.key == *key && d.path == *path)
        {
            if definition.start < definition.end {
                let er = evidence(&mut b, path, definition.start, definition.end)?;
                let id = &entities[&("module", key.clone())];
                if let Some(e) = b["entity_versions"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|e| e["id"] == *id)
                {
                    e["source_evidence_ref_id"] = json!(er);
                }
            }
        }
    }
    for (o, module) in &resolved {
        let er = evidence(&mut b, &o.path, o.start, o.end)?;
        let subject = &entities[&("module", o.module.clone())];
        let object = &entities[&("module", module.clone())];
        assertion(
            &mut b,
            &ctx,
            "evidence",
            subject,
            "IMPORTS",
            target(object),
            json!({"run_id":run_id}),
            vec![er],
            Some(json!({"status":"resolved","method":"compiler","candidate_ids":[object]})),
        )?;
    }
    for (o, reason) in &unresolved {
        let er = evidence(&mut b, &o.path, o.start, o.end)?;
        let mut u = json!({"source_entity_id":entities[&("module",o.module.clone())],"context":ctx,"evidence_ref_id":er,"status":if reason=="unsupported_generated_source" {"unsupported"}else{"unresolved"},"reason":reason,"candidate_ids":[]});
        u["id"] = json!(identity::id("unresolved", &u)?);
        add(&mut b, "unresolved_references", u);
    }
    if let Some(mut a) = analyzer.take() {
        native_witnesses.append(&mut a.witnesses);
    }
    if !native_witnesses.is_empty() {
        let mut witness = json!({"provider":producer,"correspondence":native_witnesses,"mapping":"rust-module-source-key-v1","source_snapshot":ctx["snapshot_id"],"build_context":ctx["build_context_id"]});
        normalize_native(&mut witness, sandbox.path());
        let bytes = serde_json::to_vec(&witness)?;
        let digest = identity::bytes_digest(&bytes);
        let id = identity::id("artifact", &json!({"native_mapping_digest":digest}))?;
        add(
            &mut b,
            "artifacts",
            json!({"id":id,"path":format!(".briefcase/analyzers/{id}.json"),"digest":digest,"source_kind":"transport","availability":"available"}),
        );
        payloads.insert(id, bytes);
    }
    super::passes::produce(&mut b, &mut payloads, &ctx, &coverage)?;
    super::declarations::apply(&mut b, &c, &ctx, &provider, &mut limitations)?;
    let diagnostics = json!({"requested_analyses":json!({"dependencies":true,"static_calls":false,"data":false}),"capture":c.metadata(),"limitations":limitations,"excluded_directories":EXPLICIT_EXCLUSIONS,"excluded_directories_scope":"Default untracked exclusions; tracked text overrides except internal state","exclusions":c.exclusions,"coverage":coverage,"provider":producer,"duration_ms":started.elapsed().as_millis() as u64,"rust_syntax":syntax_diagnostics(&s),"supported_scope":"Rust explicit, non-glob imports in discovered Cargo modules; no callgraph/negative runtime guarantee","mapping_assumption":"Canonical module/source keys identify versions; across snapshots equal keys are locator correspondence, not rename proof."});
    Prepared::seal(b, payloads, generation, diagnostics)
}
const EXPLICIT_EXCLUSIONS: &[&str] = super::capture::EXCLUDED;

#[cfg(test)]
mod tests {
    use super::*;

    fn capture(files: &[(&str, &str)]) -> Capture {
        Capture {
            root: PathBuf::from("/unused"),
            files: files
                .iter()
                .map(|(p, s)| ((*p).into(), s.as_bytes().to_vec()))
                .collect(),
            diagnostics: vec![],
            exclusions: vec![],
            complete: true,
            captured_at_unix_ms: None,
            inventory: serde_json::Value::Null,
        }
    }

    #[test]
    fn cargo_test_roots_own_regular_and_path_included_modules() {
        let c=capture(&[("Cargo.toml","[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\nautotests=false\n[[test]]\nname='runner'\npath='tests/runner.rs'\n"),
            ("src/lib.rs","pub fn entry() {}"),("tests/runner.rs","mod suite;"),
            ("tests/suite.rs","#[path=\"parts/leaf.rs\"] mod cases;"),("tests/parts/leaf.rs","fn checks() {}")]);
        let s=syntax(&c).unwrap();
        assert_eq!(s.modules["test::runner::suite"],"tests/suite.rs");
        assert_eq!(s.modules["test::runner::suite::cases"],"tests/parts/leaf.rs");
        assert!(s.test_owners["tests/parts/leaf.rs"].contains("tests/runner.rs"));
        assert!(s.test_manifests["tests/runner.rs"].contains("Cargo.toml"));
        assert_eq!(s.inclusions.len(),2);assert!(s.diagnostics.is_empty(),"{:?}",s.diagnostics);
    }

    #[test]
    fn disabled_test_discovery_does_not_invent_a_test_owner() {
        let c=capture(&[("Cargo.toml","[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\nautotests=false\n"),
            ("src/lib.rs","fn entry() {}"),("tests/disabled.rs","fn checks() {}")]);
        let s=syntax(&c).unwrap();assert!(!s.test_owners.contains_key("tests/disabled.rs"));
        assert!(!s.modules.contains_key("test::disabled"));
    }

    #[test]
    fn automatic_cargo_targets_use_root_directories_and_configure_nested_main_files() {
        let c=capture(&[("Cargo.toml","[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\n"),
            ("src/lib.rs","fn live() {}"),("src/bin/tool/main.rs","mod helper;"),("src/bin/tool/helper.rs","fn helper() {}"),
            ("tests/api/main.rs","mod helper;"),("tests/api/helper.rs","fn check() {}")]);
        let s=syntax(&c).unwrap();assert_eq!(s.modules["bin::tool::helper"],"src/bin/tool/helper.rs");
        assert_eq!(s.modules["test::api::helper"],"tests/api/helper.rs");
        assert!(s.test_owners["tests/api/helper.rs"].contains("tests/api/main.rs"));
        assert!(s.diagnostics.is_empty(),"{:?}",s.diagnostics);
    }

    #[test]
    fn malformed_manifest_is_diagnostic_for_optional_context_hints() {
        let contents=BTreeMap::from([("Cargo.toml".into(),"not valid [".into()),("src/lib.rs".into(),"fn live() {}".into())]);
        let s=context_syntax(&contents).unwrap();assert!(s.diagnostics.iter().any(|message|message.contains("Invalid Cargo manifest")));
        assert!(s.unsupported>0);assert!(s.visited.contains("src/lib.rs"));assert!(s.test_owners.is_empty());
    }

    #[test]
    fn shared_test_modules_keep_each_cargo_target_context() {
        let c=capture(&[("Cargo.toml","[package]\nname='demo'\nversion='0.1.0'\nedition='2021'\nautotests=false\n[[test]]\nname='a'\npath='tests/a.rs'\n[[test]]\nname='b'\npath='tests/b.rs'\n"),
            ("src/lib.rs","fn entry() {}"),("tests/a.rs","#[path=\"shared.rs\"] mod helper;"),
            ("tests/b.rs","#[path=\"shared.rs\"] mod helper;"),("tests/shared.rs","fn checks() {}")]);
        let s=syntax(&c).unwrap();assert_eq!(s.test_owners["tests/shared.rs"].len(),2);
        assert_eq!(s.modules["test::a::helper"],"tests/shared.rs");assert_eq!(s.modules["test::b::helper"],"tests/shared.rs");
        assert_eq!(s.definitions.iter().filter(|d|d.function&&d.path=="tests/shared.rs").count(),2);
        assert!(s.diagnostics.is_empty(),"{:?}",s.diagnostics);
    }

    #[test]
    fn module_cycles_are_bounded_and_remain_explicitly_incomplete() {
        let c=capture(&[("Cargo.toml","[package]\nname='demo'\nversion='0.1.0'\n"),
            ("src/lib.rs","#[path=\"child.rs\"] mod child;"),("src/child.rs","#[path=\"lib.rs\"] mod back;")]);
        let s=syntax(&c).unwrap();assert!(s.diagnostics.iter().any(|message|message.contains("cyclic Rust module")));
        assert!(s.unsupported>0);assert!(s.contexts.len()<10);
    }

    #[test]
    fn inline_path_modules_resolve_against_the_inline_directory() {
        let c=capture(&[("Cargo.toml","[package]\nname='demo'\nversion='0.1.0'\n"),
            ("src/lib.rs","mod inline { #[path=\"part.rs\"] mod child; }"),("src/inline/part.rs","fn live() {}")]);
        let s=syntax(&c).unwrap();assert_eq!(s.modules["inline::child"],"src/inline/part.rs");
        assert!(s.diagnostics.is_empty(),"{:?}",s.diagnostics);
    }

    #[test]
    fn parse_failures_are_localized_without_discarding_valid_sibling_declarations() {
        let c=capture(&[("Cargo.toml","[package]\nname='demo'\nversion='0.1.0'\n"),
            ("src/lib.rs","pub fn before() {}\n@\npub fn after() {}\n")]);
        let s=syntax(&c).unwrap();assert!(s.parse_error_count>0);
        for name in ["before#","after#"] {assert!(s.definitions.iter().any(|d|d.key.starts_with(name)));}
        assert!(s.diagnostics.iter().any(|message|message.contains("src/lib.rs:2 at bytes")));
    }

    #[test]
    fn evaluation_fixture_manifests_and_invalid_source_do_not_poison_product_syntax() {
        let c=capture(&[("Cargo.toml","[package]\nname='demo'\nversion='0.1.0'\n"),("src/lib.rs","fn live() {}"),
            ("evaluation/fixtures/Cargo.toml","invalid manifest ["),("evaluation/sources/input.rs","not valid Rust @@@")]);
        let s=syntax(&c).unwrap();assert_eq!(s.parse_error_count,0);assert!(s.diagnostics.is_empty());
        assert!(!s.visited.contains("evaluation/sources/input.rs"));assert_eq!(s.modules["crate"],"src/lib.rs");
    }

    #[test]
    fn literal_path_modules_keep_the_declared_identity_and_containment() {
        let c = capture(&[
            (
                "Cargo.toml",
                "[package]\nname='demo'\nversion='0.1.0'\n[[bin]]\nname='cli'\npath='src/cli.rs'\n",
            ),
            (
                "src/cli.rs",
                "#[path = \"custom/args.rs\"]\n// retained module\n/* with commentary */\nmod args;\nfn main() {}",
            ),
            (
                "src/custom/args.rs",
                "#[path = \"../peer.rs\"]\nmod peer;\npub fn parse() {}",
            ),
            ("src/peer.rs", "pub fn help() {}"),
        ]);
        let s = syntax(&c).unwrap();
        assert_eq!(s.modules["bin::cli::args"], "src/custom/args.rs");
        assert_eq!(s.modules["bin::cli::args::peer"], "src/peer.rs");
        assert!(s
            .definitions
            .iter()
            .any(|d| d.key.starts_with("bin::cli::args::parse#")));
        // The parent mod declaration is retained as well as the child's bytes.
        assert!(s
            .definitions
            .iter()
            .any(|d| d.key == "bin::cli::args" && d.path == "src/cli.rs"));
        assert!(s.diagnostics.is_empty(), "{:?}", s.diagnostics);
        assert_eq!(s.unsupported, 0);
    }

    #[test]
    fn unsafe_missing_and_conditional_paths_remain_unknown() {
        let c = capture(&[
            ("Cargo.toml", "[package]\nname='demo'\nversion='0.1.0'\n"),
            (
                "src/lib.rs",
                r#"
#[path = "../../external.rs"] mod escape;
#[path = "/tmp/external.rs"] mod absolute;
#[path = "absent.rs"] mod missing;
#[cfg(feature = "enabled")]
// A comment must not discard the conditional boundary.
#[path = "safe.rs"] mod conditional;
#[path = "safe.rs"]
#[cfg(feature = "enabled")] mod conditional_after;
#[path = "safe.rs"]
#[path = "safe.rs"] mod duplicate;
mod outer { #[path = "safe.rs"] mod nested; }
"#,
            ),
            ("src/safe.rs", "pub fn live() {}"),
        ]);
        let s = syntax(&c).unwrap();
        for name in [
            "escape",
            "absolute",
            "missing",
            "conditional",
            "conditional_after",
            "duplicate",
            "outer::nested",
        ] {
            assert!(
                !s.modules.contains_key(name),
                "invented identity for {name}"
            );
        }
        assert!(s.unsupported >= 7);
        assert!(s.diagnostics.iter().any(|d| d.contains("src/safe.rs")));
    }
}
