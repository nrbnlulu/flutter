# multiple_windows

A reference application demonstrating multi-window support for Flutter using a
rich semantics windowing API.

## Running
To run the example application, you must:

1. Be on Flutter's main release channel
2. Provide the `--enable-windowing` flag to flutter

## Hebrew text editing playground

From the repository root, run `task run-rust-hebrew` to build and launch
`lib/hebrew_main.dart` on the Rust shell. This standalone screen starts in
Hebrew with Flutter's Material, Cupertino, and Widgets translations and RTL
layout. Toggle English to compare LTR behavior without clearing the text.
The task enables Fontconfig in the local host engine build, then rebuilds the
runner. Fontconfig provides character fallback from installed system fonts;
without it, the default directory font manager cannot choose a fallback font
for emoji or localized editing-menu labels. Install an emoji font such as
Noto Color Emoji if your system does not have one.

Try selecting mixed Hebrew/English text and emoji, Ctrl+A/C/X/V, undo/redo,
arrow-key selection, and right-clicking a selection for the standard Flutter
editing menu. A second field provides a paste target. Use your system keyboard
layout switcher to type Hebrew. The screen uses standard TextFields without
custom shortcut or context-menu handlers so it exercises framework and shell
behavior directly.
After editing the first field, its Undo and Redo buttons show whether Flutter
has an edit in its history. Compare those buttons with Ctrl+Z and Ctrl+Shift+Z.
