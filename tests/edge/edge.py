"""Edge-case suite for hiruzen: drives real shells in a pseudo-terminal and
checks what ends up on screen. Prints one PASS/FAIL line per case."""
import fcntl, os, pty, select, struct, subprocess, sys, termios, time
import pyte

H = "hiruzen"
results = []

class Term:
    def __init__(self, argv, cols=100, rows=24, cwd="/proj", env=None, boot=3.0):
        self.cols, self.rows = cols, rows
        e = dict(os.environ); e.update(env or {})
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(cwd); os.execvpe(argv[0], argv, e)
        self.size(cols, rows)
        self.screen = pyte.Screen(cols, rows); self.stream = pyte.ByteStream(self.screen)
        self.alive = True
        self.pump(boot)
    def size(self, cols, rows):
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    def resize(self, cols, rows):
        self.cols, self.rows = cols, rows; self.size(cols, rows); self.screen.resize(rows, cols); self.pump(0.8)
    def pump(self, seconds):
        end = time.time() + seconds
        while time.time() < end:
            r, _, _ = select.select([self.fd], [], [], 0.05)
            if not r: continue
            try: data = os.read(self.fd, 65536)
            except OSError: self.alive = False; return
            if not data: self.alive = False; return
            self.stream.feed(data)
            for _ in range(data.count(b"\x1b[6n")):
                os.write(self.fd, f"\x1b[{self.screen.cursor.y+1};{self.screen.cursor.x+1}R".encode())
    def send(self, data, wait=0.4):
        if isinstance(data, str): data = data.encode()
        try: os.write(self.fd, data)
        except OSError: self.alive = False
        self.pump(wait)
    def type(self, text, wait=0.5):
        for ch in text: self.send(ch, 0.04)
        self.pump(wait)
    def enter(self, wait=1.0): self.send("\r", wait)
    def text(self): return "\n".join(l.rstrip() for l in self.screen.display)
    def close(self):
        try:
            os.write(self.fd, b"\x03"); self.pump(0.2); os.write(self.fd, b"exit\r"); self.pump(0.6)
        except OSError: pass
        try: os.kill(self.pid, 9)
        except OSError: pass
        try: os.waitpid(self.pid, 0)
        except OSError: pass

# EDGE_ONLY=text runs only the cases whose name contains the text.
ONLY = os.environ.get("EDGE_ONLY", "")

def case(name):
    def wrap(fn):
        if ONLY and ONLY not in name:
            return
        try:
            detail = fn()
            results.append((name, detail is None or detail is True, "" if detail in (None, True) else str(detail)))
        except Exception as error:  # a crash in the case is a failure of the case
            results.append((name, False, f"{type(error).__name__}: {error}"))
        ok = results[-1][1]
        print(("PASS  " if ok else "FAIL  ") + name + ("" if ok else "  -> " + results[-1][2][:300]), flush=True)
    return wrap

def expect(condition, message):
    if not condition: raise AssertionError(message)

SHELLS = {"bash": 3, "zsh": 3, "fish": 4, "pwsh": 9}
def session(shell, **kw):
    kw.setdefault("boot", SHELLS[shell])
    return Term([H, "--shell", shell], **kw)
def responsive(t, shell):
    t.send("\x03", 0.6)
    t.type("echo al" + "ive-" + shell); t.enter(1.5 if shell != "pwsh" else 3)
    expect("alive-" + shell in t.text().replace("echo alive-" + shell, ""), "shell stopped responding:\n" + t.text()[-400:])

