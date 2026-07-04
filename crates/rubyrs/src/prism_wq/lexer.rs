//! Port of `Prism::Translation::Parser::Lexer` (prism 1.9.0's
//! translation/parser/lexer.rb) — converts the prism token stream into
//! parser-gem tokens `[type, [value, range]]`.

use crate::prism_node_specs::TOKEN_TYPES;
use crate::value::Value;

use super::{decline, CRes, Ctx, Decline, PParse, PTok, R};

/// One translated parser-gem token.
pub(crate) struct OutTok {
    pub(crate) ty: &'static str,
    pub(crate) val: TokVal,
    pub(crate) r: R,
}

pub(crate) enum TokVal {
    Bytes(Vec<u8>),
    /// A frozen String value (lexer.rb's frozen literal values — the file is
    /// frozen_string_literal: true, so `""`/`"+"`/`"~"`/`"!"`/`'<<"'` surface
    /// frozen).
    BytesF(Vec<u8>),
    Nil,
    V(Value),
}

const EXPR_BEG: u32 = 0x1;
const EXPR_LABEL: u32 = 0x400;

/// Direct type translations (lexer.rb's TYPES). Unmapped-but-known-nil types
/// return Some(""), unknown types None.
fn map_type(prism: &str) -> Option<&'static str> {
    Some(match prism {
        "MISSING" | "NOT_PROVIDED" | "EMBDOC_END" | "EMBDOC_LINE" => "",
        "AMPERSAND" => "tAMPER2",
        "AMPERSAND_AMPERSAND" => "tANDOP",
        "AMPERSAND_AMPERSAND_EQUAL" => "tOP_ASGN",
        "AMPERSAND_DOT" => "tANDDOT",
        "AMPERSAND_EQUAL" => "tOP_ASGN",
        "BACK_REFERENCE" => "tBACK_REF",
        "BACKTICK" => "tXSTRING_BEG",
        "BANG" => "tBANG",
        "BANG_EQUAL" => "tNEQ",
        "BANG_TILDE" => "tNMATCH",
        "BRACE_LEFT" => "tLCURLY",
        "BRACE_RIGHT" => "tRCURLY",
        "BRACKET_LEFT" => "tLBRACK2",
        "BRACKET_LEFT_ARRAY" => "tLBRACK",
        "BRACKET_LEFT_RIGHT" => "tAREF",
        "BRACKET_LEFT_RIGHT_EQUAL" => "tASET",
        "BRACKET_RIGHT" => "tRBRACK",
        "CARET" => "tCARET",
        "CARET_EQUAL" => "tOP_ASGN",
        "CHARACTER_LITERAL" => "tCHARACTER",
        "CLASS_VARIABLE" => "tCVAR",
        "COLON" => "tCOLON",
        "COLON_COLON" => "tCOLON2",
        "COMMA" => "tCOMMA",
        "COMMENT" => "tCOMMENT",
        "CONSTANT" => "tCONSTANT",
        "DOT" => "tDOT",
        "DOT_DOT" => "tDOT2",
        "DOT_DOT_DOT" => "tDOT3",
        "EMBDOC_BEGIN" => "tCOMMENT",
        "EMBEXPR_BEGIN" => "tSTRING_DBEG",
        "EMBEXPR_END" => "tSTRING_DEND",
        "EMBVAR" => "tSTRING_DVAR",
        "EQUAL" => "tEQL",
        "EQUAL_EQUAL" => "tEQ",
        "EQUAL_EQUAL_EQUAL" => "tEQQ",
        "EQUAL_GREATER" => "tASSOC",
        "EQUAL_TILDE" => "tMATCH",
        "FLOAT" => "tFLOAT",
        "FLOAT_IMAGINARY" => "tIMAGINARY",
        "FLOAT_RATIONAL" => "tRATIONAL",
        "FLOAT_RATIONAL_IMAGINARY" => "tIMAGINARY",
        "GLOBAL_VARIABLE" => "tGVAR",
        "GREATER" => "tGT",
        "GREATER_EQUAL" => "tGEQ",
        "GREATER_GREATER" => "tRSHFT",
        "GREATER_GREATER_EQUAL" => "tOP_ASGN",
        "HEREDOC_START" => "tSTRING_BEG",
        "HEREDOC_END" => "tSTRING_END",
        "IDENTIFIER" => "tIDENTIFIER",
        "INSTANCE_VARIABLE" => "tIVAR",
        "INTEGER" => "tINTEGER",
        "INTEGER_IMAGINARY" => "tIMAGINARY",
        "INTEGER_RATIONAL" => "tRATIONAL",
        "INTEGER_RATIONAL_IMAGINARY" => "tIMAGINARY",
        "KEYWORD_ALIAS" => "kALIAS",
        "KEYWORD_AND" => "kAND",
        "KEYWORD_BEGIN" => "kBEGIN",
        "KEYWORD_BEGIN_UPCASE" => "klBEGIN",
        "KEYWORD_BREAK" => "kBREAK",
        "KEYWORD_CASE" => "kCASE",
        "KEYWORD_CLASS" => "kCLASS",
        "KEYWORD_DEF" => "kDEF",
        "KEYWORD_DEFINED" => "kDEFINED",
        "KEYWORD_DO" => "kDO",
        "KEYWORD_DO_LOOP" => "kDO_COND",
        "KEYWORD_END" => "kEND",
        "KEYWORD_END_UPCASE" => "klEND",
        "KEYWORD_ENSURE" => "kENSURE",
        "KEYWORD_ELSE" => "kELSE",
        "KEYWORD_ELSIF" => "kELSIF",
        "KEYWORD_FALSE" => "kFALSE",
        "KEYWORD_FOR" => "kFOR",
        "KEYWORD_IF" => "kIF",
        "KEYWORD_IF_MODIFIER" => "kIF_MOD",
        "KEYWORD_IN" => "kIN",
        "KEYWORD_MODULE" => "kMODULE",
        "KEYWORD_NEXT" => "kNEXT",
        "KEYWORD_NIL" => "kNIL",
        "KEYWORD_NOT" => "kNOT",
        "KEYWORD_OR" => "kOR",
        "KEYWORD_REDO" => "kREDO",
        "KEYWORD_RESCUE" => "kRESCUE",
        "KEYWORD_RESCUE_MODIFIER" => "kRESCUE_MOD",
        "KEYWORD_RETRY" => "kRETRY",
        "KEYWORD_RETURN" => "kRETURN",
        "KEYWORD_SELF" => "kSELF",
        "KEYWORD_SUPER" => "kSUPER",
        "KEYWORD_THEN" => "kTHEN",
        "KEYWORD_TRUE" => "kTRUE",
        "KEYWORD_UNDEF" => "kUNDEF",
        "KEYWORD_UNLESS" => "kUNLESS",
        "KEYWORD_UNLESS_MODIFIER" => "kUNLESS_MOD",
        "KEYWORD_UNTIL" => "kUNTIL",
        "KEYWORD_UNTIL_MODIFIER" => "kUNTIL_MOD",
        "KEYWORD_WHEN" => "kWHEN",
        "KEYWORD_WHILE" => "kWHILE",
        "KEYWORD_WHILE_MODIFIER" => "kWHILE_MOD",
        "KEYWORD_YIELD" => "kYIELD",
        "KEYWORD___ENCODING__" => "k__ENCODING__",
        "KEYWORD___FILE__" => "k__FILE__",
        "KEYWORD___LINE__" => "k__LINE__",
        "LABEL" => "tLABEL",
        "LABEL_END" => "tLABEL_END",
        "LAMBDA_BEGIN" => "tLAMBEG",
        "LESS" => "tLT",
        "LESS_EQUAL" => "tLEQ",
        "LESS_EQUAL_GREATER" => "tCMP",
        "LESS_LESS" => "tLSHFT",
        "LESS_LESS_EQUAL" => "tOP_ASGN",
        "METHOD_NAME" => "tFID",
        "MINUS" => "tMINUS",
        "MINUS_EQUAL" => "tOP_ASGN",
        "MINUS_GREATER" => "tLAMBDA",
        "NEWLINE" => "tNL",
        "NUMBERED_REFERENCE" => "tNTH_REF",
        "PARENTHESIS_LEFT" => "tLPAREN2",
        "PARENTHESIS_LEFT_PARENTHESES" => "tLPAREN_ARG",
        "PARENTHESIS_RIGHT" => "tRPAREN",
        "PERCENT" => "tPERCENT",
        "PERCENT_EQUAL" => "tOP_ASGN",
        "PERCENT_LOWER_I" => "tQSYMBOLS_BEG",
        "PERCENT_LOWER_W" => "tQWORDS_BEG",
        "PERCENT_UPPER_I" => "tSYMBOLS_BEG",
        "PERCENT_UPPER_W" => "tWORDS_BEG",
        "PERCENT_LOWER_X" => "tXSTRING_BEG",
        "PLUS" => "tPLUS",
        "PLUS_EQUAL" => "tOP_ASGN",
        "PIPE_EQUAL" => "tOP_ASGN",
        "PIPE" => "tPIPE",
        "PIPE_PIPE" => "tOROP",
        "PIPE_PIPE_EQUAL" => "tOP_ASGN",
        "QUESTION_MARK" => "tEH",
        "REGEXP_BEGIN" => "tREGEXP_BEG",
        "REGEXP_END" => "tSTRING_END",
        "SEMICOLON" => "tSEMI",
        "SLASH" => "tDIVIDE",
        "SLASH_EQUAL" => "tOP_ASGN",
        "STAR" => "tSTAR2",
        "STAR_EQUAL" => "tOP_ASGN",
        "STAR_STAR" => "tPOW",
        "STAR_STAR_EQUAL" => "tOP_ASGN",
        "STRING_BEGIN" => "tSTRING_BEG",
        "STRING_CONTENT" => "tSTRING_CONTENT",
        "STRING_END" => "tSTRING_END",
        "SYMBOL_BEGIN" => "tSYMBEG",
        "TILDE" => "tTILDE",
        "UAMPERSAND" => "tAMPER",
        "UCOLON_COLON" => "tCOLON3",
        "UDOT_DOT" => "tBDOT2",
        "UDOT_DOT_DOT" => "tBDOT3",
        "UMINUS" => "tUMINUS",
        "UMINUS_NUM" => "tUNARY_NUM",
        "UPLUS" => "tUPLUS",
        "USTAR" => "tSTAR",
        "USTAR_STAR" => "tDSTAR",
        "WORDS_SEP" => "tSPACE",
        _ => return None,
    })
}

