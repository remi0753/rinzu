"""Check every SMT proof in this directory with the Z3 executable."""

import argparse
from pathlib import Path
import subprocess


EXPECTED = {
    "future-observation-coverage.smt2": ("unsat",) * 3 + ("sat",) * 4,
    "future-observation-join.smt2": ("unsat",) * 2 + ("sat",) * 3,
    "native-publication-generation.smt2": ("unsat",) * 2 + ("sat",) * 6,
    "native-reference-frames.smt2": ("unsat",) * 4 + ("sat",) * 3,
}


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--z3", default="z3", help="path to the Z3 executable")
    args = parser.parse_args()
    directory = Path(__file__).resolve().parent
    files = {proof.name: proof for proof in directory.glob("*.smt2")}
    if files.keys() != EXPECTED.keys():
        difference = sorted(files.keys() ^ EXPECTED.keys())
        raise SystemExit(f"Proof file and expectation lists differ at {difference}")

    for name, expected in EXPECTED.items():
        try:
            result = subprocess.run(
                [args.z3, "-smt2", str(files[name])],
                capture_output=True,
                text=True,
                timeout=30,
            )
        except (OSError, subprocess.TimeoutExpired) as error:
            raise SystemExit(f"{name}: {error}") from error
        if result.returncode:
            raise SystemExit(
                f"{name}: Z3 exited with {result.returncode}\n{result.stderr}{result.stdout}"
            )
        actual = tuple(
            line
            for line in result.stdout.splitlines()
            if line in {"sat", "unsat", "unknown"}
        )
        if actual != expected:
            raise SystemExit(
                f"{name} expected {expected}, received {actual}\n{result.stdout}"
            )
        print(f"{name} passed ({len(actual)} checks)")


if __name__ == "__main__":
    main()
