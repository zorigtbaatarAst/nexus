#!/usr/bin/env python3
"""Resolve a task id against the fixture specs and manifests.

    task_lookup.py <task-id>                 -> "<repo> <commit-sha> <prompt>" on one line
    task_lookup.py <task-id> <field>         -> that field of the task, one value per line

The second form exists so that a consumer needing something else the spec already declares —
`hidden_tests`, `required_sites` — reads it from the manifest rather than reconstructing it
from the task id somewhere else. The one-line form is unchanged, so

    read -r REPO COMMIT PROMPT < <(task_lookup.py "$TASK")

still works.
"""

import json
import pathlib
import sys
import tomllib

SPECS = "tests/fixtures/specs/*/fixture.toml"


def find_task(root, task_id):
    """Return (repo, spec-relative task table) for a task id, or (None, None)."""
    for spec in sorted(root.glob(SPECS)):
        with spec.open("rb") as fh:
            doc = tomllib.load(fh)
        for task in doc.get("task", []):
            if task.get("id") == task_id:
                return spec.parent.name, task
    return None, None


def commit_sha(root, repo, commit_id):
    manifest_path = root / f"target/fixtures/{repo}.manifest.json"
    if not manifest_path.exists():
        sys.exit(f"no manifest for {repo}: run `make fixtures` first")
    manifest = json.loads(manifest_path.read_text())
    for commit in manifest["commits"]:
        if commit["id"] == commit_id:
            return commit["sha"]
    sys.exit(f"commit {commit_id} is not in {repo}'s manifest")


def main():
    if len(sys.argv) not in (2, 3):
        sys.exit(f"usage: {sys.argv[0]} <task-id> [field]")

    task_id = sys.argv[1]
    field = sys.argv[2] if len(sys.argv) == 3 else None
    root = pathlib.Path(__file__).resolve().parents[2]

    repo, task = find_task(root, task_id)
    if task is None:
        sys.exit(f"unknown task {task_id}")

    if field is not None:
        if field not in task:
            sys.exit(f"task {task_id} declares no {field}")
        value = task[field]
        # A list prints one item per line; anything else prints as itself.
        for item in value if isinstance(value, list) else [value]:
            print(item)
        return

    if "prompt" not in task:
        # M1 and any other multi-turn task carry their prompts in [[task.turns]] and have no
        # single prompt to print. Say so rather than printing an empty third field.
        sys.exit(f"task {task_id} is multi-turn: it has turns, not a prompt")

    print(repo, commit_sha(root, repo, task["commit"]), task["prompt"])


if __name__ == "__main__":
    main()