fn lambda_token_type(t: &str) -> bool {
    matches!(t, "kDO_LAMBDA" | "tLAMBDA" | "tLAMBEG")
}

fn lparen_conversion_type(t: &str) -> bool {
    matches!(
        t,
        "kBREAK" | "tCARET" | "kCASE" | "tDIVIDE" | "kFOR" | "kIF" | "kNEXT" | "kRETURN" | "kUNTIL"
            | "kWHILE" | "tAMPER" | "tANDOP" | "tBANG" | "tCOMMA" | "tDOT2" | "tDOT3" | "tEQL"
            | "tLPAREN" | "tLPAREN2" | "tLPAREN_ARG" | "tLSHFT" | "tNL" | "tOP_ASGN" | "tOROP"
            | "tPIPE" | "tSEMI" | "tSTRING_DBEG" | "tUMINUS" | "tUPLUS" | "tLCURLY"
    )
}

fn comment_continuation_type(t: Option<&str>) -> bool {
    matches!(t, Some("COMMENT") | Some("AMPERSAND_DOT") | Some("DOT"))
}

struct HeredocData {
    identifier: Vec<u8>,
    common_whitespace: u32,
}

struct Lx<'a, 'c, 'vm> {
    ctx: &'c mut Ctx<'vm>,
    lexed: &'a [PTok],
}

impl<'a, 'c, 'vm> Lx<'a, 'c, 'vm> {
    fn tname(&self, tok: &PTok) -> &'static str {
        TOKEN_TYPES.get(tok.ty as usize).copied().unwrap_or("")
    }
    fn value(&self, tok: &PTok) -> Vec<u8> {
        self.ctx.slice((tok.start, tok.end)).to_vec()
    }
    fn range(&self, start: u32, end: u32) -> R {
        self.ctx.r((start, end))
    }
    fn start_line(&self, tok: &PTok) -> i64 {
        self.ctx.line_of(tok.start)
    }
    fn start_column(&self, tok: &PTok) -> u32 {
        tok.start - self.ctx.line_start(tok.start)
    }
}

