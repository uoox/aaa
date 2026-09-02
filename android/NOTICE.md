# Third-party notice

## termlib (ConnectBot Terminal)

`org.connectbot:termlib` — Apache License 2.0, consumed as a prebuilt AAR from Maven Central
(https://github.com/connectbot/termlib). It bundles **libvterm** by Paul Evans (MIT License) as
`libjni_cb_term.so`. aaa-ui does not modify termlib; the software keyboard bridge
(`TermInput.kt`) and the mouse-reporting touch layer (`TerminalHost.kt`) are aaa-ui code that
sits beside the library's `Terminal()` composable.

## History

Until 2026-09-03 the Android client vendored Termux's `terminal-emulator` / `terminal-view`
modules (Apache-2.0). They were removed in favour of termlib; see git history for the vendored
sources and the transport-neutral `TerminalSession` reimplementation.
