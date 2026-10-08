"""Checks the float floor-division and floor-modulo algorithm of specs/LSB.md 5.6
against CPython's own `//` and `%`, bit for bit (sign of zero included).

    python dev/verify/float_floor.py

The algorithm is CPython's `_float_div_mod` / `float_rem` (Objects/floatobject.c).
CPython calls C99 `fmod` and `floor`; Python's `math.fmod` raises where C returns
NaN, so `cfmod` below restores the C behaviour. Division by zero is excluded:
CPython raises ZeroDivisionError there, while OPS forbids float errors (LSB gives
NaN). Result on CPython 3.14.4 (2026-10-08): 296,457 cases, 0 mismatches.
"""
import math
import random
import struct


def cfmod(a, b):
    if math.isnan(a) or math.isnan(b) or math.isinf(a) or b == 0:
        return math.nan
    if math.isinf(b):
        return a
    return math.fmod(a, b)


def cfloor(x):
    return x if (math.isnan(x) or math.isinf(x)) else float(math.floor(x))


def spec(a, b):
    m = cfmod(a, b)
    q = (a - m) / b
    if m != 0:  # NaN counts as non-zero, as in C's `if (mod)`
        if (b < 0) != (m < 0):
            m += b
            q -= 1.0
    else:
        m = math.copysign(0.0, b)
    if q != 0:
        fq = cfloor(q)
        if q - fq > 0.5:
            fq += 1.0
    else:
        fq = math.copysign(0.0, a / b)
    return fq, m


def same(x, y):
    return (math.isnan(x) and math.isnan(y)) or struct.pack("<d", x) == struct.pack("<d", y)


def main():
    inf, nan = math.inf, math.nan
    edge = [0.0, -0.0, 1.0, -1.0, 7.0, -7.0, 2.0, -2.0, 0.5, -0.5, 1e308, -1e308,
            5e-324, -5e-324, inf, -inf, nan, 2.0**53 + 1, 3.0, 1e100, -1e-100]
    cases = [(a, b) for a in edge for b in edge if b != 0]
    rnd = random.Random(1)

    def anybits():
        return struct.unpack("<d", rnd.getrandbits(64).to_bytes(8, "little"))[0]

    for _ in range(300000):
        a = rnd.choice([rnd.uniform(-1e6, 1e6), rnd.uniform(-10, 10), float(rnd.randint(-100, 100)), anybits()])
        b = rnd.choice([rnd.uniform(-1e6, 1e6), rnd.uniform(-10, 10), float(rnd.randint(-9, 9)), anybits()])
        if b != 0:
            cases.append((a, b))
    bad = 0
    for a, b in cases:
        fq, m = spec(a, b)
        if not (same(fq, a // b) and same(m, a % b)):
            bad += 1
            print("MISMATCH", a, b, (fq, m), (a // b, a % b))
    print(f"cases {len(cases)} mismatches {bad}")


if __name__ == "__main__":
    main()
