#!/usr/bin/env python3
import json
import os
import re
import shutil
import signal
import subprocess
import sys
from pathlib import Path

PROMPT_TEMPLATE = r'''You are a UX specialist coding web apps. You execute user stories from __PRD_FILE__ one at a time.

## Instructions

1. Read `__PRD_FILE__` to understand the project and find the next incomplete story (first story where `"passes": false`, in priority order).
2. If all stories have `"passes": true`, say "All stories complete." and exit.
3. For the chosen story:
   a. Read its description and acceptance criteria carefully.
   b. Implement the changes needed to satisfy ALL acceptance criteria.
   c. After implementing, verify each acceptance criterion is met (run compiler checks, lints, and tests as specified).
   d. If all criteria pass, update `__PRD_FILE__` — set that story's `"passes": true` and add any relevant notes.
   e. Append a short summary of what you did to `progress.txt`.
4. Only work on ONE story per iteration. After completing (or failing) one story, stop.
5. If you cannot complete a story, set its `"notes"` field with what went wrong and stop. Do NOT mark it as passing.
6. Commit your changes with a message referencing the story ID (e.g., "US-001: Add status column to tasks table").

## Important
- You have no memory of previous iterations. Read __PRD_FILE__ and progress.txt to understand current state.
- You have a lot of time. Don't call the acceptance criteria with everything broken or with things you can fix before submitting.
- Since a task can be long, do not give up on compiler errors, use your $frontend skill to solve.
- Use the context mode mcp
- Large refactors are allowed, but plan them well.
- Do not skip acceptance criteria. Every criterion must be verified.
- Do not work on more than one story.
- After finishing the story (pass or fail), stop immediately.
- any new markdown should go to aidocs/ on root folder of the project. Do not commit those
'''

CONTINUE_PROMPT = (
    "Continue this batch. Read the PRD and progress log again, then work only on the next "
    "pending item that is currently actionable under the PRD rules (not blocked, passes false and dependencies are done). If the current "
    "item ends up blocked, MARK AS 'blocked' and stop. Update notes only "
    "if you produced materially new evidence. Otherwise complete exactly one actionable story "
    "and stop. Remember to use $frontend, context mcp and subagents with gpt-5.3-codex-spark high model (use subagents whenever possible as it saves credits). Remember to mark task as blocked if it is blocked."
)

BLOCKER_PROMPT_TEMPLATE = r'''You are an engineering manager on a webapp and your team is blocked. You are handling a blocked story from __PRD_FILE__.

## Instructions

1. Read `__PRD_FILE__` and `progress.txt`.
2. Focus on the first incomplete story, which is currently blocked.
3. Your job is to unblock forward progress, not just to gather one more data point.
4. You may, if justified by evidence:
   a. implement fixes
   b. reduce the blocker into deterministic tests or replay fixtures
   c. split the blocked story into smaller prerequisite stories
   d. insert new prerequisite stories before the blocked story
   e. update dependencies so the next run has smaller actionable work
5. Prefer creating many future actionable steps over one more broad rerun.
6. If you fully satisfy the blocked story, mark it passing.
7. If you cannot fully satisfy it, you must still improve the batch:
   a. add reducers, prerequisites, or a narrower next gate
   b. update `__PRD_FILE__`
   c. update the blocked story notes with the new evidence
   d. append a short summary to `progress.txt`
8. Only work on this blocker and its immediate prerequisite decomposition. Do not drift into unrelated later stories.
9. Commit your changes with a message referencing the blocked story ID or the new prerequisite story ID you introduced.

## Important
- Do not keep rerunning the same broad gate if it is not producing a new reducer or narrower next step.
- If the blocker is too broad, rewrite the PRD around it so the next iteration has smaller, deterministic work.
- Use your $frontend and $build-web-apps:frontend-skill skills.
- Use the context mode mcp.
- any new markdown should go to aidocs/ on root folder of the project. Do not commit those
'''

SESSION_ID_PATTERN = re.compile(r"session id:\s*([0-9a-fA-F-]{36})")
RESUME_INVALID_PATTERNS = (
    "thread/resume failed: no rollout found",
    "no rollout found for thread id",
)


def eprint(*args, **kwargs):
    print(*args, file=sys.stderr, **kwargs)


def load_prd(path: Path):
    return json.loads(path.read_text())


def all_stories_pass(prd: dict) -> bool:
    stories = prd.get("userStories", [])
    return bool(stories) and all(bool(s.get("passes")) for s in stories)


