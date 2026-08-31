#!/usr/bin/env python3
"""Assert the CI smoke run behaved, from the JSON the CLI already emits.

Kept as a file rather than inlined in the workflow: a Python heredoc inside a YAML
block scalar arrives indented, which Python rejects, and the failure looks like a
BugHunter bug rather than a quoting one.
"""
import json
import sys


def main(scan_path: str, rescan_path: str) -> int:
    scan = json.load(open(scan_path))["result"]
    rescan = json.load(open(rescan_path))["result"]

    problems = []
    if scan["symbols_indexed"] < 100:
        problems.append(f"only {scan['symbols_indexed']} symbols indexed")
    if scan["files_failed"] > 0:
        problems.append(f"{scan['files_failed']} files failed to parse")
    if not rescan["unchanged"]:
        problems.append(f"a no-op rescan reported changes: {rescan['items'][:3]}")
    if scan["edges_total"] < 100:
        problems.append(f"only {scan['edges_total']} edges extracted")

    if problems:
        for p in problems:
            print(f"FAIL: {p}", file=sys.stderr)
        return 1

    in_scope = scan["edges_total"] - scan["edges_external"]
    pct = (scan["edges_resolved"] / in_scope * 100) if in_scope else 100
    print(
        f"ok: {scan['files_scanned']} files, {scan['symbols_indexed']} symbols, "
        f"{pct:.0f}% of {in_scope} in-project edges resolved, "
        f"no-op rescan {rescan['duration_ms']}ms"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1], sys.argv[2]))