pub(crate) fn translate_tokens(ctx: &mut Ctx<'_>, parse: &PParse) -> CRes<Vec<OutTok>> {
    let mut lx = Lx { ctx, lexed: &parse.tokens };
    let mut tokens: Vec<OutTok> = Vec::with_capacity(parse.tokens.len());

    let mut index = 0usize;
    let length = lx.lexed.len();

    let mut heredoc_stack: Vec<HeredocData> = Vec::new();
    let mut quote_stack: Vec<Vec<u8>> = Vec::new();
    let mut comment_newline_location: Option<R> = None;

    while index < length {
        let token = lx.lexed[index];
        let state = token.lex_state;
        index += 1;

        let prism_type = lx.tname(&token);
        if matches!(prism_type, "IGNORED_NEWLINE" | "__END__" | "EOF") {
            continue;
        }

        let Some(mut ty) = map_type(prism_type) else {
            return decline("unknown prism token type");
        };
        if ty.is_empty() {
            // MISSING / NOT_PROVIDED / stray EMBDOC rows — error files only.
            return decline("nil-mapped token type");
        }
        let mut value: TokVal = TokVal::Bytes(lx.value(&token));
        let mut location: R = lx.range(token.start, token.end);

        match ty {
            "kDO" => {
                let nearest_lambda = tokens.iter().rev().find(|t| lambda_token_type(t.ty));
                if matches!(nearest_lambda, Some(t) if t.ty == "tLAMBDA") {
                    ty = "kDO_LAMBDA";
                }
            }
            "tCHARACTER" => {
                let mut v = lx.value(&token);
                if v.first() == Some(&b'?') {
                    v.remove(0);
                }
                let unescaped = unescape_string(lx.ctx, &v, b"?")?;
                value = TokVal::Bytes(unescaped);
            }
            "tCOMMENT" => {
                if prism_type == "EMBDOC_BEGIN" {
                    let mut v = lx.value(&token);
                    let mut end_off = token.end;
                    loop {
                        let next_token = lx.lexed.get(index);
                        let at_end = index >= length - 1;
                        match next_token {
                            Some(nt) if lx.tname(nt) == "EMBDOC_END" => {
                                v.extend_from_slice(&lx.value(nt));
                                end_off = nt.end;
                                index += 1;
                                break;
                            }
                            Some(nt) if !at_end => {
                                v.extend_from_slice(&lx.value(nt));
                                index += 1;
                            }
                            Some(nt) => {
                                v.extend_from_slice(&lx.value(nt));
                                end_off = nt.end;
                                index += 1;
                                break;
                            }
                            None => break,
                        }
                    }
                    value = TokVal::Bytes(v);
                    location = lx.range(token.start, end_off);
                } else {
                    let mut v = lx.value(&token);
                    // is_at_eol = value.chomp!.nil? — true when no trailing \n.
                    let had_nl = v.ends_with(b"\n");
                    if had_nl {
                        if v.ends_with(b"\r\n") {
                            v.truncate(v.len() - 2);
                        } else {
                            v.truncate(v.len() - 1);
                        }
                    }
                    let is_at_eol = !had_nl;
                    location = lx.range(token.start, if is_at_eol { token.end } else { token.end - 1 });

                    let prev_token = if index >= 2 { lx.lexed.get(index - 2) } else { None };
                    let next_token = lx.lexed.get(index);
                    let next_type = next_token.map(|t| lx.tname(t));

                    let is_inline_comment = prev_token
                        .map(|p| lx.start_line(p) == lx.start_line(&token))
                        .unwrap_or(false);
                    if is_inline_comment && !is_at_eol && !comment_continuation_type(next_type) {
                        tokens.push(OutTok { ty: "tCOMMENT", val: TokVal::Bytes(v), r: location });
                        let nl_location = lx.range(token.end - 1, token.end);
                        tokens.push(OutTok { ty: "tNL", val: TokVal::Nil, r: nl_location });
                        continue;
                    } else if is_inline_comment && next_type == Some("COMMENT") {
                        comment_newline_location = Some(lx.range(token.end - 1, token.end));
                        value = TokVal::Bytes(v);
                    } else if comment_newline_location.is_some() && !comment_continuation_type(next_type) {
                        tokens.push(OutTok { ty: "tCOMMENT", val: TokVal::Bytes(v), r: location });
                        tokens.push(OutTok {
                            ty: "tNL",
                            val: TokVal::Nil,
                            r: comment_newline_location.unwrap(),
                        });
                        comment_newline_location = None;
                        continue;
                    } else {
                        value = TokVal::Bytes(v);
                    }
                }
            }
            "tNL" => {
                let next_token = lx.lexed.get(index);
                if matches!(next_token.map(|t| lx.tname(t)), Some("COMMENT")) {
                    comment_newline_location = Some(location);
                    continue;
                }
                value = TokVal::Nil;
            }
            "tFLOAT" => {
                let v = lx.value(&token);
                value = TokVal::V(Value::Float(parse_float(&v)));
            }
            "tIMAGINARY" => {
                let v = lx.value(&token);
                value = TokVal::V(parse_complex(lx.ctx, &v)?);
            }
            "tINTEGER" => {
                let v = lx.value(&token);
                if v.first() == Some(&b'+') {
                    tokens.push(OutTok {
                        ty: "tUNARY_NUM",
                        val: TokVal::BytesF(b"+".to_vec()),
                        r: lx.range(token.start, token.start + 1),
                    });
                    location = lx.range(token.start + 1, token.end);
                }
                value = TokVal::V(parse_integer(lx.ctx, &v)?);
            }
            "tLABEL" => {
                let mut v = lx.value(&token);
                if v.ends_with(b":") {
                    v.pop();
                }
                value = TokVal::Bytes(v);
            }
            "tLABEL_END" => {
                let mut v = lx.value(&token);
                if v.ends_with(b":") {
                    v.pop();
                }
                value = TokVal::Bytes(v);
            }
            "tLCURLY" => {
                if state == EXPR_BEG | EXPR_LABEL {
                    ty = "tLBRACE";
                }
            }
            "tLPAREN2" => {
                if tokens.is_empty() || lparen_conversion_type(tokens.last().unwrap().ty) {
                    ty = "tLPAREN";
                }
            }
            "tNTH_REF" => {
                let v = lx.value(&token);
                let digits = v.strip_prefix(b"$").unwrap_or(&v);
                value = TokVal::V(parse_integer(lx.ctx, digits)?);
            }
            "tOP_ASGN" => {
                let mut v = lx.value(&token);
                if v.ends_with(b"=") {
                    v.pop();
                }
                value = TokVal::Bytes(v);
            }
            "tRATIONAL" => {
                let v = lx.value(&token);
                value = TokVal::V(parse_rational(lx.ctx, &v)?);
            }
            "tSPACE" => {
                let v = lx.value(&token);
                let leading = percent_array_leading_whitespace(&v);
                location = lx.range(token.start, token.start + leading);
                value = TokVal::Nil;
            }
            "tSTRING_BEG" => {
                let v = lx.value(&token);
                let next_token = lx.lexed.get(index).copied();
                let next_next_token = lx.lexed.get(index + 1).copied();
                let basic_quotes = v == b"\"" || v == b"'";

                if basic_quotes && matches!(next_token, Some(nt) if lx.tname(&nt) == "STRING_END") {
                    let nt = next_token.unwrap();
                    ty = "tSTRING";
                    value = TokVal::BytesF(vec![]);
                    location = lx.range(token.start.min(nt.start), token.end.max(nt.end));
                    index += 1;
                } else if matches!(v.first(), Some(b'\'') | Some(b'"') | Some(b'%')) {
                    let mut simplified = false;
                    if let (Some(nt), Some(nnt)) = (next_token, next_next_token)
                        && lx.tname(&nt) == "STRING_CONTENT"
                        && lx.tname(&nnt) == "STRING_END"
                    {
                        let string_value = lx.value(&nt);
                        if simplify_string(&string_value, &v) {
                            let val = if percent_array(&v) {
                                percent_array_unescape(&string_value)
                            } else {
                                unescape_string(lx.ctx, &string_value, &v)?
                            };
                            let start = token.start.min(nnt.start);
                            let end = token.end.max(nnt.end);
                            index += 2;
                            tokens.push(OutTok {
                                ty: "tSTRING",
                                val: TokVal::Bytes(val),
                                r: lx.range(start, end),
                            });
                            simplified = true;
                        }
                    }
                    if simplified {
                        continue;
                    }
                    quote_stack.push(v.clone());
                    value = TokVal::Bytes(v);
                } else if prism_type == "HEREDOC_START" {
                    // quote/type from the opening: <<-"ID" etc.
                    let c2 = v.get(2).copied();
                    let (quote, _heredoc_type) = if c2 == Some(b'-') || c2 == Some(b'~') {
                        (v.get(3).copied(), c2)
                    } else {
                        (c2, None.map(|_: u8| b' '))
                    };
                    let heredoc_type = if c2 == Some(b'-') || c2 == Some(b'~') { c2 } else { None };
                    // identifier: /<<[-~]?["'`]?(?<id>.*?)["'`]?\z/
                    let mut id_start = 2usize;
                    if matches!(v.get(id_start), Some(b'-') | Some(b'~')) {
                        id_start += 1;
                    }
                    let mut id_end = v.len();
                    if matches!(v.get(id_start), Some(b'"') | Some(b'\'') | Some(b'`')) {
                        id_start += 1;
                        if id_end > id_start && matches!(v[id_end - 1], b'"' | b'\'' | b'`') {
                            id_end -= 1;
                        }
                    }
                    let identifier = v.get(id_start..id_end).unwrap_or(&[]).to_vec();
                    let mut heredoc = HeredocData { identifier, common_whitespace: 0 };

                    if quote == Some(b'`') {
                        ty = "tXSTRING_BEG";
                    }

                    if heredoc_type == Some(b'~') || heredoc_type == Some(b'`') {
                        heredoc.common_whitespace = calculate_heredoc_whitespace(&mut lx, index)?;
                    }

                    let (new_value, frozen): (Vec<u8>, bool) = match quote {
                        Some(b'\'') | Some(b'"') | Some(b'`') => {
                            let mut nv = b"<<".to_vec();
                            nv.push(quote.unwrap());
                            (nv, false) // "<<#{quote}" — interpolation, unfrozen
                        }
                        _ => (b"<<\"".to_vec(), true), // '<<"' — frozen literal
                    };
                    heredoc_stack.push(heredoc);
                    quote_stack.push(new_value.clone());
                    value = if frozen { TokVal::BytesF(new_value) } else { TokVal::Bytes(new_value) };
                } else {
                    value = TokVal::Bytes(v);
                }
            }
            "tSTRING_CONTENT" => {
                let quote_last = quote_stack.last().cloned().unwrap_or_default();
                let is_percent_array = percent_array(&quote_last);
                let full = lx.value(&token);
                let lines = byte_lines_count(&full);

                if lines <= 1 {
                    // Single-line loop: squiggly-heredoc line continuations
                    // are joined manually.
                    let mut current_string: Vec<u8> = Vec::new();
                    let mut current_length: u32 = 0;
                    let start_offset = token.start;
                    let mut tok = token;
                    loop {
                        if lx.tname(&tok) != "STRING_CONTENT" {
                            return decline("string content chain");
                        }
                        current_length += tok.end - tok.start;
                        let tok_value = lx.value(&tok);
                        let prev_token = if index >= 2 { lx.lexed.get(index - 2) } else { None };
                        let is_first_token_on_line = prev_token
                            .map(|p| lx.start_line(&tok) != lx.start_line(p))
                            .unwrap_or(false);
                        let not_nested = heredoc_stack.len() == 1;
                        let mut v = tok_value.clone();
                        if is_percent_array {
                            v = percent_array_unescape(&tok_value);
                        } else if is_first_token_on_line
                            && not_nested
                            && heredoc_stack.last().map(|h| h.common_whitespace > 0).unwrap_or(false)
                        {
                            let common = heredoc_stack.last().unwrap().common_whitespace;
                            v = trim_heredoc_whitespace(&tok_value, common);
                        }
                        current_string.extend_from_slice(&unescape_string(lx.ctx, &v, &quote_last)?);
                        let relevant_backslash_count = if quote_last.starts_with(b"%W") || quote_last.starts_with(b"%I") {
                            0 // the last backslash escapes the newline
                        } else {
                            backslashes_before_newline(&tok_value)
                        };
                        if relevant_backslash_count % 2 == 0 || !interpolation(&quote_last) {
                            tokens.push(OutTok {
                                ty: "tSTRING_CONTENT",
                                val: TokVal::Bytes(current_string),
                                r: lx.range(start_offset, start_offset + current_length),
                            });
                            break;
                        }
                        let Some(next) = lx.lexed.get(index).copied() else {
                            return decline("string continuation at EOF");
                        };
                        tok = next;
                        index += 1;
                    }
                } else {
                    // Multi-line content: split into per-line tokens, joining
                    // line continuations.
                    let mut current_line: Vec<u8> = Vec::new();
                    let mut adjustment: u32 = 0;
                    let mut start_offset = token.start;
                    let all_lines = byte_lines(&full);
                    let count = all_lines.len();
                    for (li, line) in all_lines.into_iter().enumerate() {
                        let chomped_len = chomp_len(line);
                        let chomped = &line[..chomped_len];
                        let backslash_count = chomped.iter().rev().take_while(|b| **b == b'\\').count();
                        let is_interpolation = interpolation(&quote_last);
                        let mut emit;

                        if backslash_count % 2 == 1 && (is_interpolation || is_percent_array) {
                            if is_percent_array {
                                current_line.extend_from_slice(&percent_array_unescape(line));
                                adjustment += 1;
                            } else {
                                let mut c = chomped.to_vec();
                                c.pop(); // delete_suffix!("\\")
                                current_line.extend_from_slice(&c);
                                adjustment += 2;
                            }
                            emit = li == count - 1;
                        } else {
                            current_line.extend_from_slice(line);
                            emit = true;
                        }

                        if emit {
                            let end_offset = start_offset + current_line.len() as u32 + adjustment;
                            let unescaped = unescape_string(lx.ctx, &current_line, &quote_last)?;
                            tokens.push(OutTok {
                                ty: "tSTRING_CONTENT",
                                val: TokVal::Bytes(unescaped),
                                r: lx.range(start_offset, end_offset),
                            });
                            start_offset = end_offset;
                            current_line.clear();
                            adjustment = 0;
                        }
                        let _ = &mut emit;
                    }
                }
                continue;
            }
            "tSTRING_DVAR" => {
                value = TokVal::Nil;
            }
            "tSTRING_END" => {
                let v = lx.value(&token);
                if prism_type == "HEREDOC_END" && v.ends_with(b"\n") {
                    let newline_length = if v.ends_with(b"\r\n") { 2 } else { 1 };
                    let heredoc = heredoc_stack.pop().ok_or(Decline("heredoc stack empty"))?;
                    value = TokVal::Bytes(heredoc.identifier);
                    location = lx.range(token.start, token.end - newline_length);
                } else if prism_type == "REGEXP_END" {
                    value = TokVal::Bytes(v.first().map(|b| vec![*b]).unwrap_or_default());
                    location = lx.range(token.start, token.start + 1);
                } else {
                    value = TokVal::Bytes(v);
                }

                if percent_array(&quote_stack.pop().unwrap_or_default()) {
                    let prev_token = if index >= 2 { lx.lexed.get(index - 2) } else { None };
                    let prev_type = prev_token.map(|t| lx.tname(t));
                    let empty = matches!(
                        prev_type,
                        Some("PERCENT_LOWER_I") | Some("PERCENT_LOWER_W") | Some("PERCENT_UPPER_I") | Some("PERCENT_UPPER_W")
                    );
                    let ends_with_whitespace = prev_type == Some("WORDS_SEP");
                    if !empty && !ends_with_whitespace {
                        tokens.push(OutTok {
                            ty: "tSPACE",
                            val: TokVal::Nil,
                            r: lx.range(token.start, token.start),
                        });
                    }
                }
            }
            "tSYMBEG" => {
                let next_token = lx.lexed.get(index).copied();
                let next_type = next_token.map(|t| lx.tname(&t));
                if let Some(nt) = next_token
                    && !matches!(next_type, Some("STRING_CONTENT") | Some("EMBEXPR_BEGIN") | Some("EMBVAR") | Some("STRING_END"))
                {
                    ty = "tSYMBOL";
                    let nv = lx.value(&nt);
                    value = match nv.as_slice() {
                        // Frozen hash-literal values in the gem's mapping.
                        b"~@" => TokVal::BytesF(b"~".to_vec()),
                        b"!@" => TokVal::BytesF(b"!".to_vec()),
                        _ => TokVal::Bytes(nv),
                    };
                    location = lx.range(token.start.min(nt.start), token.end.max(nt.end));
                    index += 1;
                } else {
                    quote_stack.push(lx.value(&token));
                }
            }
            "tFID" => {
                if matches!(tokens.last(), Some(t) if t.ty == "kDEF") {
                    ty = "tIDENTIFIER";
                }
            }
            "tXSTRING_BEG" => {
                let next_token = lx.lexed.get(index);
                let next_type = next_token.map(|t| lx.tname(t));
                if next_token.is_some()
                    && !matches!(next_type, Some("STRING_CONTENT") | Some("STRING_END") | Some("EMBEXPR_BEGIN"))
                {
                    // self.`()
                    ty = "tBACK_REF2";
                }
                quote_stack.push(lx.value(&token));
            }
            "tSYMBOLS_BEG" | "tQSYMBOLS_BEG" | "tWORDS_BEG" | "tQWORDS_BEG" => {
                if let Some(nt) = lx.lexed.get(index)
                    && lx.tname(nt) == "WORDS_SEP"
                {
                    index += 1;
                }
                quote_stack.push(lx.value(&token));
            }
            "tREGEXP_BEG" => {
                quote_stack.push(lx.value(&token));
            }
            _ => {}
        }

        tokens.push(OutTok { ty, val: value, r: location });

        if prism_type == "REGEXP_END" {
            let v = lx.value(&token);
            tokens.push(OutTok {
                ty: "tREGEXP_OPT",
                val: TokVal::Bytes(v.get(1..).unwrap_or(&[]).to_vec()),
                r: lx.range(token.start + 1, token.end),
            });
        }
    }

    Ok(tokens)
}

