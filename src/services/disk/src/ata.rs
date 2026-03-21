//! ATA (IDE) ディスクドライバ
//!
//! ATA/IDE インターフェースを使用したディスクアクセス実装
//! Primary/Secondary, Master/Slave の4台までのディスクをサポート

use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};
use swiftlib::libc::{inb, outb};
use swiftlib::port::{inw_words, outw_words};

/// ATAポート
#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct AtaPorts {
    /// データレジスタ
    data: u16,
    /// エラー/フィーチャレジスタ
    error_features: u16,
    /// セクタカウント
    sector_count: u16,
    /// LBA Low
    lba_low: u16,
    /// LBA Mid
    lba_mid: u16,
    /// LBA High
    lba_high: u16,
    /// ドライブ/ヘッドセレクト
    drive_head: u16,
    /// ステータス/コマンドレジスタ
    status_command: u16,
    /// コントロールレジスタ (Alternate Status)
    control: u16,
}

impl AtaPorts {
    /// Primary ATAバス（IRQ 14）
    pub const PRIMARY: Self = Self {
        data: 0x1F0,
        error_features: 0x1F1,
        sector_count: 0x1F2,
        lba_low: 0x1F3,
        lba_mid: 0x1F4,
        lba_high: 0x1F5,
        drive_head: 0x1F6,
        status_command: 0x1F7,
        control: 0x3F6,
    };

    /// Secondary ATAバス（IRQ 15）
    pub const SECONDARY: Self = Self {
        data: 0x170,
        error_features: 0x171,
        sector_count: 0x172,
        lba_low: 0x173,
        lba_mid: 0x174,
        lba_high: 0x175,
        drive_head: 0x176,
        status_command: 0x177,
        control: 0x376,
    };
}

/// ATAステータスフラグ
#[allow(dead_code)]
mod status {
    pub const ERR: u8 = 1 << 0;   // エラー
    pub const IDX: u8 = 1 << 1;   // インデックス
    pub const CORR: u8 = 1 << 2;  // 訂正データ
    pub const DRQ: u8 = 1 << 3;   // データ要求
    pub const DSC: u8 = 1 << 4;   // ドライブシーク完了
    pub const DF: u8 = 1 << 5;    // ドライブ故障
    pub const DRDY: u8 = 1 << 6;  // ドライブ準備完了
    pub const BSY: u8 = 1 << 7;   // ビジー
}

/// ATAコマンド
#[allow(dead_code)]
mod command {
    /// 読み取りセクタ
    pub const READ_SECTORS: u8 = 0x20;
    /// 書き込みセクタ
    pub const WRITE_SECTORS: u8 = 0x30;
    /// IDENTIFYドライブ情報
    pub const IDENTIFY: u8 = 0xEC;
    /// キャッシュフラッシュ
    pub const FLUSH_CACHE: u8 = 0xE7;
}

/// ATAドライブタイプ
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriveType {
    /// マスター
    Master,
    /// スレーブ
    Slave,
}

/// ATAエラー
#[derive(Debug, Clone, Copy)]
pub enum AtaError {
    /// タイムアウト
    Timeout,
    /// ドライブ未検出
    NotFound,
    /// I/Oエラー
    IoError,
    /// ドライブが準備できていない
    NotReady,
    /// 無効な引数
    InvalidArgument,
}

pub type AtaResult<T> = Result<T, AtaError>;

/// ATAドライブ
pub struct AtaDrive {
    /// ドライブに対応するI/Oポート
    ports: AtaPorts,
    /// ドライブの種類（マスター/スレーブ）
    drive_type: DriveType,
    /// ドライブのセクタ数
    sectors: u64,
    /// ドライブが初期化されているか
    initialized: AtomicBool,
}

impl AtaDrive {
    /// 新しいATAドライブインスタンスを作成
    pub const fn new(ports: AtaPorts, drive_type: DriveType) -> Self {
        Self {
            ports,
            drive_type,
            sectors: 0,
            initialized: AtomicBool::new(false),
        }
    }