def first_incomplete_story(prd: dict):
    for story in prd.get("userStories", []):
        if not story.get("passes", False):
            return story
    return None


def first_actionable_story(prd: dict):
    stories = prd.get("userStories", [])
    story_map = {s.get("id"): s for s in stories}
    for story in stories:
        if story.get("passes", False):
            continue
        if story.get("status") == "blocked":
            continue
        deps = story.get("dependsOn", [])
        if all(story_map.get(dep, {}).get("passes", False) for dep in deps):
            return story
    return None


def codex_settings():
    codex_bin = os.environ.get("CODEX_BIN") or shutil.which("codex")
    if not codex_bin:
        eprint("Error: codex not found on PATH. Set CODEX_BIN=/path/to/codex.")
        sys.exit(1)
    model = os.environ.get("CODEX_MODEL", "gpt-5.4-mini")
    reasoning = os.environ.get("CODEX_REASONING", "high")
    bypass = os.environ.get("CODEX_BYPASS_SANDBOX", "1") == "1"
    sandbox = os.environ.get("CODEX_SANDBOX", "workspace-write")

    return codex_bin, model, reasoning, bypass, sandbox


def build_codex_args(cwd: Path):
    codex_bin, model, reasoning, bypass, sandbox = codex_settings()
    args = [
        codex_bin,
        "exec",
        "-C",
        str(cwd),
        "--model",
        model,
        "--config",
        f'model_reasoning_effort="{reasoning}"',
    ]
    if bypass:
        args.append("--dangerously-bypass-approvals-and-sandbox")
    else:
        args.extend(["--sandbox", sandbox])
    args.append("-")
    return args, model, reasoning


def build_codex_resume_args(session_id: str):
    codex_bin, model, reasoning, bypass, _sandbox = codex_settings()
    args = [
        codex_bin,
        "exec",
        "resume",
        "--model",
        model,
        "--config",
        f'model_reasoning_effort="{reasoning}"',
    ]
    if bypass:
        args.append("--dangerously-bypass-approvals-and-sandbox")
    args.extend([session_id, "-"])
    return args


