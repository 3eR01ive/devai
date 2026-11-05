#!/usr/bin/env python3
"""probe cl/short-units — кандидаты длинных функций и глубокой вложенности."""
import re, glob
for f in sorted(glob.glob('crates/*/src/**/*.rs', recursive=True)):
    L = open(f).read().splitlines()
    i = 0
    while i < len(L):
        m = re.match(r'\s{0,8}(?:pub )?fn ([a-zA-Z_0-9]+)', L[i])
        if m:
            d = 0; s = False; j = i
            while j < len(L):
                d += L[j].count('{') - L[j].count('}')
                if '{' in L[j]: s = True
                j += 1
                if s and d <= 0: break
            if j - i > 40:
                print(f'{f}:{i + 1} {m.group(1)} len={j - i}')
            i = j
        else:
            i += 1