// ---------------------------------------------------------------------------
// Numeric literal parsing (Kernel#Integer/Float/Rational/Complex semantics)
// ---------------------------------------------------------------------------

fn strip_underscores(v: &[u8]) -> Vec<u8> {
    v.iter().copied().filter(|b| *b != b'_').collect()
}

/// `Integer(value) rescue 0`.
///
/// `ctx` is only touched on the `bignum` overflow arm — same shape
/// as sprintf.rs's bignum formatter.
#[cfg_attr(not(feature = "bignum"), allow(unused_variables))]
fn parse_integer(ctx: &mut Ctx<'_>, v: &[u8]) -> CRes<Value> {
    let s = strip_underscores(v);
    let (neg, body) = match s.first() {
        Some(b'-') => (true, &s[1..]),
        Some(b'+') => (false, &s[1..]),
        _ => (false, &s[..]),
    };
    let (radix, digits): (u32, &[u8]) = if body.len() >= 2 && body[0] == b'0' {
        match body[1] {
            b'x' | b'X' => (16, &body[2..]),
            b'b' | b'B' => (2, &body[2..]),
            b'o' | b'O' => (8, &body[2..]),
            b'd' | b'D' => (10, &body[2..]),
            b'0'..=b'7' => (8, &body[1..]),
            _ => (10, body),
        }
    } else {
        (10, body)
    };
    let text = std::str::from_utf8(digits).map_err(|_| Decline("integer utf8"))?;
    match i64::from_str_radix(text, radix) {
        Ok(n) => Ok(Value::Int(if neg { -n } else { n })),
        Err(_) => {
            #[cfg(feature = "bignum")]
            {
                use num_bigint::BigInt;
                let b = BigInt::parse_bytes(digits, radix).ok_or(Decline("integer parse"))?;
                let b = if neg { -b } else { b };
                ctx.check_alloc()?;
                return Ok(Value::BigInt(ctx.vm.heap.alloc(crate::heap::HeapObj::BigInt(b))));
            }
            #[allow(unreachable_code)]
            decline("integer overflow")
        }
    }
}

