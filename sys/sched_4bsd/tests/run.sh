#!/bin/sh
# Host tests for the scheduler.  test_4bsd exercises the policy with no real
# contexts; test_switch really runs cpu_switch/cpu_throw on this machine when
# a port exists for the host architecture.
set -e
cd "$(dirname "$0")/.."
CC=${CC:-cc}
OUT=${TMPDIR:-/tmp}/test_4bsd
case $(uname -m) in
    x86_64|amd64) ARCH=x86_64 ;;
    aarch64|arm64) ARCH=aarch64 ;;
    riscv64) ARCH=riscv64 ;;
    *) ARCH= ;;
esac
SAN=
if echo 'int main(void){return 0;}' | $CC -x c - -fsanitize=address,undefined -o "$OUT.probe" 2>/dev/null; then
    SAN="-fsanitize=address,undefined -fno-sanitize-recover=undefined"
fi
if [ -z "$ARCH" ]; then
    echo "no port for host architecture $(uname -m); skipping"; exit 0
fi
INC="-I c -I arch/$ARCH"
# Policy tests (the context-switch code is compiled in but never entered).
$CC -std=c11 -Wall -Wextra -g -O1 $SAN $INC tests/test_4bsd.c c/4bsd.c arch/$ARCH/machdep.c arch/$ARCH/switch.S -o "$OUT"
"$OUT"
# Real switching: no sanitizers, they do not follow stack switches.
$CC -std=gnu11 -Wall -Wextra -g -O1 $INC tests/test_switch.c c/4bsd.c arch/$ARCH/machdep.c arch/$ARCH/switch.S -o "$OUT.switch"
"$OUT.switch"
