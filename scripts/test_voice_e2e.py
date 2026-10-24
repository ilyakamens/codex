import argparse
import fcntl
import json
import os
import pty
import re
import select
import sqlite3
import struct
import subprocess
import tempfile
import termios
import time
from pathlib import Path

import pyte

parser = argparse.ArgumentParser(
    description="Exercise /voice with generated speech and the current CLI login."
)
parser.add_argument(
    "--normal", action="store_true", help="Compare ordinary spoken replies."
)
parser.add_argument(
    "--prompt-file", type=Path, help="Override the realtime prompt for diagnosis."
)
parser.add_argument("--timeout", type=float, default=60)
parser.add_argument("--microphone", default="MacBook Pro Microphone")
parser.add_argument("--speaker", default="MacBook Pro Speakers")
parser.add_argument("--cwd", type=Path)
parser.add_argument("--check-weekly-limit", action="store_true")
parser.add_argument("--status-only", action="store_true")
args = parser.parse_args()
repo = Path(__file__).resolve().parent.parent
artifacts = repo / "codex-rs/target/voice-e2e"
artifacts.mkdir(exist_ok=True)
directory = Path(tempfile.mkdtemp(prefix="run-", dir=artifacts))
working_directory = args.cwd.resolve() if args.cwd else directory
speech = directory / "question.aiff"
cases = [
    (
        "This is a test, please respond.",
        (r"test", r"respond"),
        r"test|clear|hear|heard|ready|received|yes|here",
        "either",
    ),
    (
        "Run Python to calculate seventeen plus twenty six. Show just the result.",
        (r"seventeen|\b17\b", r"twenty[ -]?six|\b26\b"),
        r"\b43\b|forty[ -]?three",
        "backend",
    ),
    (
        "What is twelve times five? Please answer with just the number.",
        (r"twelve|\b12\b", r"five|\b5\b"),
        r"\b60\b|sixty",
        "either",
    ),
]
command = [
    str(repo / "codex-rs/target/voice-text-output/bin/codex"),
    "--no-daemon",
    "--disable" if args.normal else "--enable",
    "voice_text_output",
    "--sandbox",
    "read-only",
    "--ask-for-approval",
    "never",
    "-C",
    str(working_directory),
    "-c",
    f'projects.{json.dumps(str(working_directory))}.trust_level="trusted"',
    "-c",
    f"audio.microphone={json.dumps(args.microphone)}",
    "-c",
    f"audio.speaker={json.dumps(args.speaker)}",
]
if args.prompt_file:
    command.extend(
        [
            "-c",
            f"experimental_realtime_ws_backend_prompt={json.dumps(args.prompt_file.read_text())}",
        ]
    )
master, slave = pty.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 160, 0, 0))
env = {
    key: value
    for key, value in os.environ.items()
    if key not in {"TMUX", "TMUX_PANE", "STY", "TERM_PROGRAM_VERSION"}
}
env.update({"TERM": "xterm-256color", "TERM_PROGRAM": "kitty"})
child = subprocess.Popen(
    command, stdin=slave, stdout=slave, stderr=slave, env=env, start_new_session=True
)
os.close(slave)
output = bytearray()
screen = pyte.HistoryScreen(160, 40, history=10000)
stream = pyte.Stream(screen)
answered = set()
probes = [
    (b"\x1b[6n", b"\x1b[1;1R"),
    (b"\x1b[?u", b"\x1b[?7u\x1b[?1;2c"),
    (b"\x1b]11;?", b"\x1b]10;rgb:ffff/ffff/ffff\x1b\\\x1b]11;rgb:0000/0000/0000\x1b\\"),
]
codex_home = Path(os.environ.get("CODEX_HOME", Path.home() / ".codex"))
logs = sqlite3.connect(f"file:{codex_home / 'logs_2.sqlite'}?mode=ro", uri=True)
started_at = time.time()
observed_events = []
last_log_id = 0
rollout = None
playback = None
outcomes = []


def terminal_text():
    return "\n".join(screen.display)


def pump(duration):
    end = time.monotonic() + duration
    while time.monotonic() < end:
        if select.select([master], [], [], min(0.1, end - time.monotonic()))[0]:
            chunk = os.read(master, 65536)
            output.extend(chunk)
            stream.feed(chunk.decode(errors="replace"))
        for probe, response in probes:
            if probe in output and probe not in answered:
                os.write(master, response)
                answered.add(probe)
        if child.poll() is not None:
            raise RuntimeError(f"CLI exited with {child.returncode}")


def wait_for(text):
    end = time.monotonic() + args.timeout
    while text not in terminal_text():
        failure = re.search(r"Dictation failed:[^\n]+", terminal_text())
        if failure:
            raise RuntimeError(failure.group())
        if time.monotonic() > end:
            raise TimeoutError(f"Terminal did not show {text!r}")
        pump(0.2)


def send(text):
    os.write(master, text.encode())
    pump(0.3)
    os.write(master, b"\r")


def events():
    global last_log_id
    rows = logs.execute(
        "SELECT id,feedback_log_body FROM logs WHERE ts >= ? AND id > ? AND process_uuid LIKE ? "
        "AND target='codex_api::realtime_websocket::wire' "
        "AND feedback_log_body LIKE '%realtime websocket event: %' "
        "AND feedback_log_body NOT LIKE '%session.input_audio.append%' "
        "AND feedback_log_body NOT LIKE '%session.output_audio.delta%' ORDER BY id",
        (int(started_at), last_log_id, f"pid:{child.pid}:%"),
    )
    for row_id, body in rows:
        last_log_id = row_id
        observed_events.append(
            json.loads(body.split("realtime websocket event: ", 1)[1])
        )
    return observed_events


