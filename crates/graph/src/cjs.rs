use std::collections::BTreeSet;

use oxc_ast::ast::{
    Argument, AssignmentOperator, CallExpression, Expression, ObjectPropertyKind,
    SimpleAssignmentTarget,
};
use oxc_ast::ast_kind::AstKind;
use oxc_ast_visit::Visit;

use crate::Cst;

/// Ordered, frozen-pattern CommonJS analysis over the owned Oxc CST.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CjsAnalysis {
    /// Literal `require("...")` specifiers, deduped in source order.
    pub static_requires: Vec<String>,
    /// Best-effort named exports (`exports.foo =`, `module.exports.foo =`,
    /// `Object.defineProperty(exports, "foo", ...)`), deduped in source order.
    pub named_exports: Vec<String>,
    /// Coarse signal that the file is authored as CommonJS (or TS-CJS) rather than
    /// merely mentioning the names in dead text.
    pub has_commonjs_syntax: bool,
    /// Any real ESM import/export declaration present in the file.
    pub has_esm_syntax: bool,
}

/// Walk the existing CST/AST and extract the frozen CommonJS pattern set LOAD-004
/// lowers. No second parser, no text regexes.
pub fn analyze_cjs(cst: &Cst) -> CjsAnalysis {
    let mut walker = Analyzer::default();
    walker.visit_program(cst.program());
    walker.finish()
}

#[derive(Default)]
struct Analyzer {
    static_requires: Vec<String>,
    static_requires_seen: BTreeSet<String>,
    named_exports: Vec<String>,
    named_exports_seen: BTreeSet<String>,
    has_commonjs_syntax: bool,
    has_esm_syntax: bool,
}

impl Analyzer {
    fn finish(self) -> CjsAnalysis {
        CjsAnalysis {
            static_requires: self.static_requires,
            named_exports: self.named_exports,
            has_commonjs_syntax: self.has_commonjs_syntax,
            has_esm_syntax: self.has_esm_syntax,
        }
    }

    fn mark_commonjs(&mut self) {
        self.has_commonjs_syntax = true;
    }

    fn push_require(&mut self, specifier: &str) {
        if self.static_requires_seen.insert(specifier.to_owned()) {
            self.static_requires.push(specifier.to_owned());
        }
    }

    fn is_safe_named_export(name: &str) -> bool {
        if name == "default" {
            return false;
        }
        let mut chars = name.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first == '_' || first == '$' || first.is_ascii_alphabetic()) {
            return false;
        }
        chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
    }

    fn push_named_export(&mut self, name: &str) {
        if !Self::is_safe_named_export(name) {
            return;
        }
        if self.named_exports_seen.insert(name.to_owned()) {
            self.named_exports.push(name.to_owned());
        }
    }

    fn inspect_call(&mut self, call: &CallExpression<'_>) {
        if let Some(specifier) = call.common_js_require() {
            self.mark_commonjs();
            self.push_require(specifier.value.as_str());
            return;
        }
        if call.callee.without_parentheses().is_specific_id("require") {
            self.mark_commonjs();
            return;
        }
        if !call
            .callee
            .without_parentheses()
            .is_specific_member_access("Object", "defineProperty")
        {
            return;
        }
        let Some(target) = call.arguments.first().and_then(argument_expression) else {
            return;
        };
        if !is_exports_target(target) && !is_module_exports_target(target) {
            return;
        }
        self.mark_commonjs();
        if let Some(name) = call.arguments.get(1).and_then(argument_string) {
            self.push_named_export(name);
        }
    }

    fn push_object_literal_keys(&mut self, expr: &Expression<'_>) {
        if let Expression::ObjectExpression(obj) = expr.without_parentheses() {
            for prop in &obj.properties {
                if let ObjectPropertyKind::ObjectProperty(p) = prop {
                    if let Some(name) = p.key.static_name() {
                        self.push_named_export(&name);
                    }
                }
            }
        }
    }

    fn inspect_assignment(
        &mut self,
        operator: AssignmentOperator,
        left: &SimpleAssignmentTarget<'_>,
        right: &Expression<'_>,
    ) {
        if operator != AssignmentOperator::Assign {
            return;
        }
        match left {
            SimpleAssignmentTarget::AssignmentTargetIdentifier(ident)
                if ident.name == "exports" =>
            {
                self.mark_commonjs();
            }
            oxc_ast::match_member_expression!(SimpleAssignmentTarget) => {
                let member = left.to_member_expression();
                if member.is_specific_member_access("module", "exports") {
                    self.mark_commonjs();
                    // `module.exports = { a, b }` -- including the swc/babel
                    // `0 && (module.exports = {...})` reexport hint -- means the
                    // object literal keys are the module's named exports.
                    self.push_object_literal_keys(right);
                    return;
                }
                let Some(name) = member.static_property_name() else {
                    return;
                };
                let object = member.object().without_parentheses();
                if object.is_specific_id("exports")
                    || object.is_specific_member_access("module", "exports")
                {
                    self.mark_commonjs();
                    self.push_named_export(name);
                }
            }
            _ => {}
        }
    }
}

