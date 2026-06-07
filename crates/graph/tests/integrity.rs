//! GRAPH-001 integrity tests — the I-1 / I-3 foundations and the `graph-integrity`
//! / `strip-fidelity` gate setup. Exercises ONLY the public `GraphDb` facade.

use std::path::Path;
use std::sync::Arc;

use meow_graph::{GraphDb, RuntimeIr, SemanticGraph, StripDiagnostic, StripPolicy};
use oxc_span::Span;

fn src(s: &str) -> Arc<str> {
    Arc::from(s)
}

/// Smoke: a TS file parses, gets a semantic graph, and lowers — all without panic;
/// the expected top-level bindings appear in the symbol table.
#[test]
fn parses_typescript_file() {
    let mut db = GraphDb::new();
    let id = db.set_file(
        "a.ts",
        src("const x: number = 1;\nfunction foo(a: string): string { return a; }\n"),
    );

    let cst = db.cst(id).expect("known id");
    assert!(!cst.panicked(), "parser should recover, not abort");
    assert!(
        cst.errors().is_empty(),
        "clean TS should have no parse errors"
    );
    assert!(cst.source_type().is_typescript());

    let sem = db.semantic(id).expect("known id");
    let names: Vec<&str> = sem.scoping().symbol_names().collect();
    assert!(names.contains(&"x"), "missing binding `x`: {names:?}");
    assert!(names.contains(&"foo"), "missing binding `foo`: {names:?}");

    // Stage 5: TS cannot lower yet — RT-003 owns the strip — so the seam is honest
    // and rejects rather than handing back un-stripped TS as runnable IR.
    let diags = db
        .runtime_ir(id)
        .expect("known id")
        .expect_err("TS must not lower until RT-003");
    assert!(
        diags.iter().any(|d| d.message.contains("RT-003")),
        "expected RT-003 rejection: {diags:?}"
    );
}

/// Lossless: for a corpus that stresses trivia, `write_source` reproduces the input
/// byte-for-byte. *Invariant: no source information is discarded.*
#[test]
fn cst_round_trips_losslessly() {
    let corpus: &[(&str, &str)] = &[
        (
            "comments.ts",
            "// leading\nconst a = 1; /* inline */ const b = 2;\n",
        ),
        (
            "template.js",
            "const t = `hello ${name}\n  multi ${1 + 2} line`;\n",
        ),
        ("regex.js", "const re = /ab+c/gi;\nconst y = x / 2 / 3;\n"),
        ("trailing.ts", "const z = 3;   \n\t\n   \n"),
        ("unicode.ts", "const greeting = \"héllo 世界 🌍\";\n"),
        ("empty.js", ""),
        (
            "mixed.tsx",
            "/** doc */\nexport const C = () => <div className=\"x\">{/* c */}</div>;\n",
        ),
    ];

    let mut db = GraphDb::new();
    for (path, text) in corpus {
        let id = db.set_file(*path, src(text));
        let mut out = String::new();
        db.cst(id).expect("known id").write_source(&mut out);
        assert_eq!(&out, text, "round-trip mismatch for {path}");
        assert_eq!(out.as_bytes(), text.as_bytes(), "byte mismatch for {path}");
    }
}

/// Incrementality: editing one file recomputes only its stages; the other file is
/// served from memo. *Invariant: editing one file recomputes only dependent
/// queries (CANON §7.2).* The recompute probe stands in for salsa's event log.
#[test]
fn editing_one_file_recomputes_only_its_dependents() {
    let mut db = GraphDb::new();
    let a = db.set_file("a.ts", src("export const a = 1;\n"));
    let b = db.set_file("b.ts", src("export const b = 2;\n"));

    // Prime both files' stage-2 (and transitively stage-1) memos.
    db.semantic(a);
    db.semantic(b);
    let primed = db.recomputes();
    assert_eq!(primed.cst, 2, "two files parsed once each");
    assert_eq!(primed.semantic, 2);

    // Re-reading without edits must be pure memo hits.
    db.semantic(a);
    db.semantic(b);
    assert_eq!(db.recomputes(), primed, "re-reads must not recompute");

    // Edit ONLY a.
    db.set_file("a.ts", src("export const a = 100;\n"));
    db.semantic(a); // a recomputes (cst + semantic)
    db.semantic(b); // b is untouched -> memo hit

    let after = db.recomputes();
    assert_eq!(after.cst - primed.cst, 1, "only A reparsed");
    assert_eq!(after.semantic - primed.semantic, 1, "only A re-analyzed");
}

