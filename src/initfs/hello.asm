BITS 64
section .rodata
msg: db "Hello from user ELF!",10,0
section .text
global _start
_start:
    ; write(1, msg, len)
    mov rax, 1
    mov rdi, 1
    lea rsi, [rel msg]
    mov rdx, msg_end - msg
    syscall

    ; exit(0)
    mov rax, 60
    xor rdi, rdi
    syscall

section .rodata
msg_end:
