//! Stage-5 strip policy seam.
//!
//! The erasable-only type-strip *policy* (which TS constructs are banned + the exact
//! fix messages) is owned by RT-003 (I-3). This file is RT-003's edit region: it
//! supplies the trait, the permissive default seam, and the real erasable-only
//! policy + the whitespace-preserving strip transform.

use crate::error::StripDiagnostic;
use crate::semantic::SemanticGraph;

/// Gate consulted before lowering to runtime IR. RT-003 installs the real
/// erasable-only implementation (rejecting enum / namespace / param-props /
/// `import =`); a rejection names the construct and the fix.
pub trait StripPolicy: Send + Sync {
    fn check(&self, sem: &SemanticGraph) -> Result<(), Vec<StripDiagnostic>>;
}

// === RT-003 ===
// Erasable-only TC39 type stripping (CANON §9, I-3): whitespace-preserving
// annotation erasure on the hot path. Never transpiles; never emits downlevel JS;
// never produces a sourcemap (positions are preserved by construction).

use oxc_ast::ast::{
    Class, Declaration, Decorator, FormalParameter, ImportOrExportKind, MethodDefinition,
    MethodDefinitionType, PropertyDefinition, PropertyDefinitionType, TSAccessibility,
    TSModuleDeclarationBody,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::Visit;
use oxc_span::{GetSpan, Span};

use crate::cst::Cst;

/// Default seam: accepts everything. This is an explicit placeholder, NOT a shipped
/// erasable-only guarantee — only the binary edge that opts out of `ErasablePolicy`
/// would install it.
#[derive(Default)]
pub struct PermissivePolicy;

impl StripPolicy for PermissivePolicy {
    fn check(&self, _sem: &SemanticGraph) -> Result<(), Vec<StripDiagnostic>> {
        Ok(())
    }
}

/// The erasable-only policy `meow` ships (I-3). Syntactic and stateless: it walks
/// the stage-2 node list and rejects every construct that emits runtime JS and so
/// cannot be type-stripped, each with a fix-pointing diagnostic. There is no
/// "transpile" policy — non-erasable syntax is an error, never downlevel-emitted.
#[derive(Default)]
pub struct ErasablePolicy;

impl StripPolicy for ErasablePolicy {
    fn check(&self, sem: &SemanticGraph) -> Result<(), Vec<StripDiagnostic>> {
        let nodes = sem.nodes();
        // A node is ambient (erasable) if it is, or is nested in, a `declare`
        // module/namespace — ambient declarations emit nothing.
        let is_ambient = |id| {
            nodes
                .ancestor_kinds(id)
                .any(|k| matches!(k, AstKind::TSModuleDeclaration(m) if m.declare))
        };

        let mut diags = Vec::new();
        for (id, node) in nodes.iter_enumerated() {
            match node.kind() {
                // `enum` / `const enum` (non-ambient): both still participate in emit.
                AstKind::TSEnumDeclaration(e) if !e.declare && !is_ambient(id) => {
                    diags.push(diag_enum(e.span));
                }
                // `namespace`/`module Foo { … }` with a runtime block body (non-ambient).
                // A dotted `namespace A.B { … }` reports once, at the innermost block.
                AstKind::TSModuleDeclaration(m)
                    if !m.declare
                        && matches!(m.body, Some(TSModuleDeclarationBody::TSModuleBlock(_)))
                        && !is_ambient(id) =>
                {
                    diags.push(diag_namespace(m.span));
                }
                // `constructor(public/private/protected/readonly/override x)`.
                AstKind::FormalParameter(p)
                    if p.accessibility.is_some() || p.readonly || p.r#override =>
                {
                    diags.push(diag_param_property(p.span));
                }
                AstKind::TSImportEqualsDeclaration(d) => diags.push(diag_import_equals(d.span)),
                AstKind::TSExportAssignment(d) => diags.push(diag_export_equals(d.span)),
                _ => {}
            }
        }

        if diags.is_empty() {
            Ok(())
        } else {
            Err(diags)
        }
    }
}

// ---- the non-erasable catalog (exemplar voice, CANON §9; cause + fix per entry) ----

fn diag(span: Span, message: &str, help: &str) -> StripDiagnostic {
    StripDiagnostic {
        span,
        message: message.to_string(),
        help: help.to_string(),
    }
}

/// `strip::enum`
fn diag_enum(span: Span) -> StripDiagnostic {
    diag(
        span,
        "Enums emit runtime code and cannot be type-stripped.",
        "Use a `const` object instead.",
    )
}

/// `strip::namespace`
fn diag_namespace(span: Span) -> StripDiagnostic {
    diag(
        span,
        "Namespaces with a runtime body emit code and cannot be type-stripped.",
        "Use ES modules (separate files and named exports) instead.",
    )
}

/// `strip::param_property`
fn diag_param_property(span: Span) -> StripDiagnostic {
    diag(
        span,
        "Parameter properties emit an assignment and cannot be type-stripped.",
        "Declare the field on the class and assign it in the constructor body instead.",
    )
}

/// `strip::import_equals`
fn diag_import_equals(span: Span) -> StripDiagnostic {
    diag(
        span,
        "`import =` emits runtime code and cannot be type-stripped.",
        "Use a standard ESM `import` instead.",
    )
}

/// `strip::export_equals`
fn diag_export_equals(span: Span) -> StripDiagnostic {
    diag(
        span,
        "`export =` emits CommonJS and cannot be type-stripped.",
        "Use `export default` or named exports instead.",
    )
}

/// Erase erasable TS syntax to whitespace. The output has the SAME byte length as
/// `source`: every retained byte keeps its original offset, and erased bytes become
/// ASCII spaces except `\n`/`\r`, which are preserved so line numbers are exact.
/// No transpile, no codegen, no sourcemap (CANON §9).
///
/// Pure and syntactic: it reads only the stage-1 `Cst`. Callers run the
/// [`ErasablePolicy`] gate first, so non-erasable constructs (which would not blank
/// to valid JS) never reach this function; ambient declarations and plain JS strip
/// cleanly to valid JS.
pub fn strip(source: &str, cst: &Cst) -> String {
    let mut blanker = Blanker {
        src: source.as_bytes(),
        ranges: Vec::new(),
    };
    blanker.visit_program(cst.program());

    let mut out = source.as_bytes().to_vec();
    for (start, end) in blanker.ranges {
        for byte in &mut out[start as usize..end as usize] {
            if *byte != b'\n' && *byte != b'\r' {
                *byte = b' ';
            }
        }
    }
    // Invariant: every range endpoint is a char boundary (AST spans and the ASCII
    // tokens found by scanning), and each touched byte is replaced 1:1 by an ASCII
    // space, so the result is valid UTF-8 of identical length.
    String::from_utf8(out).expect("blanking type spans preserves UTF-8 validity")
}

/// Collects byte ranges to blank by walking the AST. Ranges may overlap freely —
/// blanking is idempotent (every touched byte becomes a space).
struct Blanker<'s> {
    src: &'s [u8],
    ranges: Vec<(u32, u32)>,
}