/// `Float(value) rescue 0.0`.
fn parse_float(v: &[u8]) -> f64 {
    let s = strip_underscores(v);
    std::str::from_utf8(&s).ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0)
}

/// `Rational(value)` for a (possibly "r"-suffixed) literal.
fn parse_rational(ctx: &mut Ctx<'_>, v: &[u8]) -> CRes<Value> {
    let mut s = v.to_vec();
    if s.ends_with(b"r") {
        s.pop();
    }
    let has_radix_prefix = s.len() >= 2 && s[0] == b'0' && matches!(s[1], b'b' | b'B' | b'o' | b'O' | b'd' | b'D' | b'x' | b'X');
    if has_radix_prefix {
        let n = parse_integer(ctx, &s)?;
        let Value::Int(n) = n else { return decline("bignum rational") };
        return ctx.rational_val(n, 1);
    }
    let s = strip_underscores(&s);
    // digits[.digits] — Rational("1.23") = 123/100.
    if let Some(dot) = s.iter().position(|b| *b == b'.') {
        let int_part = std::str::from_utf8(&s[..dot]).map_err(|_| Decline("rational utf8"))?;
        let frac = &s[dot + 1..];
        let frac_str = std::str::from_utf8(frac).map_err(|_| Decline("rational utf8"))?;
        let mantissa: i64 = format!("{}{}", int_part, frac_str).parse().map_err(|_| Decline("rational parse"))?;
        let den = 10i64.checked_pow(frac.len() as u32).ok_or(Decline("rational den overflow"))?;
        ctx.rational_val(mantissa, den)
    } else {
        let text = std::str::from_utf8(&s).map_err(|_| Decline("rational utf8"))?;
        let n: i64 = text.parse().map_err(|_| Decline("rational parse"))?;
        ctx.rational_val(n, 1)
    }
}

