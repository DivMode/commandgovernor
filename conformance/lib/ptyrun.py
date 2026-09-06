#!/usr/bin/env python3
"""Run a command on a real pty so a TUI client behaves exactly as it does for a
user, while this process's own stdin/stdout stay ordinary pipes.

usage: ptyrun.py <output-log> <cmd> [args...]
Bytes written to this process's stdin are forwarded to the pty (keystrokes).
Everything the child writes is appended to <output-log> and mirrored to stdout.

Keystrokes are BUFFERED and written in small chunks, only when the master says
it is writable, and the write's return value decides how far the buffer
advanced. A single large os.write() deadlocks, and the deadlock is silent: a
raw-mode pty slave holds ~1 KB of input, so a write bigger than that blocks in
the master until the TUI drains it -- but this loop is the only thing draining
the TUI's OUTPUT, and it is stuck inside that write. Both sides then sleep
forever. Measured on this machine: 1022 bytes go through, 1023 blocks. A test
that pastes a paragraph therefore hangs until its own timeout, which looks
exactly like the product being slow.
"""
import fcntl, os, select, signal, struct, subprocess, sys, termios

log_path = sys.argv[1]
cmd = sys.argv[2:]

# Small enough to sit well inside the slave's input queue, so a chunk never
# waits on the TUI having drained the previous one.
CHUNK = 512

master, slave = os.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 140, 0, 0))
# Non-blocking master: with select() choosing the moment and os.write()
# reporting how much it took, a full input queue costs one loop iteration
# instead of parking this process inside the kernel.
fcntl.fcntl(master, fcntl.F_SETFL, fcntl.fcntl(master, fcntl.F_GETFL) | os.O_NONBLOCK)
proc = subprocess.Popen(cmd, stdin=slave, stdout=slave, stderr=slave, close_fds=True, start_new_session=True)
os.close(slave)

sys.stdout.write("PTY_PID=%d\n" % proc.pid)
sys.stdout.flush()

log = open(log_path, "wb", buffering=0)
stdin_fd = sys.stdin.fileno()
open_fds = [master, stdin_fd]
pending = b""  # keystrokes accepted from stdin but not yet taken by the pty
try:
    while True:
        if proc.poll() is not None:
            # drain whatever is left
            try:
                while True:
                    data = os.read(master, 65536)
                    if not data:
                        break
                    log.write(data)
            except OSError:
                pass
            break
        # The writers list is what parks this loop while the pty's input queue
        # is full; the returned set is unused, because the write below tries
        # regardless and reports for itself.
        r, _, _ = select.select(open_fds, [master] if pending else [], [], 0.2)
        for fd in r:
            if fd == master:
                try:
                    data = os.read(master, 65536)
                except BlockingIOError:
                    continue
                except OSError:
                    data = b""
                if not data:
                    open_fds = [f for f in open_fds if f != master]
                    continue
                log.write(data)
            else:
                try:
                    data = os.read(stdin_fd, 65536)
                except BlockingIOError:
                    continue
                except OSError:
                    data = b""
                if not data:
                    open_fds = [f for f in open_fds if f != stdin_fd]
                    continue
                pending += data
        # One chunk per pass, and only what the pty actually took: the next
        # pass reads the child's output before offering it any more input.
        #
        # Attempted every pass, not only when select() named the master
        # writable: `w` was decided before this pass read stdin, so waiting for
        # it would delay every keystroke by one select timeout. The master is
        # non-blocking, so an attempt that is too early costs a BlockingIOError
        # and the next select -- which does watch for writability -- parks us
        # until there is room, with no busy loop.
        if pending:
            try:
                written = os.write(master, pending[:CHUNK])
            except BlockingIOError:
                written = 0
            except OSError:
                pending = b""
                open_fds = [f for f in open_fds if f != master]
                continue
            pending = pending[written:]
finally:
    log.close()
    try:
        os.close(master)
    except OSError:
        pass
sys.stdout.write("PTY_EXIT=%s\n" % proc.returncode)
sys.stdout.flush()