for shell in SHELLS:
    slow = 2.0 if shell == "pwsh" else 0.6

    @case(f"{shell}: dropdown appears and Tab accepts")
    def _():
        t = session(shell)
        try:
            t.type("git ch", slow); expect("cherry-pick" in t.text(), "no dropdown:\n" + t.text())
            t.send("\t", slow); expect("git checkout" in t.text(), "Tab did not accept:\n" + t.text())
            responsive(t, shell)
        finally: t.close()

    @case(f"{shell}: Down then Enter inserts, plain Enter runs")
    def _():
        t = session(shell)
        try:
            t.type("git ", slow); t.send("\x1b[B", 0.3); t.send("\x1b[B", 0.3); t.send("\r", slow)
            line = [l for l in t.text().splitlines() if "git " in l][-1]
            expect(len(line.split("git ")[-1].strip()) > 0, "Enter did not insert a suggestion: " + line)
            expect("usage: git" not in t.text(), "Enter ran git instead of inserting")
            t.send("\x03", 0.5); t.type("echo ran-plain"); t.enter(slow + 0.5)
            expect(t.text().count("ran-plain") >= 2, "plain Enter did not run the command")
        finally: t.close()

    @case(f"{shell}: exit status is recorded; a typo is recorded as not found")
    def _():
        cfg = f"/tmp/cfg-status-{shell}"
        t = session(shell, env={"HIRUZEN_CONFIG_DIR": cfg})
        try:
            for cmd in ["zzznotacommand arg", "git status --bogus-flag-xyz", "echo fine"]:
                t.type(cmd, 0.2); t.enter(slow + 0.8)
            t.type("echo flush", 0.2); t.enter(slow + 0.8)
        finally: t.close()
        rows = [l.split("\t") for l in open(cfg + "/history.tsv").read().splitlines()]
        status = {r[1]: (r[4] if len(r) > 4 else "?") for r in rows}
        expect(status.get("echo fine") == "0", f"success not recorded as 0: {status}")
        expect(status.get("git status --bogus-flag-xyz") not in ("0", "", "?", None), f"failure not recorded: {status}")
        if shell != "pwsh":
            expect(status.get("zzznotacommand arg") == "127", f"typo not recorded as 127: {status}")
        out = subprocess.run([H, "complete", "zzznot"], capture_output=True, text=True, cwd="/proj", env={**os.environ, "HIRUZEN_CONFIG_DIR": cfg}).stdout
        if shell != "pwsh":
            expect("zzznotacommand" not in out, "a mistyped command was learned")

    @case(f"{shell}: suggestions follow the project, not other folders")
    def _():
        cfg = f"/tmp/cfg-proj-{shell}"
        t = session(shell, env={"HIRUZEN_CONFIG_DIR": cfg})
        try:
            t.type("echo deploy-to staging-zz", 0.2); t.enter(slow + 0.8); t.type("echo x", 0.2); t.enter(slow + 0.8)
        finally: t.close()
        env = {**os.environ, "HIRUZEN_CONFIG_DIR": cfg}
        here = subprocess.run([H, "complete", "echo deploy-to "], capture_output=True, text=True, cwd="/proj/src", env=env).stdout
        there = subprocess.run([H, "complete", "echo deploy-to "], capture_output=True, text=True, cwd="/other", env=env).stdout
        expect("staging-zz" in here, "value not offered inside its project:\n" + here)
        expect("staging-zz" not in there, "value leaked into another project")

    @case(f"{shell}: Escape hides, Ctrl+C and empty Enter are harmless")
    def _():
        t = session(shell)
        try:
            t.type("git ch", slow); t.send("\x1b", slow)
            expect("cherry-pick" not in t.text(), "Escape did not hide the dropdown")
            t.send("\x03", 0.5); t.enter(0.5); t.enter(0.5); responsive(t, shell)
        finally: t.close()

    @case(f"{shell}: pasted text with tabs and newlines is not treated as keys")
    def _():
        t = session(shell)
        try:
            t.send("echo pasted\tword", slow)
            expect("pasted" in t.text(), "paste was swallowed:\n" + t.text()[-300:])
            responsive(t, shell)
        finally: t.close()

    @case(f"{shell}: pipes, sudo and variable assignments still complete")
    def _():
        t = session(shell)
        try:
            t.type("echo hi | git cherr", slow); expect("cherry-pick" in t.text(), "after a pipe:\n" + t.text()[-300:]); t.send("\x03", 0.5)
            t.type("sudo git cherr", slow); expect("cherry-pick" in t.text(), "after sudo"); t.send("\x03", 0.5)
            if shell in ("bash", "zsh"):
                t.type("FOO=1 git cherr", slow); expect("cherry-pick" in t.text(), "after an assignment"); t.send("\x03", 0.5)
            responsive(t, shell)
        finally: t.close()

    @case(f"{shell}: unbalanced quotes and odd characters do not break anything")
    def _():
        t = session(shell)
        try:
            for junk in ['git commit -m "unfinished', "echo 'half", "echo $(", "ls \\", "echo 日本語 テスト ", "git --=--= -- -"]:
                t.type(junk, 0.3); t.send("\x03", 0.5)
            responsive(t, shell)
        finally: t.close()

    @case(f"{shell}: file names with spaces are inserted so the shell reads one argument")
    def _():
        t = session(shell, cwd="/proj/spaced")
        try:
            t.type("cat my", slow); expect("my file.txt" in t.text(), "file not offered:\n" + t.text()[-300:])
            t.send("\t", slow); t.enter(slow + 0.8)
            shown = "\n".join(l for l in t.text().splitlines() if l.strip())
            expect("contents-of-spaced-file" in t.text(), "accepted name was not read as one file:\n" + shown)
        finally: t.close()

    @case(f"{shell}: very small and resized terminals do not crash it")
    def _():
        t = session(shell, cols=30, rows=6)
        try:
            t.type("git ch", slow); t.resize(12, 3); t.resize(120, 30); t.resize(40, 10)
            t.send("\x03", 0.5); t.resize(100, 24); responsive(t, shell)
        finally: t.close()

    @case(f"{shell}: a long command that wraps and scrolls keeps completing")
    def _():
        t = session(shell, cols=50, rows=8)
        try:
            for _ in range(8): t.type("echo fill", 0.1); t.enter(0.4 if shell != "pwsh" else 1.2)
            t.type("git commit --amend --no-edit --allow-empty --no-verify --quiet --sig", slow)
            expect("signoff" in t.text(), "no suggestion on a wrapped line:\n" + t.text())
            t.send("\x03", 0.5); responsive(t, shell)
        finally: t.close()

    @case(f"{shell}: screen clear, a full-screen program and history keys leave it working")
    def _():
        t = session(shell)
        try:
            t.send("\x0c", 0.6); t.type("git ch", slow); t.send("\x03", 0.5)
            t.type("less /proj/a.txt"); t.enter(1.0); t.send("q", 0.8)
            t.send("\x1b[A", 0.4); t.send("\x1b[A", 0.4); t.send("\x1b[B", 0.4); t.send("\x03", 0.5)
            t.type("git ch", slow); expect("cherry-pick" in t.text(), "no dropdown afterwards:\n" + t.text()[-400:])
            responsive(t, shell)
        finally: t.close()

    @case(f"{shell}: its working directory disappearing does not crash it")
    def _():
        t = session(shell, cwd="/proj")
        try:
            t.type("mkdir -p /tmp/gone-" + shell); t.enter(slow); t.type("cd /tmp/gone-" + shell); t.enter(slow)
            subprocess.run(["rm", "-rf", "/tmp/gone-" + shell])
            t.type("ls ", slow); t.send("\x03", 0.5); t.type("cat ./", slow); t.send("\x03", 0.5)
            t.type("cd /proj"); t.enter(slow); responsive(t, shell)
        finally: t.close()

    @case(f"{shell}: a burst of typing ends with the right text and a matching dropdown")
    def _():
        t = session(shell)
        try:
            t.send("git checkout fea", 1.5 if shell != "pwsh" else 3)
            expect("git checkout fea" in t.text(), "typed text is wrong:\n" + t.text()[-300:])
            expect("feature/login" in t.text(), "dropdown does not match the final text:\n" + t.text()[-400:])
        finally: t.close()

    @case(f"{shell}: the shell's exit status is passed on")
    def _():
        t = session(shell)
        t.type("exit 7", 0.2); t.send("\r", 2.5 if shell != "pwsh" else 5)
        _, status = os.waitpid(t.pid, 0)
        expect(os.WIFEXITED(status) and os.WEXITSTATUS(status) == 7, f"exit status was {status}")