/// `parse_complex` — value ends with "i".
fn parse_complex(ctx: &mut Ctx<'_>, v: &[u8]) -> CRes<Value> {
    let mut s = v.to_vec();
    if s.ends_with(b"i") {
        s.pop();
    }
    let imag: Value = if s.ends_with(b"r") {
        parse_rational(ctx, &s)?
    } else if s.len() >= 2 && s[0] == b'0' && matches!(s[1], b'b' | b'B' | b'o' | b'O' | b'd' | b'D' | b'x' | b'X') {
        parse_integer(ctx, &s)?
    } else if s.contains(&b'.') || s.contains(&b'e') || s.contains(&b'E') {
        Value::Float(parse_float(&s))
    } else {
        parse_integer(ctx, &s)?
    };
    ctx.complex_val(Value::Int(0), imag)
}

// ---------------------------------------------------------------------------
// String escape handling
// ---------------------------------------------------------------------------

fn interpolation(quote: &[u8]) -> bool {
    !quote.ends_with(b"'")
        && !quote.starts_with(b"%q")
        && !quote.starts_with(b"%w")
        && !quote.starts_with(b"%i")
        && !quote.starts_with(b"%s")
}

fn regexp(quote: &[u8]) -> bool {
    quote == b"/" || quote.starts_with(b"%r")
}

fn percent_array(quote: &[u8]) -> bool {
    quote.starts_with(b"%w") || quote.starts_with(b"%W") || quote.starts_with(b"%i") || quote.starts_with(b"%I")
}

const REGEXP_META_CHARACTERS: &[u8] = b"\\$()*+.<>?[]^{|}";

fn delimiter_symmetry(d: u8) -> Option<u8> {
    match d {
        b'[' => Some(b']'),
        b'(' => Some(b')'),
        b'{' => Some(b'}'),
        b'<' => Some(b'>'),
        _ => None,
    }
}

