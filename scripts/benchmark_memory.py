#!/usr/bin/env python3
"""Measure a release binary against preserved sources and compare JSON contents."""
import argparse
from collections import Counter
import hashlib
import json
from pathlib import Path
import resource
import shutil
import subprocess
import time


def fingerprint_batches(root):
    results = {}
    for catalog in ("medicines", "cosmetics", "cannabis", "portaria-344", "sigtap"):
        digests = []
        statuses = Counter()
        for batch in sorted((root / catalog).rglob("batch-*.json")):
            rows = json.loads(batch.read_text())
            if not 0 < len(rows) <= 1000:
                raise ValueError(f"Invalid batch size: {batch}")
            for row in rows:
                canonical = json.dumps(row, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
                digests.append(hashlib.sha256(canonical.encode()).digest())
                if "regulatory_status" in row:
                    statuses[row["regulatory_status"]] += 1
        combined = hashlib.sha256()
        for digest in sorted(digests):
            combined.update(digest)
        results[catalog] = {
            "records": len(digests),
            "sha256_sorted_records": combined.hexdigest(),
            "statuses": dict(statuses),
        }
    return results


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--source-run", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--label", required=True)
    parser.add_argument("--baseline", type=Path)
    args = parser.parse_args()
    if not Path("/proc/self/status").exists():
        raise RuntimeError("Memory sampling requires Linux /proc")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if not args.label or args.label in (".", "..") or Path(args.label).name != args.label:
        raise ValueError("Label must be a single file name")
    work = output / "work"
    if work.resolve() == args.source_run.resolve():
        raise ValueError("Benchmark output must differ from the preserved source run")
    sources = work / "sources"
    sources.mkdir(parents=True, exist_ok=True)
    control = json.loads((args.source_run / "download-control.json").read_text())
    latest = {entry["name"]: entry for entry in control["files"]}
    for entry in latest.values():
        destination = work / entry["filename"]
        if not destination.exists():
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(args.source_run / entry["filename"], destination)
    shutil.copyfile(args.source_run / "download-control.json", work / "download-control.json")
    archive = output / "binaries" / args.label
    archive.parent.mkdir(exist_ok=True)
    shutil.copy2(args.binary, archive)
    command = [str(archive), "--output", str(work), "--reuse-sources", "--batch-size", "1000"]
    log_path = output / f"{args.label}.log"
    phase = "medicines"
    phase_rss = Counter()
    peak_rss = 0
    markers = {"[1/5]": "medicines", "[2/5]": "cosmetics", "[3/5]": "cannabis", "[4/5]": "portaria", "[5/5]": "sigtap"}
    started = time.monotonic()
    with log_path.open("w") as log, log_path.open() as observed:
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)
        while True:
            for line in observed.readlines():
                for marker, name in markers.items():
                    if marker in line:
                        phase = name
            try:
                status = Path(f"/proc/{process.pid}/status").read_text()
                for line in status.splitlines():
                    if line.startswith("VmRSS:"):
                        rss = int(line.split()[1])
                        phase_rss[phase] = max(phase_rss[phase], rss)
                    elif line.startswith("VmHWM:"):
                        peak_rss = max(peak_rss, int(line.split()[1]))
            except FileNotFoundError:
                pass
            if process.poll() is not None:
                break
            time.sleep(0.1)
    elapsed = time.monotonic() - started
    if process.returncode:
        raise RuntimeError(f"Execution failed ({process.returncode}); see {log_path}")
    peak_rss = max(peak_rss, resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss)
    manifest = json.loads((work / "manifest.json").read_text())
    if not all(source["reused_from_disk"] for source in manifest["sources"].values()):
        raise ValueError("Benchmark must reuse every source")
    fingerprints = fingerprint_batches(work)
    if args.baseline:
        baseline = json.loads(args.baseline.read_text())
        if fingerprints != baseline["fingerprints"]:
            raise ValueError("Generated records differ from the baseline")
        if manifest["counts"] != baseline["counts"]:
            raise ValueError("Manifest counts differ from the baseline")
        hashes = {name: source["sha256"] for name, source in manifest["sources"].items()}
        if hashes != baseline["source_sha256"]:
            raise ValueError("Sources differ from the baseline")
    measurement = {
        "label": args.label,
        "binary_sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
        "wall_seconds": round(elapsed, 3),
        "peak_rss_kib": peak_rss,
        "sampled_phase_peak_rss_kib": dict(phase_rss),
        "application_elapsed_ms": manifest["total_elapsed_ms"],
        "timings_ms": manifest["timings_ms"],
        "counts": manifest["counts"],
        "source_sha256": {name: source["sha256"] for name, source in manifest["sources"].items()},
        "fingerprints": fingerprints,
        "matches_baseline": True if args.baseline else None,
    }
    path = output / f"{args.label}.json"
    path.write_text(json.dumps(measurement, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({key: measurement[key] for key in ("label", "wall_seconds", "peak_rss_kib", "sampled_phase_peak_rss_kib", "matches_baseline")}, indent=2))
    print(path)


if __name__ == "__main__":
    main()
