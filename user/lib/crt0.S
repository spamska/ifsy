/* Entry point. The kernel starts a new image as entry(argc, argv) with the
 * arguments in rdi/rsi (System V registers) and a 16-byte aligned stack. */
	.text
	.globl	_start
	.type	_start,@function
_start:
	xorl	%ebp, %ebp
	andq	$-16, %rsp
	call	_main
	ud2
	.section .note.GNU-stack,"",@progbits
