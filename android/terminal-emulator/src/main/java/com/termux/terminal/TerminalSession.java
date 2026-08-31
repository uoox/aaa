package com.termux.terminal;

import android.os.Handler;
import android.os.Looper;
import java.nio.charset.StandardCharsets;
import java.util.UUID;

/**
 * A terminal session, consisting of a byte transport coupled to a terminal interface.
 *
 * aaa-ui patch: the upstream class couples the emulator to a local PTY through JNI
 * (createSubprocess / reader + writer threads / waiter thread). aaa-ui drives the emulator
 * from a remote WebSocket instead, so the process/JNI lifecycle is removed and replaced by
 * {@link #receive(byte[])} (remote PTY output enters here) and an abstract-by-default
 * {@link #write(byte[], int, int)} that subclasses forward to the remote PTY. Everything the
 * view layer observes — emulator callbacks (title/bell/clipboard/colors), main-thread
 * dispatch of screen updates, writeCodePoint UTF-8 encoding, running/exit-status semantics —
 * mirrors the upstream implementation (see NOTICE.md for the vendored commit).
 */
public class TerminalSession extends TerminalOutput {

    public final String mHandle = UUID.randomUUID().toString();
    protected TerminalEmulator mEmulator;
    protected TerminalSessionClient mClient;
    /** aaa-ui patch: upstream uses the real shell pid; remote sessions use 1 while attached. */
    protected int mShellPid = 0;
    protected int mShellExitStatus = 0;
    protected final Integer mTranscriptRows;
    /** Main-thread handler; null on the JVM (unit tests), in which case dispatch is synchronous. */
    private final Handler mMainThreadHandler = makeMainThreadHandler();
    /** Buffer for a single UTF-8 code point (max 4 bytes) plus an optional prefixed escape (27). */
    private final byte[] mUtf8InputBuffer = new byte[5];

    public TerminalSession(String shellPath, String cwd, String[] args, String[] env, Integer transcriptRows, TerminalSessionClient client) {
        mTranscriptRows = transcriptRows;
        mClient = client;
    }

    private static Handler makeMainThreadHandler() {
        try {
            return new Handler(Looper.getMainLooper());
        } catch (Throwable t) {
            return null; // aaa-ui patch: JVM unit tests have no Android Looper; run synchronously.
        }
    }

    public void updateTerminalSessionClient(TerminalSessionClient client) {
        mClient = client;
        if (mEmulator != null) mEmulator.updateTerminalSessionClient(client);
    }

    /** Inform the attached pty of changed size and reflect that in the terminal emulator. */
    public void updateSize(int columns, int rows, int cellWidthPixels, int cellHeightPixels) {
        if (mEmulator == null) initializeEmulator(columns, rows, cellWidthPixels, cellHeightPixels);
        else mEmulator.resize(columns, rows, cellWidthPixels, cellHeightPixels);
    }

    /** The terminal title as set through escape sequences or null if none set. */
    public String getTitle() {
        return (mEmulator == null) ? null : mEmulator.getTitle();
    }

    /**
     * Set the terminal emulator's window size and start terminal emulation.
     * aaa-ui patch: no subprocess is spawned; the remote daemon owns the real PTY.
     */
    public void initializeEmulator(int columns, int rows, int cellWidthPixels, int cellHeightPixels) {
        mEmulator = new TerminalEmulator(this, columns, rows, cellWidthPixels, cellHeightPixels, mTranscriptRows, mClient);
        mShellPid = 1;
        if (mClient != null) mClient.setTerminalShellPid(this, mShellPid);
    }

    /** Write data to the remote pty. Overridden by transports; base implementation drops bytes. */
    @Override
    public void write(byte[] data, int offset, int count) { }

