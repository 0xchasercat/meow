// Copyright 2018-2026 the Deno authors. MIT license.

use deno_core::op2;
use deno_core::url::Url;
use deno_error::JsErrorBox;
use oxc_allocator::Allocator;
use oxc_codegen::Codegen;
use oxc_parser::{ParseOptions, Parser};
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{TransformOptions, Transformer};

#[op2]
#[string]
pub fn op_node_strip_typescript_types(
    #[string] code: String,
    #[string] mode: &str,
    source_map: bool,
) -> Result<String, JsErrorBox> {
    let specifier = Url::parse("file:///stripTypeScriptTypes.ts").unwrap();
    if mode == "strip" && source_map {
        return Err(JsErrorBox::generic(
            "source maps are not supported in strip mode",
        ));
    }
    transform_typescript_with_oxc(&specifier, &code, mode)
}

fn transform_typescript_with_oxc(
    specifier: &Url,
    source_text: &str,
    mode: &str,
) -> Result<String, JsErrorBox> {
    let allocator = Allocator::default();
    let source_path = specifier
        .to_file_path()
        .unwrap_or_else(|_| std::path::PathBuf::from("stripTypeScriptTypes.ts"));
    let source_type = SourceType::from_path(&source_path).unwrap_or_else(|_| SourceType::ts());
    let parsed = Parser::new(&allocator, source_text, source_type)
        .with_options(ParseOptions {
            allow_return_outside_function: true,
            ..ParseOptions::default()
        })
        .parse();

    if parsed.panicked || !parsed.diagnostics.is_empty() {
        let message = parsed
            .diagnostics
            .into_iter()
            .map(|error| format!("{:?}", error.with_source_code(source_text.to_owned())))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(JsErrorBox::generic(format!(
            "Oxc parse failed for {specifier}: {message}"
        )));
    }

    let mut program = parsed.program;
    let semantic = SemanticBuilder::new()
        .with_excess_capacity(2.0)
        .with_enum_eval(true)
        .build(&program);
    if !semantic.diagnostics.is_empty() {
        let message = semantic
            .diagnostics
            .into_iter()
            .map(|error| format!("{:?}", error.with_source_code(source_text.to_owned())))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(JsErrorBox::generic(format!(
            "Oxc semantic analysis failed for {specifier}: {message}"
        )));
    }

    let transform_options = TransformOptions {
        typescript: oxc_transformer::TypeScriptOptions {
            only_remove_type_imports: true,
            ..oxc_transformer::TypeScriptOptions::default()
        },
        ..TransformOptions::default()
    };
    let transformed = Transformer::new(&allocator, &source_path, &transform_options)
        .build_with_scoping(semantic.semantic.into_scoping(), &mut program);
    if !transformed.diagnostics.is_empty() {
        let message = transformed
            .diagnostics
            .into_iter()
            .map(|error| format!("{:?}", error.with_source_code(source_text.to_owned())))
            .collect::<Vec<_>>()
            .join("\n");
        return Err(JsErrorBox::generic(format!(
            "Oxc transform failed for {specifier}: {message}"
        )));
    }

    if mode != "strip" && mode != "transform" {
        return Err(JsErrorBox::generic(format!(
            "unsupported TypeScript transform mode: {mode}"
        )));
    }

    Ok(Codegen::new().build(&program).code)
}
