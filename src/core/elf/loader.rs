extern crate alloc;
use crate::result::{Kernel, Process};
use alloc::vec::Vec;
use core::convert::TryInto;

/// ELF64ヘッダとプログラムヘッダの定義
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Ehdr {
    /// ELF識別子
    pub e_ident: [u8; 16],
    /// ELFのタイプ
    pub e_type: u16,
    /// マシンアーキテクチャ
    pub e_machine: u16,
    /// ELFのバージョン
    pub e_version: u32,
    /// エントリポイントの仮想アドレス
    pub e_entry: u64,
    /// プログラムヘッダテーブルのファイルオフセット
    pub e_phoff: u64,
    /// セクションヘッダテーブルのファイルオフセット
    pub e_shoff: u64,
    /// ELFフラグ
    pub e_flags: u32,
    /// ELFヘッダのサイズ
    pub e_ehsize: u16,
    /// プログラムヘッダエントリのサイズ
    pub e_phentsize: u16,
    /// プログラムヘッダエントリの数
    pub e_phnum: u16,
    /// セクションヘッダエントリのサイズ
    pub e_shentsize: u16,
    /// セクションヘッダエントリの数
    pub e_shnum: u16,
    /// セクション名文字列テーブルのインデックス
    pub e_shstrndx: u16,
}

/// プログラムヘッダ
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64Phdr {
    /// プログラムヘッダのタイプ
    pub p_type: u32,
    /// プログラムヘッダのフラグ
    pub p_flags: u32,
    /// ファイル内のオフセット
    pub p_offset: u64,
    /// 仮想アドレス
    pub p_vaddr: u64,
    /// 物理アドレス
    pub p_paddr: u64,
    /// ファイル内のサイズ
    pub p_filesz: u64,
    /// メモリ上のサイズ
    pub p_memsz: u64,
    /// アライメント
    pub p_align: u64,
}

/// プログラムヘッダタイプ
pub const PT_NULL: u32 = 0;
/// ロード可能セグメント
pub const PT_LOAD: u32 = 1;

/// ELFヘッダをパースする
///
/// ## Arguments
/// - `data`: ELFファイルのバイト列
///
/// ## Returns
/// ELFヘッダの構造体。パースに失敗した場合はNone
pub fn parse_elf_header(data: &[u8]) -> Option<Elf64Ehdr> {
    if data.len() < 64 {
        return None;
    }

    // ELFヘッダの最初の16バイトは識別子で、残りは固定サイズのフィールド
    let mut e_ident = [0u8; 16];
    e_ident.copy_from_slice(&data[0..16]);

    /// ELFのマジックが正しいか
    if &e_ident[0..4] != b"\x7fELF" {
        return None;
    }

    // 怒涛のパースと代入（）
    let e_type = u16::from_le_bytes(data[16..18].try_into().ok()?);
    let e_machine = u16::from_le_bytes(data[18..20].try_into().ok()?);

    // ELFアーキテクチャ検証: x86-64 (EM_X86_64 = 0x3E) のみ受け付ける (MED-07)
    const EM_X86_64: u16 = 0x3E;
    if e_machine != EM_X86_64 {
        return None;
    }

    let e_version = u32::from_le_bytes(data[20..24].try_into().ok()?);
    let e_entry = u64::from_le_bytes(data[24..32].try_into().ok()?);
    let e_phoff = u64::from_le_bytes(data[32..40].try_into().ok()?);
    let e_shoff = u64::from_le_bytes(data[40..48].try_into().ok()?);
    let e_flags = u32::from_le_bytes(data[48..52].try_into().ok()?);
    let e_ehsize = u16::from_le_bytes(data[52..54].try_into().ok()?);
    let e_phentsize = u16::from_le_bytes(data[54..56].try_into().ok()?);
    let e_phnum = u16::from_le_bytes(data[56..58].try_into().ok()?);
    let e_shentsize = u16::from_le_bytes(data[58..60].try_into().ok()?);
    let e_shnum = u16::from_le_bytes(data[60..62].try_into().ok()?);
    let e_shstrndx = u16::from_le_bytes(data[62..64].try_into().ok()?);

    Some(Elf64Ehdr {
        e_ident,
        e_type,
        e_machine,
        e_version,
        e_entry,
        e_phoff,
        e_shoff,
        e_flags,
        e_ehsize,
        e_phentsize,
        e_phnum,
        e_shentsize,
        e_shnum,
        e_shstrndx,
    })
}

/// プログラムヘッダをパースする
///
/// ## Arguments
/// - `data`: ELFファイルのバイト列
/// - `offset`: プログラムヘッダの開始オフセット
///
/// ## Returns
/// プログラムヘッダの構造体。パースに失敗した場合はNone
pub fn parse_phdr(data: &[u8], offset: usize) -> Option<Elf64Phdr> {
    if data.len() < offset + 56 {
        return None;
    }

    let p_type = u32::from_le_bytes(data[offset..offset + 4].try_into().ok()?);
    let p_flags = u32::from_le_bytes(data[offset + 4..offset + 8].try_into().ok()?);
    let p_offset = u64::from_le_bytes(data[offset + 8..offset + 16].try_into().ok()?);
    let p_vaddr = u64::from_le_bytes(data[offset + 16..offset + 24].try_into().ok()?);
    let p_paddr = u64::from_le_bytes(data[offset + 24..offset + 32].try_into().ok()?);
    let p_filesz = u64::from_le_bytes(data[offset + 32..offset + 40].try_into().ok()?);
    let p_memsz = u64::from_le_bytes(data[offset + 40..offset + 48].try_into().ok()?);
    let p_align = u64::from_le_bytes(data[offset + 48..offset + 56].try_into().ok()?);

    Some(Elf64Phdr {
        p_type,
        p_flags,
        p_offset,
        p_vaddr,
        p_paddr,
        p_filesz,
        p_memsz,
        p_align,
    })
}

/// ロード可能セグメントのリストを取得する
///
/// ## Arguments
/// - `data`: ELFファイルのバイト列
///
/// ## Returns
/// セグメントのベクタ。各セグメントは (仮想アドレス, メモリサイズ, ファイルサイズ, オフセット, フラグ) のタプル。
pub type LoadableSegment = (u64, u64, u64, u64, u32);

pub fn list_loadable_segments(data: &[u8]) -> Option<Vec<LoadableSegment>> {
    let eh = parse_elf_header(data)?;
    let mut res = Vec::new();
    let phoff = eh.e_phoff as usize;
    let phentsize = eh.e_phentsize as usize;
    let phnum = eh.e_phnum as usize;

    for i in 0..phnum {
        let off = phoff + i * phentsize;
        let ph = parse_phdr(data, off)?;
        if ph.p_type == PT_LOAD {
            res.push((ph.p_vaddr, ph.p_memsz, ph.p_filesz, ph.p_offset, ph.p_flags));
        }
    }

    Some(res)
}

/// エントリポイントの仮想アドレスを取得する
///
/// ## Arguments
/// - `data`: ELFファイルのバイト列
///
/// ## Returns
/// エントリポイントの仮想アドレス。パースに失敗した場合はNone
pub fn entry_point(data: &[u8]) -> Option<u64> {
    let eh = parse_elf_header(data)?;
    Some(eh.e_entry)
}