@case("aliases and functions: bash alias completes as its command, function is offered")
def _():
    t = Term([H, "--shell", "bash"], env={"HOME": "/home-alias"})
    try:
        t.type("gco fea", 0.8); expect("feature/login" in t.text(), "alias not expanded:\n" + t.text()[-300:]); t.send("\x03", 0.5)
        t.type("mkc", 0.8); expect("mkcd" in t.text(), "function not offered:\n" + t.text()[-300:])
    finally: t.close()

@case("aliases and functions: zsh alias completes as its command, function is offered")
def _():
    t = Term([H, "--shell", "zsh"], env={"HOME": "/home-alias", "ZDOTDIR": "/home-alias"})
    try:
        t.type("gco fea", 0.8); expect("feature/login" in t.text(), "alias not expanded:\n" + t.text()[-300:]); t.send("\x03", 0.5)
        t.type("mkc", 0.8); expect("mkcd" in t.text(), "function not offered:\n" + t.text()[-300:])
    finally: t.close()

@case("aliases and functions: fish function is offered")
def _():
    t = Term([H, "--shell", "fish"], env={"HOME": "/home-alias", "XDG_CONFIG_HOME": "/home-alias/.config"}, boot=4)
    try:
        t.type("mkc", 0.8); expect("mkcd" in t.text(), "function not offered:\n" + t.text()[-300:])
    finally: t.close()

