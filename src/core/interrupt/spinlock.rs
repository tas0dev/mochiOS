//! 割込み安全なスピンロック実装
//!
//! 割込みコンテキストでも安全に使用できるスピンロックを提供

use core::cell::UnsafeCell;
use core::ops::{Deref, DerefMut};
use core::sync::atomic::{AtomicBool, Ordering};

/// 割込み安全なスピンロック
///
/// ロック取得時に割込みを無効化し、解放時に復元することで
/// デッドロックを防止する
pub struct SpinLock<T> {
    /// ロック状態を表すフラグ
    locked: AtomicBool,
    /// 保護されるデータ
    data: UnsafeCell<T>,
}

unsafe impl<T: Send> Sync for SpinLock<T> {}
unsafe impl<T: Send> Send for SpinLock<T> {}

impl<T> SpinLock<T> {
    /// 新しいスピンロックを作成
    ///
    /// ## Arguments
    /// - `data`: ロックで保護されるデータ
    ///
    /// ## Returns
    /// - `SpinLock<T>`: 新しいスピンロックインスタンス
    pub const fn new(data: T) -> Self {
        Self {
            locked: AtomicBool::new(false),
            data: UnsafeCell::new(data),
        }
    }

    /// ロックを取得（割込みを無効化）
    ///
    /// ロック取得時に割込みフラグの状態を保存し、
    /// 割込みを無効化する
    ///
    /// ## Arguments
    /// - `&self`: スピンロックの参照
    ///
    /// ## Returns
    /// - `SpinLockGuard<'_, T>`: ロックガード。ドロップ時にロックを解放し、割込み状態を復元する
    pub fn lock(&self) -> SpinLockGuard<'_, T> {
        // 現在の割込みフラグを保存
        let interrupt_enabled = x86_64::instructions::interrupts::are_enabled();

        // 割込みを無効化
        x86_64::instructions::interrupts::disable();

        // ロック取得を試みる
        while self
            .locked
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // ロックが取得できるまでスピン
            while self.locked.load(Ordering::Relaxed) {
                core::hint::spin_loop();
            }
        }

        SpinLockGuard {
            lock: self,
            interrupt_enabled,
        }
    }

    /// ロックを試行（割込みを無効化）
    ///
    /// ロックが既に取得されている場合はNoneを返す
    ///
    /// ## Arguments
    /// - `&self`: スピンロックの参照
    ///
    /// ## Returns
    /// - `Option<SpinLockGuard<'_, T>>`: ロックガード。ロックが取得できた場合はSome、そうでない場合はNone
    pub fn try_lock(&self) -> Option<SpinLockGuard<'_, T>> {
        let interrupt_enabled = x86_64::instructions::interrupts::are_enabled();

        x86_64::instructions::interrupts::disable();

        if self
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok()
        {
            Some(SpinLockGuard {
                lock: self,
                interrupt_enabled,
            })
        } else {
            // ロック取得失敗、割込み状態を復元
            if interrupt_enabled {
                x86_64::instructions::interrupts::enable();
            }
            None
        }
    }

    /// 内部データへの可変参照を取得（unsafe）
    ///
    /// 呼び出し側は、他のスレッドがデータにアクセスしていないことを保証する必要がある
    ///
    /// ## Arguments
    /// - `&self`: スピンロックの参照
    ///
    /// ## Returns
    /// - `&mut T`: 内部データへの可変参照
    ///
    /// # Safety
    /// 呼び出し側は、現在このロックを保持しているか、他スレッドから同時アクセスされないことを保証する必要がある。
    pub unsafe fn force_unlock(&self) {
        self.locked.store(false, Ordering::Release);
    }

    /// 内部データへの不変ポインタを取得
    ///
    /// データ配置アドレスが必要な用途向け。排他制御は呼び出し側で担保すること。
    pub fn as_ptr(&self) -> *const T {
        self.data.get() as *const T
    }
}

/// スピンロックガード
///
/// ドロップ時に自動的にロックを解放し、割込み状態を復元する
///
/// ## Lifetime Parameters
/// - `'a`: スピンロックのライフタイム
pub struct SpinLockGuard<'a, T> {
    /// 保護されるスピンロックへの参照
    lock: &'a SpinLock<T>,
    /// ロック取得時の割込み状態
    interrupt_enabled: bool,
}

impl<T> Deref for SpinLockGuard<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        unsafe { &*self.lock.data.get() }
    }
}

impl<T> DerefMut for SpinLockGuard<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { &mut *self.lock.data.get() }
    }
}

impl<T> Drop for SpinLockGuard<'_, T> {
    fn drop(&mut self) {
        // ロックを解放
        self.lock.locked.store(false, Ordering::Release);

        // 割込み状態を復元
        if self.interrupt_enabled {
            x86_64::instructions::interrupts::enable();
        }
    }
}
