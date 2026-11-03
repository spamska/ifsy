#!/bin/sh
# Emit one C-callable stub per Plan 9 system call, from sys.h (riscv64).
#
# riscv64 does not copy the amd64 convention of passing arguments on the user
# stack: the kernel takes the call number in a7 and up to six arguments in
# a0-a5 (the C calling convention's own argument registers), and returns the
# result in a0. So a stub is just "load the number, ecall, return".
echo '	.text'
sed -n 's/^#define[ \t]*\([A-Za-z0-9_]*\)[ \t]*\([0-9]*\).*/\1 \2/p' "$1" | while read name num; do
	low=$(echo "$name" | tr 'A-Z' 'a-z')
	case $low in
	exits|nsec|seek) low=_$low;;
	esac
	cat <<EOT
	.globl	$low
	.type	$low,@function
$low:
	li	a7, $num
	ecall
	ret
EOT
done
echo '	.section .note.GNU-stack,"",@progbits'