/// `unescape_string(string, quote)`.
fn unescape_string(ctx: &Ctx<'_>, string: &[u8], quote: &[u8]) -> CRes<Vec<u8>> {
    if quote == b"<<'" {
        return Ok(string.to_vec());
    }
    if !string.contains(&b'\\') {
        return Ok(string.to_vec());
    }
    let delimiter = *quote.last().unwrap_or(&b'"');

    if regexp(quote) {
        if REGEXP_META_CHARACTERS.contains(&delimiter) {
            Ok(string.to_vec())
        } else {
            // gsub(/\\(#{delimiter})/, '\1')
            let mut out = Vec::with_capacity(string.len());
            let mut i = 0;
            while i < string.len() {
                if string[i] == b'\\' && i + 1 < string.len() && string[i + 1] == delimiter {
                    out.push(delimiter);
                    i += 2;
                } else {
                    out.push(string[i]);
                    i += 1;
                }
            }
            Ok(out)
        }
    } else if interpolation(quote) {
        let mut result: Vec<u8> = Vec::with_capacity(string.len());
        let mut pos = 0usize;
        while let Some(rel) = string[pos..].iter().position(|b| *b == b'\\') {
            result.extend_from_slice(&string[pos..pos + rel]);
            pos += rel + 1; // consume the backslash
            escape_read(&mut result, string, &mut pos, false, false)?;
        }
        result.extend_from_slice(&string[pos..]);
        let _ = ctx;
        Ok(result)
    } else {
        // gsub(/\\([\\#{delimiter}#{symmetry}])/, '\1')
        let sym = delimiter_symmetry(delimiter);
        let mut out = Vec::with_capacity(string.len());
        let mut i = 0;
        while i < string.len() {
            if string[i] == b'\\' && i + 1 < string.len() {
                let c = string[i + 1];
                if c == b'\\' || c == delimiter || Some(c) == sym {
                    out.push(c);
                    i += 2;
                    continue;
                }
            }
            out.push(string[i]);
            i += 1;
        }
        Ok(out)
    }
}

fn escape_build(value: u8, control: bool, meta: bool) -> u8 {
    let mut value = value;
    if control {
        value &= 0x9f;
    }
    if meta {
        value |= 0x80;
    }
    value
}

/// `escape_read(result, scanner, control, meta)` — cursor-based port.
fn escape_read(result: &mut Vec<u8>, s: &[u8], pos: &mut usize, control: bool, meta: bool) -> CRes<()> {
    let rest = &s[*pos..];
    // Line continuation.
    if rest.first() == Some(&b'\n') {
        *pos += 1;
        return Ok(());
    }
    // Simple escapes.
    if let Some(b) = rest.first() {
        let simple = match b {
            b'a' => Some(0x07u8),
            b'b' => Some(0x08),
            b'e' => Some(0x1b),
            b'f' => Some(0x0c),
            b'n' => Some(b'\n'),
            b'r' => Some(b'\r'),
            b's' => Some(b' '),
            b't' => Some(b'\t'),
            b'v' => Some(0x0b),
            b'\\' => Some(b'\\'),
            _ => None,
        };
        if let Some(byte) = simple {
            result.push(byte);
            *pos += 1;
            return Ok(());
        }
    }
    // \nnn octal.
    if matches!(rest.first(), Some(b'0'..=b'7')) {
        let mut n: u32 = 0;
        let mut taken = 0;
        while taken < 3 && matches!(rest.get(taken), Some(b'0'..=b'7')) {
            n = n * 8 + (rest[taken] - b'0') as u32;
            taken += 1;
        }
        result.push(escape_build(n as u8, control, meta));
        *pos += taken;
        return Ok(());
    }
    // \xnn hex.
    if rest.first() == Some(&b'x') && matches!(rest.get(1), Some(c) if c.is_ascii_hexdigit()) {
        let mut n: u32 = 0;
        let mut taken = 1;
        while taken <= 2 && matches!(rest.get(taken), Some(c) if c.is_ascii_hexdigit()) {
            n = n * 16 + (rest[taken] as char).to_digit(16).unwrap();
            taken += 1;
        }
        result.push(escape_build(n as u8, control, meta));
        *pos += taken;
        return Ok(());
    }
    // \unnnn.
    if rest.first() == Some(&b'u')
        && rest.len() >= 5
        && rest[1..5].iter().all(|c| c.is_ascii_hexdigit())
    {
        let cp = u32::from_str_radix(std::str::from_utf8(&rest[1..5]).unwrap(), 16).unwrap();
        let ch = char::from_u32(cp).ok_or(Decline("invalid \\u codepoint"))?;
        let mut buf = [0u8; 4];
        result.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        *pos += 5;
        return Ok(());
    }
    // \u{} — https://github.com/whitequark/parser/issues/856
    if rest.starts_with(b"u{}") {
        *pos += 3;
        return Ok(());
    }
    // \u{nnnn ...} — non-greedy scan of u{.*?}.
    if rest.starts_with(b"u{")
        && let Some(close) = rest.iter().position(|b| *b == b'}')
    {
        let inner = &rest[2..close];
        for part in inner.split(|b| matches!(b, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)) {
            if part.is_empty() {
                continue;
            }
            let cp = u32::from_str_radix(
                std::str::from_utf8(part).map_err(|_| Decline("\\u{} utf8"))?,
                16,
            )
            .map_err(|_| Decline("\\u{} parse"))?;
            let ch = char::from_u32(cp).ok_or(Decline("invalid \\u{} codepoint"))?;
            let mut buf = [0u8; 4];
            result.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
        *pos += close + 1;
        return Ok(());
    }
    // \cx / \C-x (control), lookahead printable.
    let is_print = |b: u8| (0x20..=0x7e).contains(&b);
    if rest.first() == Some(&b'c') {
        let skip = if rest.get(1) == Some(&b'\\') { 2 } else { 1 };
        if matches!(rest.get(skip), Some(c) if is_print(*c)) {
            *pos += skip;
            return escape_read(result, s, pos, true, meta);
        }
    }
    if rest.starts_with(b"C-") {
        let skip = if rest.get(2) == Some(&b'\\') { 3 } else { 2 };
        if matches!(rest.get(skip), Some(c) if is_print(*c)) {
            *pos += skip;
            return escape_read(result, s, pos, true, meta);
        }
    }
    // \M-x (meta).
    if rest.starts_with(b"M-") {
        let skip = if rest.get(2) == Some(&b'\\') { 3 } else { 2 };
        if matches!(rest.get(skip), Some(c) if is_print(*c)) {
            *pos += skip;
            return escape_read(result, s, pos, control, true);
        }
    }
    // Anything else after an escape: scan_byte.
    if let Some(byte) = rest.first() {
        if control && *byte == 0x3f {
            result.push(escape_build(0x7f, false, meta));
        } else {
            result.push(escape_build(*byte, control, meta));
        }
        *pos += 1;
    }
    Ok(())
}

/// `simplify_string?(value, quote)`.
fn simplify_string(value: &[u8], quote: &[u8]) -> bool {
    match quote {
        b"'" => !value.contains(&b'\n'),
        b"\"" => byte_lines(value).iter().all(|line| {
            if !line.ends_with(b"\n") {
                true
            } else {
                // odd backslash count immediately before the newline
                let chomped = &line[..line.len() - 1];
                let n = chomped.iter().rev().take_while(|b| **b == b'\\').count();
                n % 2 == 1
            }
        }),
        _ => false,
    }
}

/// `percent_array_unescape(string)` — drop ONE leading backslash from every
/// backslash-run that precedes a whitespace character. (The gem's
/// `Regexp.last_match[1]` is the single-char capture — always length 1/odd.)
fn percent_array_unescape(string: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(string.len());
    let mut i = 0;
    while i < string.len() {
        if string[i] == b'\\' {
            // find run of backslashes
            let mut j = i;
            while j < string.len() && string[j] == b'\\' {
                j += 1;
            }
            if j < string.len() && matches!(string[j], b' ' | 0x0c | b'\n' | b'\r' | b'\t' | 0x0b) {
                // full match = run + ws; delete one leading backslash.
                out.extend_from_slice(&string[i + 1..=j]);
                i = j + 1;
                continue;
            }
            out.extend_from_slice(&string[i..j]);
            i = j;
        } else {
            out.push(string[i]);
            i += 1;
        }
    }
    out
}

/// `percent_array_leading_whitespace(string)` (in CHARACTERS — ASCII
/// whitespace only, so bytes == chars).
fn percent_array_leading_whitespace(string: &[u8]) -> u32 {
    if string.starts_with(b"\n") {
        return 1;
    }
    let mut n = 0;
    for b in string {
        if *b == b'\n' {
            break;
        }
        n += 1;
    }
    n
}

fn byte_lines(s: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, b) in s.iter().enumerate() {
        if *b == b'\n' {
            out.push(&s[start..=i]);
            start = i + 1;
        }
    }
    if start < s.len() {
        out.push(&s[start..]);
    }
    out
}

