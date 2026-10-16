// Clang AST/preprocessor analysis over an in-memory retained filesystem only.
// The adapter does not build repository code, run compiler plugins or read PCH,
// modules, compilation databases, system headers or external source files.
#include <algorithm>
#include <clang/AST/ASTConsumer.h>
#include <clang/AST/Attr.h>
#include <clang/AST/ParentMapContext.h>
#include <clang/AST/RecursiveASTVisitor.h>
#include <clang/Basic/Version.h>
#include <clang/Frontend/CompilerInstance.h>
#include <clang/Frontend/FrontendActions.h>
#include <clang/Index/USRGeneration.h>
#include <clang/Lex/Lexer.h>
#include <clang/Lex/PPCallbacks.h>
#include <clang/Tooling/Tooling.h>
#include <iostream>
#include <llvm/Support/JSON.h>
#include <llvm/Support/VirtualFileSystem.h>
#include <map>
#include <set>
#include <stdexcept>
#include <string>
#include <vector>

using namespace clang;
namespace json = llvm::json;
static constexpr size_t MAX_FILES = 4096, MAX_DEFINITIONS = 4096,
                        MAX_OCCURRENCES = 16384, MAX_TRANSLATIONS = 256;
static const std::string BASE = "/briefcase-retained/";

struct Span {
  std::string path;
  unsigned start = 0, end = 0;
  bool macro = false;
  explicit operator bool() const { return !path.empty() && start < end; }
};
struct Definition {
  Span span;
  std::string key, usr, signature;
  bool callable = false, body = false, blocked = false;
};
struct Pending {
  Span span;
  std::string family, usr, caller, reason;
};
static std::string encoded(const json::Value &value) {
  std::string result;
  llvm::raw_string_ostream stream(result);
  stream << value;
  return result;
}
static bool suffix(const std::string &path, llvm::StringRef extension) {
  return llvm::StringRef(path).ends_with(extension);
}
static bool translation(const std::string &path, bool cpp) {
  return cpp ? suffix(path, ".cpp") || suffix(path, ".cc") ||
                   suffix(path, ".cxx") || suffix(path, ".c++") ||
                   suffix(path, ".C")
             : suffix(path, ".c");
}
static bool source(const std::string &path, bool cpp) {
  return translation(path, cpp) || suffix(path, ".h") || suffix(path, ".H") ||
         (suffix(path, ".inc") || suffix(path, ".def")) ||
         (cpp && (suffix(path, ".hh") || suffix(path, ".hpp") ||
                  suffix(path, ".hxx") || suffix(path, ".h++") ||
                  suffix(path, ".ipp") || suffix(path, ".tpp") ||
                  suffix(path, ".inl") || suffix(path, ".tcc") ||
                  suffix(path, ".txx") || suffix(path, ".ixx") ||
                  suffix(path, ".cppm")));
}
static bool safe(const std::string &path, bool root = false) {
  if (root && path == ".")
    return true;
  if (path.empty() || path[0] == '/' || path.find('\\') != std::string::npos ||
      path.find(':') != std::string::npos ||
      path.find('\0') != std::string::npos)
    return false;
  llvm::SmallVector<llvm::StringRef> parts;
  llvm::StringRef(path).split(parts, '/');
  return std::all_of(parts.begin(), parts.end(), [](auto part) {
    return !part.empty() && part != "." && part != "..";
  });
}
static bool within(const std::string &path, const std::string &root) {
  return root == "." || llvm::StringRef(path).starts_with(root + "/");
}

class Adapter {
public:
  std::map<std::string, std::string> files;
  std::vector<std::string> roots, includes, defines;
  std::string standard, target;
  bool cpp = false, syntaxOnly = false, identityOnly = false;
  std::map<std::string, std::map<std::string, json::Object>> rows;
  std::map<std::string, Definition> definitions;
  std::vector<Pending> pending;
  void queue(Pending value) {
    if (pending.size() >= MAX_OCCURRENCES)
      throw std::runtime_error(
          "Clang pending occurrence quota exceeded (16384)");
    pending.push_back(std::move(value));
  }
  std::set<std::string> omitted, visited, failed;
  size_t occurrences = 0, translations = 0;

