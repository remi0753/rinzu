#!/usr/bin/env python3
"""Measure opt-in compiler phases using a reproducible Error/call graph.

Pass a second executable with --baseline to interleave warm comparisons. This
does not benchmark application execution or claim results for arbitrary apps.
"""
import argparse
import hashlib
import json
import os
import platform
from pathlib import Path
import statistics
import subprocess
import tempfile
import time


def source(functions):
    declarations = [
        'error Failed {}',
        'fn f0(value: Int) -> Int { if value < 0 { throw Failed(message: "failed"); } return value; }',
    ]
    for index in range(1, functions + 1):
        declarations.append(
            f'fn f{index}(value: Int) -> Int {{ '
            f'if value == {index} {{ return f{index - 1}(value - 1); }} '
            f'return f{index - 1}(value); }}'
        )
    declarations.append(f'fn main() {{ try {{ let value = f{functions}(7); }} catch errors {{}} }}')
    return '\n'.join(declarations) + '\n'


def run(binary, path, cache):
    started = time.perf_counter()
    result = subprocess.run(
        [str(binary), 'check', str(path)], text=True, capture_output=True,
        env=dict(os.environ, RINZU_COMPILE_TIMINGS='1', RINZU_CACHE_DIR=str(cache)),
    )
    elapsed = time.perf_counter() - started
    if result.returncode:
        raise RuntimeError(f'{binary} failed ({result.returncode}):\n{result.stderr}')
    timings = [json.loads(line.split('=', 1)[1]) for line in result.stderr.splitlines()
               if line.startswith('RINZU_COMPILE_TIMING=')]
    if not timings:
        raise RuntimeError(f'{binary} did not emit compilation timings')
    return {'wall_s': elapsed, 'timings': timings}


def summarize(runs):
    phases = {}
    for result in runs:
        totals = {}
        for span in result['timings']:
            totals[span['phase']] = totals.get(span['phase'], 0) + span['elapsed_ns'] / 1e6
        for name, value in totals.items():
            phases.setdefault(name, []).append(value)
    return {
        'wall_median_s': statistics.median(result['wall_s'] for result in runs),
        'wall_range_s': [min(result['wall_s'] for result in runs), max(result['wall_s'] for result in runs)],
        'phase_inclusive_median_ms': {name: statistics.median(values) for name, values in sorted(phases.items())},
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', type=Path)
    parser.add_argument('--baseline', type=Path)
    parser.add_argument('--runs', type=int, default=5)
    parser.add_argument('--functions', type=int, default=300)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.runs < 1 or args.functions < 1:
        parser.error('--runs and --functions must be positive')
    binaries = {'candidate': args.binary.resolve(strict=True)}
    if args.baseline:
        binaries = {'baseline': args.baseline.resolve(strict=True), **binaries}
    results = {name: {'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(), 'runs': []}
               for name, binary in binaries.items()}
    text = source(args.functions)
    with tempfile.TemporaryDirectory(prefix='rinzu-frontend-benchmark-') as temporary:
        root = Path(temporary)
        path = root / 'effects-chain.rinzu'
        path.write_text(text)
        for name, binary in binaries.items():
            results[name]['warmup'] = run(binary, path, root / 'cache')
        for _ in range(args.runs):
            for name, binary in binaries.items():
                results[name]['runs'].append(run(binary, path, root / 'cache'))
    for result in results.values():
        result['summary'] = summarize(result['runs'])
    report = {'schema': 'rinzu.frontend-benchmark-v1', 'machine': platform.machine(),
              'system': platform.platform(), 'source_sha256': hashlib.sha256(text.encode()).hexdigest(),
              'functions': args.functions, 'results': results}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({name: result['summary'] for name, result in results.items()}, indent=2))


if __name__ == '__main__':
    main()
