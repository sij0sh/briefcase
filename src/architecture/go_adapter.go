// Native Go syntax and go/types resolution over retained inputs only.
// This program never builds, runs or downloads repository code.
package main

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"go/ast"
	"go/build"
	"go/parser"
	"go/token"
	"go/types"
	"go/version"
	"io"
	"io/fs"
	"os"
	"path"
	"path/filepath"
	"runtime"
	"sort"
	"strconv"
	"strings"
)

const maxFiles = 4096
const maxDefinitions = 4096
const maxOccurrences = 16384

type request struct {
	Files        map[string]string `json:"files"`
	Roots        []string          `json:"source_roots"`
	Tags         []string          `json:"tags"`
	GOOS         string            `json:"goos"`
	GOARCH       string            `json:"goarch"`
	Tests        bool              `json:"tests"`
	SyntaxOnly   bool              `json:"syntax_only"`
	IdentityOnly bool              `json:"identity_only"`
}
type row = map[string]any

type source struct {
	path     string
	text     string
	file     *ast.File
	group    *group
	selected bool
	parsed   bool
	module   row
}
type definition struct {
	key      string
	node     ast.Node
	ident    *ast.Ident
	src      *source
	callable bool
}
type group struct {
	key, dir, name, importPath, version, moduleRoot string
	sources                                         []*source
	info                                            *types.Info
	pkg                                             *types.Package
	state                                           int
	healthy                                         bool
	excluded                                        bool
}
type analysis struct {
	req              request
	fset             *token.FileSet
	sources          []*source
	groups           map[string]*group
	importsByPath    map[string][]*group
	defs             map[types.Object]*definition
	syntaxDefs       map[*ast.Ident]*definition
	funcDefs         map[*ast.FuncDecl]*definition
	output           row
	count            int
	importDepth      int
	standardRoot     string
	standardIndex    map[string]bool
	standardPackages map[string]*types.Package
	standardState    map[string]int
	standardDepth    int
	current          *group
	workspace        bool
}

func array(a row, name string) []row { return a[name].([]row) }
func (a *analysis) add(name string, value row) {
	if name == "definitions" && len(array(a.output, name)) >= maxDefinitions {
		panic("Go definition limit exceeded (4096)")
	}
	if name == "imports" || name == "references" || name == "calls" || name == "unresolved" {
		a.count++
		if a.count > maxOccurrences {
			panic("Go occurrence limit exceeded (16384)")
		}
	}
	a.output[name] = append(array(a.output, name), value)
}
func (a *analysis) span(s *source, n ast.Node) row {
	start, end := a.fset.Position(n.Pos()).Offset, a.fset.Position(n.End()).Offset
	if start < 0 || start >= end || end > len(s.text) {
		panic("Go source position outside retained bytes")
	}
	return row{"path": s.path, "start": start, "end": end}
}
func (a *analysis) unknown(s *source, n ast.Node, predicate, reason, caller string, candidates []string) {
	r := a.span(s, n)
	r["predicate"], r["reason"], r["caller"] = predicate, reason, caller
	r["status"] = "unresolved"
	if len(candidates) > 0 {
		r["status"] = "ambiguous"
	}
	sort.Strings(candidates)
	if candidates == nil {
		candidates = []string{}
	}
	r["candidate_keys"] = candidates
	a.add("unresolved", r)
}
func (a *analysis) site(s *source, n ast.Node, name, target, caller, subject string) {
	r := a.span(s, n)
	r["target"], r["caller"], r["subject"] = target, caller, subject
	a.add(name, r)
}
func (a *analysis) diagnostic(s *source, message string) {
	a.add("diagnostics", row{"path": s.path, "message": message})
}
func roots(r []string) []string {
	if len(r) == 0 {
		r = []string{"."}
	}
	if len(r) > 64 {
		panic("Go source root limit exceeded (64)")
	}
	seen := map[string]bool{}
	for _, p := range r {
		if p == "" || path.IsAbs(p) || path.Clean(p) != p || strings.Contains(p, "\\") || p == ".." || strings.HasPrefix(p, "../") || seen[p] {
			panic("Go source roots must be unique normalized relative paths")
		}
		seen[p] = true
	}
	sort.Strings(r)
	return r
}
func within(p, r string) bool { return r == "." || p == r || strings.HasPrefix(p, r+"/") }