    /// ドライブを初期化して検出
    pub fn init(&mut self) -> AtaResult<()> {
        // ドライブを選択
        self.select_drive();
        self.wait_400ns();

        // IDENTIFYコマンドを送信
        unsafe {
            self.write_command(command::IDENTIFY);
        }

        // ステータスをチェック
        let status = unsafe { self.read_status() };
        if status == 0 || status == 0xFF {
            // ドライブが存在しない
            return Err(AtaError::NotFound);
        }

        // ビジー待ち
        self.wait_not_busy()?;

        // DRQまたはERRを待つ (H-15修正: タイムアウトを追加して無限ループを防ぐ)
        let mut drq_waited = false;
        for _ in 0..50_000 {
            let status = unsafe { self.read_status() };
            if status == 0 || status == 0xFF {
                return Err(AtaError::NotFound);
            }
            if status & status::ERR != 0 {
                return Err(AtaError::IoError);
            }
            if status & status::DRQ != 0 {
                drq_waited = true;
                break;
            }
            core::hint::spin_loop();
        }
        if !drq_waited {
            return Err(AtaError::Timeout);
        }

        // IDENTIFY情報を読み取る（512バイト）
        let mut identify_data = [0u16; 256];
        if inw_words(self.ports.data, &mut identify_data).is_err() {
            return Err(AtaError::IoError);
        }

        // セクタ数を取得（LBA28の場合はワード60-61、LBA48の場合はワード100-103）
        let lba28_sectors = (identify_data[61] as u64) << 16 | identify_data[60] as u64;
        self.sectors = if lba28_sectors != 0 {
            lba28_sectors
        } else {
            // LBA48対応の場合
            (identify_data[103] as u64) << 48
                | (identify_data[102] as u64) << 32
                | (identify_data[101] as u64) << 16
                | identify_data[100] as u64
        };

        self.initialized.store(true, Ordering::Release);
        Ok(())
    }

    /// セクタ数を取得
    pub fn sector_count(&self) -> u64 {
        self.sectors
    }

    /// 初期化済みかチェック
    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// 複数セクタを連続読み取りする（LBA28モード）
    pub fn read_sectors(&self, lba: u64, count: u8, buffer: &mut [u8]) -> AtaResult<()> {
        if !self.is_initialized() {
            return Err(AtaError::NotReady);
        }
        if count == 0 {
            return Err(AtaError::InvalidArgument);
        }

        let count_usize = count as usize;
        let total_bytes = count_usize
            .checked_mul(512)
            .ok_or(AtaError::InvalidArgument)?;
        if buffer.len() < total_bytes {
            return Err(AtaError::InvalidArgument);
        }

        let max_lba_exclusive = 1u64 << 28;
        let end_lba_exclusive = lba
            .checked_add(count as u64)
            .ok_or(AtaError::InvalidArgument)?;
        if end_lba_exclusive > max_lba_exclusive {
            return Err(AtaError::InvalidArgument);
        }

        self.select_drive();
        self.wait_400ns();

        unsafe {
            self.write_lba28(lba);
            self.write_sector_count(count);
            self.write_command(command::READ_SECTORS);
        }

        self.wait_not_busy()?;

        for sector_idx in 0..count_usize {
            self.wait_drq()?;

            let start = sector_idx * 512;
            let end = start + 512;
            let word_buffer = unsafe {
                core::slice::from_raw_parts_mut(buffer[start..end].as_mut_ptr() as *mut u16, 256)
            };
            if inw_words(self.ports.data, word_buffer).is_err() {
                return Err(AtaError::IoError);
            }
        }

        self.wait_not_busy()?;
        Ok(())
    }

    /// セクタを読み取る（LBA28モード）
    pub fn read_sector(&self, lba: u64, buffer: &mut [u8]) -> AtaResult<()> {
        self.read_sectors(lba, 1, buffer)
    }

