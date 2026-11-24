//! An accessibility tree as JSON, shaped as `egui_inspection_tools` writes one for `egui_mcp`.
//!
//! Roles are spelled as AccessKit spells them, and the states that format lacks
//! go by AccessKit's names, as kittest prints them. Its ids are left out.

use egui::accesskit::Role;
use kittest::{AccessKitNode, Queryable as _};
use serde::{Deserialize, Serialize};

use super::{By, TreeNode, snapshot::Unmatched};

/// A flag is omitted when false, which is the state of nearly every widget.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Widget {
    /// Omitted for a role of `Unknown`, which says nothing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    numeric_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    min_numeric_value: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    max_numeric_value: Option<f64>,
    /// `True`, `False` or `Mixed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    toggled: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    selected: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expanded: Option<bool>,
    #[serde(default, skip_serializing_if = "is_false")]
    focused: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    disabled: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    read_only: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    placeholder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    children: Vec<Self>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde passes a reference"
)]
fn is_false(flag: &bool) -> bool {
    !*flag
}

/// The widgets under `root`, or only the one `by` matches, with what is inside each.
/// Host code only: a tree can only be walked by the copy of AccessKit that built it.
///
/// # Errors
/// When `by` does not match one widget, in sight.
pub(crate) fn json(root: AccessKitNode<'_>, by: Option<&By>) -> Result<String, Unmatched> {
    let Some(by) = by else {
        return Ok(written(&widgets(&root)));
    };
    let root = TreeNode { node: root };
    let found: Vec<_> = root.query_all(by.kittest()).collect();
    match found.as_slice() {
        [one] if one.node.is_hidden() => Err(Unmatched::Hidden),
        // Never passed over: the check is of it.
        [one] => Ok(written(&[widget(&one.node)])),
        [] => Err(Unmatched::Nothing),
        several => Err(Unmatched::Several(several.len())),
    }
}

/// `json` as this writes it, so that a reference compares alike
/// however a formatter or a checkout has laid it out.
///
/// # Errors
/// When it is not such a tree.
pub(crate) fn rewritten(json: &[u8]) -> Result<String, String> {
    let json = json.strip_prefix(b"\xef\xbb\xbf").unwrap_or(json);
    serde_json::from_slice::<Vec<Widget>>(json)
        .map(|widgets| written(&widgets))
        .map_err(|e| e.to_string())
}

fn written(widgets: &[Widget]) -> String {
    let mut json = serde_json::to_string_pretty(widgets).expect("BUG: a tree has no odd keys");
    json.push('\n');
    json
}

/// `node` as a widget, or the widgets inside it when it says nothing itself:
/// egui makes such a node for every `Ui`, and eight to resize a window by.
fn widgets(node: &AccessKitNode<'_>) -> Vec<Widget> {
    if node.is_hidden() {
        return Vec::new();
    }
    let widget = widget(node);
    let passed_over = match node.role() {
        Role::TextRun => true,
        Role::Window => node.is_root(),
        Role::GenericContainer | Role::Unknown | Role::Splitter | Role::ScrollBar => {
            widget.says_nothing()
        }
        _ => false,
    };
    if passed_over {
        widget.children
    } else {
        vec![widget]
    }
}

fn widget(node: &AccessKitNode<'_>) -> Widget {
    let data = node.data();
    let role = node.role();
    let finite = |number: Option<f64>| number.filter(|it| it.is_finite()).map(shortest);
    Widget {
        role: (role != Role::Unknown).then(|| format!("{role:?}")),
        label: node.label(),
        value: node.value(),
        numeric_value: finite(node.numeric_value()),
        min_numeric_value: finite(node.min_numeric_value()),
        max_numeric_value: finite(node.max_numeric_value()),
        toggled: node.toggled().map(|toggled| format!("{toggled:?}")),
        selected: node.is_selected(),
        expanded: data.is_expanded(),
        focused: node.is_focused(),
        disabled: node.is_disabled(),
        read_only: data.is_read_only(),
        required: node.is_required(),
        placeholder: said(node.placeholder()),
        description: said(node.description().as_deref()),
        children: node.children().flat_map(|child| widgets(&child)).collect(),
    }
}

impl Widget {
    /// Nothing beyond its role and what is inside it.
    /// Being disabled is left to the widgets inside, which are disabled with it.
    fn says_nothing(&self) -> bool {
        matches!(
            self,
            Self {
                role: _,
                label: None,
                value: None,
                numeric_value: None,
                min_numeric_value: None,
                max_numeric_value: None,
                toggled: None,
                selected: None,
                expanded: None,
                focused: false,
                disabled: _,
                read_only: false,
                required: false,
                placeholder: None,
                description: None,
                children: _,
            }
        )
    }
}

fn said(text: Option<&str>) -> Option<String> {
    text.filter(|text| !text.is_empty()).map(str::to_owned)
}

/// `number` without the digits an `f32` gains by becoming an `f64`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "kept only when it loses nothing"
)]
fn shortest(number: f64) -> f64 {
    let narrow = number as f32;
    if f64::from(narrow) == number {
        narrow.to_string().parse().unwrap_or(number)
    } else {
        number
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_reads_as_short_as_it_was_written_and_reads_back_the_same() {
        assert_eq!(shortest(f64::from(0.1_f32)), 0.1);
        assert_eq!(shortest(16_777_217.0), 16_777_217.0);
        for number in [1.0 / 3.0, 5e-324, 1.797_693_134_862_315_7e308, -0.000_1] {
            let json = format!("[{{\"numeric_value\":{number}}}]");
            let once = rewritten(json.as_bytes()).expect("a tree");
            assert_eq!(
                rewritten(once.as_bytes()).as_deref(),
                Ok(once.as_str()),
                "{number}"
            );
            assert!(
                once.contains(&serde_json::json!(number).to_string()),
                "{once}"
            );
        }
    }

    #[test]
    fn a_byte_order_mark_is_no_part_of_a_tree() {
        assert_eq!(rewritten("\u{feff}[]".as_bytes()).as_deref(), Ok("[]\n"));
    }

    #[test]
    fn what_is_no_tree_is_refused() {
        for json in [
            "{}",
            "[{\"rôle\": \"Button\"}]",
            "[] []",
            "version https://git-lfs",
        ] {
            assert!(rewritten(json.as_bytes()).is_err(), "{json}");
        }
    }
}
