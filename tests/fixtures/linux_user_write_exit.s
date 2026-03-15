	.text
	.globl _start
	.type _start,@function
_start:
	C.BSTART.STD
	addi zero, 1, ->a0
	addtpc .Lmessage, ->a1
	addi a1, .Lmessage, ->a1
	addi zero, 19, ->a2
	addi zero, 64, ->a7
	acrc 1
	addi zero, 0, ->a0
	addi zero, 93, ->a7
	acrc 1
	C.BSTOP

	.section .rodata
.Lmessage:
	.asciz "hello from linxisa\n"
