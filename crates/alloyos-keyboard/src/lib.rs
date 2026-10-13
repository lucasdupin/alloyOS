//! Reusable soft keyboard: layout, gestures, slide policy, key actions, and
//! liquid displacement. Hosts own the editor, focus, window, and presentation.
#![no_std]
extern crate alloc;

mod motion;
pub use motion::LiquidDisplacement;

/// Standalone preview component, independent of Ferrofluid System UI.
#[cfg(feature = "preview")]
pub mod preview {
    slint::include_modules!();
}

/// Typed actions at the keyboard/editor boundary.
#[derive(Debug, PartialEq)]
pub enum SoftKey {
    Text(slint::SharedString),
    Backspace,
    Submit,
}

impl From<slint::SharedString> for SoftKey {
    fn from(value: slint::SharedString) -> Self {
        match value.as_str() {
            "backspace" => Self::Backspace,
            "return" => Self::Submit,
            _ => Self::Text(value),
        }
    }
}

impl SoftKey {
    /// Feed the focused Slint editor via a pressed/released event pair.
    pub fn into_slint_text(self) -> slint::SharedString {
        match self {
            Self::Text(text) => text,
            Self::Backspace => slint::platform::Key::Backspace.into(),
            Self::Submit => slint::platform::Key::Return.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soft_keys_preserve_text_and_translate_editor_actions() {
        assert_eq!(
            SoftKey::from(slint::SharedString::from("é")),
            SoftKey::Text("é".into())
        );
        assert_eq!(
            SoftKey::from(slint::SharedString::from(" "))
                .into_slint_text()
                .as_str(),
            " "
        );
        assert_eq!(
            SoftKey::from(slint::SharedString::from("backspace")).into_slint_text(),
            slint::SharedString::from(slint::platform::Key::Backspace)
        );
        assert_eq!(
            SoftKey::from(slint::SharedString::from("return")).into_slint_text(),
            slint::SharedString::from(slint::platform::Key::Return)
        );
    }
}
