#!/usr/bin/env python3
"""Real PTY + Unix socket integration test; no SSH server or Agent subscription needed."""
import json
import os
from pathlib import Path
import pty
import select
import shlex
import socket
import subprocess
import sys
import tempfile
import threading
import time

BINARY = str(Path(sys.argv[1] if len(sys.argv) > 1 else "target/debug/agentdrop").resolve())
START, END = b"\x1b[200~", b"\x1b[201~"


def until(predicate, timeout=8):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(0.02)
    raise AssertionError("timed out waiting for integration test condition")


class Terminal:
    def __init__(self, command, env):
        self.pid, self.fd = pty.fork()
        self.output = b""
        if self.pid == 0:
            os.execve(command[0], command, env)

    def drain(self):
        while select.select([self.fd], [], [], 0)[0]:
            try:
                data = os.read(self.fd, 65536)
            except OSError:
                break
            if not data:
                break
            self.output += data
        return self.output

    def ready(self):
        until(lambda: b"AGENT_READY" in self.drain())

    def send(self, data):
        os.write(self.fd, data)

    def close(self):
        try:
            os.kill(self.pid, 15)
        except ProcessLookupError:
            pass
        os.close(self.fd)
        os.waitpid(self.pid, 0)


class Bridge:
    def __init__(self, root, label):
        self.path = str(root / f"{label}.sock")
        self.token = ("a" if label == "one" else "b") * 64
        self.requests = []
        self.errors = []
        self.listener = socket.socket(socket.AF_UNIX)
        self.listener.bind(self.path)
        self.listener.listen()
        self.listener.settimeout(0.2)
        self.stopped = False
        self.thread = threading.Thread(target=self.serve)
        self.thread.start()

    def serve(self):
        while not self.stopped:
            try:
                conn, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                return
            with conn:
                try:
                    conn.settimeout(5)
                    request = json.loads(conn.makefile("rb").readline())
                    assert request["token"] == self.token
                    assert request["version"] == 3
                    self.requests.append(request)
                    if request["op"] == "clipboard_image":
                        response, data = {"status": "no_clipboard_image"}, b""
                    else:
                        data = b"\x00image\xff\n"
                        response = {"status": "file", "name": "图片 ' one.png", "size": len(data)}
                        if "truncated" in request["path"]:
                            data = b"short"
                    conn.sendall(json.dumps({"version": 3, **response}).encode() + b"\n" + data)
                except Exception as error:
                    self.errors.append(error)

    @property
    def endpoint(self):
        return f"{self.path}:{self.token}"

    def close(self):
        self.stopped = True
        self.thread.join(timeout=2)
        self.listener.close()
        assert not self.errors, self.errors


def main():
    with tempfile.TemporaryDirectory(prefix="agentdrop-e2e-") as directory:
        root = Path(directory)
        output = root / "input.bin"
        agent = root / "agent.py"
        agent.write_text("""import os, sys, tty
from pathlib import Path
tty.setraw(0)
print('\\x1b[?2004hAGENT_READY', flush=True)
with open(sys.argv[1], 'ab', buffering=0) as f:
    while True:
        data = os.read(0, 4096)
        if not data: break
        f.write(data)
""")
        env = dict(os.environ, HOME=str(root), TERM="xterm-256color")
        env.pop("TMUX", None)
        env.pop("TMUX_PANE", None)
        one, two = Bridge(root, "one"), Bridge(root, "two")
        agent_args = [sys.executable, str(agent), str(output)]
        env["AGENTDROP_BRIDGE"] = one.endpoint
        terminal = Terminal([BINARY, "proxy", "--", *agent_args], env)
        try:
            terminal.ready()
            keys = b"abc\x01\x05\x12\x1b[A\x1b[B\x16"
            text = START + b"ordinary text with \x16 inside" + END
            terminal.send(keys + text)
            until(lambda: output.exists() and output.read_bytes() == keys + text)
            assert [r["op"] for r in one.requests] == ["clipboard_image"]
            drop = START + b'"C:\\a one.png" "D:\\b.png" ' + END
            terminal.send(drop)
            until(lambda: output.read_bytes().count(END) == 2)
            content = output.read_bytes().split(START)[-1].split(END)[0]
            paths = shlex.split(content.decode())
            assert len(paths) == 2 and paths[0] != paths[1]
            for path in paths:
                assert Path(path).read_bytes() == b"\x00image\xff\n"
                assert Path(path).stat().st_mode & 0o777 == 0o600
            assert [r["path"] for r in one.requests[1:]] == [r"C:\a one.png", r"D:\b.png"]
            before = output.read_bytes()
            failed = START + b"C:\\truncated.png " + END
            terminal.send(failed)
            until(lambda: output.read_bytes() == before + failed)
            assert len(list((root / ".cache/agentdrop/files").iterdir())) == 2
            print("PASS: PTY keys, plain paste, Ctrl-V fallback, multi-file upload, private files, truncated transfer cleanup")
        finally:
            terminal.close()

        if not __import__('shutil').which("tmux"):
            raise RuntimeError("tmux is required for the reconnect integration test")
        # Dedicated tmux server: wrapper isolates tests from the user's sessions.
        tools = root / "bin"
        tools.mkdir()
        real_tmux = __import__('shutil').which("tmux")
        wrapper = tools / "tmux"
        wrapper.write_text(f"#!/bin/sh\nexec {shlex.quote(real_tmux)} -S {shlex.quote(str(root / 'tmux.sock'))} \"$@\"\n")
        wrapper.chmod(0o755)
        env["PATH"] = str(tools) + os.pathsep + env["PATH"]
        env["AGENTDROP_BRIDGE"] = one.endpoint
        terminal = Terminal([BINARY, "attach", "--session", "coding", "--", *agent_args], env)
        try:
            terminal.ready()
            first_count = len(one.requests)
            terminal.send(b"\x16")
            until(lambda: len(one.requests) == first_count + 1)
            # A second attach through agentdrop must refuse to take over the active client.
            refused = subprocess.run([BINARY, "attach", "--session", "coding", "--", *agent_args], env=env, capture_output=True)
            assert refused.returncode != 0 and b"already attached" in refused.stderr
            subprocess.run([str(wrapper), "detach-client", "-s", "coding"], env=env, check=True)
            terminal.close()
            env["AGENTDROP_BRIDGE"] = two.endpoint
            terminal = Terminal([BINARY, "attach", "--session", "coding", "--", *agent_args], env)
            terminal.ready()
            terminal.send(b"\x16")
            until(lambda: len(two.requests) == 1)
            assert len(one.requests) == first_count + 1
            print("PASS: real tmux initial binding, active-client refusal, reconnect routes running proxy to the new bridge")
        finally:
            subprocess.run([str(wrapper), "kill-server"], env=env, capture_output=True)
            terminal.close()
            one.close()
            two.close()


if __name__ == "__main__":
    main()
