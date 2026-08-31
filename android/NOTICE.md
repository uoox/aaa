# Vendor notice

Source: https://github.com/termux/termux-app.git
Commit: `3b66f8799635a4dba4a206563048ff0e6792c487`

The `terminal-emulator` and `terminal-view` modules are copied from the Termux App repository and remain under the Apache License 2.0. Original license headers are retained in copied source files. aaa-ui removes the native JNI/NDK PTY implementation and provides a remote byte transport instead; local UI behavior and terminal emulation code are otherwise retained. Any aaa-ui changes are marked with `// aaa-ui patch`.

Deviation detail: `terminal-emulator/src/main/java/com/termux/terminal/TerminalSession.java` is a
transport-neutral reimplementation of the upstream class (the upstream file is inseparable from
JNI.createSubprocess + reader/writer/waiter threads). It preserves the upstream-observable surface —
TerminalOutput callbacks (title/bell/clipboard/colors), main-thread emulator dispatch, writeCodePoint
UTF-8 encoding, updateSize / running / exit-status semantics — and replaces the process plumbing with
`receive(byte[])` (remote PTY output in) and an overridable `write(byte[],int,int)` (user input out).
Behavior is locked in by `app/src/test/java/cc/uoox/aaaui/RemoteTerminalSessionTest.kt` plus the 145
upstream emulator tests, which are vendored unchanged and kept green.
