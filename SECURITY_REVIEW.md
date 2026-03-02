# SwiftCore セキュリティ・アーキテクチャ レビュー

**作成日**: 2026-03-02
**対象**: SwiftCore OS カーネル全体
**重大度凡例**: 🔴 Critical / 🟠 High / 🟡 Medium / 🔵 Low / ℹ️ Informational

---

## エグゼクティブ・サマリー

SwiftCore はRustで実装されたx86-64向けハイブリッドカーネルOSであり、言語レベルでのメモリ安全性は担保されている。しかし、OS カーネルとしての **セキュリティ境界**（カーネル空間とユーザー空間の分離）に関する重大な脆弱性が多数存在する。主な問題はシステムコールハンドラにおける **ユーザー空間ポインタの無検証なデリファレンス**であり、悪意のあるユーザープロセスがカーネルメモリを読み書きできる状態にある。

---

## 目次

1. [アーキテクチャ概要](#アーキテクチャ概要)
2. [Critical 脆弱性](#critical-脆弱性)
3. [High 脆弱性](#high-脆弱性)
4. [Medium 脆弱性](#medium-脆弱性)
5. [Low 脆弱性・バグ](#low-脆弱性バグ)
6. [Informational / 設計上の懸念点](#informational--設計上の懸念点)
7. [修正優先度マトリクス](#修正優先度マトリクス)

---

## アーキテクチャ概要

```
┌─────────────────────────────────────────────┐
│           ユーザー空間 (Ring 3)              │
│  ┌──────────┐ ┌──────────┐ ┌─────────────┐  │
│  │core.svc  │ │disk.svc  │ │ fs.service  │  │
│  └──────────┘ └──────────┘ └─────────────┘  │
│       ↑             ↑              ↑         │
│   syscall(int 0x80 / SYSCALL instruction)    │
├─────────────────────────────────────────────┤
│           カーネル空間 (Ring 0)              │
│  ┌────────┐ ┌────────┐ ┌────────┐           │
│  │ ELF    │ │ Task   │ │ Memory │           │
│  │ Loader │ │ Sched  │ │ Mgr    │           │
│  └────────┘ └────────┘ └────────┘           │
│  ┌──────────────────────────────┐            │
│  │    Syscall Dispatcher        │            │
│  │  (src/core/syscall/mod.rs)   │            │
│  └──────────────────────────────┘            │
│  ┌────────┐ ┌────────┐ ┌────────┐           │
│  │ IDT    │ │ GDT    │ │ Paging │           │
│  └────────┘ └────────┘ └────────┘           │
└─────────────────────────────────────────────┘
```

### 使用する主要機能
- **SYSCALL/SYSRET**: モダンなシステムコール機構（MSR設定済み）
- **int 0x80**: レガシー割り込みベースのシステムコール（同時サポート）
- **4段階ページテーブル**: x86-64 標準ページング
- **PIC 8259A**: ハードウェア割り込みコントローラ
- **プリエンプティブスケジューラ**: タイムスライス型ラウンドロビン

---

## Critical 脆弱性

### CRIT-01: 🔴 ユーザー空間ポインタの全面的な無検証 (カーネルメモリ読み書き可能)

**ファイル**: `src/core/syscall/io.rs`, `src/core/syscall/ipc.rs`, `src/core/syscall/process.rs`, `src/core/syscall/fs.rs`
**CVSS**: 9.8 (Critical)

**問題の詳細**:
システムコールハンドラが受け取るユーザー空間のポインタを**一切検証せずに**カーネル内でデリファレンスしている。攻撃者はカーネルアドレスを引数として渡すことで、カーネルメモリを任意に読み書きできる。

**具体的な箇所**:

```rust
// src/core/syscall/io.rs:50-53
// TODO: ユーザー空間のアドレスが有効か適切に検証する  ← 自ら認識している問題
let buf = unsafe {
    slice::from_raw_parts(buf_ptr as *const u8, len as usize)
};
```

```rust
// src/core/syscall/process.rs:235-237
if status_ptr != 0 {
    unsafe {
        *(status_ptr as *mut i32) = 0;  // wait() syscall - 無検証
    }
}
```

```rust
// src/core/syscall/process.rs:396-399
ARCH_GET_FS => {
    // ...
    unsafe { core::ptr::write(addr as *mut u64, val) };  // 無検証で書き込み
}
```

```rust
// src/core/syscall/process.rs:358-360
FUTEX_WAIT => {
    let current_val = unsafe { core::ptr::read_volatile(uaddr as *const u32) };
    // uaddr はnullチェックのみ、範囲チェックなし
}
```

**影響**: ユーザープロセスがカーネルメモリを任意読み書き → 完全な権限昇格

**修正方針**:
```rust
/// ユーザーポインタの有効性を検証する関数を実装
fn validate_user_ptr(ptr: u64, len: u64) -> bool {
    if ptr == 0 { return false; }
    // ユーザー空間の上限アドレス (0x0000_7FFF_FFFF_FFFF)
    const USER_SPACE_END: u64 = 0x0000_7FFF_FFFF_FFFF;
    let end = ptr.checked_add(len).unwrap_or(u64::MAX);
    end <= USER_SPACE_END && ptr < USER_SPACE_END
}
```

---

### CRIT-02: 🔴 I/Oポートへの無制限アクセス (ハードウェア直接制御)

**ファイル**: `src/core/syscall/io_port.rs`
**CVSS**: 9.1 (Critical)

**問題の詳細**:
`PortIn` (syscall 520) および `PortOut` (syscall 521) が**全ての**ユーザープロセスから呼び出せる。ポート番号の範囲チェックのみで、呼び出しプロセスの権限チェックが存在しない。

```rust
// src/core/syscall/io_port.rs:14-18
pub fn port_in(port: u64, size: u64) -> u64 {
    if port > 0xFFFF {
        return EINVAL;
    }
    // 権限チェックなし！ 任意のユーザープロセスがハードウェアに直接アクセス可能
    let port = port as u16;
    unsafe {
        match size {
            1 => { /* inb */ }
            // ...
        }
    }
}
```

**攻撃シナリオ**:
- `port_out(0x70, 0x00, 1)` → CMOSクリアでBIOSリセット
- `port_out(0x20, 0x20, 1)` → PIC EOI操作（割り込み制御の妨害）
- `port_in(0x60, 1)` → PS/2コントローラ直接アクセス
- ATA/IDEポート(0x1F0-0x1F7)への直接ディスクアクセス
- DMAコントローラ操作によるメモリ読み書き

**修正方針**: 特権プロセス（サービス）のみがポートアクセスを許可するよう、呼び出し元プロセスの権限レベル確認を追加する。

---

### CRIT-03: 🔴 Fork がページテーブルを共有 (プロセス分離なし)

**ファイル**: `src/core/syscall/process.rs:169`
**CVSS**: 8.8 (Critical)

**問題の詳細**:
`fork()` システムコールの実装が、親プロセスと子プロセスが**同一の物理メモリ**を共有するページテーブルをそのまま使用する。Copy-on-Write (CoW) が実装されていないため、プロセス分離が完全に欠落している。

```rust
// src/core/syscall/process.rs:168-170
let mut child_proc = crate::task::Process::new(/* ... */);
child_proc.set_page_table(parent_pt);  // 同一物理ページテーブルを共有！
```

**影響**:
- 子プロセスへの書き込みが親プロセスのメモリを直接変更
- プロセス間のメモリ分離が完全に破綻
- 悪意のある子プロセスが親プロセスのスタック/ヒープを破壊・盗み見できる
- マルチプロセス環境での競合状態が発生する

**修正方針**: fork時に新しいページテーブルを作成し、各ページを物理的にコピー（CoW実装またはフル物理コピー）する。

---

### CRIT-04: 🔴 ELFローダーにおける境界チェックなし (バッファオーバーリード)

**ファイル**: `src/core/syscall/exec.rs:98`, `src/core/syscall/exec.rs:561`
**CVSS**: 8.6 (Critical)

**問題の詳細**:
ELFプログラムヘッダから読み取った値（`p_offset`, `p_filesz`）を検証せずにバッファのスライス計算に使用している。整数オーバーフローまたは範囲外アクセスによりカーネルパニックまたはメモリ破壊が発生する。

```rust
// src/core/syscall/exec.rs:98 (exec_internal)
let seg_src = &data[src_off..src_off + filesz as usize];
// ↑ src_off + filesz がオーバーフロー、またはdata.len()を超える場合にパニック

// src/core/syscall/exec.rs:561 (execve_syscall)
let seg_src =
    &data[ph.p_offset as usize..ph.p_offset as usize + ph.p_filesz as usize];
// ↑ 同様の問題
```

**攻撃シナリオ**:
- `p_filesz = u64::MAX` → `usize`変換でオーバーフロー → 巨大スライス作成
- `p_offset = data.len() - 1, p_filesz = 2` → 範囲外アクセスでパニック
- initfsに悪意のあるELFを含めることでDoSが可能

**追加問題**: ELFヘッダの `e_machine` フィールドが検証されていないため、非x86-64 ELFファイルを処理しようとする。

**修正方針**:
```rust
// 安全な範囲チェック
let src_end = src_off.checked_add(filesz as usize)
    .filter(|&e| e <= data.len())
    .ok_or(EINVAL)?;
let seg_src = &data[src_off..src_end];
// e_machine == 0x3E (EM_X86_64) の検証も追加
```

---

### CRIT-05: 🔴 ELFセグメントをカーネルアドレスにマップ可能

**ファイル**: `src/core/mem/paging.rs:550-644` (`map_and_copy_segment_to`)
**CVSS**: 8.4 (Critical)

**問題の詳細**:
ELFローダーが `p_vaddr`（仮想アドレス）の値を検証しないため、ユーザープロセスのELFがカーネル空間（アドレス `0xFFFF_8000_0000_0000` 以上）にセグメントをマップしようとしても拒否されない。`USER_ACCESSIBLE` フラグが付与されていてもカーネルメモリ領域へのマップは危険。

```rust
// src/core/syscall/exec.rs:83-89
let vaddr = ph.p_vaddr;  // 検証なし
let memsz = ph.p_memsz; // 検証なし
// ...
crate::mem::paging::map_and_copy_segment_to(new_pt_phys, vaddr, ...)
// ↑ vaddr がカーネルアドレスでも処理を続ける
```

**修正方針**: `vaddr + memsz < USER_SPACE_END` の検証を追加する。

---

## High 脆弱性

### HIGH-01: 🟠 シングルCPU前提のグローバル状態によるfork()競合状態

**ファイル**: `src/core/syscall/syscall_entry.rs:17-29`, `src/core/syscall/process.rs:133-135`
**CVSS**: 7.5 (High)

**問題の詳細**:
SYSCALL時のユーザーRSP/RIP/RFLAGSをグローバルな`AtomicU64`に保存しており、`fork()`がこれらの値を読み取る。シングルCPU前提だが、タイマー割り込みによるコンテキストスイッチ時にこれらの値が上書きされる可能性がある（TOCTOU競合）。

```rust
// syscall_entry.rs
pub static SYSCALL_TEMP_USER_RSP: AtomicU64 = AtomicU64::new(0);
pub static SYSCALL_SAVED_USER_RIP: AtomicU64 = AtomicU64::new(0);
pub static SYSCALL_SAVED_USER_RFLAGS: AtomicU64 = AtomicU64::new(0);

// process.rs - fork()
let user_rsp = SYSCALL_TEMP_USER_RSP.load(Ordering::Relaxed);
let user_rip = SYSCALL_SAVED_USER_RIP.load(Ordering::Relaxed);
// ↑ syscall entryで保存→fork()で読み取るまでの間に割り込みが入ると不整合
```

**影響**: fork()で誤ったRSP/RIPを持つ子スレッドが生成され、システムクラッシュまたは意図しないコード実行が発生する可能性がある。

---

### HIGH-02: 🟠 ファイルディスクリプタテーブルのUAF競合（use-after-free）

**ファイル**: `src/core/syscall/fs.rs:244-264`
**CVSS**: 7.8 (High)

**問題の詳細**:
`fs::read()`でスピンロックを解放した後、生ポインタ経由で`FileHandle`にアクセスしている。別スレッドが同時に`close()`を呼び出すと、解放済みメモリへのアクセス（use-after-free）が発生する。

```rust
// src/core/syscall/fs.rs:244-264
let mut table = FD_TABLE.lock();
let ptr = table[idx];
// ...
table[idx] = table[idx];  // no-op
drop(table);               // ← ロック解放

// ← ここで他スレッドがclose()を実行するとptrが指すメモリが解放される
let fh = unsafe { &mut *(ptr as *mut FileHandle) };  // UAF!
```

---

### HIGH-03: 🟠 brk/mmap のアドレス範囲検証なし（カーネルアドレスへの書き込み）

**ファイル**: `src/core/syscall/process.rs:53-122, 261-334`
**CVSS**: 7.6 (High)

**問題の詳細**:
`brk()`と`mmap()`に渡すアドレスがユーザー空間の範囲内かどうかチェックしていない。悪意のあるユーザープロセスがカーネルアドレスを指定すると、カーネルメモリ領域にページをマップできる可能性がある。

```rust
// src/core/syscall/process.rs:295-297
let map_start = if addr != 0 {
    (addr + 4095) & !4095  // カーネルアドレスでもそのまま使用
} else { /* ... */ };
```

---

### HIGH-04: 🟠 カーネルスタックプールにガードページなし

**ファイル**: `src/core/task/thread.rs:48-66`
**CVSS**: 7.0 (High)

**問題の詳細**:
カーネルスタックを固定サイズのプール(`KSTACK_POOL = [u8; 4096 * 64]`)から連続割り当てしており、スタック間にガードページが存在しない。あるスレッドのカーネルスタックオーバーフローが隣接スレッドのスタックまたはデータを上書きする。

```rust
// src/core/task/thread.rs:48-66
const KSTACK_POOL_SIZE: usize = 4096 * 64; // 256 KiB
static mut KSTACK_POOL: [u8; KSTACK_POOL_SIZE] = [0; KSTACK_POOL_SIZE];
// ガードページなし - スタックオーバーフローが検出されない
```

---

### HIGH-05: 🟠 フレームアロケータに解放機能なし（物理メモリリーク）

**ファイル**: `src/core/mem/frame.rs`
**CVSS**: 7.2 (High)

**問題の詳細**:
`BitmapFrameAllocator`は物理フレームの割り当て(`allocate_frame`)のみを実装しており、**解放機能が存在しない**。プロセス終了時にELFセグメント、スタック、ヒープに使用した物理フレームが解放されないため、システムが長時間稼働すると物理メモリが枯渇する。

```rust
// src/core/mem/frame.rs:80-113
unsafe impl FrameAllocator<Size4KiB> for BitmapFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame> {
        // 割り当てのみ実装
        // deallocate_frame() が存在しない
    }
}
```

---

### HIGH-06: 🟠 ページングの全物理メモリに WRITABLE フラグ（カーネルコード書き換え可能）

**ファイル**: `src/core/mem/paging.rs:118`
**CVSS**: 7.4 (High)

**問題の詳細**:
ページング初期化時に物理メモリの全領域を `PRESENT | WRITABLE` でマップしており、読み取り専用にすべきカーネルコードセクションも書き込み可能になっている。

```rust
// src/core/mem/paging.rs:118
let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
// カーネルコード(.text)も書き込み可能！WPビット非考慮
```

**影響**: 権限昇格に成功した攻撃者がカーネルコードを書き換えてバックドアを設置できる。

---

## Medium 脆弱性

### MED-01: 🟡 プロセス名による認可（なりすまし攻撃）

**ファイル**: `src/core/syscall/exec.rs:32-41`
**CVSS**: 6.5 (Medium)

**問題の詳細**:
`.service` ファイルの実行権限チェックがプロセス名の文字列比較に依存しており、セキュアなIDベースの認可ではない。

```rust
// src/core/syscall/exec.rs:32-41
let caller_is_core = crate::task::with_process(pid, |p| {
    let name = p.name();
    name == "core.service" || name == "core"  // 名前での判定
}).unwrap_or(false);
```

プロセス名は `exec_kernel(path, name_override)` の `name_override` を通じて設定できる。カーネル内部関数 `exec_kernel_with_name` を呼び出すことで任意の名前を設定可能。

---

### MED-02: 🟡 KPTI（カーネルページテーブル分離）未実装

**ファイル**: `src/core/syscall/mod.rs:165` (コメント参照)
**CVSS**: 6.2 (Medium)

**問題の詳細**:
ユーザープロセスのページテーブルにカーネルマッピングが含まれており、KPTI（Kernel Page-Table Isolation）が実装されていない。Meltdown脆弱性(CVE-2017-5754)のような投機的実行攻撃に対して無防備。

```rust
// src/core/syscall/mod.rs コメント
// ユーザーのページテーブルはカーネルのマッピングをすべて含んでいるため、
// CR3の切り替えは不要。ユーザーメモリへのアクセスもそのまま可能。
```

---

### MED-03: 🟡 ユーザースタックに NX ビットなし（user.rs の alloc_user_stack）

**ファイル**: `src/core/mem/user.rs:57-59`
**CVSS**: 5.6 (Medium)

**問題の詳細**:
`alloc_user_stack()` はスタックページを `NO_EXECUTE` フラグなしでマップする。スタック上のシェルコードが実行可能になる。

```rust
// src/core/mem/user.rs:57-59
let flags = PageTableFlags::PRESENT
    | PageTableFlags::WRITABLE
    | PageTableFlags::USER_ACCESSIBLE;
// NO_EXECUTE フラグがない！スタックが実行可能
```

（注: `exec.rs` 側ではスタックを `executable=false` でマップしているが、`user.rs` の `alloc_user_stack` を使用する場合は NX が設定されない。）

---

### MED-04: 🟡 ASLR（アドレス空間配置ランダム化）未実装

**ファイル**: `src/core/syscall/exec.rs:243-245`
**CVSS**: 5.4 (Medium)

**問題の詳細**:
全プロセスが固定仮想アドレスに配置される。スタックは `0x0000_7FFF_FFF0_0000`、ヒープは `0x4000_0000` に固定。ASLRがないため、Return-to-libc攻撃や ROP チェーン構築が容易になる。

```rust
// src/core/syscall/exec.rs:243-245
let stack_end_vaddr: u64 = 0x0000_7FFF_FFF0_0000;  // 固定アドレス
let default_heap_base: u64 = 0x4000_0000;            // 固定アドレス
```

---

### MED-05: 🟡 セキュリティクリティカルな AtomicU64 に Relaxed 順序付け

**ファイル**: `src/core/syscall/syscall_entry.rs:96`
**CVSS**: 5.5 (Medium)

**問題の詳細**:
カーネルスタックポインタ（SYSCALL時に使用）の更新に `Ordering::Relaxed` を使用している。メモリ順序の保証が弱く、マルチコア環境では他のCPUから古い値が見える可能性がある。

```rust
// src/core/syscall/syscall_entry.rs:96
pub fn update_kernel_rsp(rsp: u64) {
    SYSCALL_KERNEL_RSP.store(rsp, Ordering::Relaxed);  // SeqCstまたはReleaseが必要
}
```

---

### MED-06: 🟡 IPC メッセージのサイズ検証バイパス可能

**ファイル**: `src/core/syscall/ipc.rs:71-109`
**CVSS**: 5.3 (Medium)

**問題の詳細**:
IPC送信で `len > MAX_MSG_SIZE(256)` のチェックは行うが、ユーザーポインタの検証がない（CRIT-01と関連）。また、メールボックスが満杯（`MAILBOX_CAP=64`）のとき `EAGAIN` を返すが、送信元スレッドIDは信頼できるソースから取得しているため偽装は困難。ただし、メールボックスをスパムすることでDoSが可能。

---

### MED-07: 🟡 ELF アーキテクチャ検証なし

**ファイル**: `src/core/elf/loader.rs:74-119`
**CVSS**: 5.0 (Medium)

**問題の詳細**:
ELFヘッダの `e_machine` フィールド（アーキテクチャ識別子）が検証されていない。ARM等の非x86-64 ELFを誤って読み込む可能性がある。

```rust
// src/core/elf/loader.rs:88-90 付近
let e_machine = u16::from_le_bytes(data[18..20].try_into().ok()?);
// e_machine == 0x3E (EM_X86_64) のチェックなし
```

---

### MED-08: 🟡 ELF phnum オーバーフロー

**ファイル**: `src/core/syscall/exec.rs:79-80`
**CVSS**: 5.0 (Medium)

**問題の詳細**:
`phnum * phentsz` の乗算がオーバーフローし、`parse_phdr` に不正なオフセットを渡す可能性がある。`phentsz`が0の場合、無限ループが発生する。

```rust
// src/core/syscall/exec.rs:79-80
let phnum = eh.e_phnum as usize;
for i in 0..phnum {
    let off_hdr = phoff + i * phentsz;  // オーバーフロー可能
```

---

## Low 脆弱性・バグ

### LOW-01: 🔵 generic_interrupt_handler の EOI 送信が不正確

**ファイル**: `src/core/interrupt/idt.rs:772-775`
**CVSS**: 3.5 (Low)

**問題の詳細**:
全ての割り込みでマスター・スレーブ両方の PIC に EOI（End of Interrupt）を送信している。IRQ 0-7（マスターのみ）の場合、スレーブへの EOI は不正であり、スプリアス割り込みの原因になる。

```rust
unsafe {
    super::pic::PIC_SLAVE.end_of_interrupt();
    super::pic::PIC_MASTER.end_of_interrupt();
}
```

---

### LOW-02: 🔵 execve_syscall が `.service` ファイルを常に拒否（core.service を含む）

**ファイル**: `src/core/syscall/exec.rs:528-530`
**CVSS**: 3.1 (Low - 機能バグ)

**問題の詳細**:
`execve_syscall` は `.service` 拡張子のファイルを常に `EPERM` で拒否するが、`exec_kernel` は `core.service` からの呼び出しのみ許可する。この非一貫性により、正規の `execve()` によるサービス起動が完全に不可能になっている。

---

### LOW-03: 🔵 wait() syscall が実質的に未実装

**ファイル**: `src/core/syscall/process.rs:231-246`
**CVSS**: 2.5 (Low - 機能バグ)

**問題の詳細**:
`wait()` システムコールが pid/options を無視し、常に status=0 を書き込んでブロッキング待機を実装していない。POSIX セマンティクスとの互換性が得られない。

```rust
pub fn wait(_pid: u64, status_ptr: u64, options: u64) -> u64 {
    // pid は完全に無視される
    if status_ptr != 0 {
        unsafe { *(status_ptr as *mut i32) = 0; }  // 常に0
    }
    // ブロッキング待機なし
}
```

---

### LOW-04: 🔵 munmap() が未実装（ページ解放なし）

**ファイル**: `src/core/syscall/process.rs:337-340`
**CVSS**: 2.3 (Low - メモリリーク)

```rust
pub fn munmap(_addr: u64, _length: u64) -> u64 {
    // TODO: ページテーブルからマッピングを削除する
    SUCCESS  // 常に成功を返すが何もしない
}
```

---

### LOW-05: 🔵 sleep() が不正確（タイマーベースでない）

**ファイル**: `src/core/syscall/process.rs:212-223`
**CVSS**: 2.0 (Low - 機能バグ)

```rust
pub fn sleep(milliseconds: u64) -> u64 {
    let yield_count = (milliseconds / 10).max(1).min(100);
    for _ in 0..yield_count {
        crate::task::yield_now();  // タイマーでなくyield回数で代替
    }
    SUCCESS
}
```

実際のスリープ時間がシステム負荷に依存し、指定ミリ秒と全く異なる可能性がある。

---

### LOW-06: 🔵 デバッグコードが本番コードに残存（情報漏洩リスク）

**ファイル**: `src/core/task/context.rs:310-460`
**CVSS**: 2.8 (Low)

**問題の詳細**:
`switch_to_thread_from_isr` 関数内に、コンテキストスイッチのたびにGDTエントリ、ユーザーコードのバイト列、スタック内容をシリアルコンソールにダンプするデバッグコードが残存している。

```rust
// context.rs - 毎回のコンテキストスイッチで実行される
crate::info!("user code @ {:#x}: {:02x?}", saved.rip, bytes);
crate::info!("user stack @ {:#x}: {:02x?}", saved.rsp, bytes);
// GDTエントリの詳細ダンプ...
```

**影響**: パフォーマンス低下 + シリアルポートに機密情報が漏洩。

---

### LOW-07: 🔵 static mut KSTACK_POOL への unsafe アクセス

**ファイル**: `src/core/task/thread.rs:64-66`
**CVSS**: 3.0 (Low)

```rust
static mut KSTACK_POOL: [u8; KSTACK_POOL_SIZE] = [0; KSTACK_POOL_SIZE];
// ...
let ptr = unsafe { &raw const KSTACK_POOL as *const _ as usize + off } as u64;
```

`static mut` への raw pointer アクセスは未定義動作の可能性がある。アトミック操作やMutexで保護すべき。

---

### LOW-08: 🔵 fstat() が FD を検証しない

**ファイル**: `src/core/syscall/fs.rs:142-149`
**CVSS**: 2.5 (Low)

```rust
pub fn fstat(fd: u64, stat_ptr: u64) -> u64 {
    if stat_ptr == 0 { return EFAULT; }
    let _ = fd;  // fd は完全に無視
    SUCCESS  // 常に成功
}
```

任意のFDに対して成功を返し、statの内容も書き込まない。stat構造体が初期化されないまま使用されるリスク。

---

## Informational / 設計上の懸念点

### INFO-01: ℹ️ Thread ID の予測可能性（IPC なりすまし）

**ファイル**: `src/core/task/ids.rs` (推測), `src/core/syscall/ipc.rs`

スレッドIDは単調増加するカウンタから割り当てられる。攻撃者が「次に生成されるスレッドIDは何番か」を予測可能であり、特定サービスを対象としたIPCのターゲティングに悪用できる。

---

### INFO-02: ℹ️ カーネルヒープの単一アロケータ

`linked_list_allocator` を使用しているが、OOM発生時に現在のプロセスを終了させる実装は、カーネル自身のアロケーションが失敗した場合にシステム全体がパニックする可能性がある。

---

### INFO-03: ℹ️ initfs はメモリ上の読み取り専用FS（設計として正常）

`exec_kernel` によるサービス起動はinitfsからのみ読み取るため、外部から悪意のある実行ファイルを注入することはできない（現時点ではinitfsに含まれるファイルのみ実行可能）。

---

### INFO-04: ℹ️ シングルCPU設計の明示が必要

コードベース全体が `// シングルCPU前提` というコメントを持つが、マルチコア対応時には広範なリファクタリングが必要になる。

---

### INFO-05: ℹ️ カーネルスタックサイズが小さい（4096 * 4 = 16KiB）

```rust
// exec.rs
const KERNEL_THREAD_STACK_SIZE: usize = 4096 * 4;
```

再帰的な処理やデバッグ出力（大量のformat!マクロ）でスタックが枯渇しやすい。特にIST（Double Fault Stack）のサイズも確認が必要。

---

## 修正優先度マトリクス

| ID | 重大度 | 影響範囲 | 修正難易度 | 優先度 |
|----|--------|----------|-----------|--------|
| CRIT-01 | Critical | 全syscall | 中（検証関数追加） | **最優先** |
| CRIT-02 | Critical | 全プロセス | 低（権限チェック追加） | **最優先** |
| CRIT-04 | Critical | ELF実行 | 低（範囲チェック追加） | **最優先** |
| CRIT-03 | Critical | fork() | 高（CoW実装） | 高 |
| CRIT-05 | Critical | ELF実行 | 低（アドレス検証） | 高 |
| HIGH-02 | High | FS syscall | 低（ロック保持延長） | 高 |
| HIGH-06 | High | メモリ全体 | 中（ページフラグ修正） | 高 |
| HIGH-04 | High | スレッド | 中（ガードページ追加） | 中 |
| HIGH-05 | High | メモリ管理 | 高（フレーム解放実装） | 中 |
| HIGH-01 | High | fork() | 中（per-CPUデータ） | 中 |
| HIGH-03 | High | mmap/brk | 低（範囲チェック） | 中 |
| MED-01 | Medium | exec | 高（ID-based認可） | 中 |
| MED-02 | Medium | 全体 | 高（KPTI実装） | 低 |
| MED-03 | Medium | スタック | 低（フラグ追加） | 低 |
| MED-04 | Medium | 全体 | 中（乱数生成必要） | 低 |
| MED-05 | Medium | syscall | 低（Ordering変更） | 低 |
| LOW-06 | Low | パフォーマンス | 低（コード削除） | すぐに対応 |
| LOW-01 | Low | 割り込み | 低（EOI制御修正） | 低 |
| LOW-03/04 | Low | 機能 | 高（実装） | バックログ |

---

## 推奨される修正アプローチ（フェーズ別）

### フェーズ1: 即時対応（クリティカル修正）

1. **ユーザーポインタ検証関数の実装**
   - `is_valid_user_ptr(ptr, len) -> bool` をカーネルに追加
   - ユーザー空間上限（`0x0000_7FFF_FFFF_FFFF`）との比較
   - 全syscallハンドラで一貫して使用

2. **I/Oポートアクセスの権限チェック**
   - 呼び出し元プロセスの `PrivilegeLevel` を確認
   - `Service` または `Core` 権限のみ許可

3. **ELFローダーの境界チェック強化**
   - `p_offset + p_filesz` の checked_add
   - `e_machine` の検証（`EM_X86_64 = 0x3E`）
   - `p_vaddr` のユーザー空間制限チェック

4. **デバッグログの削除**
   - `context.rs` の `switch_to_thread_from_isr` 内のデバッグダンプを除去

### フェーズ2: 短期対応（Highリスク修正）

5. **FDテーブルのロック範囲修正（UAF修正）**
   - `fs::read()` でロックを保持したままFileHandleにアクセス

6. **ページングフラグの適切な設定**
   - カーネル`.text`セクションを読み取り専用でマップ

7. **カーネルスタックガードページ**
   - スタック割り当て時に1ページのガードページを追加

8. **MmapとBrkのアドレス範囲検証**

### フェーズ3: 中期対応（システム強化）

9. **物理フレームの解放機能実装**
10. **Forkの物理コピー実装（CoWまたはフルコピー）**
11. **ASLRの実装**
12. **ユーザースタックへのNXビット設定統一**

### フェーズ4: 長期対応（根本的なアーキテクチャ改善）

13. **KPTI（カーネルページテーブル分離）の実装**
14. **per-CPU データ構造の導入（マルチコア対応）**
15. **ID-based プロセス認可機能**
16. **wait()/munmap() の完全実装**

---

*このレビューは2026年3月2日に実施されたアーキテクチャおよびコード静的解析に基づく。動的解析・ファジングテストは実施していないため、追加の脆弱性が存在する可能性がある。*
