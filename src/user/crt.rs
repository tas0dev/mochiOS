#![no_std]
#![no_main]

// _startシンボルを定義

use core::arch::global_asm;

global_asm!(
    ".section .text",
    ".global _start",
    "_start:",
    // カーネルによってスタック上に argc, argv, envp が構築されている。
    // RSP は argc を指している。

    // argc を取得 (RDI)
    "pop rdi",

    // argv を取得 (RSI)
    // argc を pop した直後の rsp が argv 配列の先頭を指している
    "mov rsi, rsp",

    // スタックアライメント
    // main 呼び出し前に rsp を 16バイト境界に合わせる
    "and rsp, -16",


    "call main",

    // main の戻り値 (rax) を引数に exit を呼ぶ
    "mov edi, eax",
    "call _exit",
);

extern "C" {
    fn main(argc: i32, argv: *const *const u8) -> i32;
    fn _exit(code: i32) -> !;
}

