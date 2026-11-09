#!/usr/bin/env python3
"""Create a typed source SDK scaffold without replacing an existing directory."""
from pathlib import Path
import argparse,re,hashlib

def files(name,language):
    prefix='rinzu_'+name.replace('.','_')+'_'+hashlib.sha256(name.encode()).hexdigest()[:8]
    guard=prefix.upper()+'_H'
    extension={'c':'c','cpp':'cpp','objcxx':'mm'}[language]
    header=f'''#ifndef {guard}
#define {guard}
#include <stdint.h>
#ifdef __cplusplus
#define RINZU_SHIM_NOEXCEPT noexcept
extern "C" {{
#else
#define RINZU_SHIM_NOEXCEPT
#endif
struct ValueResult {{int32_t status;int64_t value;}};
void {prefix}_calculate(int64_t value,struct ValueResult *output) RINZU_SHIM_NOEXCEPT;
#ifdef __cplusplus
}}
#endif
#undef RINZU_SHIM_NOEXCEPT
#endif
'''
    arithmetic='if(value>INT64_MAX-17){output->status=-7604;output->value=0;return;}\n    output->status=0;output->value=value+17;'
    source=f'#include "api.h"\n#include <limits.h>\n'
    if language!='c':source+='#include <stdexcept>\n#include <new>\n#include <cstdlib>\n'
    if language=='objcxx':source+='#import <Foundation/Foundation.h>\n'
    source+=f'void {prefix}_calculate(int64_t value,struct ValueResult *output)'
    if language!='c':source+=' noexcept'
    source+=' {\n'
    if language=='objcxx':source+='  @autoreleasepool {\n'
    if language!='c':source+='  try {\n'
    if language!='c':source+='    if(value==-1) throw std::runtime_error("shim");\n'
    if language=='objcxx':source+='  @try {\n'
    source+='    '+arithmetic+'\n'
    if language=='objcxx':source+='  } @catch(NSException *exception) {(void)exception;output->status=-7602;output->value=0;}\n'
    if language!='c':source+='  } catch(const std::bad_alloc &) {std::_Exit(70);} catch(...) {output->status=-7601;output->value=0;}\n'
    if language=='objcxx':source+='  }\n'
    source+='}\n'
    manifest=f'[package]\nname="{name}"\nlanguage="0.5"\nkind="sdk"\nentry="src/api.rinzu"\n[native]\nsources=["native/src/api.{extension}"]\nheaders=["native/include/api.h"]\ninclude-dirs=["native/include"]\n'
    if language!='c':manifest+='libraries=["c++"]\n'
    if language=='objcxx':manifest+='frameworks=["Foundation"]\n'
    rinzu=f'''import rinzu.native;

native {{
    record ValueResult {{
        status: CInt,
        value: Int
    }}

    fn rawCalculate(value: Int, output: ValueResult) -> Unit symbol "{prefix}_calculate";
}}

fn calculate(value: Int) -> Int {{
    let output = ValueResult(status: CInt(0), value: 0);
    rawCalculate(value, output);
    let status = rinzu.native.checkStatus(Int(output.status), "{name}.calculate");
    return output.value;
}}

export calculate;
'''
    guide=f'''# {name} typed source SDK

Run `rinzu check` and `rinzu build` on this SDK, then declare it as a path dependency.
The public API is calculate(value)->Int. A negative native status throws Error.
Handle failures with a List catch before Main, Worker or a native callback returns.
The output native record is borrowed for the call and automatically written back.
The checked C header is the public ABI. Replace the sample arithmetic inside the shim.

Document external allocator/release, owner/thread, cancellation/physical stop, and non-unwinding facts here.
C++/ObjC++ contain exceptions and OOM; ObjC uses ARC and a call-scope autorelease pool.
No callback registration or native-owned pointer is inferred from this scaffold.
Run `cargo xtask c-fmt` / `c-lint` in the source repository. Keep source/header/link inputs in the manifest.
'''
    return {'rinzu.toml':manifest,'src/api.rinzu':rinzu,'native/include/api.h':header,'native/src/api.'+extension:source,'README.md':guide}

def create(root,name,language):
    if not re.fullmatch(r'[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*',name):raise ValueError('use a dotted lowercase package name')
    if root.exists():raise ValueError('destination already exists; scaffold never overwrites it')
    contents=files(name,language);root.mkdir(parents=True)
    for path,text in contents.items():
        output=root/path;output.parent.mkdir(parents=True,exist_ok=True);output.write_text(text)
    return root
if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('path',type=Path);parser.add_argument('--name',required=True);parser.add_argument('--language',choices=['c','cpp','objcxx'],default='c');args=parser.parse_args()
    try:print(create(args.path,args.name,args.language))
    except ValueError as error:parser.error(str(error))
