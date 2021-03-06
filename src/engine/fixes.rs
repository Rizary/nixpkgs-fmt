use std::cmp::min;

use rnix::{
    NodeOrToken, SyntaxElement,
    SyntaxKind::{NODE_STRING, NODE_STRING_INTERPOL, TOKEN_COMMENT, TOKEN_STRING_CONTENT},
    SyntaxNode, SyntaxToken, TextRange, TextUnit,
};

use crate::{
    engine::{
        indentation::{
            indent_anchor, indent_custom_anchor, single_line_comment_indent,
            string_interpol_indent, IndentLevel,
        },
        BlockPosition, FmtModel,
    },
    pattern::{Pattern, PatternSet},
    tree_utils::walk_non_whitespace,
    AtomEdit,
};

pub(super) fn fix(element: SyntaxElement, model: &mut FmtModel, anchor_set: &PatternSet<&Pattern>) {
    match element {
        NodeOrToken::Node(node) => {
            let multiline_string = node.children_with_tokens().any(|e| {
                e.as_token()
                    .map(|n| n.text().trim_start_matches(' ').starts_with('\n'))
                    .unwrap_or(false)
            });
            if node.kind() == NODE_STRING && multiline_string {
                fix_string_indentation(&node, model, anchor_set)
            }
            if node.kind() == NODE_STRING_INTERPOL {
                fix_string_interpolation(&node, model, anchor_set)
            }
        }
        NodeOrToken::Token(token) => {
            if let TOKEN_COMMENT = token.kind() {
                fix_comment_indentation(&token, model, anchor_set)
            }
        }
    }
}

fn fix_string_indentation(
    node: &SyntaxNode,
    model: &mut FmtModel,
    anchor_set: &PatternSet<&Pattern>,
) {
    let element: SyntaxElement = node.clone().into();
    let quote_indent = {
        let inside_interpolation = node.ancestors().any(|e| e.kind() == NODE_STRING_INTERPOL);
        let default_indent = IndentLevel::default();
        let block = model.block_for(&element, BlockPosition::Before);
        if block.text().contains('\n') {
            IndentLevel::from_whitespace_block(block.text())
        } else {
            let multiline_interpol_string = node
                .ancestors()
                .find(|e| e.kind() == NODE_STRING_INTERPOL)
                .map(|n| {
                    n.descendants_with_tokens()
                        .take_while(|d| d.kind() != NODE_STRING)
                        .any(|t| t.as_token().map(|e| e.text().contains("\n")).unwrap_or(false))
                })
                .unwrap_or(false);

            if inside_interpolation {
                if multiline_interpol_string {
                    match indent_anchor(&element, model, anchor_set) {
                        None => return,
                        Some((_element, indent)) => indent,
                    }
                } else {
                    indent_custom_anchor(&element, model, NODE_STRING_INTERPOL, anchor_set)
                        .unwrap_or(default_indent)
                }
            } else {
                match indent_anchor(&element, model, anchor_set) {
                    None => return,
                    Some((_element, indent)) => indent,
                }
            }
        }
    };
    let content_indent = quote_indent.indent();

    let indent_ranges: Vec<TextRange> = node_indent_ranges(node).collect();

    let (first_indent, last_indent) = match (indent_ranges.first(), indent_ranges.last()) {
        (Some(first), Some(last)) => (first, last),
        _ => return,
    };

    let first_line_is_blank =
        first_indent.start() == node.text_range().start() + TextUnit::of_str("''\n");

    let last_line_is_blank = last_indent.end() + TextUnit::of_str("''") == node.text_range().end();

    if !first_line_is_blank {
        return;
    }

    let content_ranges =
        if last_line_is_blank { &indent_ranges[..indent_ranges.len() - 1] } else { &indent_ranges };

    let common_indent = match content_ranges.iter().map(|it| it.len()).min() {
        Some(it) => it,
        None => return,
    };

    if content_indent != IndentLevel::from_len(common_indent) {
        for &range in content_ranges.iter() {
            let delete = TextRange::offset_len(range.start(), min(common_indent, range.len()));
            model.raw_edit(AtomEdit { delete, insert: content_indent.into() })
        }
    }

    if last_line_is_blank && last_indent.len() != quote_indent.len() {
        model.raw_edit(AtomEdit { delete: *last_indent, insert: quote_indent.into() })
    }
}

