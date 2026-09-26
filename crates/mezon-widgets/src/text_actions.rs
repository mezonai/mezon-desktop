use gpui::{App, KeyBinding, actions};

pub const TEXT_INPUT_CONTEXT: &str = "MezonTextInput";

actions!(
    mezon_text_input,
    [
        Backspace,
        Delete,
        Enter,
        Newline,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectAll,
        Home,
        End,
        MoveToPreviousWordStart,
        MoveToNextWordEnd,
        SelectToPreviousWordStart,
        SelectToNextWordEnd,
        DeleteToPreviousWordStart,
        DeleteToNextWordEnd,
        SelectToLineStart,
        SelectToLineEnd,
        MoveToDocStart,
        MoveToDocEnd,
        SelectToDocStart,
        SelectToDocEnd,
        DeleteToLineStart,
        DeleteToLineEnd,
        Undo,
        Redo,
        ShowCharacterPalette,
        Paste,
        Cut,
        Copy,
    ]
);

pub fn text_input_bindings() -> Vec<KeyBinding> {
    let mut bindings = vec![
        KeyBinding::new("backspace", Backspace, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("delete", Delete, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("enter", Enter, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("shift-enter", Newline, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("left", Left, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("right", Right, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("up", Up, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("down", Down, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("shift-left", SelectLeft, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("shift-right", SelectRight, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("shift-up", SelectUp, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("shift-down", SelectDown, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("home", Home, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("end", End, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("shift-home", SelectToLineStart, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("shift-end", SelectToLineEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("secondary-a", SelectAll, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("secondary-v", Paste, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("secondary-c", Copy, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("secondary-x", Cut, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("secondary-z", Undo, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("secondary-shift-z", Redo, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new(
            "ctrl-cmd-space",
            ShowCharacterPalette,
            Some(TEXT_INPUT_CONTEXT),
        ),
    ];

    #[cfg(target_os = "macos")]
    bindings.extend([
        KeyBinding::new(
            "alt-left",
            MoveToPreviousWordStart,
            Some(TEXT_INPUT_CONTEXT),
        ),
        KeyBinding::new("alt-right", MoveToNextWordEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new(
            "alt-shift-left",
            SelectToPreviousWordStart,
            Some(TEXT_INPUT_CONTEXT),
        ),
        KeyBinding::new(
            "alt-shift-right",
            SelectToNextWordEnd,
            Some(TEXT_INPUT_CONTEXT),
        ),
        KeyBinding::new(
            "alt-backspace",
            DeleteToPreviousWordStart,
            Some(TEXT_INPUT_CONTEXT),
        ),
        KeyBinding::new("alt-delete", DeleteToNextWordEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-left", Home, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-right", End, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new(
            "cmd-shift-left",
            SelectToLineStart,
            Some(TEXT_INPUT_CONTEXT),
        ),
        KeyBinding::new("cmd-shift-right", SelectToLineEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-up", MoveToDocStart, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-down", MoveToDocEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-shift-up", SelectToDocStart, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-shift-down", SelectToDocEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("cmd-delete", DeleteToLineEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-a", Home, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-e", End, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-p", Up, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-n", Down, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-d", Delete, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-h", Backspace, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-k", DeleteToLineEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-shift-a", SelectToLineStart, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-shift-e", SelectToLineEnd, Some(TEXT_INPUT_CONTEXT)),
    ]);

    #[cfg(not(target_os = "macos"))]
    bindings.extend([
        KeyBinding::new(
            "ctrl-left",
            MoveToPreviousWordStart,
            Some(TEXT_INPUT_CONTEXT),
        ),
        KeyBinding::new("ctrl-right", MoveToNextWordEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new(
            "ctrl-shift-left",
            SelectToPreviousWordStart,
            Some(TEXT_INPUT_CONTEXT),
        ),
        KeyBinding::new(
            "ctrl-shift-right",
            SelectToNextWordEnd,
            Some(TEXT_INPUT_CONTEXT),
        ),
        KeyBinding::new(
            "ctrl-backspace",
            DeleteToPreviousWordStart,
            Some(TEXT_INPUT_CONTEXT),
        ),
        KeyBinding::new("ctrl-delete", DeleteToNextWordEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-home", MoveToDocStart, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-end", MoveToDocEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new(
            "ctrl-shift-home",
            SelectToDocStart,
            Some(TEXT_INPUT_CONTEXT),
        ),
        KeyBinding::new("ctrl-shift-end", SelectToDocEnd, Some(TEXT_INPUT_CONTEXT)),
        KeyBinding::new("ctrl-y", Redo, Some(TEXT_INPUT_CONTEXT)),
    ]);

    bindings
}

pub fn init(cx: &mut App) {
    cx.bind_keys(text_input_bindings());
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::KeyContext;
    use std::collections::HashMap;

    #[test]
    fn no_keystroke_is_bound_twice() {
        let mut seen: HashMap<String, String> = HashMap::new();
        for binding in text_input_bindings() {
            let keystroke = binding
                .keystrokes()
                .iter()
                .map(|key| format!("{key:?}"))
                .collect::<Vec<_>>()
                .join(" ");
            let action = binding.action().name().to_string();
            if let Some(previous) = seen.insert(keystroke.clone(), action.clone()) {
                panic!("{keystroke} bound to both {previous} and {action}");
            }
        }
    }

    #[test]
    fn every_binding_fires_only_inside_the_text_input_context() {
        let mut inside = KeyContext::default();
        inside.add(TEXT_INPUT_CONTEXT);
        let outside = KeyContext::default();

        for binding in text_input_bindings() {
            let name = binding.action().name().to_string();
            let predicate = binding
                .predicate()
                .unwrap_or_else(|| panic!("{name} must be scoped to a context"));
            assert!(
                predicate.eval(std::slice::from_ref(&inside)),
                "{name} must fire inside {TEXT_INPUT_CONTEXT}"
            );
            assert!(
                !predicate.eval(std::slice::from_ref(&outside)),
                "{name} must not fire outside {TEXT_INPUT_CONTEXT}"
            );
        }
    }
}