    /** Write the Unicode code point to the terminal encoded in UTF-8. */
    public void writeCodePoint(boolean prependEscape, int codePoint) {
        if (codePoint > 1114111 || (codePoint >= 0xD800 && codePoint <= 0xDFFF)) {
            // 1114111 (= 2**16 + 1024**2 - 1) is the highest code point, [0xD800,0xDFFF] is the surrogate range.
            throw new IllegalArgumentException("Invalid code point: " + codePoint);
        }
        int bufferPosition = 0;
        if (prependEscape) mUtf8InputBuffer[bufferPosition++] = 27;
        byte[] encoded = new String(Character.toChars(codePoint)).getBytes(StandardCharsets.UTF_8);
        System.arraycopy(encoded, 0, mUtf8InputBuffer, bufferPosition, encoded.length);
        write(mUtf8InputBuffer, 0, bufferPosition + encoded.length);
    }

    public TerminalEmulator getEmulator() {
        return mEmulator;
    }

    /** Notify the {@link #mClient} that the screen has changed. */
    protected void notifyScreenUpdate() {
        if (mClient != null) mClient.onTextChanged(this);
    }

    /**
     * aaa-ui patch: remote PTY output bytes enter the vendor emulator here (upstream reads them
     * from the subprocess fd instead). Bytes are copied before dispatch so callers may reuse
     * their buffer, and — like upstream's MSG_NEW_INPUT handling — the emulator is only ever
     * mutated on the main thread. On the JVM (unit tests) dispatch is synchronous.
     */
    public void receive(byte[] data, int offset, int count) {
        final byte[] copy = new byte[count];
        System.arraycopy(data, offset, copy, 0, count);
        Runnable feed = () -> {
            if (mEmulator == null) initializeEmulator(80, 24, 10, 20);
            mEmulator.append(copy, copy.length);
            notifyScreenUpdate();
        };
        if (mMainThreadHandler == null || Looper.myLooper() == Looper.getMainLooper()) feed.run();
        else mMainThreadHandler.post(feed);
    }

    public void receive(byte[] data) {
        receive(data, 0, data.length);
    }

    /** Reset state for terminal emulator state. */
    public void reset() {
        if (mEmulator != null) {
            mEmulator.reset();
            notifyScreenUpdate();
        }
    }

    /**
     * Finish this terminal session. aaa-ui patch: upstream sends SIGKILL to the local shell and a
     * waiter thread later reports the exit; remote transports call this when the daemon reports
     * the session process is gone. Idempotent, like upstream's cleanupResources path.
     */
    public void finishIfRunning() {
        finish(0);
    }

    /** aaa-ui patch: record the remote exit status and notify the client once. */
    public void finish(int exitStatus) {
        boolean wasRunning;
        synchronized (this) {
            wasRunning = mShellPid != -1;
            mShellPid = -1;
            mShellExitStatus = exitStatus;
        }
        if (wasRunning && mClient != null) {
            Runnable notify = () -> mClient.onSessionFinished(this);
            if (mMainThreadHandler == null || Looper.myLooper() == Looper.getMainLooper()) notify.run();
            else mMainThreadHandler.post(notify);
        }
    }

    public synchronized boolean isRunning() {
        return mShellPid != -1;
    }

    /** Only valid if not {@link #isRunning()}. */
    public synchronized int getExitStatus() {
        return mShellExitStatus;
    }

    @Override
    public void titleChanged(String oldTitle, String newTitle) {
        if (mClient != null) mClient.onTitleChanged(this);
    }

    @Override
    public void onCopyTextToClipboard(String text) {
        if (mClient != null) mClient.onCopyTextToClipboard(this, text);
    }

    @Override
    public void onPasteTextFromClipboard() {
        if (mClient != null) mClient.onPasteTextFromClipboard(this);
    }

    @Override
    public void onBell() {
        if (mClient != null) mClient.onBell(this);
    }

    @Override
    public void onColorsChanged() {
        if (mClient != null) mClient.onColorsChanged(this);
    }

    public int getPid() {
        return mShellPid;
    }

    /** aaa-ui patch: upstream inspects /proc/<pid>/cwd; meaningless for a remote session. */
    public String getCwd() {
        return null;
    }
}