impl<'s> Blanker<'s> {
    fn blank(&mut self, span: Span) {
        if span.start < span.end {
            self.ranges.push((span.start, span.end));
        }
    }

    fn blank_between(&mut self, start: u32, end: u32) {
        if start < end {
            self.ranges.push((start, end));
        }
    }

    /// Blank the keyword `word` if it occurs as a standalone token in `[start, end)`.
    fn blank_word(&mut self, start: u32, end: u32, word: &str) {
        if let Some(pos) = find_word(self.src, start, end, word) {
            self.ranges.push((pos, pos + word.len() as u32));
        }
    }

    /// Blank the first occurrence of ASCII `ch` in `[start, end)`.
    fn blank_char(&mut self, start: u32, end: u32, ch: u8) {
        let (s, e) = (start as usize, end as usize);
        if s >= e || e > self.src.len() {
            return;
        }
        if let Some(off) = self.src[s..e].iter().position(|&b| b == ch) {
            let abs = (s + off) as u32;
            self.ranges.push((abs, abs + 1));
        }
    }

    /// Blank a type-only import/export specifier and its adjacent comma so the
    /// surviving list stays syntactically valid (no leading/dangling comma).
    fn blank_specifier(&mut self, span: Span) {
        self.blank(span);
        // Prefer the trailing comma; fall back to the leading one (last element).
        let mut j = span.end as usize;
        while j < self.src.len() && is_space(self.src[j]) {
            j += 1;
        }
        if j < self.src.len() && self.src[j] == b',' {
            self.ranges.push((j as u32, j as u32 + 1));
            return;
        }
        let mut k = span.start as usize;
        while k > 0 && is_space(self.src[k - 1]) {
            k -= 1;
        }
        if k > 0 && self.src[k - 1] == b',' {
            self.ranges.push((k as u32 - 1, k as u32));
        }
    }

