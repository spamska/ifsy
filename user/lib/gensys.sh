#!/bin/sh
# Emit one C-callable stub per Plan 9 system call, from sys.h.
#
# The kernel takes the call number in %rbp and the arguments as 8-byte words
# on the user stack, starting at 8(%rsp) (the word at 0(%rsp) is where a
# return address would be). The C caller has the arguments in registers, so
# each stub pushes them into that layout, makes the call, and pops them.
echo '	.text'
sed -n 's/^#define[ 	]*\([A-Za-z0-9_]*\)[ 	]*\([0-9]*\).*/\1 \2/p' "$1" | while read name num; do
	low=$(echo "$name" | tr 'A-Z' 'a-z')
	case $low in
	exits|nsec|seek) low=_$low;;
	esac
	cat <<EOT
	.globl	$low
	.type	$low,@function
$low:
	pushq	%rbp
	pushq	%r9
	pushq	%r8
	pushq	%rcx
	pushq	%rdx
	pushq	%rsi
	pushq	%rdi
	pushq	\$0
	movl	\$$num, %ebp
	syscall
	addq	\$56, %rsp
	popq	%rbp
	ret
EOT
done
echo '	.section .note.GNU-stack,"",@progbits'
