#!/bin/sh
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
	mov	x8, #$num
	svc	#0
	ret
EOT
done
echo '	.section .note.GNU-stack,"",@progbits'
