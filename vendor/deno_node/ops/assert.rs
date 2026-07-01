// Copyright 2018-2026 the Deno authors. MIT license.

use deno_core::op2;
use deno_core::v8;
use deno_error::JsErrorBox;
use oxc_allocator::Allocator;
use oxc_parser::config::RuntimeParserConfig;
use oxc_parser::{Parser, Token};
use oxc_span::SourceType;

use oxc_parser::Kind;

#[op2]
pub fn op_node_get_first_expression<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    arg: v8::Local<v8::Value>,
) -> Result<v8::Local<'s, v8::Value>, JsErrorBox> {
    if !arg.is_object() {
        return Err(JsErrorBox::type_error("Argument must be an object"));
    }

    let msg = v8::Exception::create_message(scope, arg);

    let source_line: String;
    if let Some(inner_source_line) = msg.get_source_line(scope) {
        source_line = inner_source_line.to_rust_string_lossy(scope);
    } else {
        return Ok(v8::undefined(scope).into());
    }

    let start_column = msg.get_start_column();
    let result = get_first_expression(&source_line, start_column);

    Ok(v8::String::new(scope, result).unwrap().into())
}

/// Tokens that represent member access operators: `.`, `[`, `]`.
/// Optional chaining `?.` is handled by detecting `?` + `.` token sequence.
fn is_member_access_token(kind: Kind) -> bool {
    matches!(
        kind,
        Kind::Dot | Kind::QuestionDot | Kind::LBrack | Kind::RBrack
    )
}

fn is_member_name_token(kind: Kind) -> bool {
    kind.is_identifier_name() || kind.is_literal()
}

fn is_ident_word(kind: Kind) -> bool {
    kind.is_identifier_name()
}

fn token_text<'a>(code: &'a str, range: &std::ops::Range<usize>) -> Option<&'a str> {
    if range.start <= range.end
        && range.end <= code.len()
        && code.is_char_boundary(range.start)
        && code.is_char_boundary(range.end)
    {
        Some(&code[range.start..range.end])
    } else {
        None
    }
}

fn is_question_token(code: &str, range: &std::ops::Range<usize>) -> bool {
    token_text(code, range) == Some("?")
}

fn adjust_start_column_for_non_ascii(code: &str, mut start_column: usize) -> usize {
    // Match the JS behavior that used `charCodeAt` on UTF-16 code units.
    let utf16_code_units: Vec<u16> = code.encode_utf16().collect();
    let mut index = 0;
    while index < start_column {
        if utf16_code_units.get(index).copied().unwrap_or_default() > 127 {
            start_column += 1;
        }
        index += 1;
    }
    start_column
}

/// Get the first expression in a code string at the start_column.
///
/// This mirrors Node.js's implementation
/// https://github.com/nodejs/node/blob/70f6b58ac655234435a99d72b857dd7b316d34bf/lib/internal/errors/error_source.js#L61-L142
fn get_first_expression(code: &str, original_start_col_index: usize) -> &str {
    let start_index = adjust_start_column_for_non_ascii(code, original_start_col_index);

    let allocator = Allocator::default();
    let parsed = Parser::new(&allocator, code, SourceType::mjs())
        .with_config(RuntimeParserConfig::new(true))
        .parse();
    let tokens: Vec<(Token, std::ops::Range<usize>)> = parsed
        .tokens
        .into_iter()
        .filter(|token| !token.kind().is_eof())
        .map(|token| (token, token.start() as usize..token.end() as usize))
        .collect();

    let mut last_token = None;
    let mut second_last_token = None;
    let mut first_member_access_name_token = None; // start position
    let mut terminating_col = None;
    let mut paren_lvl = 0;

    for (token, range) in &tokens {
        // Peek before the startColumn.
        if range.start < start_index {
            // There is a semicolon. This is a statement before the startColumn,
            // so reset the memo.
            if matches!(token.kind(), Kind::Semicolon) {
                first_member_access_name_token = None;
                second_last_token = last_token;
                last_token = Some((token, range));
                continue;
            }

            // Try to memo the member access expressions before the startColumn,
            // so that the returned source code contains more info:
            //   assert.ok(value)
            //          ^ startColumn
            // The member expression can also be like
            //   assert['ok'](value) or assert?.ok(value)
            //               ^ startColumn      ^ startColumn
            let prev_is_question = last_token
                .map(|(_, last_range)| is_question_token(code, last_range))
                .unwrap_or(false);

            let is_optional_chain_dot =
                matches!(token.kind(), Kind::Dot | Kind::QuestionDot) && prev_is_question;

            let is_member_access = is_member_access_token(token.kind()) || is_optional_chain_dot;

            let member_access_base_token = if is_optional_chain_dot {
                second_last_token
            } else {
                last_token
            };

            if is_member_access
                && first_member_access_name_token.is_none()
                && let Some((last_tok, last_range)) = member_access_base_token
                && is_ident_word(last_tok.kind())
            {
                first_member_access_name_token = Some(last_range.start);
            } else if !is_member_access
                && !is_member_name_token(token.kind())
                && !is_question_token(code, range)
            {
                // Reset the memo if it is not a simple member access.
                // For example: assert[(() => 'ok')()](value)
                //                                    ^ startColumn
                first_member_access_name_token = None;
            }

            second_last_token = last_token;
            last_token = Some((token, range));
            continue;
        }

        // Now after the startColumn, this must be an expression.
        if matches!(token.kind(), Kind::LParen) {
            paren_lvl += 1;
            continue;
        }

        if matches!(token.kind(), Kind::RParen) {
            paren_lvl -= 1;
            if paren_lvl == 0 {
                // A matched closing parenthesis found after the startColumn,
                // terminate here. Include the token.
                //   (assert.ok(false), assert.ok(true))
                //           ^ startColumn
                terminating_col = Some(range.start + 1);
                break;
            }
            continue;
        }

        if matches!(token.kind(), Kind::Semicolon) {
            // A semicolon found after the startColumn, terminate here.
            //   assert.ok(false); assert.ok(true));
            //          ^ startColumn
            terminating_col = Some(range.start);
            break;
        }
        // If no semicolon found after the startColumn. The string after the
        // startColumn must be the expression.
        //   assert.ok(false)
        //          ^ startColumn
    }

    let start = first_member_access_name_token.unwrap_or(start_index);
    let end = terminating_col.unwrap_or(code.len());
    if start <= end && end <= code.len() {
        &code[start..end]
    } else {
        code
    }
}