    /// Blank the TS-only modifier keywords on a class member, skipping any leading
    /// decorators (kept verbatim) and never touching JS modifiers (`static`,
    /// `accessor`, `async`, `get`/`set`).
    fn blank_member_modifiers(
        &mut self,
        member_start: u32,
        decorators: &[Decorator],
        key_start: u32,
        mods: &[(bool, &str)],
    ) {
        let region_start = decorators
            .iter()
            .map(|d| d.span.end)
            .max()
            .unwrap_or(member_start)
            .max(member_start);
        for &(present, word) in mods {
            if present {
                self.blank_word(region_start, key_start, word);
            }
        }
    }
}

impl<'a, 's> Visit<'a> for Blanker<'s> {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        match kind {
            // Whole type-only spans.
            AstKind::TSTypeAnnotation(t) => self.blank(t.span),
            AstKind::TSTypeAliasDeclaration(t) => self.blank(t.span),
            AstKind::TSInterfaceDeclaration(t) => self.blank(t.span),
            AstKind::TSTypeParameterDeclaration(t) => self.blank(t.span),
            AstKind::TSTypeParameterInstantiation(t) => self.blank(t.span),
            // Ambient enum / namespace reach strip only after the policy gate, and
            // ambient declarations emit nothing: erase the whole declaration (its
            // span includes the leading `declare`).
            AstKind::TSEnumDeclaration(t) => self.blank(t.span),
            AstKind::TSModuleDeclaration(t) => self.blank(t.span),

            // Expression suffixes/prefixes: keep the operand, blank the type part.
            AstKind::TSAsExpression(e) => self.blank_between(e.expression.span().end, e.span.end),
            AstKind::TSSatisfiesExpression(e) => {
                self.blank_between(e.expression.span().end, e.span.end)
            }
            AstKind::TSNonNullExpression(e) => {
                self.blank_between(e.expression.span().end, e.span.end)
            }
            AstKind::TSTypeAssertion(e) => {
                self.blank_between(e.span.start, e.expression.span().start)
            }

            // `this: T` typed receiver is not valid JS — erase it and its comma.
            AstKind::TSThisParameter(t) => self.blank_specifier(t.span),

            // Type-only imports/exports (statement-level and inline specifiers).
            AstKind::ImportDeclaration(d) => {
                if matches!(d.import_kind, ImportOrExportKind::Type) {
                    self.blank(d.span);
                }
            }
            AstKind::ImportSpecifier(s) => {
                if matches!(s.import_kind, ImportOrExportKind::Type) {
                    self.blank_specifier(s.span);
                }
            }
            AstKind::ExportNamedDeclaration(d) => {
                let erasable_decl = d.declaration.as_ref().is_some_and(is_erasable_declaration);
                if matches!(d.export_kind, ImportOrExportKind::Type) || erasable_decl {
                    self.blank(d.span);
                }
            }
            AstKind::ExportSpecifier(s) => {
                if matches!(s.export_kind, ImportOrExportKind::Type) {
                    self.blank_specifier(s.span);
                }
            }
            AstKind::ExportAllDeclaration(d) => {
                if matches!(d.export_kind, ImportOrExportKind::Type) {
                    self.blank(d.span);
                }
            }