@case("failure: if hiruzen itself fails, the shell keeps working and the failure is logged")
def _():
    cfg = "/tmp/cfg-crash"
    t = Term([H, "--shell", "bash"], env={"HIRUZEN_CONFIG_DIR": cfg, "HIRUZEN_TEST_PANIC": "1"})
    try:
        t.type("git ch", 0.8); expect("cherry-pick" in t.text(), "no dropdown before the failure")
        t.send("\x1b[99~", 0.8); t.send("\x03", 0.5)
        t.type("echo survived-$((6*7))"); t.enter(1.5)
        expect("survived-42" in t.text(), "the shell did not survive:\n" + t.text()[-300:])
    finally: t.close()
    expect("HIRUZEN_TEST_PANIC" in open(cfg + "/crash.log").read(), "the failure was not logged")

@case("screen clear: suggestions continue on the same line after Ctrl+L")
def _():
    t = Term([H, "--shell", "zsh"])
    try:
        t.type("echo one"); t.enter(0.8); t.type("git cherr", 0.6); t.send("\x0c", 0.8); t.type("y", 0.8)
        expect("cherry-pick" in t.text(), "no dropdown after the clear:\n" + t.text()[-300:])
    finally: t.close()

@case("cursor: suggestions at a gap between words, none inside a word")
def _():
    t = Term([H, "--shell", "zsh"])
    try:
        t.type("git  main", 0.6); t.send("\x1b[D" * 5, 0.8)
        expect("checkout" in t.text(), "nothing offered at a gap:\n" + t.text()[-300:])
        t.send("\x03", 0.5); t.type("git checkout", 0.6); t.send("\x1b[D" * 3, 0.8)
        expect("Switch branches or restore" not in t.text().split("git checkout")[-1], "offered inside a word")
    finally: t.close()

@case("secrets: passwords and tokens are not written to the record")
def _():
    cfg = "/tmp/cfg-secret"
    t = Term([H, "--shell", "bash"], env={"HIRUZEN_CONFIG_DIR": cfg})
    try:
        t.type("echo --password hunter2 API_TOKEN=abc123xyz https://me:pw99@example.com", 0.2); t.enter(1.0)
        t.type("echo flush", 0.2); t.enter(1.0)
    finally: t.close()
    record = open(cfg + "/history.tsv").read()
    for secret in ["hunter2", "abc123xyz", "pw99"]:
        expect(secret not in record, f"{secret} was written to the record")
    expect("<redacted>" in record, "nothing was redacted")