class Runner:
    def __init__(
        self,
        prd_file: Path,
        progress_file: Path,
        max_iterations: int,
        blocked_retry_limit: int,
        continue_mode: bool,
        session_file: Path,
    ):
        self.prd_file = prd_file
        self.progress_file = progress_file
        self.max_iterations = max_iterations
        self.blocked_retry_limit = blocked_retry_limit
        self.continue_mode = continue_mode
        self.session_file = session_file
        self.current_proc = None
        self.blocked_retries: dict[str, int] = {}

    def ensure_files(self):
        if not self.prd_file.is_file():
            eprint(f"Error: {self.prd_file} not found.")
            sys.exit(1)
        if not self.progress_file.exists():
            self.progress_file.write_text("# Ralph Progress Log\n\n")

    def handle_signal(self, signum, _frame):
        if self.current_proc and self.current_proc.poll() is None:
            try:
                os.killpg(os.getpgid(self.current_proc.pid), signum)
            except ProcessLookupError:
                pass
            except Exception:
                try:
                    self.current_proc.send_signal(signum)
                except Exception:
                    pass
            try:
                self.current_proc.wait(timeout=5)
            except Exception:
                pass
        raise SystemExit(130)

    def prompt_for(self):
        return PROMPT_TEMPLATE.replace("__PRD_FILE__", self.prd_file.name)

    def blocker_prompt_for(self):
        return BLOCKER_PROMPT_TEMPLATE.replace("__PRD_FILE__", self.prd_file.name)

    def continue_prompt_for(self):
        return CONTINUE_PROMPT

    def load_session_id(self):
        if not self.session_file.exists():
            return None
        session_id = self.session_file.read_text().strip()
        return session_id or None

    def save_session_id(self, session_id: str):
        self.session_file.write_text(session_id + "\n")

    def clear_session_id(self):
        try:
            self.session_file.unlink()
        except FileNotFoundError:
            pass

    def should_stop_on_blocked(self, story: dict) -> bool:
        if story.get("status") != "blocked":
            return False
        story_id = story.get("id", "")
        count = self.blocked_retries.get(story_id, 0) + 1
        self.blocked_retries[story_id] = count
        if count > self.blocked_retry_limit:
            print(f"Blocked story {story_id} exceeded retry limit ({self.blocked_retry_limit}). Stopping.")
            return True
        print(f"Blocked story {story_id} retry {count}/{self.blocked_retry_limit}.")
        return False

    def run(self):
        self.ensure_files()
        args, model, reasoning = build_codex_args(Path.cwd())

        print("=== Ralph Loop ===")
        print(f"PRD: {self.prd_file.name}")
        print(f"Model: {model}")
        print(f"Reasoning: {reasoning}")
        print(f"Session mode: {'continuable' if self.continue_mode else 'ephemeral'}")
        print(f"Blocked retry limit: {self.blocked_retry_limit}")
        print(f"Continue mode: {'on' if self.continue_mode else 'off'}")
        print()

        signal.signal(signal.SIGINT, self.handle_signal)
        signal.signal(signal.SIGTERM, self.handle_signal)

        for iteration in range(1, self.max_iterations + 1):
            prd = load_prd(self.prd_file)
            if all_stories_pass(prd):
                print("All stories complete!")
                return 0

            blocked_attempt = False
            story = first_actionable_story(prd)
            if story is None:
                blocked_story = first_incomplete_story(prd)
                if blocked_story is not None:
                    if self.should_stop_on_blocked(blocked_story):
                        return 2
                    print(
                        "No actionable incomplete stories remain in this iteration. "
                        f"First incomplete story is {blocked_story.get('id')} "
                        f"with status={blocked_story.get('status', 'pending')}. "
                        "Attempting to solve the blocker directly."
                    )
                    print()
                    story = blocked_story
                    blocked_attempt = True
                else:
                    print("All stories complete!")
                    return 0

            next_story = f"{story.get('id')}: {story.get('title')}"
            now = subprocess.check_output(["date", "+%H:%M:%S"], text=True).strip()
            print(f"--- Iteration {iteration} — {now} — next story: {next_story} ---")

            session_id = self.load_session_id() if self.continue_mode else None
            attempted_resume = bool(self.continue_mode and session_id)
            while True:
                if attempted_resume:
                    iter_args = build_codex_resume_args(session_id)
                    prompt = self.blocker_prompt_for() if blocked_attempt else self.continue_prompt_for()
                else:
                    iter_args = args
                    prompt = self.blocker_prompt_for() if blocked_attempt else self.prompt_for()

                self.current_proc = subprocess.Popen(
                    iter_args,
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.STDOUT,
                    preexec_fn=os.setsid,
                    text=True,
                    bufsize=1,
                )
                assert self.current_proc.stdin is not None
                assert self.current_proc.stdout is not None
                self.current_proc.stdin.write(prompt)
                self.current_proc.stdin.close()
                seen_session_id = session_id if attempted_resume else None
                output_lines = []
                for line in self.current_proc.stdout:
                    output_lines.append(line)
                    sys.stdout.write(line)
                    sys.stdout.flush()
                    if self.continue_mode and not seen_session_id:
                        match = SESSION_ID_PATTERN.search(line)
                        if match:
                            seen_session_id = match.group(1)
                rc = self.current_proc.wait()
                self.current_proc = None

                combined_output = "".join(output_lines)
                if attempted_resume and rc != 0 and any(p in combined_output for p in RESUME_INVALID_PATTERNS):
                    print("Stored .session_id is invalid. Clearing it and retrying this iteration with a fresh prompt.")
                    self.clear_session_id()
                    session_id = None
                    attempted_resume = False
                    continue
                break

            if rc != 0:
                print(f"Codex exited with status {rc}.")
                return rc

            if self.continue_mode and seen_session_id:
                self.save_session_id(seen_session_id)

            print()
            print(f"Iteration {iteration} complete.")
            print()

        print(f"Reached max iterations ({self.max_iterations}). Stopping.")
        return 1


def main(argv: list[str]) -> int:
    args = list(argv[1:])
    continue_mode = False
    if "--continue" in args:
        args.remove("--continue")
        continue_mode = True
    prd_arg = args[0] if args else "prd_next_long_batch.json"
    runner = Runner(
        prd_file=Path(prd_arg),
        progress_file=Path("progress.txt"),
        max_iterations=int(os.environ.get("MAX_ITERATIONS", "300")),
        blocked_retry_limit=int(os.environ.get("BLOCKED_RETRY_LIMIT", "3")),
        continue_mode=continue_mode,
        session_file=Path(".session_id"),
    )
    return runner.run()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
