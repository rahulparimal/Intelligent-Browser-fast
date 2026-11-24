//! What each accessibility role means for a model.

/// How a role is treated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Class {
    /// Something to act on; the string is the short role shown to the model.
    Interactive(&'static str),
    /// A heading (level comes from the node's properties).
    Heading,
    /// Visible text.
    Text,
    /// An image: shown only when it has an accessible name.
    Image,
    /// Starts a new block of text: ends any text run in progress.
    Block,
    /// A label: descend (the labelled control may be inside it) but do not repeat its text, which is already the control's name.
    Label,
    /// Structure only: descend, emit nothing.
    Transparent,
    /// Nothing useful inside: skip the subtree.
    Drop,
}

pub(crate) fn classify(role: &str) -> Class {
    match role {
        "button" | "DisclosureTriangle" | "PopUpButtonMenuItem" => Class::Interactive("btn"),
        "link" => Class::Interactive("a"),
        "textbox" | "searchbox" | "TextField" | "textarea" => Class::Interactive("input"),
        "combobox" | "PopUpButton" | "ComboBoxSelect" => Class::Interactive("select"),
        "checkbox" => Class::Interactive("chk"),
        "radio" => Class::Interactive("radio"),
        "switch" => Class::Interactive("switch"),
        "slider" | "spinbutton" => Class::Interactive("range"),
        "tab" => Class::Interactive("tab"),
        "menuitem" | "menuitemcheckbox" | "menuitemradio" => Class::Interactive("menuitem"),
        "option" => Class::Interactive("option"),
        "treeitem" => Class::Interactive("treeitem"),
        "heading" => Class::Heading,
        "StaticText" => Class::Text,
        "image" | "img" | "graphics-document" => Class::Image,
        "paragraph" | "listitem" | "cell" | "gridcell" | "columnheader" | "rowheader" | "row" | "article"
        | "blockquote" | "caption" | "figcaption" | "main" | "navigation" | "banner" | "contentinfo"
        | "complementary" | "region" | "form" | "list" | "table" | "term" | "definition" | "note" | "dialog"
        | "alertdialog" | "alert" | "status" => Class::Block,
        "InlineTextBox" | "LineBreak" | "separator" | "scrollbar" => Class::Drop,
        "LabelText" => Class::Label,
        _ => Class::Transparent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_roles_are_classified() {
        assert_eq!(classify("button"), Class::Interactive("btn"));
        assert_eq!(classify("link"), Class::Interactive("a"));
        assert_eq!(classify("combobox"), Class::Interactive("select"));
        assert_eq!(classify("heading"), Class::Heading);
        assert_eq!(classify("StaticText"), Class::Text);
        assert_eq!(classify("paragraph"), Class::Block);
        assert_eq!(classify("generic"), Class::Transparent);
        assert_eq!(classify("InlineTextBox"), Class::Drop);
        assert_eq!(classify("LabelText"), Class::Label);
        assert_eq!(
            classify("listbox"),
            Class::Transparent,
            "a listbox is a container; its options are the interactive parts"
        );
        assert_eq!(classify("something-new"), Class::Transparent);
    }
}