@case("picks: a suggestion picked repeatedly moves to the top")
def _():
    cfg = "/tmp/cfg-picks"; env = {"HIRUZEN_CONFIG_DIR": cfg}
    t = Term([H, "--shell", "bash"], env=env)
    try:
        for round_number in range(4):
            t.type("git ch", 0.6)
            # Wait for the dropdown: a key sent before it shows goes to the shell.
            for _ in range(20):
                if "cherry-pick" in t.text(): break
                t.pump(0.2)
            expect("cherry-pick" in t.text(), "no dropdown to pick from")
            # Pick by name, not by position: the position changes as hiruzen
            # learns. The highlighted row carries the accept-key symbol.
            def highlighted():
                current = t.text().split("git ch")[-1]
                return any(mark in current for mark in ("cherry-pick ⇥", "cherry-pick ↵"))
            for _ in range(6):
                if highlighted(): break
                t.send("\x1b[B", 0.4)
            expect(highlighted(), f"could not highlight cherry-pick in round {round_number}:\n" + t.text()[-300:])
            t.send("\t", 0.6)
            t.send("\x03", 0.6)
    finally: t.close()
    picked = [line.split("\t")[2] for line in open(cfg + "/picks.tsv").read().splitlines() if line.count("\t") >= 2]
    expect(picked == ["cherry-pick"] * 4, f"expected four picks of cherry-pick, recorded {picked}")
    out = subprocess.run([H, "complete", "git ch"], capture_output=True, text=True, cwd="/proj", env={**os.environ, **env}).stdout
    record = cfg + "/history.tsv"
    detail = "\npicks:\n" + open(cfg + "/picks.tsv").read() + "\nrecord:\n" + (open(record).read() if os.path.exists(record) else "(none)")
    expect(out.splitlines()[0].startswith("cherry-pick"), "the picked suggestion is not first:\n" + out[:200] + detail)

@case("learn: a tool with no spec gets suggestions from its --help")
def _():
    os.makedirs("/tmp/fakebin", exist_ok=True)
    tool = "/tmp/fakebin/zztool"
    open(tool, "w").write("#!/bin/sh\ncat <<'EOT'\nUsage: zztool [OPTIONS] <COMMAND>\n\nCommands:\n  launch <target>  Launch a target\n  status           Show status\n\nOptions:\n  -q, --quiet      Say less\n      --mode <M>   How to run (choices: fast, slow)\nEOT\n")
    os.chmod(tool, 0o755)
    env = {**os.environ, "HIRUZEN_CONFIG_DIR": "/tmp/cfg-learn", "PATH": "/tmp/fakebin:" + os.environ["PATH"]}
    before = subprocess.run([H, "complete", "zztool la"], capture_output=True, text=True, cwd="/other", env=env).stdout
    expect("launch" not in before, "the tool was known before learning")
    learned = subprocess.run([H, "learn", "zztool"], capture_output=True, text=True, env=env)
    expect(learned.returncode == 0 and "2 commands" in learned.stdout, learned.stdout + learned.stderr)
    after = subprocess.run([H, "complete", "zztool la"], capture_output=True, text=True, cwd="/other", env=env).stdout
    expect("launch" in after, "no suggestions after learning:\n" + after)
    values = subprocess.run([H, "complete", "zztool --mode "], capture_output=True, text=True, cwd="/other", env=env).stdout
    expect("fast" in values and "slow" in values, "option values not learned:\n" + values)
    refuse = subprocess.run([H, "learn", "git"], capture_output=True, text=True, env=env)
    expect(refuse.returncode == 2 and "already has a spec" in refuse.stderr, "learning over a built-in spec was not refused")

@case("settings: an invalid settings file falls back to defaults and doctor reports it")
def _():
    cfg = "/tmp/cfg-bad"; os.makedirs(cfg, exist_ok=True); open(cfg + "/settings.json", "w").write("{ not json")
    env = {"HIRUZEN_CONFIG_DIR": cfg}
    t = Term([H, "--shell", "bash"], env=env)
    try:
        t.type("git ch", 0.8); expect("cherry-pick" in t.text(), "no dropdown with a broken settings file")
    finally: t.close()
    doctor = subprocess.run([H, "doctor"], capture_output=True, text=True, env={**os.environ, **env})
    expect("not valid JSON" in doctor.stdout and doctor.returncode == 1, "doctor did not report it:\n" + doctor.stdout)

@case("settings: a read-only settings directory does not stop a session")
def _():
    cfg = "/tmp/cfg-ro"; os.makedirs(cfg, exist_ok=True); os.chmod(cfg, 0o500)
    t = Term([H, "--shell", "bash"], env={"HIRUZEN_CONFIG_DIR": cfg})
    try:
        t.type("echo one"); t.enter(1.0); t.type("git ch", 0.8); expect("cherry-pick" in t.text(), "no dropdown")
    finally: t.close(); os.chmod(cfg, 0o700)

