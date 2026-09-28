#!/usr/bin/env python3
"""End-to-end driver for zedcomp-helper serve mode (LSP + HTTP intake)."""
import json
import os
import select
import socket
import subprocess
import sys
import time
import urllib.request

BIN = os.path.abspath(sys.argv[1])
ROOT = os.path.abspath(sys.argv[2])            # scratch dir
BASE_PORT = int(sys.argv[3])                   # first port to use
FIXTURES = os.path.abspath(sys.argv[4])

failures = []


def check(cond, label, detail=""):
    if cond:
        print(f"  ok   {label}")
    else:
        print(f"  FAIL {label} {detail}")
        failures.append(f"{label} {detail}")


def read_message(proc, timeout=8.0):
    deadline = time.time() + timeout
    header = b""
    while b"\r\n\r\n" not in header:
        remaining = deadline - time.time()
        if remaining <= 0:
            return None
        ready, _, _ = select.select([proc.stdout], [], [], remaining)
        if not ready:
            return None
        chunk = proc.stdout.read(1)
        if not chunk:
            return None
        header += chunk
    length = 0
    for line in header.split(b"\r\n"):
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1].strip())
    body = proc.stdout.read(length)
    return json.loads(body.decode())


def send(proc, message):
    body = json.dumps(message).encode()
    proc.stdin.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    proc.stdin.flush()