impl<'a> Visit<'a> for Analyzer {
    fn enter_node(&mut self, kind: AstKind<'a>) {
        match kind {
            AstKind::ImportDeclaration(_) => self.has_esm_syntax = true,
            AstKind::ExportNamedDeclaration(_) => self.has_esm_syntax = true,
            AstKind::ExportDefaultDeclaration(_) => self.has_esm_syntax = true,
            AstKind::ExportAllDeclaration(_) => self.has_esm_syntax = true,
            AstKind::TSImportEqualsDeclaration(_) => self.mark_commonjs(),
            AstKind::TSExportAssignment(_) => self.mark_commonjs(),
            AstKind::CallExpression(call) => self.inspect_call(call),
            AstKind::AssignmentExpression(expr) => {
                if let Some(left) = expr.left.as_simple_assignment_target() {
                    self.inspect_assignment(expr.operator, left, &expr.right);
                }
            }
            _ => {}
        }
    }
}

fn argument_expression<'a>(arg: &'a Argument<'a>) -> Option<&'a Expression<'a>> {
    if arg.is_spread() {
        None
    } else {
        Some(arg.to_expression())
    }
}

fn argument_string<'a>(arg: &'a Argument<'a>) -> Option<&'a str> {
    match arg {
        Argument::StringLiteral(lit) => Some(lit.value.as_str()),
        _ => None,
    }
}
fn is_exports_target(expr: &Expression<'_>) -> bool {
    expr.without_parentheses().is_specific_id("exports")
}

fn is_module_exports_target(expr: &Expression<'_>) -> bool {
    expr.without_parentheses()
        .is_specific_member_access("module", "exports")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use oxc_span::SourceType;

    use crate::cst::compute_cst;

    use super::{analyze_cjs, CjsAnalysis};

    fn analyze(source: &str, source_type: SourceType) -> CjsAnalysis {
        let cst = compute_cst(Arc::from(source), source_type);
        analyze_cjs(&cst)
    }

    #[test]
    fn collects_literal_requires_and_named_exports_in_source_order() {
        let analysis = analyze(
            "const dep = require('dep');\nexports.foo = 1;\nmodule.exports.bar = 2;\nObject.defineProperty(exports, 'baz', { value: 3 });\nconst again = require('dep');\n",
            SourceType::cjs(),
        );
        assert_eq!(analysis.static_requires, vec!["dep"]);
        assert_eq!(analysis.named_exports, vec!["foo", "bar", "baz"]);
        assert!(analysis.has_commonjs_syntax);
        assert!(!analysis.has_esm_syntax);
    }

    #[test]
    fn skips_default_and_non_identifier_named_exports() {
        let analysis = analyze(
            "module.exports.default = 1;\nexports['a-b'] = 2;\nexports.good_name = 3;\n",
            SourceType::cjs(),
        );
        assert_eq!(analysis.named_exports, vec!["good_name"]);
    }

    #[test]
    fn flags_real_esm_syntax_separately() {
        let analysis = analyze(
            "import x from 'dep';\nconst y = require('other');\nexport default x;\n",
            SourceType::default().with_module(true),
        );
        assert_eq!(analysis.static_requires, vec!["other"]);
        assert!(analysis.has_commonjs_syntax);
        assert!(analysis.has_esm_syntax);
    }

    #[test]
    fn detects_module_exports_object_literal_reexport_hint() {
        // swc/Next emit `0 && (module.exports = {...})` purely as a static hint
        // for CJS named-export lexers; the object keys are the named exports.
        // The Oxc Visit walker descends into the dead `0 &&` branch (no DCE), so
        // this verifies the AssignmentExpression node is reached + keys read.
        let analysis = analyze(
            "0 && (module.exports = { nextBuild: null, saveCpuProfile: null });\n",
            SourceType::cjs(),
        );
        assert!(analysis.named_exports.contains(&"nextBuild".to_string()));
        assert!(analysis
            .named_exports
            .contains(&"saveCpuProfile".to_string()));
        assert!(analysis.has_commonjs_syntax);
    }
}