@case("settings: disabling, per-command disabling and theme changes apply at the next prompt")
def _():
    cfg = "/tmp/cfg-live"; env = {"HIRUZEN_CONFIG_DIR": cfg}
    t = Term([H, "--shell", "bash"], env=env)
    try:
        t.type("hiruzen settings autocomplete.disableForCommands git"); t.enter(1.5)
        t.type("git ch", 0.8); expect("cherry-pick" not in t.text(), "git was not disabled"); t.send("\x03", 0.5)
        t.type("docker ru", 0.8); expect("Run a command" in t.text() or "run" in t.text().splitlines()[-1] or "> run" in t.text(), "other commands stopped too"); t.send("\x03", 0.5)
        t.type("hiruzen settings autocomplete.disable on"); t.enter(1.5)
        t.type("docker ru", 0.8); expect("> run" not in t.text().split("autocomplete.disable = true")[-1], "disable did not apply"); t.send("\x03", 0.5)
    finally: t.close()

@case("nesting: hiruzen inside hiruzen refuses, and --auto is quiet")
def _():
    t = Term([H, "--shell", "bash"])
    try:
        t.type("hiruzen"); t.enter(1.5); expect("already running" in t.text(), "nested start was not refused:\n" + t.text()[-300:])
        t.type("hiruzen --auto; echo code=$?"); t.enter(1.5); expect("code=3" in t.text(), "--auto did not return 3")
    finally: t.close()

@case("no terminal: refuses plainly, --auto returns 3, complete still works")
def _():
    plain = subprocess.run([H], capture_output=True, text=True, stdin=subprocess.DEVNULL)
    expect(plain.returncode == 2 and "interactive terminal" in plain.stderr, f"{plain.returncode} {plain.stderr}")
    auto = subprocess.run([H, "--auto"], capture_output=True, text=True, stdin=subprocess.DEVNULL)
    expect(auto.returncode == 3 and auto.stderr == "", f"{auto.returncode} {auto.stderr}")
    out = subprocess.run([H, "complete", "git ch"], capture_output=True, text=True, cwd="/proj").stdout
    expect("checkout" in out, out)

@case("large directory: 5,000 files complete quickly")
def _():
    started = time.time()
    out = subprocess.run([H, "complete", "cat file-49"], capture_output=True, text=True, cwd="/big").stdout
    elapsed = time.time() - started
    expect("file-49" in out, "no files offered"); expect(elapsed < 1.0, f"took {elapsed:.2f}s")

@case("commands: bad arguments give an error and status 2, never a crash")
def _():
    for argv in [["--shell"], ["--shell", "nosuchshell"], ["--bogus"], ["init"], ["init", "cmd"], ["settings", "nope", "1"], ["settings", "autocomplete.height", "0"],
                 ["theme", "nope"], ["run", "../../etc/passwd"], ["scripts", "new", "a/b"], ["plugins", "remove", "nope"], ["evaluate", "--file"], ["integrations", "explode"], ["complete"]]:
        r = subprocess.run([H] + argv, capture_output=True, text=True, stdin=subprocess.DEVNULL, env={**os.environ, "HIRUZEN_CONFIG_DIR": "/tmp/cfg-args"})
        expect(r.returncode in (1, 2) and "panicked" not in r.stderr, f"{argv}: status {r.returncode}, {r.stderr[:120]}")

@case("complete: hostile input never crashes")
def _():
    lines = ["", " ", "\t", "'", '"', "\\", "|", "||", "&&", ";", "git " + "x" * 5000, "git " + "a " * 2000, "日本語 ", "$(", "`", "git --", "git -" * 50,
             "sudo sudo sudo sudo sudo sudo sudo sudo sudo sudo git ", "a=b c=d e=f ", "../../../ ", "~", "~/", "git checkout '", "🙂 ", "git\tcheckout\t"]
    for line in lines:
        r = subprocess.run([H, "complete", "--", line], capture_output=True, text=True, cwd="/proj", timeout=20)
        expect(r.returncode in (0, 1, 2) and "panicked" not in r.stderr, f"{line[:30]!r}: status {r.returncode} {r.stderr[:200]}")

passed = sum(1 for _, ok, _ in results if ok)
print(f"\n{passed} of {len(results)} cases passed")
sys.exit(0 if passed == len(results) else 1)