// Go module/workspace directives are read as data. Unsupported replacements
// never become an excuse to read external paths or invoke the go command.
func directives(text string) map[string][][]string {
	result := map[string][][]string{}
	block := ""
	for _, line := range strings.Split(text, "\n") {
		var fields []string
		for len(strings.TrimSpace(line)) > 0 {
			line = strings.TrimSpace(line)
			if strings.HasPrefix(line, "//") {
				break
			}
			if line[0] == '"' || line[0] == '`' {
				quote := line[0]
				end := 1
				for end < len(line) {
					if line[end] == quote {
						end++
						break
					}
					if line[end] == '\\' && quote == '"' {
						end++
					}
					end++
				}
				if end > len(line) {
					panic("Invalid Go module/workspace quoted directive")
				}
				value, err := strconv.Unquote(line[:end])
				if err != nil {
					panic("Invalid Go module/workspace directive")
				}
				fields = append(fields, value)
				line = line[end:]
			} else {
				end := strings.IndexAny(line, " \t\r")
				if end < 0 {
					end = len(line)
				}
				fields = append(fields, line[:end])
				line = line[end:]
			}
		}
		if len(fields) == 0 {
			continue
		}
		if fields[0] == ")" {
			block = ""
			continue
		}
		if block != "" {
			result[block] = append(result[block], fields)
			continue
		}
		if len(fields) == 2 && fields[1] == "(" {
			block = fields[0]
			continue
		}
		result[fields[0]] = append(result[fields[0]], fields[1:])
	}
	if block != "" {
		panic("Unclosed Go module/workspace directive")
	}
	return result
}
func validateGoVersion(d map[string][][]string) {
	if len(d["go"]) > 1 || (len(d["go"]) == 1 && len(d["go"][0]) != 1) {
		panic("Invalid Go version directive")
	}
	if len(d["go"]) == 1 {
		selected := "go" + d["go"][0][0]
		if !version.IsValid(selected) || version.Compare(version.Lang(selected), version.Lang(runtime.Version())) > 0 {
			panic("Retained Go version requires a newer selected compiler")
		}
	}
}
func targetToolTags(goarch string) []string {
	tags := []string{}
	for _, tag := range build.Default.ToolTags {
		if strings.HasPrefix(tag, "goexperiment.") || goarch == runtime.GOARCH {
			tags = append(tags, tag)
		}
	}
	if goarch != runtime.GOARCH {
		switch goarch {
		case "386":
			tags = append(tags, "386.sse2")
		case "amd64":
			tags = append(tags, "amd64.v1")
		case "arm":
			tags = append(tags, "arm.5", "arm.6", "arm.7")
		case "arm64":
			tags = append(tags, "arm64.v8.0")
		case "mips", "mipsle", "mips64", "mips64le":
			tags = append(tags, goarch+".hardfloat")
		case "ppc64", "ppc64le":
			tags = append(tags, goarch+".power8")
		case "riscv64":
			tags = append(tags, "riscv64.rva20u64")
		}
	}
	sort.Strings(tags)
	return tags
}
func moduleInfo(files map[string]string) (map[string]string, map[string]string, map[string]bool, bool) {
	names, versions := map[string]string{}, map[string]string{}
	for p, text := range files {
		if path.Base(p) != "go.mod" {
			continue
		}
		d := directives(text)
		validateGoVersion(d)
		if len(d["module"]) != 1 || len(d["module"][0]) != 1 || d["module"][0][0] == "" {
			panic("Go module requires one retained module directive")
		}
		dir := path.Dir(p)
		names[dir] = d["module"][0][0]
		if len(d["go"]) == 1 && len(d["go"][0]) == 1 {
			versions[dir] = "go" + d["go"][0][0]
		}
	}
	active := map[string]bool{}
	unsafe := false
	if work, ok := files["go.work"]; ok {
		d := directives(work)
		validateGoVersion(d)
		if len(d["replace"]) > 0 {
			unsafe = true
		}
		for _, use := range d["use"] {
			if len(use) != 1 {
				panic("Unsupported Go workspace use directive")
			}
			dir := path.Clean(use[0])
			if path.IsAbs(dir) || dir == ".." || strings.HasPrefix(dir, "../") || names[dir] == "" {
				panic("Go workspace module is outside retained inputs")
			}
			active[dir] = true
		}
		if len(active) == 0 {
			panic("Go workspace contains no retained modules")
		}
	} else if names["."] != "" {
		active["."] = true
	} else {
		for dir := range names {
			active[dir] = true
		}
	}
	for dir := range active {
		if len(directives(files[path.Join(dir, "go.mod")])["replace"]) > 0 {
			unsafe = true
		}
	}
	return names, versions, active, unsafe
}
func (a *analysis) load() {
	names, versions, active, unsafe := moduleInfo(a.req.Files)
	_, a.workspace = a.req.Files["go.work"]
	var moduleRoots []string
	for dir := range names {
		moduleRoots = append(moduleRoots, dir)
	}
	sort.Slice(moduleRoots, func(i, j int) bool {
		if len(moduleRoots[i]) != len(moduleRoots[j]) {
			return len(moduleRoots[i]) > len(moduleRoots[j])
		}
		return moduleRoots[i] < moduleRoots[j]
	})
	var paths []string
	for p := range a.req.Files {
		if strings.HasSuffix(p, ".go") {
			paths = append(paths, p)
		}
	}
	sort.Strings(paths)
	if len(paths) == 0 || len(paths) > maxFiles {
		panic("Go analysis needs 1..4096 retained .go files")
	}
	ctx := build.Default
	ctx.GOOS, ctx.GOARCH, ctx.CgoEnabled = a.req.GOOS, a.req.GOARCH, false
	ctx.BuildTags = a.req.Tags
	ctx.ToolTags = targetToolTags(a.req.GOARCH)
	ctx.GOROOT, ctx.GOPATH = "", ""
	ctx.OpenFile = func(p string) (io.ReadCloser, error) {
		text, ok := a.req.Files[path.Clean(p)]
		if !ok {
			return nil, os.ErrNotExist
		}
		return io.NopCloser(strings.NewReader(text)), nil
	}
	packageRows := map[string]bool{}
	brokenDirs := map[string]bool{}
	for _, p := range paths {
		text := a.req.Files[p]
		s := &source{path: p, text: text}
		s.module = row{"path": p, "package": nil, "parsed": false}
		a.sources = append(a.sources, s)
		selected := false
		for _, r := range a.req.Roots {
			selected = selected || within(p, r)
		}
		dir := path.Dir(p)
		for _, part := range strings.Split(dir, "/") {
			if part == "vendor" || part == "testdata" || (part != "." && (strings.HasPrefix(part, ".") || strings.HasPrefix(part, "_"))) {
				selected = false
			}
		}
		moduleRoot := ""
		for _, r := range moduleRoots {
			if within(p, r) {
				moduleRoot = r
				break
			}
		}
		if moduleRoot != "" && !active[moduleRoot] {
			selected = false
		}
		match, err := ctx.MatchFile(dir, path.Base(p))
		if err != nil {
			a.diagnostic(s, "Invalid build constraints: "+err.Error())
			selected = false
		}
		if !match || (!a.req.Tests && strings.HasSuffix(p, "_test.go")) {
			selected = false
		}
		f, err := parser.ParseFile(a.fset, p, text, parser.ParseComments|parser.AllErrors)
		if err != nil {
			a.diagnostic(s, "Parse failure: "+err.Error())
			if selected {
				brokenDirs[dir] = true
			}
			if len(text) > 0 {
				a.add("unresolved", row{"path": p, "start": 0, "end": len(text), "predicate": "IMPORTS", "reason": "extraction_failure", "status": "unsupported", "candidate_keys": []string{}})
			}
			a.add("modules", s.module)
			continue
		}
		s.file, s.parsed = f, true
		s.module["parsed"] = true
		for _, im := range f.Imports {
			value, _ := strconv.Unquote(im.Path.Value)
			if value == "C" {
				if selected {
					brokenDirs[dir] = true
				}
				selected = false
				a.diagnostic(s, "cgo source is outside isolated Go analysis")
			}
		}
		if !selected {
			a.output["omitted"] = append(a.output["omitted"].([]string), p)
		}
		s.selected = selected
		packageKey := "package::" + dir
		if !packageRows[packageKey] {
			a.add("packages", row{"key": packageKey, "parent": nil, "source": p})
			packageRows[packageKey] = true
		}
		key := "go-package::" + dir + "::" + f.Name.Name
		g := a.groups[key]
		if g == nil {
			importPath := ""
			if moduleRoot != "" {
				suffix := strings.TrimPrefix(strings.TrimPrefix(dir, moduleRoot), "/")
				if moduleRoot == "." {
					suffix = dir
					if suffix == "." {
						suffix = ""
					}
				}
				importPath = strings.TrimSuffix(names[moduleRoot]+"/"+suffix, "/")
			}
			g = &group{key: key, dir: dir, name: f.Name.Name, importPath: importPath, moduleRoot: moduleRoot, version: versions[moduleRoot], healthy: true, excluded: unsafe}
			a.groups[key] = g
			a.add("groups", row{"key": key, "parent": packageKey, "source": p})
		}
		if selected {
			g.sources = append(g.sources, s)
		}
		s.group = g
		s.module["parent"] = g.key
		for _, comment := range f.Comments {
			if strings.Contains(comment.Text(), "go:linkname") {
				g.excluded = true
			}
			for _, c := range comment.List {
				if strings.HasPrefix(c.Text, "//go:linkname") {
					g.excluded = true
				}
			}
		}
		a.add("modules", s.module)
		a.declarations(s)
	}
	var keys []string
	for k := range a.groups {
		keys = append(keys, k)
	}
	sort.Strings(keys)
	for _, key := range keys {
		g := a.groups[key]
		if len(g.sources) == 0 {
			continue
		}
		if brokenDirs[g.dir] {
			g.excluded = true
		}
		if g.excluded {
			a.diagnostic(g.sources[0], "Go replacements, cgo, parse failures or linkname prevent exact local binding guarantees")
		}
		if g.importPath != "" && g.name != "main" && !strings.HasSuffix(g.name, "_test") {
			a.importsByPath[g.importPath] = append(a.importsByPath[g.importPath], g)
		}
	}
	// Multiple ordinary package names in a directory are compiler errors.
	dirs := map[string][]*group{}
	for _, key := range keys {
		g := a.groups[key]
		if len(g.sources) > 0 && !strings.HasSuffix(g.name, "_test") {
			dirs[g.dir] = append(dirs[g.dir], g)
		}
	}
	for _, gs := range dirs {
		if len(gs) > 1 {
			for _, g := range gs {
				g.excluded = true
				a.diagnostic(g.sources[0], "Competing Go package declarations in one directory")
			}
		}
	}
}
func receiver(t ast.Expr) string {
	switch t := t.(type) {
	case *ast.Ident:
		return t.Name
	case *ast.StarExpr:
		return receiver(t.X)
	case *ast.IndexExpr:
		return receiver(t.X)
	case *ast.IndexListExpr:
		return receiver(t.X)
	}
	return "receiver"
}
func (a *analysis) declarations(s *source) {
	qualifiers := []string{}
	stack := [][]string{}
	ast.Inspect(s.file, func(n ast.Node) bool {
		if n == nil {
			qualifiers = stack[len(stack)-1]
			stack = stack[:len(stack)-1]
			return false
		}
		stack = append(stack, append([]string{}, qualifiers...))
		declare := func(id *ast.Ident, node ast.Node, callable bool) *definition {
			if id == nil || id.Name == "_" {
				return nil
			}
			names := append(append([]string{}, qualifiers...), id.Name)
			key := fmt.Sprintf("%s::%s#%d", s.path, strings.Join(names, "::"), a.fset.Position(node.Pos()).Offset)
			d := &definition{key: key, node: node, ident: id, src: s, callable: callable && s.selected}
			a.syntaxDefs[id] = d
			r := a.span(s, node)
			r["key"], r["callable"] = key, d.callable
			a.add("definitions", r)
			return d
		}
		switch n := n.(type) {
		case *ast.FuncDecl:
			if n.Recv != nil && len(n.Recv.List) > 0 {
				qualifiers = append(qualifiers, receiver(n.Recv.List[0].Type))
			}
			a.funcDefs[n] = declare(n.Name, n, n.Body != nil)
			qualifiers = append(qualifiers, n.Name.Name)
		case *ast.FuncLit:
			qualifiers = append(qualifiers, fmt.Sprintf("lambda@%d", a.fset.Position(n.Pos()).Offset))
		case *ast.TypeSpec:
			declare(n.Name, n, false)
			qualifiers = append(qualifiers, n.Name.Name)
		case *ast.ValueSpec:
			for _, id := range n.Names {
				declare(id, id, false)
			}
		case *ast.Field:
			for _, id := range n.Names {
				declare(id, id, false)
			}
		case *ast.AssignStmt:
			if n.Tok == token.DEFINE {
				for _, e := range n.Lhs {
					if id, ok := e.(*ast.Ident); ok {
						declare(id, id, false)
					}
				}
			}
		case *ast.RangeStmt:
			if n.Tok == token.DEFINE {
				for _, e := range []ast.Expr{n.Key, n.Value} {
					if id, ok := e.(*ast.Ident); ok {
						declare(id, id, false)
					}
				}
			}
		}
		return true
	})
}
func (a *analysis) standardImport(p string) (*types.Package, error) {
	if p == "unsafe" {
		return types.Unsafe, nil
	}
	if !a.standardIndex[p] {
		return nil, fmt.Errorf("Unavailable Go standard package: %s", p)
	}
	if a.standardState[p] == 1 {
		return nil, fmt.Errorf("Go platform import cycle: %s", p)
	}
	if a.standardState[p] == 2 {
		if pkg := a.standardPackages[p]; pkg != nil {
			return pkg, nil
		}
		return nil, fmt.Errorf("Go platform package unavailable: %s", p)
	}
	if a.standardDepth >= 128 {
		return nil, errors.New("Go platform traversal limit (128)")
	}
	a.standardState[p] = 1
	a.standardDepth++
	defer func() { a.standardDepth-- }()
	dir := filepath.Join(a.standardRoot, filepath.FromSlash(p))
	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil, err
	}
	ctx := build.Default
	ctx.GOOS = a.req.GOOS
	ctx.GOARCH = a.req.GOARCH
	ctx.CgoEnabled = false
	ctx.BuildTags = nil
	ctx.ToolTags = targetToolTags(a.req.GOARCH)
	files := []*ast.File{}
	for _, e := range entries {
		if e.IsDir() || e.Type()&os.ModeSymlink != 0 || !strings.HasSuffix(e.Name(), ".go") || strings.HasSuffix(e.Name(), "_test.go") {
			continue
		}
		match, err := ctx.MatchFile(dir, e.Name())
		if err != nil {
			return nil, err
		}
		if !match {
			continue
		}
		f, err := parser.ParseFile(a.fset, filepath.Join(dir, e.Name()), nil, parser.SkipObjectResolution)
		if err != nil {
			return nil, err
		}
		for _, im := range f.Imports {
			value, _ := strconv.Unquote(im.Path.Value)
			if value == "C" {
				return nil, errors.New("cgo platform declarations unavailable")
			}
		}
		files = append(files, f)
	}
	if len(files) == 0 {
		return nil, fmt.Errorf("No selected platform source: %s", p)
	}
	conf := types.Config{Importer: standardImporter{a}, IgnoreFuncBodies: true, Sizes: types.SizesFor("gc", a.req.GOARCH)}
	pkg, err := conf.Check(p, a.fset, files, nil)
	a.standardState[p] = 2
	if err != nil {
		return nil, err
	}
	a.standardPackages[p] = pkg
	return pkg, nil
}