/// If we indent multiline block comment, we should indent it's content as well.
fn fix_comment_indentation(
    token: &SyntaxToken,
    model: &mut FmtModel,
    anchor_set: &PatternSet<&Pattern>,
) {
    let is_block_comment = token.text().starts_with("/*");
    let block = model.block_for(&token.clone().into(), BlockPosition::Before);
    if !is_block_comment {
        single_line_comment_indent(token, model, anchor_set);
        return;
    }

    let (old_indent, new_indent) =
        match (indent_level(block.original_text()), indent_level(block.text())) {
            (Some(old), Some(new)) => (old, new),
            _ => return,
        };
    if old_indent == new_indent {
        return;
    }
    let mut curr_offset = token.text_range().start();
    let mut first = true;
    for line in token.text().lines() {
        let offset = curr_offset;
        curr_offset += TextUnit::of_str(line) + TextUnit::of_char('\n');
        if first {
            first = false;
            continue;
        }

        if let Some(ws_end) = line.find(|it| it != ' ') {
            if let Some(to_add) = new_indent.checked_sub(old_indent) {
                let indent = IndentLevel::from_len(TextUnit::from_usize(to_add));
                model.raw_edit(AtomEdit {
                    delete: TextRange::offset_len(offset, 0.into()),
                    insert: indent.into(),
                })
            } else {
                model.raw_edit(AtomEdit {
                    delete: TextRange::offset_len(
                        offset,
                        TextUnit::from_usize(min(ws_end, old_indent - new_indent)),
                    ),
                    insert: "".into(),
                })
            }
        }
    }

    fn indent_level(text: &str) -> Option<usize> {
        text.rfind('\n').map(|idx| text.len() - idx - 1)
    }
}

fn fix_string_interpolation(
    node: &SyntaxNode,
    model: &mut FmtModel,
    anchor_set: &PatternSet<&Pattern>,
) {
    for element in walk_non_whitespace(node) {
        if element.parent().map(|it| it.text_range().start()) == Some(element.text_range().start())
        {
            continue;
        }
        string_interpol_indent(&element, model, anchor_set);
    }
}

/// For indented string like
///
/// ```nix
/// ''
///   hello
///     world
/// ''
/// ```
///
/// returns the ranges, corresponding to indentation. That is `"  "` before
/// hello, `"    "` before world and `""` before the last `''`.
fn node_indent_ranges(indented_string: &SyntaxNode) -> impl Iterator<Item = TextRange> {
    indented_string
        .children_with_tokens()
        .filter_map(|it| it.into_token())
        .filter(|it| it.kind() == TOKEN_STRING_CONTENT)
        .flat_map(|string_bit| {
            let start_offset = string_bit.text_range().start();
            string_indent_ranges(string_bit.text())
                .into_iter()
                .map(move |range| range + start_offset)
        })
}

fn string_indent_ranges(mut s: &str) -> Vec<TextRange> {
    let mut offset = 0;
    std::iter::from_fn(move || loop {
        let indent_start = s.find('\n')? + 1;
        s = &s[indent_start..];
        offset += indent_start;

        let indent_len = s.find(|c| c != ' ').unwrap_or_else(|| s.len());
        s = &s[indent_len..];
        offset += indent_len;
        if s.starts_with('\n') {
            continue;
        }

        return Some(TextRange::from_to(
            TextUnit::from_usize(offset - indent_len),
            TextUnit::from_usize(offset),
        ));
    })
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_indent_ranges() {
        let text = r#"{
  python =
    ''
    for i in range(10):
        print(i)
    '';
}"#;

        let ast = rnix::parse(text);
        let node = crate::tree_utils::walk(&ast.node())
            .filter_map(|it| it.into_node())
            .find(|node| node.kind() == NODE_STRING)
            .unwrap();
        let indent_ranges: Vec<TextRange> = node_indent_ranges(&node).collect();
        assert_eq!(
            indent_ranges,
            vec![
                TextRange::from_to(20.into(), 24.into()),
                TextRange::from_to(44.into(), 52.into()),
                TextRange::from_to(61.into(), 65.into()),
            ]
        );

        let text = r#"{
  python =
    ''python
    for i in range(${range}):
        print(i)
    '';
}"#;

        let ast = rnix::parse(text);
        let node = crate::tree_utils::walk(&ast.node())
            .filter_map(|it| it.into_node())
            .find(|node| node.kind() == NODE_STRING)
            .unwrap();
        let indent_ranges: Vec<TextRange> = node_indent_ranges(&node).collect();
        assert_eq!(
            indent_ranges,
            vec![
                TextRange::from_to(26.into(), 30.into()),
                TextRange::from_to(56.into(), 64.into()),
                TextRange::from_to(73.into(), 77.into()),
            ]
        );
    }
}
