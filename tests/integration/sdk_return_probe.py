"""Inject a cancellation window after an actual native return in a private SDK."""
import re


def delayed_native_return(source, name, marker):
    pattern = r'\bfn\s+' + re.escape(name) + r'\s*\(([^)]*)\)\s*->\s*(\w+)\s+symbol'
    matches = list(re.finditer(pattern, source, re.S))
    assert len(matches) == 1, (name, len(matches))
    match = matches[0]
    parameters, result = match.groups()
    arguments = ','.join(item.split(':')[0].strip() for item in parameters.split(',') if item.strip())
    declaration = match.group().replace(name, 'probeNativeReturn', 1)
    source = source[:match.start()] + declaration + source[match.end():]
    delay = f'rinzu.stdio.writeLine("{marker}");var delay=0;while delay<20000 {{delay=delay+1;}}'
    return ('import rinzu.stdio;\n' + source +
            f'\nfn {name}({parameters})->{result} {{let value=probeNativeReturn({arguments});{delay}return value;}}\n')