  void add(const std::string &family, json::Object row) {
    std::string key = encoded(json::Value(json::Object(row)));
    if (rows[family].count(key))
      return;
    if ((family == "imports" || family == "references" || family == "calls" ||
         family == "unresolved") &&
        ++occurrences > MAX_OCCURRENCES)
      throw std::runtime_error("Clang occurrence quota exceeded (16384)");
    rows[family].emplace(key, std::move(row));
  }
  std::string path(SourceLocation location, const SourceManager &sm) const {
    if (location.isInvalid())
      return "";
    auto name = sm.getFilename(sm.getExpansionLoc(location));
    if (!name.starts_with(BASE))
      return "";
    std::string relative = name.drop_front(BASE.size()).str();
    return files.count(relative) ? relative : "";
  }
  Span span(SourceRange range, const SourceManager &sm,
            const LangOptions &language) const {
    Span out;
    if (range.isInvalid())
      return out;
    out.macro = range.getBegin().isMacroID() || range.getEnd().isMacroID();
    SourceLocation start = sm.getExpansionLoc(range.getBegin());
    SourceLocation end = Lexer::getLocForEndOfToken(
        sm.getExpansionLoc(range.getEnd()), 0, sm, language);
    out.path = path(start, sm);
    if (out.path.empty() || end.isInvalid() ||
        sm.getFileID(start) != sm.getFileID(end))
      return {};
    out.start = sm.getFileOffset(start);
    out.end = sm.getFileOffset(end);
    if (out.end > files.at(out.path).size())
      return {};
    return out;
  }
  Span line(SourceLocation location, const SourceManager &sm) const {
    Span out;
    out.path = path(location, sm);
    if (out.path.empty())
      return out;
    out.start = sm.getFileOffset(sm.getExpansionLoc(location));
    auto end = files.at(out.path).find('\n', out.start);
    out.end = end == std::string::npos ? files.at(out.path).size() : end;
    return out;
  }
  static json::Object record(const Span &span) {
    return json::Object{
        {"path", span.path}, {"start", span.start}, {"end", span.end}};
  }
  void unknown(const Span &span, const std::string &predicate,
               const std::string &reason, const std::string &caller = "",
               std::vector<std::string> candidates = {}) {
    if (!span)
      return;
    std::sort(candidates.begin(), candidates.end());
    candidates.erase(std::unique(candidates.begin(), candidates.end()),
                     candidates.end());
    auto row = record(span);
    row["predicate"] = predicate;
    row["reason"] = reason;
    row["caller"] = caller.empty() ? json::Value(nullptr) : json::Value(caller);
    row["status"] = candidates.size() > 1 ? "ambiguous" : "unresolved";
    json::Array keys;
    for (const auto &key : candidates)
      keys.push_back(key);
    row["candidate_keys"] = std::move(keys);
    add("unresolved", std::move(row));
  }
  void site(const Span &span, const std::string &family, const std::string &to,
            const std::string &caller = "") {
    if (!span)
      return;
    auto row = record(span);
    row["target"] = to;
    row["caller"] = caller.empty() ? json::Value(nullptr) : json::Value(caller);
    add(family, std::move(row));
  }
  void diagnostic(const std::string &path, const std::string &message) {
    add("diagnostics", json::Object{{"path", path.empty() ? json::Value(nullptr)
                                                          : json::Value(path)},
                                    {"message", message}});
  }
  std::string usr(const NamedDecl *decl) const {
    if (!decl)
      return "";
    llvm::SmallString<256> bytes;
    if (index::generateUSRForDecl(decl->getCanonicalDecl(), bytes))
      return "";
    return bytes.str().str();
  }
  std::string declare(const NamedDecl *decl, const Span &span, bool callable,
                      bool body, bool blocked) {
    if (!span || decl->isImplicit() || decl->getNameAsString().empty())
      return "";
    std::string name = decl->getQualifiedNameAsString();
    if (name.empty())
      name = decl->getNameAsString();
    std::string key =
        span.path + "::" + name + "#" + std::to_string(span.start);
    auto found = definitions.find(key);
    if (found == definitions.end()) {
      if (definitions.size() >= MAX_DEFINITIONS)
        throw std::runtime_error("Clang declaration quota exceeded (4096)");
      definitions.emplace(key, Definition{span, key, usr(decl),
                                          isa<ValueDecl>(decl)
                                              ? cast<ValueDecl>(decl)
                                                    ->getType()
                                                    .getCanonicalType()
                                                    .getAsString()
                                              : "",
                                          callable, body, blocked});
    } else {
      found->second.blocked |=
          blocked || found->second.usr != usr(decl) ||
          (isa<ValueDecl>(decl) &&
           found->second.signature != cast<ValueDecl>(decl)
                                          ->getType()
                                          .getCanonicalType()
                                          .getAsString());
      found->second.callable &= callable;
      found->second.body |= body;
    }
    return key;
  }
  void read() {
    std::string input;
    char bytes[65536];
    while (std::cin) {
      std::cin.read(bytes, sizeof(bytes));
      input.append(bytes, static_cast<size_t>(std::cin.gcount()));
      if (input.size() > 272ULL * 1024 * 1024)
        throw std::runtime_error("Clang input byte quota exceeded");
    }
    auto parsed = json::parse(input);
    if (!parsed)
      throw std::runtime_error("Invalid Clang adapter input");
    auto &object = *parsed->getAsObject();
    cpp = object.getString("language").value_or("") == "cpp";
    syntaxOnly = object.getBoolean("syntax_only").value_or(false);
    identityOnly = object.getBoolean("identity_only").value_or(false);
    for (const auto &item : *object.getObject("files")) {
      std::string path = item.first.str();
      if (!safe(path))
        throw std::runtime_error("Unsafe retained Clang path");
      files.emplace(path, item.second.getAsString()->str());
    }
    auto list = [&](const char *name, std::vector<std::string> &out,
                    bool directory) {
      for (const auto &item : *object.getArray(name)) {
        std::string value = item.getAsString()->str();
        if (directory && !safe(value, true))
          throw std::runtime_error(
              "Clang roots must be normalized relative paths");
        out.push_back(value);
      }
      if (out.size() > 64)
        throw std::runtime_error("Clang root/define quota exceeded (64)");
      std::sort(out.begin(), out.end());
      if (std::adjacent_find(out.begin(), out.end()) != out.end())
        throw std::runtime_error("Clang settings must be unique");
    };
    list("source_roots", roots, true);
    list("include_roots", includes, true);
    list("defines", defines, false);
    if (roots.empty())
      roots.push_back(".");
    if (includes.empty())
      includes.push_back(".");
    std::set<std::string> names;
    for (const auto &define : defines) {
      auto equal = define.find('=');
      std::string name = define.substr(0, equal);
      if (!names.insert(name).second || define.size() > 256 || name.empty() ||
          !(isalpha(static_cast<unsigned char>(name[0])) || name[0] == '_') ||
          !std::all_of(
              name.begin(), name.end(),
              [](unsigned char c) { return isalnum(c) || c == '_'; }) ||
          define.find_first_of("\n\r\"'") != std::string::npos)
        throw std::runtime_error(
            "Clang defines require NAME or NAME=integer/identifier");
      if (equal != std::string::npos &&
          !std::all_of(define.begin() + equal + 1, define.end(),
                       [](unsigned char c) {
                         return isalnum(c) || c == '_' || c == '-' || c == '+';
                       }))
        throw std::runtime_error("Unsafe Clang define value");
    }
    standard =
        object.getString("standard").value_or(cpp ? "c++17" : "c17").str();
    const std::set<std::string> standards =
        cpp ? std::set<std::string>{"c++11", "c++14", "c++17", "c++20", "c++23"}
            : std::set<std::string>{"c89", "c99", "c11", "c17", "c23"};
    if (!standards.count(standard))
      throw std::runtime_error(
          "Unsupported non-extension Clang source standard");
    target = object.getString("target").value_or("").str();
    if (target.empty() || target[0] == '-' ||
        !std::all_of(target.begin(), target.end(), [](unsigned char c) {
          return isalnum(c) || c == '_' || c == '-' || c == '.';
        }))
      throw std::runtime_error(
          "Clang target must be an explicit compiler target triple");
  }
  void analyze();
  json::Object output() {
    json::Object result;
    for (auto &[family, list] : rows) {
      json::Array values;
      for (auto &[key, row] : list)
        values.push_back(std::move(row));
      result[family] = std::move(values);
    }
    json::Array excluded, sourceRoots, includeRoots, symbols;
    for (const auto &path : omitted)
      excluded.push_back(path);
    for (const auto &root : roots)
      sourceRoots.push_back(root);
    for (const auto &root : includes)
      includeRoots.push_back(root);
    for (const auto &define : defines)
      symbols.push_back(define);
    result["schema_version"] = "1.0";
    result["omitted"] = std::move(excluded);
    result["provider"] =
        json::Object{{"name", "clang-ast"},
                     {"version", CLANG_VERSION_STRING},
                     {"language", cpp ? "cpp" : "c"},
                     {"standard", standard},
                     {"target", target},
                     {"source_roots", std::move(sourceRoots)},
                     {"include_roots", std::move(includeRoots)},
                     {"defines", std::move(symbols)},
                     {"syntax_only", syntaxOnly},
                     {"executes_source", false},
                     {"emit", false},
                     {"plugins", false},
                     {"filesystem", "retained-memory-only"},
                     {"mapping", "retained-clang-usr-definitions-v1"}};
    return result;
  }
};