/// Single-parser surface: the only route to a parsed artifact is `GraphDb`. The
/// public API exposes no parser/allocator constructor — this test obtains a fully
/// parsed + analyzed artifact touching nothing but the facade. P15's grep
/// (`Parser::new` confined to `/(graph|parse)/`) is the cheap floor; this is the
/// structural proof. *Invariant: one parse entrypoint (I-1).*
#[test]
fn single_parser_surface() {
    let mut db = GraphDb::new();
    let id = db.set_file("only.ts", src("const only = 1;\n"));

    // A real parsed AST is reachable without ever naming Parser/Allocator.
    let program = db.cst(id).expect("known id").program();
    assert_eq!(program.body.len(), 1);
    assert!(db
        .semantic(id)
        .expect("known id")
        .scoping()
        .symbol_names()
        .any(|n| n == "only"));

    // Path-keyed convenience routes through the same single entrypoint.
    assert!(db.cst_of(Path::new("only.ts")).is_some());
    assert!(db.cst_of(Path::new("missing.ts")).is_none());
}

/// A policy double that rejects a marked construct, to prove the stage-5 strip seam
/// is wired (`strip-fidelity` setup). The erasable-only rules themselves are RT-003.
struct RejectAll;

impl StripPolicy for RejectAll {
    fn check(&self, _sem: &SemanticGraph) -> Result<(), Vec<StripDiagnostic>> {
        Err(vec![StripDiagnostic {
            span: Span::new(0, 4),
            message: "Enums emit runtime code and cannot be type-stripped.".to_string(),
            help: "Use a `const` object instead.".to_string(),
        }])
    }
}

#[test]
fn strip_seam_is_wired() {
    // Clean JS under the permissive default -> Some(Ok) with position-preserving IR.
    let mut ok_db = GraphDb::new();
    let id = ok_db.set_file("ok.js", src("const x = 1;\n"));
    let ir: &RuntimeIr = ok_db
        .runtime_ir(id)
        .expect("known id")
        .expect("clean JS lowers under permissive policy");
    assert!(ir.positions_preserved);
    assert_eq!(
        &*ir.code, "const x = 1;\n",
        "identity seam keeps source verbatim"
    );

    // TS input under the default policy -> Some(Err): the strip is RT-003's, so the
    // seam refuses to pass un-stripped TS off as runnable IR.
    let mut ts_db = GraphDb::new();
    let id = ts_db.set_file("typed.ts", src("const x: number = 1;\n"));
    let diags = ts_db
        .runtime_ir(id)
        .expect("known id")
        .expect_err("TS must not lower until RT-003");
    assert_eq!(diags.len(), 1);
    assert!(
        diags[0].message.contains("RT-003"),
        "expected RT-003 rejection: {diags:?}"
    );

    // A file with a recovered parse error -> Some(Err): broken input is never
    // lowered into fabricated IR.
    let mut err_db = GraphDb::new();
    let id = err_db.set_file("broken.js", src("const = ;\n"));
    let diags = err_db
        .runtime_ir(id)
        .expect("known id")
        .expect_err("broken input must not lower");
    assert!(!diags.is_empty());
    assert!(
        diags[0].message.contains("parse"),
        "expected a parse/semantic error summary: {diags:?}"
    );

    // Rejecting policy on clean JS -> Some(Err) carrying message + help + span.
    let mut rej_db = GraphDb::with_policy(Arc::new(RejectAll));
    let id = rej_db.set_file("bad.js", src("const e = 1;\n"));
    let diags = rej_db
        .runtime_ir(id)
        .expect("known id")
        .expect_err("rejecting policy blocks IR");
    assert_eq!(diags.len(), 1);
    assert!(diags[0].message.contains("type-stripped"));
    assert!(!diags[0].help.is_empty());
    assert_eq!(diags[0].span, Span::new(0, 4));

    // A stale id (file removed) -> None, not a panic.
    rej_db.remove_file(Path::new("bad.js"));
    assert!(rej_db.runtime_ir(id).is_none(), "retired id yields None");
}