def start(port, workspace=None, extra_env=None):
    env = dict(os.environ)
    env["ZEDCOMP_PORT"] = str(port)
    env["ZEDCOMP_NO_OPEN"] = "1"
    env["RUST_BACKTRACE"] = "1"
    if workspace is None:
        env.pop("ZEDCOMP_WORKSPACE", None)
    else:
        env["ZEDCOMP_WORKSPACE"] = workspace
    env.update(extra_env or {})
    return subprocess.Popen(
        [BIN, "serve"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        env=env,
        bufsize=0,  # unbuffered: select() must see raw pipe readiness
    )


def handshake(proc, root_uri, options):
    send(proc, {"jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"rootUri": root_uri, "initializationOptions": options}})
    reply = read_message(proc)
    assert reply is not None, "no initialize response"
    check(reply.get("id") == 1, "initialize response carries id 1")
    check(reply.get("result") == {"capabilities": {}},
          "initialize capabilities == {}", json.dumps(reply.get("result")))
    send(proc, {"jsonrpc": "2.0", "method": "initialized", "params": {}})
    ready = read_message(proc)
    assert ready is not None, "no window/logMessage after initialized"
    check(ready.get("method") == "window/logMessage", "initialized -> window/logMessage")
    return ready["params"].get("message", "")


def post(port, payload_path, timeout=10, attempts=40):
    """POST the payload; the listener may need up to ~250 ms to rebind."""
    with open(payload_path, "rb") as handle:
        body = handle.read()
    last_error = None
    for _ in range(attempts):
        request = urllib.request.Request(
            f"http://127.0.0.1:{port}/", data=body,
            headers={"Content-Type": "application/json"}, method="POST")
        try:
            with urllib.request.urlopen(request, timeout=timeout) as response:
                return response.status, response.read()
        except Exception as err:  # noqa: BLE001
            last_error = err
            time.sleep(0.1)
    raise last_error


def close(proc, expect_code=0):
    try:
        send(proc, {"jsonrpc": "2.0", "id": 99, "method": "shutdown", "params": None})
        reply = None
        deadline = time.time() + 8
        while time.time() < deadline:
            candidate = read_message(proc, deadline - time.time())
            if candidate is None:
                break
            if candidate.get("id") == 99:  # skip interleaved notifications
                reply = candidate
                break
        check(reply is not None and reply.get("id") == 99, "shutdown answered")
        send(proc, {"jsonrpc": "2.0", "method": "exit", "params": None})
        code = proc.wait(timeout=10)
    except Exception as err:  # noqa: BLE001
        proc.kill()
        code = proc.wait()
        check(False, "clean shutdown", str(err))
    check(code == expect_code, f"exit code {expect_code}", f"got {code}")
    stderr = proc.stderr.read().decode(errors="replace")
    proc.stderr.close()
    proc.stdout.close()
    proc.stdin.close()
    return stderr


def read_until_log(proc, timeout=8.0):
    deadline = time.time() + timeout
    while time.time() < deadline:
        message = read_message(proc, deadline - time.time())
        if message is None:
            return None
        if message.get("method") == "window/logMessage":
            return message["params"].get("message", "")
    return None


def free_port(start_port):
    port = start_port
    while port < start_port + 200:
        with socket.socket() as probe:
            try:
                probe.bind(("127.0.0.1", port))
                return port
            except OSError:
                port += 1
    raise RuntimeError("no free port")


# ---------------------------------------------------------------- scenario A
print("scenario A: env workspace wins, port override rebinds, cf fixture")
port_a = free_port(BASE_PORT)
port_b = free_port(port_a + 1)
ws_a = os.path.join(ROOT, "ws-a")
# Empty config dir: no template.cpp, so the default (empty) main.cpp applies even
# when the developer running this script has a real ~/.config/zedcomp/template.cpp.
config_a = os.path.join(ROOT, "config-a")
os.makedirs(ws_a, exist_ok=True)
os.makedirs(config_a, exist_ok=True)
proc = start(port_a, workspace=ws_a, extra_env={"ZEDCOMP_CONFIG_DIR": config_a})
ready = handshake(proc, "file:///nonexistent/should-be-ignored", {"port": port_b})
check(f"127.0.0.1:{port_b}" in ready, "logMessage reports override port", ready)
check(ws_a in ready, "logMessage reports env workspace", ready)
check("main.cpp is written empty" in ready, "logMessage reports the empty default", ready)

status, body = post(port_b, os.path.join(FIXTURES, "cf.json"))
check(status == 200 and body == b"", "POST -> 200 with empty body", f"{status} {body!r}")
problem_dir = os.path.join(ws_a, "cf", "118", "A")
check(os.path.isdir(problem_dir), "cf/118/A created", problem_dir)
for name in ("main.cpp", "in1", "ans1", "in2", "ans2", "in3", "ans3", "problem.json"):
    check(os.path.isfile(os.path.join(problem_dir, name)), f"{name} written")
with open(os.path.join(problem_dir, "problem.json"), "rb") as handle:
    check(handle.read() == open(os.path.join(FIXTURES, "cf.json"), "rb").read(),
          "problem.json is the raw POST body")
# No template configured: main.cpp must be a 0-byte file (no banner/placeholder).
main_size = os.path.getsize(os.path.join(problem_dir, "main.cpp"))
check(main_size == 0, "default main.cpp is an empty 0-byte file", f"{main_size} bytes")
check(open(os.path.join(problem_dir, "in2")).read() == "Codeforces\n", "in2 content")
check(open(os.path.join(problem_dir, "ans3")).read() == ".b.c.b\n", "ans3 content")

log = read_until_log(proc)
check(log is not None and "cf/118/A" in log, "POST triggers window/logMessage", str(log))

try:
    post(port_a, os.path.join(FIXTURES, "cf.json"), timeout=3, attempts=1)
    check(False, "old port released after rebind")
except Exception:
    check(True, "old port released after rebind")

stderr = close(proc)
check("listening for Competitive Companion" in stderr, "stderr startup log", stderr[-200:])

# ---------------------------------------------------------------- scenario B
print("scenario B: rootUri percent-decoding, luogu fixture, two-level dir")
port_c = free_port(port_b + 1)
ws_b = os.path.join(ROOT, "ws b")   # space must survive percent-decoding
os.makedirs(ws_b, exist_ok=True)
root_uri = "file://" + ws_b.replace(" ", "%20")
proc = start(port_c)
handshake(proc, root_uri, {})
status, body = post(port_c, os.path.join(FIXTURES, "luogu.json"))
check(status == 200 and body == b"", "luogu POST -> 200 empty")
problem_dir = os.path.join(ws_b, "luogu", "P1001")
check(os.path.isdir(problem_dir), "rootUri(percent-encoded) -> luogu/P1001", problem_dir)
check(open(os.path.join(problem_dir, "ans2")).read() == "300\n", "luogu ans2 content")
close(proc)

# ---------------------------------------------------------------- scenario C
print("scenario C: occupied port degrades to LSP-only, no exit")
port_d = free_port(port_c + 1)
blocker = socket.socket()
blocker.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
blocker.bind(("127.0.0.1", port_d))
blocker.listen(1)
proc = start(port_d, workspace=ws_a)
handshake(proc, "file://" + ws_a, {})
check(proc.poll() is None, "helper still running despite port conflict")
send(proc, {"jsonrpc": "2.0", "id": 7, "method": "workspace/symbol", "params": {}})
reply = read_message(proc)
check(reply is not None and reply.get("id") == 7 and reply.get("result") is None,
      "unknown request answered with null result")
try:
    post(port_d, os.path.join(FIXTURES, "cf.json"), timeout=3, attempts=1)
    check(False, "occupied port serves nothing")
except Exception:
    check(True, "occupied port serves nothing")
stderr = close(proc)
check("cannot bind" in stderr, "stderr explains port conflict", stderr[-200:])
blocker.close()

# ---------------------------------------------------------------- scenario D
print("scenario D: stdin EOF terminates the helper")
port_e = free_port(port_d + 1)
proc = start(port_e, workspace=ws_a)
handshake(proc, "file://" + ws_a, {})
proc.stdin.close()
try:
    code = proc.wait(timeout=10)
except subprocess.TimeoutExpired:
    proc.kill()
    code = -1
check(code == 0, "EOF on stdin exits cleanly", f"got {code}")
proc.stdout.close()
proc.stderr.close()

# ---------------------------------------------------------------- scenario E
print("scenario E: Expect: 100-continue (raw socket, body sent after interim reply)")
port_f = free_port(port_e + 1)
proc = start(port_f, workspace=ws_a)
handshake(proc, "file://" + ws_a, {})
with open(os.path.join(FIXTURES, "cf.json"), "rb") as handle:
    payload = handle.read()
sock = socket.create_connection(("127.0.0.1", port_f), timeout=5)
headers = (b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nExpect: 100-continue\r\n"
           b"Content-Type: application/json\r\nContent-Length: %d\r\n\r\n" % len(payload))
sock.sendall(headers)
interim = sock.recv(4096)
check(interim.startswith(b"HTTP/1.1 100 Continue"), "interim 100 Continue", interim[:60])
# Trickle the body so a non-blocking accepted socket (macOS inherits O_NONBLOCK
# from the listener) would fail here with EAGAIN instead of waiting.
for offset in range(0, len(payload), 16):
    sock.sendall(payload[offset:offset + 16])
    time.sleep(0.01)
final = b""
while b"\r\n\r\n" not in final:
    chunk = sock.recv(4096)
    if not chunk:
        break
    final += chunk
check(final.startswith(b"HTTP/1.1 200 OK"), "200 after deferred body", final[:60])
sock.close()
close(proc)

# ---------------------------------------------------------------- scenario F
print("scenario F: initializationOptions.templatePath renders main.cpp")
port_g = free_port(port_f + 1)
ws_f = os.path.join(ROOT, "ws-f")
config_dir = os.path.join(ROOT, "config-f")
os.makedirs(ws_f, exist_ok=True)
os.makedirs(config_dir, exist_ok=True)
# Level 3 of the template priority: a config-dir template that must lose.
with open(os.path.join(config_dir, "template.cpp"), "w") as handle:
    handle.write("// CONFIG-DIR TEMPLATE (must lose)\n")
custom_path = os.path.join(ROOT, "custom-template.cpp")
with open(custom_path, "w") as handle:
    handle.write("// CUSTOM {{OJ}}/{{CONTEST}}/{{PROBLEM_ID}}\n"
                 "// {{PROBLEM_NAME}}\n"
                 "// {{URL}}\n"
                 "#include <bits/stdc++.h>\n")
proc = start(port_g, workspace=ws_f, extra_env={"ZEDCOMP_CONFIG_DIR": config_dir})
ready = handshake(proc, "file://" + ws_f, {"templatePath": custom_path})
check(custom_path in ready, "logMessage reports the template source", ready)
status, body = post(port_g, os.path.join(FIXTURES, "cf.json"))
check(status == 200 and body == b"", "custom-template POST -> 200 empty", f"{status} {body!r}")
custom_main = os.path.join(ws_f, "cf", "118", "A", "main.cpp")
check(os.path.isfile(custom_main), "custom-template main.cpp written", custom_main)
with open(os.path.join(FIXTURES, "cf.json")) as handle:
    fixture = json.load(handle)
main_cpp = open(custom_main).read()
check(main_cpp.startswith("// CUSTOM cf/118/A\n"), "templatePath beats the config dir",
      repr(main_cpp[:60]))
check(f"// {fixture['name']}\n" in main_cpp, "{{PROBLEM_NAME}} substituted")
check(f"// {fixture['url']}\n" in main_cpp, "{{URL}} substituted")
check("CONFIG-DIR TEMPLATE" not in main_cpp, "config-dir template not used")
close(proc)

# ---------------------------------------------------------------- scenario G
print("scenario G: $ZEDCOMP_CONFIG_DIR/template.cpp applies without any options")
port_h = free_port(port_g + 1)
ws_g = os.path.join(ROOT, "ws-g")
config_g = os.path.join(ROOT, "config-g")
os.makedirs(ws_g, exist_ok=True)
os.makedirs(config_g, exist_ok=True)
with open(os.path.join(config_g, "template.cpp"), "w") as handle:
    handle.write("// CONFIG-DIR {{OJ}}/{{PROBLEM_ID}}\n")
proc = start(port_h, workspace=ws_g, extra_env={"ZEDCOMP_CONFIG_DIR": config_g})
ready = handshake(proc, "file://" + ws_g, {})
check(config_g in ready, "logMessage reports the config-dir template", ready)
status, body = post(port_h, os.path.join(FIXTURES, "cf.json"))
check(status == 200 and body == b"", "config-dir-template POST -> 200 empty")
with open(os.path.join(ws_g, "cf", "118", "A", "main.cpp")) as handle:
    main_cpp = handle.read()
check(main_cpp == "// CONFIG-DIR cf/A\n", "config-dir template rendered", repr(main_cpp))
close(proc)

print()
if failures:
    print(f"{len(failures)} FAILURE(S):")
    for failure in failures:
        print(" -", failure)
    sys.exit(1)
print("all end-to-end checks passed")
