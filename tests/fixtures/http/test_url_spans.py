#!/usr/bin/env python3
"""Compare URL values/errors against the bytewise model in Source/AOT and GC stress."""
import argparse
import json
from pathlib import Path
import random
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[3]


def quoted(value):
    pieces = ['"']
    for char in value:
        if char in ['"', '\\']:
            pieces.append('\\' + char)
        elif ord(char) < 32 or ord(char) == 127:
            pieces.append('\\u{' + format(ord(char), 'x') + '}')
        else:
            pieces.append(char)
    return ''.join(pieces) + '"'


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--rinzu', type=Path, required=True)
    p.add_argument('--before', type=Path, required=True)
    p.add_argument('--workdir', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    work = args.workdir.resolve()
    work.mkdir(parents=True, exist_ok=True)
    shutil.copy2(args.before, work / 'old_url.rinzu')
    shutil.copy2(ROOT / 'tests/fixtures/http/url.rinzu', work / 'new_url.rinzu')
    cases = ['', 'plain', 'a%20b', '+%2B+', '%E6%97%A5%E6%9C%AC', '%F0%9F%99%82',
             '%', '%0', '%G', '%GG', '%0G', '%00', '%2f', '%5C', '%ff', '%c0%af',
             '日%20🙂', '\0%20', 'a'*64+'%20'+'b'*64, '%20'+'+'*128, 'x+'*64+'%21']
    randomizer = random.Random(4191)
    alphabet = ['a', '0', 'F', 'G', '%', '+', '/', '\\', '?', '&', '=', '日', '🙂', '\0']
    for _ in range(256):
        cases.append(''.join(randomizer.choice(alphabet) for _ in range(randomizer.randrange(65))))
    for length in [7, 8, 9, 15, 16, 17, 63, 64, 65, 512]:
        cases.extend(['a'*length+'%20x', '%20'+'a'*length, 'a'*length+'+x%21', '%41'*length])
    program = '''import "old_url.rinzu" as before;
import "new_url.rinzu" as after;
import rinzu.stdio;
fn check(index: Int, value: String, plus: Bool, path: Bool) {
    var oldValue="";var oldMessage="";var oldFailed=false;
    try {oldValue=before.decode(value,plus,path);}
    catch [oldProblem:before.InputError,...oldRest] {oldFailed=true;oldMessage=oldProblem.message;}
    catch oldOthers {panic("unexpectedOld");}
    var newValue="";var newMessage="";var newFailed=false;
    try {newValue=after.decode(value,plus,path);}
    catch [newProblem:after.InputError,...newRest] {newFailed=true;newMessage=newProblem.message;}
    catch newOthers {panic("unexpectedNew");}
    if oldValue!=newValue || oldMessage!=newMessage || oldFailed!=newFailed {
        rinzu.stdio.writeLine(f"URL-MISMATCH:{index}:{plus}:{path}:{oldMessage}:{newMessage}");
        panic("mismatch");
    }
}
fn main(){try{
    let values: List<String> = [__CASES__];
    var index=0;
    while index<listLength(values){
        switch listGet(values,index){
            case Option.Some(value):{
                check(index,value,false,false);check(index,value,false,true);
                check(index,value,true,false);check(index,value,true,true);
            }
            case Option.None:{panic("missing");}
        }
        index=index+1;
    }
    rinzu.stdio.writeLine("URL-DIFFERENTIAL:passed");
}catch errors{panic("unexpected");}}
'''.replace('__CASES__', ','.join(quoted(value) for value in cases))
    entry = work / 'differential.rinzu'
    entry.write_text(program)
    rows = []
    for stress in [False, True]:
        import os
        env = dict(os.environ, RINZU_GC_STRESS='1' if stress else '0')
        run = subprocess.run([str(args.rinzu.resolve()), 'verify-aot', str(entry)], env=env,
                             capture_output=True, text=True, timeout=600)
        assert run.returncode == 0, run.stdout + run.stderr
        assert 'outcome=success' in run.stdout, run
        rows.append(dict(stress=stress, passed=True, stdout=run.stdout, stderr=run.stderr))
    args.output.write_text(json.dumps(dict(passed=True, cases=len(cases)*4, runs=rows), indent=2)+'\n')
    print(json.dumps(dict(passed=True, cases=len(cases)*4)))


if __name__ == '__main__':
    main()