            // Ambient `declare` var statements: whole span (includes `declare`).
            AstKind::VariableDeclaration(d) if d.declare => self.blank(d.span),
            // Ambient `declare function` + overload signatures (no body): whole span.
            AstKind::Function(f) if f.declare || f.body.is_none() => self.blank(f.span),

            AstKind::Class(c) => self.strip_class(c),
            AstKind::PropertyDefinition(p) => self.strip_property(p),
            AstKind::MethodDefinition(m) => self.strip_method(m),
            AstKind::AccessorProperty(p) => self.strip_accessor(p),
            // `[k: string]: T` index signatures are type-only class members — they
            // emit nothing and would be invalid JS if left, so erase the whole node.
            AstKind::TSIndexSignature(s) => self.blank(s.span),

            AstKind::FormalParameter(p) => self.strip_param(p),
            AstKind::VariableDeclarator(d) if d.definite => {
                let from = d.id.span().end;
                let to = d
                    .type_annotation
                    .as_ref()
                    .map(|t| t.span.start)
                    .or_else(|| d.init.as_ref().map(|e| e.span().start))
                    .unwrap_or(d.span.end);
                self.blank_char(from, to, b'!');
            }
            _ => {}
        }
    }
}

impl Blanker<'_> {
    fn strip_class(&mut self, c: &Class) {
        // A `declare class` is ambient — erase it whole (span includes `declare`).
        if c.declare {
            self.blank(c.span);
            return;
        }
        // `abstract` keyword (the class itself is kept).
        if c.r#abstract {
            let region_start = c
                .decorators
                .iter()
                .map(|d| d.span.end)
                .max()
                .unwrap_or(c.span.start)
                .max(c.span.start);
            let region_end =
                c.id.as_ref()
                    .map(|id| id.span.start)
                    .unwrap_or(c.body.span.start);
            self.blank_word(region_start, region_end, "abstract");
        }
        // `implements I, J` clause. Locate it by SPAN, never by substring: scan only
        // the gap *after* the heritage (id / type params / `extends <expr><args>`),
        // which contains nothing but whitespace, the `implements` keyword, and the
        // clauses — so a string literal inside an `extends B<"implements">` type
        // argument can never be hit.
        if let Some(last) = c.implements.last() {
            let heritage_end = [
                c.id.as_ref().map(|id| id.span.end),
                c.type_parameters.as_ref().map(|t| t.span.end),
                c.super_class.as_ref().map(|e| e.span().end),
                c.super_type_arguments.as_ref().map(|t| t.span.end),
            ]
            .into_iter()
            .flatten()
            .max()
            .unwrap_or(c.span.start);
            self.blank_between(heritage_end, last.span.end);
        }
    }

    fn strip_property(&mut self, p: &PropertyDefinition) {
        let abstract_ = matches!(
            p.r#type,
            PropertyDefinitionType::TSAbstractPropertyDefinition
        );
        // `declare`/`abstract` fields emit no runtime slot — erase the whole member
        // (decorators included), not just the modifier keyword.
        if p.declare || abstract_ {
            self.blank(p.span);
            return;
        }
        let key_start = p.key.span().start;
        self.blank_member_modifiers(
            p.span.start,
            &p.decorators,
            key_start,
            &[
                (p.declare, "declare"),
                (abstract_, "abstract"),
                (p.accessibility == Some(TSAccessibility::Public), "public"),
                (p.accessibility == Some(TSAccessibility::Private), "private"),
                (
                    p.accessibility == Some(TSAccessibility::Protected),
                    "protected",
                ),
                (p.readonly, "readonly"),
                (p.r#override, "override"),
            ],
        );
        let key_end = p.key.span().end;
        if p.optional {
            self.blank_char(key_end, p.span.end, b'?');
        }
        if p.definite {
            self.blank_char(key_end, p.span.end, b'!');
        }
    }

    fn strip_method(&mut self, m: &MethodDefinition) {
        let abstract_ = matches!(m.r#type, MethodDefinitionType::TSAbstractMethodDefinition);
        // An abstract method or a body-less signature (overload / ambient) emits
        // nothing at runtime — erase the whole member (decorators included).
        if abstract_ || m.value.body.is_none() {
            self.blank(m.span);
            return;
        }
        let key_start = m.key.span().start;
        self.blank_member_modifiers(
            m.span.start,
            &m.decorators,
            key_start,
            &[
                (abstract_, "abstract"),
                (m.accessibility == Some(TSAccessibility::Public), "public"),
                (m.accessibility == Some(TSAccessibility::Private), "private"),
                (
                    m.accessibility == Some(TSAccessibility::Protected),
                    "protected",
                ),
                (m.r#override, "override"),
            ],
        );
        if m.optional {
            self.blank_char(m.key.span().end, m.value.span.start, b'?');
        }
    }

    fn strip_accessor(&mut self, p: &oxc_ast::ast::AccessorProperty) {
        let abstract_ = matches!(
            p.r#type,
            oxc_ast::ast::AccessorPropertyType::TSAbstractAccessorProperty
        );
        // An `abstract accessor` emits no runtime slot — erase the whole member.
        if abstract_ {
            self.blank(p.span);
            return;
        }
        let key_start = p.key.span().start;
        self.blank_member_modifiers(
            p.span.start,
            &p.decorators,
            key_start,
            &[
                (abstract_, "abstract"),
                (p.accessibility == Some(TSAccessibility::Public), "public"),
                (p.accessibility == Some(TSAccessibility::Private), "private"),
                (
                    p.accessibility == Some(TSAccessibility::Protected),
                    "protected",
                ),
                (p.r#override, "override"),
            ],
        );
        if p.definite {
            self.blank_char(p.key.span().end, p.span.end, b'!');
        }
    }

    fn strip_param(&mut self, p: &FormalParameter) {
        // Parameter properties are rejected by the policy gate, so the only TS
        // syntax left on a parameter that reaches strip is the optional `?` marker
        // (the `: T` annotation is blanked as a `TSTypeAnnotation` child).
        if p.optional {
            let from = p.pattern.span().end;
            let to = p
                .type_annotation
                .as_ref()
                .map(|t| t.span.start)
                .or_else(|| p.initializer.as_ref().map(|e| e.span().start))
                .unwrap_or(p.span.end);
            self.blank_char(from, to, b'?');
        }
    }
}

/// A declaration that, when erased, leaves its `export` wrapper dangling — so the
/// whole `export <decl>` statement must be blanked, not just the inner declaration.
fn is_erasable_declaration(d: &Declaration) -> bool {
    match d {
        Declaration::TSTypeAliasDeclaration(_) | Declaration::TSInterfaceDeclaration(_) => true,
        // Ambient (non-ambient enum/namespace are rejected before strip runs).
        Declaration::TSEnumDeclaration(e) => e.declare,
        Declaration::TSModuleDeclaration(m) => m.declare,
        Declaration::ClassDeclaration(c) => c.declare,
        Declaration::FunctionDeclaration(f) => f.declare || f.body.is_none(),
        Declaration::VariableDeclaration(v) => v.declare,
        _ => false,
    }
}

fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0c)
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$' || b >= 0x80
}

/// Absolute byte offset of `word` as a standalone token within `src[start..end)`.
fn find_word(src: &[u8], start: u32, end: u32, word: &str) -> Option<u32> {
    let (s, e) = (start as usize, (end as usize).min(src.len()));
    if s >= e {
        return None;
    }
    let hay = &src[s..e];
    let needle = word.as_bytes();
    let mut i = 0;
    while i + needle.len() <= hay.len() {
        if &hay[i..i + needle.len()] == needle {
            let before_ok = i == 0 || !is_ident_byte(hay[i - 1]);
            let after = i + needle.len();
            let after_ok = after >= hay.len() || !is_ident_byte(hay[after]);
            if before_ok && after_ok {
                return Some((s + i) as u32);
            }
        }
        i += 1;
    }
    None
}
// === /RT-003 ===

#[cfg(test)]
mod tests {
    use std::rc::Rc;
    use std::sync::Arc;

    use oxc_span::SourceType;

    use super::*;
    use crate::cst::{compute_cst, Cst};

    fn ts(src: &str) -> Cst {
        compute_cst(Arc::from(src), SourceType::ts())
    }

    fn strip_ts(src: &str) -> String {
        strip(src, &ts(src))
    }

    fn check(src: &str) -> Result<(), Vec<StripDiagnostic>> {
        let sem = SemanticGraph::build(Rc::new(ts(src)));
        ErasablePolicy.check(&sem)
    }

    /// Re-parse stripped output as plain JS (TS grammar disabled): no error, no
    /// panic ⇒ every byte of TS-only syntax is gone.
    fn assert_valid_js(stripped: &str) {
        let cst = compute_cst(Arc::from(stripped), SourceType::cjs());
        assert!(
            cst.errors().is_empty() && !cst.panicked(),
            "stripped output is not clean JS: {stripped:?}\nerrors: {:?}",
            cst.errors()
        );
    }

    /// The defining strip-fidelity property: `stripped` is `source` with some bytes
    /// blanked to spaces — same length, same byte at every retained offset, and
    /// every newline preserved (so line/column geometry is identical, no sourcemap).
    fn assert_is_blanking_of(source: &str, stripped: &str) {
        assert_eq!(
            source.len(),
            stripped.len(),
            "length changed: {source:?} -> {stripped:?}"
        );
        for (i, (s, o)) in source.bytes().zip(stripped.bytes()).enumerate() {
            if s == b'\n' || s == b'\r' {
                assert_eq!(o, s, "newline at byte {i} not preserved");
            } else {
                assert!(
                    o == s || o == b' ',
                    "byte {i} mutated to non-space: {s:?} -> {o:?}"
                );
            }
        }
    }

    // ---- PROPERTY: meaning preservation + position stability over a corpus ----

    #[test]
    fn strip_fidelity_corpus() {
        // (source, type-tokens that MUST be gone, value-tokens that MUST remain)
        let corpus: &[(&str, &[&str], &[&str])] = &[
            ("const x: number = 1;", &["number"], &["const", "x", "1"]),
            (
                "function f<T>(a: T, b: string): T { return a; }",
                &["string"],
                &["function", "return", "a"],
            ),
            ("let v = obj as Foo;", &["as", "Foo"], &["let", "v", "obj"]),
            (
                "const u = data satisfies Schema;",
                &["satisfies", "Schema"],
                &["const", "data"],
            ),
            ("const n = maybe!.value;", &[], &["maybe", "value"]),
            ("let z = <number>raw;", &["number"], &["raw", "z"]),
            ("type Alias = number; const k = 2;", &["Alias"], &["k", "2"]),
            (
                "interface Shape { area(): number; } const s = 1;",
                &["interface", "Shape", "area"],
                &["const", "s"],
            ),
            (
                "import { foo } from 'm'; import type { Bar } from 'm';",
                &["Bar"],
                &["foo"],
            ),
            ("import { used, type Only } from 'm';", &["Only"], &["used"]),
            (
                "export { real, type Phantom } from 'm';",
                &["Phantom"],
                &["real"],
            ),
            (
                "class C { private readonly w: number = 1; m(p?: string): void {} }",
                &["private", "readonly", "number", "string", "void"],
                &["class", "w", "m"],
            ),
            (
                "abstract class A { abstract g(): void; }",
                &["abstract", "void"],
                &["class"],
            ),
            (
                "class K extends B implements I, J {}",
                &["implements"],
                &["class", "extends", "B"],
            ),
            (
                "function fn(this: T, a: number) { return a; }",
                &["this", "number"],
                &["function", "return", "a"],
            ),
            (
                "let d!: number; const e = call<Generic>();",
                &["number", "Generic"],
                &["let", "d", "call"],
            ),
            (
                "declare const amb: number; declare function af(x: number): void;",
                &["declare", "amb", "af"],
                &[],
            ),
            (
                "declare enum E { A } declare namespace N { const z = 1; }",
                &["enum", "namespace"],
                &[],
            ),
        ];

        for (src, removed, kept) in corpus {
            let out = strip_ts(src);
            assert_is_blanking_of(src, &out);
            assert_valid_js(&out);
            for r in *removed {
                assert!(
                    !word_present(&out, r),
                    "type token {r:?} survived strip of {src:?} -> {out:?}"
                );
            }
            for k in *kept {
                assert!(
                    word_present(&out, k),
                    "value token {k:?} lost in strip of {src:?} -> {out:?}"
                );
            }
        }
    }

    fn word_present(text: &str, word: &str) -> bool {
        super::find_word(text.as_bytes(), 0, text.len() as u32, word).is_some()
    }

    // ---- plain JS strips to itself (identity) ----

    #[test]
    fn plain_js_is_identity() {
        let js = "const x = 1;\nfunction f(a) { return a + 1; }\nclass C { m() { return 2; } }\n";
        let cst = compute_cst(Arc::from(js), SourceType::cjs());
        assert_eq!(strip(js, &cst), js);
    }

    // ---- multi-line annotations preserve line numbers ----

    #[test]
    fn multiline_annotation_preserves_lines() {
        let src = "const config: {\n  a: number;\n  b: string;\n} = load();\nconst after = 1;\n";
        let out = strip_ts(src);
        assert_is_blanking_of(src, &out);
        assert_valid_js(&out);
        assert_eq!(
            src.matches('\n').count(),
            out.matches('\n').count(),
            "line count drifted"
        );
        // `after` keeps its exact byte offset (line + column stable, no sourcemap).
        assert_eq!(src.find("after"), out.find("after"));
    }

    // ---- non-erasable catalog: each construct -> its exact diagnostic ----

    fn assert_one(src: &str, message: &str, help: &str) {
        let diags = check(src).expect_err("expected a rejection");
        assert_eq!(
            diags.len(),
            1,
            "expected exactly one diagnostic for {src:?}"
        );
        let d = &diags[0];
        assert_eq!(d.message, message);
        assert_eq!(d.help, help);
        assert!(d.span.end > d.span.start, "span must cover the construct");
    }

    #[test]
    fn rejects_enum() {
        assert_one(
            "enum Color { Red, Green }",
            "Enums emit runtime code and cannot be type-stripped.",
            "Use a `const` object instead.",
        );
    }

    #[test]
    fn rejects_const_enum() {
        assert_one(
            "const enum Dir { Up, Down }",
            "Enums emit runtime code and cannot be type-stripped.",
            "Use a `const` object instead.",
        );
    }

    #[test]
    fn rejects_runtime_namespace() {
        assert_one(
            "namespace N { export const x = 1; }",
            "Namespaces with a runtime body emit code and cannot be type-stripped.",
            "Use ES modules (separate files and named exports) instead.",
        );
    }

    #[test]
    fn rejects_parameter_property() {
        assert_one(
            "class C { constructor(public readonly x: number) {} }",
            "Parameter properties emit an assignment and cannot be type-stripped.",
            "Declare the field on the class and assign it in the constructor body instead.",
        );
    }

    #[test]
    fn rejects_import_equals() {
        assert_one(
            "import x = require('y');",
            "`import =` emits runtime code and cannot be type-stripped.",
            "Use a standard ESM `import` instead.",
        );
    }

    #[test]
    fn rejects_export_equals() {
        assert_one(
            "export = thing;",
            "`export =` emits CommonJS and cannot be type-stripped.",
            "Use `export default` or named exports instead.",
        );
    }

    // ---- ambient forms are allowed (erasable, no diagnostic) ----

    #[test]
    fn allows_ambient_forms() {
        assert!(check("declare enum E { A }").is_ok());
        assert!(check("declare namespace N { const x = 1; }").is_ok());
        assert!(check("declare module 'm' { export const x: number; }").is_ok());
    }

    // ---- value imports retained; type-only specifier removed ----

    #[test]
    fn retains_value_imports() {
        let src = "import { foo } from 'x';\nimport type { T } from 'y';\nfoo();\n";
        let out = strip_ts(src);
        assert_is_blanking_of(src, &out);
        assert_valid_js(&out);
        assert!(word_present(&out, "foo"));
        assert!(!word_present(&out, "T"));
        // the whole `import type … 'y'` line is blanked, but its newline survives.
        assert_eq!(out.matches('\n').count(), src.matches('\n').count());
    }

    // ---- multi-error collection: all diagnostics in one pass ----

    #[test]
    fn collects_all_diagnostics() {
        let src = "enum A {} enum B {} class C { constructor(private x: number) {} }";
        let diags = check(src).expect_err("expected rejections");
        assert_eq!(diags.len(), 3, "got: {diags:?}");
        let enums = diags
            .iter()
            .filter(|d| d.message.starts_with("Enums"))
            .count();
        let props = diags
            .iter()
            .filter(|d| d.message.starts_with("Parameter properties"))
            .count();
        assert_eq!((enums, props), (2, 1));
    }

    // ---- RT-003 craft-gate regressions ----

    /// Finding 1: a `declare` field emits no runtime slot — the whole member is gone.
    #[test]
    fn erases_declare_field_entirely() {
        let src = "class C { declare x: number; real = 1; }";
        let out = strip_ts(src);
        assert_is_blanking_of(src, &out);
        assert_valid_js(&out);
        // No runtime `x` field survives (the key, not just the `: number`).
        assert!(!word_present(&out, "declare"));
        assert!(!word_present(&out, "x"), "declare field leaked: {out:?}");
        assert!(word_present(&out, "real"), "runtime field lost: {out:?}");
    }

    /// Finding 1: an `abstract` method is gone; the concrete sibling stays.
    #[test]
    fn erases_abstract_method_entirely() {
        let src = "abstract class A { abstract g(): void; method() {} }";
        let out = strip_ts(src);
        assert_is_blanking_of(src, &out);
        assert_valid_js(&out);
        assert!(!word_present(&out, "abstract"));
        assert!(!word_present(&out, "g"), "abstract method leaked: {out:?}");
        assert!(word_present(&out, "method"), "runtime method lost: {out:?}");
    }

    /// Finding 1: an index signature is type-only syntax — it must vanish whole, or
    /// the leftover `[k: string]:` would be invalid JS.
    #[test]
    fn erases_index_signature_entirely() {
        let src = "class C { [k: string]: number; real = 1; }";
        let out = strip_ts(src);
        assert_is_blanking_of(src, &out);
        assert_valid_js(&out);
        assert!(!word_present(&out, "k"), "index sig key leaked: {out:?}");
        assert!(word_present(&out, "real"), "runtime field lost: {out:?}");
    }

    /// Finding 2: `implements` is located by span, not substring. The `"implements"`
    /// string literal is a *runtime* argument to the `extends` super-class expression,
    /// so it must survive while the real `implements I` clause is erased. The old
    /// substring scan blanked from inside the string literal → invalid JS.
    #[test]
    fn implements_located_by_span_not_substring() {
        let src = "class C extends pick(\"implements\") implements I {}";
        let out = strip_ts(src);
        assert_is_blanking_of(src, &out);
        assert_valid_js(&out);
        // The runtime super-class expression survives, string literal intact.
        assert!(word_present(&out, "extends"), "extends lost: {out:?}");
        assert!(word_present(&out, "pick"), "super-class call lost: {out:?}");
        assert!(
            out.contains("\"implements\""),
            "runtime string literal hit: {out:?}"
        );
        // The trailing `implements I` interface clause is gone (no bare `I` token).
        assert!(
            !word_present(&out, "I"),
            "implements clause leaked: {out:?}"
        );
    }
}