    /// セクタに書き込む（LBA28モード）
    #[allow(dead_code)]
    pub fn write_sector(&mut self, lba: u64, buffer: &[u8]) -> AtaResult<()> {
        if !self.is_initialized() {
            return Err(AtaError::NotReady);
        }

        if buffer.len() < 512 {
            return Err(AtaError::InvalidArgument);
        }

        if lba >= (1 << 28) {
            return Err(AtaError::InvalidArgument);
        }

        // ドライブを選択してLBAを設定
        self.select_drive();
        self.wait_400ns();

        unsafe {
            self.write_lba28(lba);
            self.write_sector_count(1);
            self.write_command(command::WRITE_SECTORS);
        }

        // DRQ待ち
        self.wait_drq()?;

        // データを書き込む（512バイト = 256ワード）
        let word_buffer =
            unsafe { core::slice::from_raw_parts(buffer.as_ptr() as *const u16, 256) };

        if outw_words(self.ports.data, word_buffer).is_err() {
            return Err(AtaError::IoError);
        }

        // キャッシュフラッシュ
        unsafe {
            self.write_command(command::FLUSH_CACHE);
        }
        self.wait_not_busy()?;

        Ok(())
    }

    /// ドライブを選択
    fn select_drive(&self) {
        let value = match self.drive_type {
            DriveType::Master => 0xE0, // LBA, Master
            DriveType::Slave => 0xF0,  // LBA, Slave
        };
        outb(self.ports.drive_head, value);
    }

    /// LBA28アドレスを書き込む
    unsafe fn write_lba28(&self, lba: u64) {
        let lba_low = (lba & 0xFF) as u8;
        let lba_mid = ((lba >> 8) & 0xFF) as u8;
        let lba_high = ((lba >> 16) & 0xFF) as u8;
        // H-16修正: ドライブ種別に応じてマスタ(0xE0)またはスレーブ(0xF0)を選択する
        // 以前は常に 0xE0 (マスタ) を使用しておりスレーブへのアクセスでデータ破壊の恐れがあった
        let drive_sel: u8 = match self.drive_type {
            DriveType::Master => 0xE0,
            DriveType::Slave => 0xF0,
        };
        let lba_top = (((lba >> 24) & 0x0F) as u8) | drive_sel;

        outb(self.ports.lba_low, lba_low);
        outb(self.ports.lba_mid, lba_mid);
        outb(self.ports.lba_high, lba_high);
        outb(self.ports.drive_head, lba_top);
    }

    /// セクタカウントを書き込む
    unsafe fn write_sector_count(&self, count: u8) {
        outb(self.ports.sector_count, count);
    }

    /// コマンドを書き込む
    unsafe fn write_command(&self, cmd: u8) {
        outb(self.ports.status_command, cmd);
    }

    /// ステータスを読み取る
    unsafe fn read_status(&self) -> u8 {
        inb(self.ports.status_command)
    }

    /// 代替ステータスを読み取る（割り込みフラグをクリアしない）
    #[allow(dead_code)]
    unsafe fn read_alt_status(&self) -> u8 {
        inb(self.ports.control)
    }

    /// ビジーが解除されるまで待つ
    fn wait_not_busy(&self) -> AtaResult<()> {
        for _ in 0..200_000 {
            let status = unsafe { self.read_status() };
            if status == 0 || status == 0xFF {
                return Err(AtaError::NotFound);
            }
            if status & status::BSY == 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(AtaError::Timeout)
    }

    /// DRQフラグが立つまで待つ
    fn wait_drq(&self) -> AtaResult<()> {
        for _ in 0..200_000 {
            let status = unsafe { self.read_status() };
            if status == 0 || status == 0xFF {
                return Err(AtaError::NotFound);
            }
            if status & status::ERR != 0 {
                return Err(AtaError::IoError);
            }
            if status & status::DRQ != 0 {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(AtaError::Timeout)
    }

    /// 400nsの遅延（ポートを4回読むことで実現）
    fn wait_400ns(&self) {
        for _ in 0..4 {
            unsafe {
                let _ = self.read_status();
            }
        }
    }
}

impl fmt::Debug for AtaDrive {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.debug_struct("AtaDrive")
            .field("drive_type", &self.drive_type)
            .field("sectors", &self.sectors)
            .field("initialized", &self.is_initialized())
            .finish()
    }
}
