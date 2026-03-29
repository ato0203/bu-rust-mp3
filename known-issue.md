# Known Issues

## Fcitx5 Hangul search input in TUI

When using `fcitx5` with the Hangul input method, composed Korean text may not render inside the playlist search box in real time. The terminal itself can still accept Hangul normally, but this TUI may only receive committed key events and may not display IME preedit text while composition is in progress.

Observed behavior:
- Hangul input in a normal terminal prompt works.
- Hangul input in the app search box can appear laggy or blank while composing.
- Search interaction is affected more than plain terminal typing because the app reads terminal key events through `crossterm`.

Likely cause:
- This is likely a terminal/TUI IME composition limitation rather than a bug in `fcitx5-hangul` itself.
- `ratatui` + `crossterm` does not provide full toolkit-style IME preedit rendering.
- Behavior may vary by terminal emulator and input protocol support.

References:
- OpenAI Codex IME composition rendering issue: <https://github.com/openai/codex/issues/2718>
- Gemini CLI Korean IME issue: <https://github.com/google-gemini/gemini-cli/issues/3014>
- Crossterm input handling tracking issue: <https://github.com/crossterm-rs/crossterm/issues/685>
- Fcitx5 terminal/app-specific behavior report: <https://github.com/fcitx/fcitx5/issues/799>
- Fcitx5 setup notes for terminal IME integration: <https://www.fcitx-im.org/wiki/Setup_Fcitx_5>
- Kitty IME integration background: <https://github.com/kovidgoyal/kitty/issues/469>