class Diagnostics : public DiagnosticConsumer {
  Adapter &adapter;

public:
  std::string translation;
  bool error = false;
  Diagnostics(Adapter &adapter) : adapter(adapter) {}
  void HandleDiagnostic(DiagnosticsEngine::Level level,
                        const Diagnostic &info) override {
    DiagnosticConsumer::HandleDiagnostic(level, info);
    if (level < DiagnosticsEngine::Warning)
      return;
    llvm::SmallString<256> message;
    info.FormatDiagnostic(message);
    adapter.diagnostic(translation, message.str().str());
    if (level >= DiagnosticsEngine::Error)
      error = true;
  }
};
class PreprocessorEvents : public PPCallbacks {
  Adapter &adapter;
  SourceManager &sm;
  const LangOptions &language;

public:
  PreprocessorEvents(Adapter &adapter, CompilerInstance &compiler)
      : adapter(adapter), sm(compiler.getSourceManager()),
        language(compiler.getLangOpts()) {}
  void InclusionDirective(SourceLocation hash, const Token &, llvm::StringRef,
                          bool, CharSourceRange range,
                          OptionalFileEntryRef file, llvm::StringRef,
                          llvm::StringRef, const Module *, bool,
                          SrcMgr::CharacteristicKind) override {
    Span span = adapter.span(SourceRange(hash, range.getEnd()), sm, language);
    if (range.getBegin().isMacroID() || range.getEnd().isMacroID())
      span.macro = true;
    if (file && llvm::StringRef(file->getName()).starts_with(BASE) &&
        !span.macro) {
      std::string to =
          llvm::StringRef(file->getName()).drop_front(BASE.size()).str();
      if (adapter.files.count(to) && source(to, adapter.cpp)) {
        adapter.site(span, "imports", to);
        adapter.visited.insert(to);
        return;
      }
    }
    adapter.unknown(span, "IMPORTS",
                    span.macro ? "unsupported_generated_source"
                               : "missing_dependency");
  }
  bool FileNotFound(llvm::StringRef) override { return false; }
  void MacroExpands(const Token &, const MacroDefinition &, SourceRange range,
                    const MacroArgs *) override {
    Span span = adapter.span(range, sm, language);
    adapter.unknown(span, "CALLS_STATIC", "unsupported_generated_source");
    adapter.unknown(span, "REFERENCES", "unsupported_generated_source");
  }
  void SourceRangeSkipped(SourceRange range, SourceLocation) override {
    Span span = adapter.span(range, sm, language);
    if (span)
      adapter.diagnostic(span.path, "Inactive preprocessor source is retained "
                                    "text outside this build selection");
  }
  void PragmaDirective(SourceLocation location, PragmaIntroducerKind) override {
    Span span = adapter.line(location, sm);
    if (span) {
      auto text =
          adapter.files.at(span.path).substr(span.start, span.end - span.start);
      if (text.find("once") == std::string::npos) {
        adapter.unknown(span, "CALLS_STATIC", "unsupported_context");
        adapter.diagnostic(span.path,
                           "Pragma is outside retained exact binding scope");
      }
    }
  }
};
class Visitor : public RecursiveASTVisitor<Visitor> {
  Adapter &adapter;
  ASTContext &context;
  SourceManager &sm;
  const LangOptions &language;
  std::string caller;
  std::set<std::string> unitDefinitions;

public:
  Visitor(Adapter &adapter, ASTContext &context)
      : adapter(adapter), context(context), sm(context.getSourceManager()),
        language(context.getLangOpts()) {}
  bool shouldVisitTemplateInstantiations() const { return false; }
  bool supported(const FunctionDecl *function) const {
    if (!function || function->isImplicit() ||
        !function->doesThisDeclarationHaveABody() ||
        function->isDependentContext() || function->isConsteval() ||
        function->getDescribedFunctionTemplate() ||
        function->getTemplatedKind() != FunctionDecl::TK_NonTemplate ||
        function->hasAttr<AliasAttr>() || function->hasAttr<WeakAttr>() ||
        function->hasAttr<AsmLabelAttr>() || function->hasAttr<IFuncAttr>() ||
        function->hasAttr<WeakRefAttr>())
      return false;
    if (auto method = dyn_cast<CXXMethodDecl>(function))
      if (method->getParent()->isLambda())
        return false;
    Span span = adapter.span(function->getSourceRange(), sm, language);
    return span && !span.macro && !adapter.syntaxOnly;
  }
  std::string key(const FunctionDecl *function) const {
    Span span = adapter.span(function->getSourceRange(), sm, language);
    return span ? span.path + "::" + function->getQualifiedNameAsString() +
                      "#" + std::to_string(span.start)
                : "";
  }
  bool TraverseDecl(Decl *decl) {
    std::string saved = caller;
    if (auto function = dyn_cast_or_null<FunctionDecl>(decl))
      caller = supported(function) ? key(function) : "";
    else if (isa_and_nonnull<ParmVarDecl>(decl) ||
             isa_and_nonnull<CXXRecordDecl>(decl) ||
             isa_and_nonnull<StaticAssertDecl>(decl))
      caller.clear();
    bool result = RecursiveASTVisitor<Visitor>::TraverseDecl(decl);
    caller = saved;
    return result;
  }
  bool TraverseTypeLoc(TypeLoc location, bool qualifier = true) {
    std::string saved = caller;
    caller.clear();
    bool result =
        RecursiveASTVisitor<Visitor>::TraverseTypeLoc(location, qualifier);
    caller = saved;
    return result;
  }
  bool TraverseStmt(Stmt *statement) {
    std::string saved = caller;
    if (auto call = dyn_cast_or_null<CallExpr>(statement)) {
      if (auto callee = call->getDirectCallee())
        if (callee->getBuiltinID() || callee->isConsteval())
          caller.clear();
    }
    if (auto condition = dyn_cast_or_null<IfStmt>(statement)) {
      if (condition->isConstexpr()) {
        adapter.unknown(adapter.span(condition->getSourceRange(), sm, language),
                        "CALLS_STATIC", "unsupported_context", caller);
        caller.clear();
      }
    }
    if (isa_and_nonnull<ChooseExpr>(statement) ||
        isa_and_nonnull<GenericSelectionExpr>(statement)) {
      adapter.unknown(adapter.span(statement->getSourceRange(), sm, language),
                      "CALLS_STATIC", "unsupported_context", caller);
      caller.clear();
    }
    if (isa_and_nonnull<UnaryExprOrTypeTraitExpr>(statement) ||
        isa_and_nonnull<CXXNoexceptExpr>(statement) ||
        isa_and_nonnull<RequiresExpr>(statement) ||
        isa_and_nonnull<CXXTypeidExpr>(statement))
      caller.clear();
    bool result = RecursiveASTVisitor<Visitor>::TraverseStmt(statement);
    caller = saved;
    return result;
  }
  bool TraverseLambdaExpr(LambdaExpr *expression) {
    adapter.unknown(adapter.span(expression->getSourceRange(), sm, language),
                    "CALLS_STATIC", "dynamic_property", caller);
    std::string saved = caller;
    caller.clear();
    bool result = RecursiveASTVisitor<Visitor>::TraverseLambdaExpr(expression);
    caller = saved;
    return result;
  }
  bool VisitNamedDecl(NamedDecl *decl) {
    Span span = adapter.span(decl->getSourceRange(), sm, language);
    auto function = dyn_cast<FunctionDecl>(decl);
    std::string identity = adapter.declare(
        decl, span, function && supported(function),
        function             ? function->doesThisDeclarationHaveABody()
        : isa<TagDecl>(decl) ? cast<TagDecl>(decl)->isCompleteDefinition()
        : isa<VarDecl>(decl)
            ? cast<VarDecl>(decl)->isThisDeclarationADefinition() !=
                  VarDecl::DeclarationOnly
            : false,
        span.macro || decl->isInvalidDecl());
    if (!identity.empty())
      unitDefinitions.insert(identity);
    if (isa<FunctionTemplateDecl>(decl) || isa<ClassTemplateDecl>(decl))
      adapter.unknown(span, "CALLS_STATIC", "unsupported_context", caller);
    if (isa<CXXConstructorDecl>(decl) || isa<CXXDestructorDecl>(decl) ||
        decl->hasAttr<CleanupAttr>() || decl->hasAttr<ConstructorAttr>() ||
        decl->hasAttr<DestructorAttr>())
      adapter.unknown(span, "CALLS_STATIC", "unsupported_operation", caller);
    if (function &&
        (function->hasAttr<AliasAttr>() || function->hasAttr<WeakAttr>() ||
         function->hasAttr<AsmLabelAttr>() || function->hasAttr<IFuncAttr>() ||
         function->hasAttr<WeakRefAttr>()))
      adapter.unknown(span, "CALLS_STATIC", "unsupported_context", caller);
    return true;
  }
  bool VisitTagTypeLoc(TagTypeLoc location) {
    reference(location.getSourceRange(), location.getDecl());
    return true;
  }
  bool VisitTypedefTypeLoc(TypedefTypeLoc location) {
    reference(location.getSourceRange(), location.getTypePtr()->getDecl());
    return true;
  }
  bool
  VisitTemplateSpecializationTypeLoc(TemplateSpecializationTypeLoc location) {
    reference(location.getSourceRange(),
              location.getTypePtr()->getTemplateName().getAsTemplateDecl());
    return true;
  }
  bool VisitUsingDirectiveDecl(UsingDirectiveDecl *decl) {
    reference(decl->getSourceRange(), decl->getNominatedNamespace());
    return true;
  }
  bool VisitDeclRefExpr(DeclRefExpr *expression) {
    reference(expression->getSourceRange(), expression->getDecl());
    return true;
  }
  bool VisitMemberExpr(MemberExpr *expression) {
    reference(expression->getSourceRange(), expression->getMemberDecl());
    return true;
  }
  void reference(SourceRange range, NamedDecl *decl) {
    if (adapter.syntaxOnly)
      return;
    Span span = adapter.span(range, sm, language);
    if (!span)
      return;
    if (span.macro)
      adapter.unknown(span, "REFERENCES", "unsupported_generated_source",
                      caller);
    else
      adapter.queue({span, "references", adapter.usr(decl), caller, ""});
  }
  bool exact(const FunctionDecl *function, const CallExpr *call) const {
    if (auto method = dyn_cast<CXXMethodDecl>(function)) {
      if (!method->isVirtual() || method->hasAttr<FinalAttr>() ||
          method->getParent()->hasAttr<FinalAttr>())
        return true;
      const Expr *callee = call->getCallee()->IgnoreParenImpCasts();
      if (auto member = dyn_cast<MemberExpr>(callee))
        return member->hasQualifier();
      return false;
    }
    return true;
  }
  bool VisitCallExpr(CallExpr *call) {
    Span span = adapter.span(call->getSourceRange(), sm, language);
    if (!span)
      return true;
    const FunctionDecl *target = call->getDirectCallee();
    if (isa<CXXOperatorCallExpr>(call) ||
        (isa<CXXMemberCallExpr>(call) && call->getRParenLoc().isInvalid())) {
      adapter.unknown(span, "CALLS_STATIC", "unsupported_operation", caller);
      return true;
    }
    if (!span.macro && !caller.empty() && target && exact(target, call) &&
        !adapter.syntaxOnly)
      adapter.queue({span, "calls", adapter.usr(target), caller, ""});
    else
      adapter.unknown(span, "CALLS_STATIC",
                      span.macro       ? "unsupported_generated_source"
                      : caller.empty() ? "unsupported_context"
                      : target         ? "trait_dispatch"
                                       : "dynamic_property",
                      caller);
    for (auto argument : call->arguments())
      if (isa<CXXDefaultArgExpr>(argument))
        adapter.unknown(span, "CALLS_STATIC", "unsupported_operation", caller);
    return true;
  }
  bool VisitCXXConstructExpr(CXXConstructExpr *expression) {
    Span span = adapter.span(expression->getSourceRange(), sm, language);
    bool explicitSyntax = isa<CXXTemporaryObjectExpr>(expression);
    for (const auto &parent : context.getParents(*expression))
      if (parent.get<CXXNewExpr>() || parent.get<CXXFunctionalCastExpr>())
        explicitSyntax = true;
    if (span && explicitSyntax && !span.macro && !caller.empty() &&
        !adapter.syntaxOnly)
      adapter.queue({span, "calls", adapter.usr(expression->getConstructor()),
                     caller, ""});
    else
      adapter.unknown(span, "CALLS_STATIC", "unsupported_operation", caller);
    return true;
  }
  bool VisitVarDecl(VarDecl *decl) {
    if (const auto *record = decl->getType()->getAsCXXRecordDecl())
      if (!record->hasTrivialDestructor())
        adapter.unknown(adapter.span(decl->getSourceRange(), sm, language),
                        "CALLS_STATIC", "unsupported_operation", caller);
    return true;
  }
  bool VisitStmt(Stmt *stmt) {
    if (isa<GCCAsmStmt>(stmt) || isa<MSAsmStmt>(stmt)) {
      Span span = adapter.span(stmt->getSourceRange(), sm, language);
      adapter.unknown(span, "CALLS_STATIC", "unsupported_operation", caller);
      adapter.unknown(span, "REFERENCES", "unsupported_operation", caller);
    }
    if (isa<CXXNewExpr>(stmt) || isa<CXXDeleteExpr>(stmt) ||
        isa<CXXForRangeStmt>(stmt) || isa<CXXBindTemporaryExpr>(stmt) ||
        isa<CoroutineBodyStmt>(stmt) || isa<CXXThrowExpr>(stmt))
      adapter.unknown(adapter.span(stmt->getSourceRange(), sm, language),
                      "CALLS_STATIC", "unsupported_operation", caller);
    if (isa<UnresolvedLookupExpr>(stmt) ||
        isa<CXXDependentScopeMemberExpr>(stmt) ||
        isa<DependentScopeDeclRefExpr>(stmt)) {
      Span span = adapter.span(stmt->getSourceRange(), sm, language);
      adapter.unknown(span, "REFERENCES", "unsupported_operation", caller);
      adapter.unknown(span, "CALLS_STATIC", "unsupported_operation", caller);
    }
    return true;
  }
  void invalid() {
    for (const auto &key : unitDefinitions)
      adapter.definitions.at(key).blocked = true;
  }
};
class Consumer : public ASTConsumer {
  Adapter &adapter;

public:
  Consumer(Adapter &adapter) : adapter(adapter) {}
  void HandleTranslationUnit(ASTContext &context) override {
    Visitor visitor(adapter, context);
    visitor.TraverseDecl(context.getTranslationUnitDecl());
    if (context.getDiagnostics().hasErrorOccurred())
      visitor.invalid();
  }
};
class Action : public ASTFrontendAction {
  Adapter &adapter;

public:
  Action(Adapter &adapter) : adapter(adapter) {}
  bool BeginSourceFileAction(CompilerInstance &compiler) override {
    compiler.getPreprocessor().addPPCallbacks(
        std::make_unique<PreprocessorEvents>(adapter, compiler));
    return true;
  }
  std::unique_ptr<ASTConsumer> CreateASTConsumer(CompilerInstance &,
                                                 llvm::StringRef) override {
    return std::make_unique<Consumer>(adapter);
  }
};
void Adapter::analyze() {
  auto filesystem = llvm::makeIntrusiveRefCnt<llvm::vfs::InMemoryFileSystem>();
  filesystem->setCurrentWorkingDirectory(BASE);
  std::vector<std::string> units, headers;
  size_t count = 0;
  for (const auto &[path, text] : files) {
    if (!source(path, cpp))
      continue;
    if (++count > MAX_FILES)
      throw std::runtime_error("Clang source file quota exceeded (4096)");
    bool selected =
        std::any_of(roots.begin(), roots.end(),
                    [&](const auto &root) { return within(path, root); }) ||
        (!translation(path, cpp) &&
         std::any_of(includes.begin(), includes.end(),
                     [&](const auto &root) { return within(path, root); }));
    if (!selected || suffix(path, ".ixx") || suffix(path, ".cppm")) {
      omitted.insert(path);
      continue;
    }
    filesystem->addFile(
        BASE + path, 0,
        llvm::MemoryBuffer::getMemBufferCopy(text, BASE + path));
    if (translation(path, cpp))
      units.push_back(path);
    else
      headers.push_back(path);
  }
  if (count == 0)
    throw std::runtime_error("Clang analysis needs retained language source");
  auto analyzeUnit = [&](const std::string &unit, bool header) {
    if (++translations > MAX_TRANSLATIONS)
      throw std::runtime_error("Clang translation-unit quota exceeded (256)");
    FileSystemOptions settings;
    settings.WorkingDir = BASE;
    auto manager = llvm::makeIntrusiveRefCnt<FileManager>(settings, filesystem);
    Diagnostics diagnostics(*this);
    diagnostics.translation = unit;
    std::vector<std::string> flags = {"clang-tool",
                                      "--no-default-config",
                                      "-fsyntax-only",
                                      "-nostdinc",
                                      "-Wno-unused-command-line-argument",
                                      "-fno-modules",
                                      "-fno-implicit-modules",
                                      "-fno-delayed-template-parsing",
                                      "-ferror-limit=64",
                                      "-Wno-pragma-once-outside-header",
                                      "-x",
                                      cpp ? "c++" : "c",
                                      "-std=" + standard,
                                      "--target=" + target};
    for (const auto &root : includes)
      flags.push_back("-I" + BASE + (root == "." ? "" : root));
    for (const auto &define : defines)
      flags.push_back("-D" + define);
    flags.push_back(BASE + unit);
    visited.insert(unit);
    size_t before = pending.size();
    tooling::ToolInvocation invocation(flags, std::make_unique<Action>(*this),
                                       manager.get());
    invocation.setDiagnosticConsumer(&diagnostics);
    bool success = invocation.run();
    if (!success || diagnostics.error) {
      failed.insert(unit);
      pending.resize(before);
      diagnostic(unit, "Translation-unit errors prevent exact retained "
                       "reference/call binding");
      Span span{unit, 0, static_cast<unsigned>(files.at(unit).size()), false};
      for (const auto &predicate : {"IMPORTS", "REFERENCES", "CALLS_STATIC"})
        unknown(span, predicate, "extraction_failure");
    }
    if (header)
      diagnostic(
          unit, "Header analyzed standalone under the selected language/flags; "
                "includer-specific variants are outside scope");
  };
  for (const auto &unit : units)
    analyzeUnit(unit, false);
  for (const auto &header : headers)
    if (!visited.count(header))
      analyzeUnit(header, true);
  std::map<std::string, std::vector<Definition *>> symbols;
  for (auto &[key, definition] : definitions) {
    definition.callable &= !definition.blocked;
    auto row = record(definition.span);
    row["key"] = key;
    row["callable"] = definition.callable;
    add("definitions", std::move(row));
    if (!definition.usr.empty())
      symbols[definition.usr].push_back(&definition);
  }
  // A source occurrence seen in multiple translation units must agree on the
  // target. External linkage needs one retained compatible implementation.
  std::map<std::string, std::vector<Pending *>> sites;
  for (auto &site : pending)
    sites[site.family + site.span.path + ":" + std::to_string(site.span.start) +
          ":" + std::to_string(site.span.end) + ":" + site.caller]
        .push_back(&site);
  for (const auto &[key, occurrences] : sites) {
    const auto &occurrence = *occurrences.front();
    std::set<std::string> nativeTargets;
    for (auto site : occurrences)
      nativeTargets.insert(site->usr);
    std::vector<Definition *> candidates;
    for (const auto &target : nativeTargets)
      candidates.insert(candidates.end(), symbols[target].begin(),
                        symbols[target].end());
    std::set<std::string> signatures;
    bool blocked = false;
    for (const auto *definition : candidates) {
      signatures.insert(definition->signature);
      blocked |= definition->blocked;
    }
    bool call = occurrence.family == "calls";
    if (call)
      candidates.erase(std::remove_if(candidates.begin(), candidates.end(),
                                      [](auto def) { return !def->callable; }),
                       candidates.end());
    else {
      bool body =
          std::any_of(candidates.begin(), candidates.end(),
                      [](auto def) { return def->body && !def->blocked; });
      candidates.erase(std::remove_if(candidates.begin(), candidates.end(),
                                      [&](auto def) {
                                        return def->blocked ||
                                               (body && !def->body);
                                      }),
                       candidates.end());
    }
    std::vector<std::string> keys;
    for (const auto *candidate : candidates)
      keys.push_back(candidate->key);
    bool caller = !call || (definitions.count(occurrence.caller) &&
                            definitions.at(occurrence.caller).callable);
    if (nativeTargets.size() == 1 && signatures.size() == 1 && !blocked &&
        candidates.size() == 1 && caller && !syntaxOnly) {
      site(occurrence.span, occurrence.family, candidates[0]->key,
           occurrence.caller);
      if (!call && candidates[0]->span.path != occurrence.span.path)
        site(occurrence.span, "imports", candidates[0]->span.path,
             occurrence.caller);
    } else {
      unknown(occurrence.span, call ? "CALLS_STATIC" : "REFERENCES",
              "missing_dependency", occurrence.caller, keys);
      if (!call)
        unknown(occurrence.span, "IMPORTS", "missing_dependency",
                occurrence.caller);
    }
  }
  for (const auto &[path, text] : files)
    if (source(path, cpp))
      add("modules", json::Object{{"path", path},
                                  {"package", nullptr},
                                  {"parsed", visited.count(path) != 0 &&
                                                 !failed.count(path)}});
}
int main() {
  try {
    Adapter adapter;
    for (const auto &family :
         {"packages", "groups", "modules", "definitions", "imports",
          "references", "calls", "unresolved", "diagnostics"})
      adapter.rows[family];
    adapter.read();
    if (!adapter.identityOnly)
      adapter.analyze();
    std::string result = encoded(adapter.output());
    if (result.size() > 16 * 1024 * 1024)
      throw std::runtime_error("Clang output byte quota exceeded");
    std::cout << result;
  } catch (const std::exception &error) {
    std::cerr << error.what() << '\n';
    return 1;
  }
}