type standardImporter struct{ a *analysis }

func (i standardImporter) Import(p string) (*types.Package, error) { return i.a.standardImport(p) }
func (a *analysis) platformIndex() {
	root := os.Getenv("GOROOT")
	if root == "" {
		panic("Missing trusted Go compiler source root")
	}
	a.standardRoot = filepath.Join(root, "src")
	a.standardIndex = map[string]bool{"unsafe": true}
	a.standardPackages = map[string]*types.Package{}
	a.standardState = map[string]int{}
	err := filepath.WalkDir(a.standardRoot, func(p string, d fs.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if d.Type()&os.ModeSymlink != 0 {
			return errors.New("Symlink in Go platform source")
		}
		if d.IsDir() {
			if d.Name() == "testdata" || d.Name() == "vendor" || (p != a.standardRoot && (strings.HasPrefix(d.Name(), ".") || strings.HasPrefix(d.Name(), "_"))) {
				return filepath.SkipDir
			}
			return nil
		}
		if strings.HasSuffix(d.Name(), ".go") && !strings.HasSuffix(d.Name(), "_test.go") {
			dir, err := filepath.Rel(a.standardRoot, filepath.Dir(p))
			if err != nil {
				return err
			}
			a.standardIndex[filepath.ToSlash(dir)] = true
		}
		return nil
	})
	if err != nil {
		panic(err)
	}
}
func internalAllowed(p, from string) bool {
	parts := strings.Split(p, "/")
	for i, part := range parts {
		if part != "internal" {
			continue
		}
		if i == 0 {
			return false
		}
		parent := strings.Join(parts[:i], "/")
		if from != parent && !strings.HasPrefix(from, parent+"/") {
			return false
		}
	}
	return true
}
func (a *analysis) localCandidates(p string, g *group) []*group {
	if g == nil || !internalAllowed(p, g.importPath) || a.standardIndex[p] {
		return nil
	}
	candidates := []*group{}
	for _, target := range a.importsByPath[p] {
		if a.workspace || target.moduleRoot == g.moduleRoot {
			candidates = append(candidates, target)
		}
	}
	return candidates
}
func (a *analysis) Import(p string) (*types.Package, error) {
	if a.current == nil || !internalAllowed(p, a.current.importPath) {
		return nil, fmt.Errorf("Go internal package is outside importer scope: %s", p)
	}
	if a.standardIndex[p] {
		return a.standardImport(p)
	}
	candidates := a.localCandidates(p, a.current)
	if len(candidates) != 1 {
		return nil, fmt.Errorf("unavailable or ambiguous retained Go package: %s", p)
	}
	g := candidates[0]
	if g.state == 1 {
		return nil, errors.New("retained Go import cycle")
	}
	if a.importDepth >= 128 {
		return nil, errors.New("Go retained import traversal limit (128)")
	}
	a.importDepth++
	a.check(g)
	a.importDepth--
	if g.pkg == nil || !g.healthy || g.excluded {
		return nil, fmt.Errorf("retained Go package is incomplete: %s", p)
	}
	return g.pkg, nil
}
func (a *analysis) check(g *group) {
	if g.state != 0 || len(g.sources) == 0 {
		return
	}
	g.state = 1
	g.info = &types.Info{Defs: map[*ast.Ident]types.Object{}, Uses: map[*ast.Ident]types.Object{}, Types: map[ast.Expr]types.TypeAndValue{}, Selections: map[*ast.SelectorExpr]*types.Selection{}, Instances: map[*ast.Ident]types.Instance{}}
	files := []*ast.File{}
	for _, s := range g.sources {
		files = append(files, s.file)
	}
	conf := types.Config{Importer: a, GoVersion: g.version, Sizes: types.SizesFor("gc", a.req.GOARCH), Error: func(err error) {
		g.healthy = false
		p := g.sources[0].path
		if e, ok := err.(types.Error); ok {
			p = a.fset.Position(e.Pos).Filename
		}
		a.add("diagnostics", row{"path": p, "message": err.Error()})
	}}
	packagePath := g.importPath
	if packagePath == "" {
		packagePath = g.key
	}
	previous := a.current
	a.current = g
	g.pkg, _ = conf.Check(packagePath, a.fset, files, g.info)
	a.current = previous
	g.state = 2
	for id, obj := range g.info.Defs {
		if d := a.syntaxDefs[id]; d != nil && obj != nil {
			a.defs[obj] = d
		}
	}
}
func unwrap(e ast.Expr) ast.Expr {
	switch e := e.(type) {
	case *ast.ParenExpr:
		return unwrap(e.X)
	case *ast.IndexExpr:
		return unwrap(e.X)
	case *ast.IndexListExpr:
		return unwrap(e.X)
	}
	return e
}
func (a *analysis) callsAndReferences(s *source) {
	g := s.group
	caller := ""
	stack := []string{}
	ast.Inspect(s.file, func(n ast.Node) bool {
		if n == nil {
			caller = stack[len(stack)-1]
			stack = stack[:len(stack)-1]
			return false
		}
		stack = append(stack, caller)
		switch n := n.(type) {
		case *ast.FuncDecl:
			caller = ""
			if d := a.funcDefs[n]; d != nil && d.callable {
				caller = d.key
			}
		case *ast.FuncLit:
			caller = ""
		case *ast.ImportSpec:
			return true
		case *ast.Ident:
			if g.info.Defs[n] != nil || n == s.file.Name {
				return true
			}
			obj := g.info.Uses[n]
			if obj == nil {
				return true
			}
			if obj.Parent() == types.Universe {
				return true
			}
			if d := a.defs[obj]; d != nil && g.healthy && !g.excluded && d.src.group.healthy && !d.src.group.excluded {
				a.site(s, n, "references", d.key, caller, "")
			} else if p, ok := obj.(*types.PkgName); ok && len(a.localCandidates(p.Imported().Path(), g)) == 1 && g.healthy && !g.excluded {
				a.site(s, n, "references", a.localCandidates(p.Imported().Path(), g)[0].key, caller, "")
			} else {
				a.unknown(s, n, "REFERENCES", "missing_dependency", caller, nil)
			}
		case *ast.RangeStmt:
			if value, ok := g.info.Types[n.X]; ok && value.Type != nil {
				if _, function := value.Type.Underlying().(*types.Signature); function {
					a.unknown(s, n, "CALLS_STATIC", "unsupported_operation", caller, nil)
				}
			}
		case *ast.CallExpr:
			if tv, ok := g.info.Types[n.Fun]; ok && tv.IsType() {
				return true
			}
			e := unwrap(n.Fun)
			var obj types.Object
			reason := "dynamic_property"
			switch e := e.(type) {
			case *ast.Ident:
				obj = g.info.Uses[e]
			case *ast.SelectorExpr:
				if sel := g.info.Selections[e]; sel != nil {
					if _, ok := sel.Recv().Underlying().(*types.Interface); ok {
						reason = "trait_dispatch"
					} else {
						obj = sel.Obj()
					}
				} else {
					obj = g.info.Uses[e.Sel]
				}
			}
			fn, ok := obj.(*types.Func)
			if ok {
				obj = fn.Origin()
			}
			d := a.defs[obj]
			if ok && fn != nil && d != nil && d.callable && caller != "" && g.healthy && !g.excluded && d.src.group.healthy && !d.src.group.excluded {
				a.site(s, n, "calls", d.key, caller, "")
			} else {
				if obj != nil && obj.Pkg() != nil && d == nil {
					reason = "missing_dependency"
				}
				if !g.healthy {
					reason = "extraction_failure"
				}
				a.unknown(s, n, "CALLS_STATIC", reason, caller, nil)
			}
		}
		return true
	})
}
func (a *analysis) execution() {
	var keys []string
	for key := range a.groups {
		keys = append(keys, key)
	}
	sort.Strings(keys)
	if !a.req.SyntaxOnly {
		for _, key := range keys {
			a.check(a.groups[key])
		}
	}
	for _, s := range a.sources {
		if !s.selected || !s.parsed {
			continue
		}
		g := s.group
		for _, im := range s.file.Imports {
			p, err := strconv.Unquote(im.Path.Value)
			if err != nil {
				continue
			}
			candidates := a.localCandidates(p, g)
			if a.standardIndex[p] {
				a.unknown(s, im, "IMPORTS", "missing_dependency", "", nil)
				continue
			}
			candidateKeys := []string{}
			for _, c := range candidates {
				candidateKeys = append(candidateKeys, c.key)
			}
			if !a.req.SyntaxOnly && !g.excluded && len(candidates) == 1 && !candidates[0].excluded {
				a.site(s, im, "imports", candidates[0].key, "", s.path)
				a.site(s, im, "imports", candidates[0].key, "", g.key)
			} else {
				if len(candidates) < 2 {
					candidateKeys = nil
				}
				a.unknown(s, im, "IMPORTS", "missing_dependency", "", candidateKeys)
			}
		}
		if !a.req.SyntaxOnly {
			a.callsAndReferences(s)
		} else {
			ast.Inspect(s.file, func(n ast.Node) bool {
				if call, ok := n.(*ast.CallExpr); ok {
					a.unknown(s, call, "CALLS_STATIC", "unsupported_operation", "", nil)
				}
				return true
			})
		}
	}
}
func main() {
	defer func() {
		if err := recover(); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(1)
		}
	}()
	var req request
	decoder := json.NewDecoder(io.LimitReader(os.Stdin, 256*1024*1024+16*1024*1024))
	if err := decoder.Decode(&req); err != nil {
		panic(err)
	}
	if req.GOOS == "" {
		req.GOOS = runtime.GOOS
	}
	if req.GOARCH == "" {
		req.GOARCH = runtime.GOARCH
	}
	if !strings.HasPrefix(runtime.Version(), "go1.") {
		panic("Go analysis requires a supported Go 1.x compiler")
	}
	if types.SizesFor("gc", req.GOARCH) == nil {
		panic("Unsupported Go GOARCH")
	}
	supportedOS := map[string]bool{"aix": true, "android": true, "darwin": true, "dragonfly": true, "freebsd": true, "hurd": true, "illumos": true, "ios": true, "js": true, "linux": true, "netbsd": true, "openbsd": true, "plan9": true, "solaris": true, "wasip1": true, "windows": true}
	if !supportedOS[req.GOOS] {
		panic("Unsupported Go GOOS")
	}
	if len(req.Tags) > 64 {
		panic("Go build tag limit exceeded (64)")
	}
	seenTags := map[string]bool{}
	for _, tag := range req.Tags {
		if tag == "" || seenTags[tag] || strings.ContainsAny(tag, " ,/\\\t\n") {
			panic("Go tags must be unique tag names")
		}
		seenTags[tag] = true
	}
	req.Roots = roots(req.Roots)
	sort.Strings(req.Tags)
	provider := row{"name": "go-ast-types", "version": runtime.Version(), "goos": req.GOOS, "goarch": req.GOARCH, "tags": req.Tags, "tests": req.Tests, "source_roots": req.Roots, "syntax_only": req.SyntaxOnly, "cgo": false, "executes_source": false, "emit": false, "mapping": "retained-go-packages-bindings-v1", "release_tags": build.Default.ReleaseTags, "tool_tags": targetToolTags(req.GOARCH), "module_resolution": "retained module/workspace inputs and trusted standard-library types; external targets unavailable"}
	out := row{"schema_version": "1.0", "provider": provider, "omitted": []string{}}
	for _, k := range []string{"packages", "groups", "modules", "definitions", "imports", "references", "calls", "unresolved", "diagnostics"} {
		out[k] = []row{}
	}
	if !req.IdentityOnly {
		a := &analysis{req: req, fset: token.NewFileSet(), groups: map[string]*group{}, importsByPath: map[string][]*group{}, defs: map[types.Object]*definition{}, syntaxDefs: map[*ast.Ident]*definition{}, funcDefs: map[*ast.FuncDecl]*definition{}, output: out}
		a.platformIndex()
		a.load()
		a.execution()
	}
	// Type checker maps never control serialized ordering or canonical identity.
	for _, k := range []string{"packages", "groups", "modules", "definitions", "imports", "references", "calls", "unresolved", "diagnostics"} {
		rs := array(out, k)
		sort.SliceStable(rs, func(i, j int) bool {
			a, _ := json.Marshal(rs[i])
			b, _ := json.Marshal(rs[j])
			return bytes.Compare(a, b) < 0
		})
	}
	encoded, err := json.Marshal(out)
	if err != nil {
		panic(err)
	}
	if len(encoded) > 16*1024*1024 {
		panic("Go output byte quota exceeded")
	}
	if _, err := os.Stdout.Write(encoded); err != nil {
		panic(err)
	}
}
