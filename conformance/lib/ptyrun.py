#!/usr/bin/env python3
"""Run a command on a real pty so a TUI client behaves exactly as it does for a
user, while this process's own stdin/stdout stay ordinary pipes.

usage: ptyrun.py <output-log> <cmd> [args...]
Bytes written to this process's stdin are forwarded to the pty (keystrokes).
Everything the child writes is appended to <output-log> and mirrored to stdout.
"""
import fcntl, os, select, signal, struct, subprocess, sys, termios

log_path = sys.argv[1]
cmd = sys.argv[2:]

master, slave = os.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 45, 140, 0, 0))
proc = subprocess.Popen(cmd, stdin=slave, stdout=slave, stderr=slave, close_fds=True, start_new_session=True)
os.close(slave)

sys.stdout.write("PTY_PID=%d\n" % proc.pid)
sys.stdout.flush()

log = open(log_path, "wb", buffering=0)
stdin_fd = sys.stdin.fileno()
open_fds = [master, stdin_fd]
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
        r, _, _ = select.select(open_fds, [], [], 0.2)
        for fd in r:
            if fd == master:
                try:
                    data = os.read(master, 65536)
                except OSError:
                    data = b""
                if not data:
                    open_fds = [f for f in open_fds if f != master]
                    continue
                log.write(data)
            else:
                try:
                    data = os.read(stdin_fd, 65536)
                except OSError:
                    data = b""
                if not data:
                    open_fds = [f for f in open_fds if f != stdin_fd]
                    continue
                os.write(master, data)
finally:
    log.close()
    try:
        os.close(master)
    except OSError:
        pass
sys.stdout.write("PTY_EXIT=%s\n" % proc.returncode)
sys.stdout.flush()