def rollout_events():
    global rollout
    if rollout is None:
        row = logs.execute(
            "SELECT thread_id FROM logs WHERE ts >= ? AND process_uuid LIKE ? "
            "AND thread_id IS NOT NULL ORDER BY ts,id LIMIT 1",
            (int(started_at), f"pid:{child.pid}:%"),
        ).fetchone()
        if row is None:
            return []
        rollout = next((codex_home / "sessions").rglob(f"*{row[0]}.jsonl"), None)
        if rollout is None:
            return []
    records = []
    for line in rollout.read_text().splitlines(keepends=True):
        if not line.endswith("\n"):
            continue
        records.append(json.loads(line))
    return records


def backend_completed(answer_pattern):
    for event in rollout_events():
        payload = event.get("payload", {})
        if event["type"] == "event_msg" and payload.get("type") == "task_complete":
            reply = payload.get("last_agent_message") or ""
            if re.search(answer_pattern, reply, re.IGNORECASE):
                return True
    return False


def result(transcript_patterns, answer_pattern, event_offset):
    observed = events()[event_offset:]
    transcript = "".join(
        event["item"]["text"]
        for event in observed
        if event["type"] == "input_transcript.added"
    ).lower()
    if not args.normal:
        user_messages = [
            " ".join(
                content["text"]
                for content in event["payload"]["item"]["content"]
                if content["type"] == "text"
            )
            for event in rollout_events()
            if event["type"] == "event_msg"
            and event.get("payload", {}).get("type") == "item_completed"
            and event["payload"].get("item", {}).get("type") == "UserMessage"
        ]
        matching = [
            text
            for text in user_messages
            if all(
                re.search(pattern, text, re.IGNORECASE)
                for pattern in transcript_patterns
            )
        ]
        return {
            "transcript": len(matching) == 1,
            "ordinary_user_message": len(matching) == 1
            and "<realtime_delegation>" not in matching[0],
            "backend_completed": backend_completed(answer_pattern),
            "no_voice_session": len(events()) == 0,
            "rendered_answer": re.search(answer_pattern, terminal_text(), re.IGNORECASE)
            is not None,
        }
    visible_text = " ".join(terminal_text().lower().split())
    direct_replies = [
        " ".join(event["turn"].get("transcript", "").lower().split())
        for event in observed
        if event["type"] == "turn.done" and event["turn"]["role"] == "assistant"
    ]
    return {
        "transcript": all(
            re.search(pattern, transcript) for pattern in transcript_patterns
        ),
        "delegated": any(event["type"] == "delegation.created" for event in observed),
        "backend_completed": backend_completed(answer_pattern),
        "direct_reply_visible": any(
            reply
            and re.search(answer_pattern, reply, re.IGNORECASE)
            and reply in visible_text
            for reply in direct_replies
        ),
        "rendered_answer": re.search(answer_pattern, terminal_text(), re.IGNORECASE)
        is not None,
    }


def passed(outcome, reply_source):
    if not outcome["transcript"] or not outcome["rendered_answer"]:
        return False
    if args.normal:
        return outcome["direct_reply_visible"]
    return (
        outcome["ordinary_user_message"]
        and outcome["backend_completed"]
        and outcome["no_voice_session"]
    )


try:
    print(f"Artifacts: {directory}", flush=True)
    wait_for("Ask Codex to do anything")
    if args.check_weekly_limit or args.status_only:
        wait_for("weekly")
        print("Weekly usage is visible before the first turn.", flush=True)
    if args.status_only:
        raise SystemExit(0)
    send("/voice")
    wait_for("listening")
    send("/voice mute")
    wait_for("muted")
    end = time.monotonic() + args.timeout
    while args.normal and not any(
        event["type"] == "session.started" for event in events()
    ):
        if time.monotonic() > end:
            raise TimeoutError(
                "Voice transport connected but the realtime session did not start"
            )
        pump(0.2)
    for index, (
        question,
        transcript_patterns,
        answer_pattern,
        reply_source,
    ) in enumerate(cases):
        subprocess.run(
            ["say", "-v", "Samantha", "-r", "145", "-o", str(speech), question],
            check=True,
        )
        event_offset = len(events())
        send("/voice mute")
        wait_for("listening")
        print(f"Playing: {question}", flush=True)
        playback = subprocess.Popen(["afplay", str(speech)])
        muted = False
        end = time.monotonic() + args.timeout
        while True:
            pump(0.5)
            if (args.normal or index > 0) and not muted and playback.poll() is not None:
                send("/voice mute")
                wait_for("muted")
                muted = True
            outcome = result(transcript_patterns, answer_pattern, event_offset)
            if not args.normal and index == 0 and not muted and outcome["transcript"]:
                send("/voice mute")
                wait_for("muted")
                muted = True
            if passed(outcome, reply_source) or time.monotonic() > end:
                break
        playback.wait(timeout=10)
        if not muted:
            send("/voice mute")
            wait_for("muted")
        outcomes.append({"question": question, **outcome})
        print(json.dumps(outcome), flush=True)
        if not passed(outcome, reply_source):
            raise AssertionError(
                f"Spoken request did not reach a visible {reply_source} reply"
            )
    print("Live voice E2E passed.", flush=True)
finally:
    (directory / "terminal.txt").write_text(terminal_text())
    (directory / "events.json").write_text(json.dumps(observed_events, indent=2))
    (directory / "result.json").write_text(json.dumps(outcomes, indent=2))
    if playback is not None and playback.poll() is None:
        playback.terminate()
        playback.wait(timeout=5)
    if child.poll() is None:
        send("/voice stop")
        pump(1)
        os.write(master, b"\x03\x03")
        try:
            child.wait(timeout=5)
        except subprocess.TimeoutExpired:
            child.terminate()
            child.wait(timeout=5)
    logs.close()
    os.close(master)