fn byte_lines_count(s: &[u8]) -> usize {
    byte_lines(s).len()
}

fn chomp_len(line: &[u8]) -> usize {
    if line.ends_with(b"\r\n") {
        line.len() - 2
    } else if line.ends_with(b"\n") || line.ends_with(b"\r") {
        line.len() - 1
    } else {
        line.len()
    }
}

/// `token.value[/(\\{1,})\n/, 1]&.length || 0` — the backslash run before the
/// FIRST backslash+\n occurrence.
fn backslashes_before_newline(v: &[u8]) -> usize {
    let mut i = 0;
    while i < v.len() {
        if v[i] == b'\\' {
            let mut j = i;
            while j < v.len() && v[j] == b'\\' {
                j += 1;
            }
            if j < v.len() && v[j] == b'\n' {
                return j - i;
            }
            i = j;
        } else {
            i += 1;
        }
    }
    0
}

/// `calculate_heredoc_whitespace(heredoc_token_index)`.
fn calculate_heredoc_whitespace(lx: &mut Lx<'_, '_, '_>, heredoc_token_index: usize) -> CRes<u32> {
    let mut next_token_index = heredoc_token_index;
    let mut nesting_level: i32 = 0;
    let mut previous_line: i64 = -1;
    let mut result: u32 = u32::MAX;

    while let Some(next_token) = lx.lexed.get(next_token_index).copied() {
        next_token_index += 1;
        let next_next_token = lx.lexed.get(next_token_index).copied();
        let first_token_on_line = lx.start_column(&next_token) == 0;
        let tname = lx.tname(&next_token);

        if tname == "HEREDOC_START" || tname == "EMBEXPR_BEGIN" {
            if nesting_level == 0 && first_token_on_line {
                result = 0;
            }
            nesting_level += 1;
        } else if tname == "HEREDOC_END" || tname == "EMBEXPR_END" {
            nesting_level -= 1;
            if nesting_level == -1 {
                break;
            }
        } else if tname == "STRING_CONTENT" && nesting_level == 0 && first_token_on_line {
            let value = lx.value(&next_token);
            let mut common_whitespace: u32 = 0;
            for b in &value {
                match b {
                    b'\t' => common_whitespace = (common_whitespace / 8 + 1) * 8,
                    b' ' | b'\n' | b'\r' | 0x0b | 0x0c => common_whitespace += 1,
                    _ => break,
                }
            }
            let is_first_token_on_line = lx.start_line(&next_token) != previous_line;
            // Whitespace is significant if followed by interpolation.
            // value.length is in CHARACTERS.
            let char_len = match std::str::from_utf8(&value) {
                Ok(s) => s.chars().count() as u32,
                Err(_) => value.len() as u32,
            };
            let whitespace_only = common_whitespace == char_len
                && next_next_token
                    .map(|nn| lx.start_line(&nn) != lx.start_line(&next_token))
                    .unwrap_or(true);
            if is_first_token_on_line && !whitespace_only && common_whitespace < result {
                result = common_whitespace;
                previous_line = lx.start_line(&next_token);
            }
        }
    }
    Ok(result)
}

/// `trim_heredoc_whitespace(string, heredoc)`.
fn trim_heredoc_whitespace(string: &[u8], common_whitespace: u32) -> Vec<u8> {
    let mut trimmed_whitespace: u32 = 0;
    let mut trimmed_characters: usize = 0;
    while matches!(string.get(trimmed_characters), Some(b'\t') | Some(b' '))
        && trimmed_whitespace < common_whitespace
    {
        if string[trimmed_characters] == b'\t' {
            trimmed_whitespace = (trimmed_whitespace / 8 + 1) * 8;
            if trimmed_whitespace > common_whitespace {
                break;
            }
        } else {
            trimmed_whitespace += 1;
        }
        trimmed_characters += 1;
    }
    string[trimmed_characters..].to_vec()
}
