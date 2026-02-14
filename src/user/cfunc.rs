use core::ffi::c_char;

#[allow(dead_code)]
extern "C" {
    /// 標準出力にフォーマットされた文字列を出力する
    ///
    /// ## 引数
    /// - `format`: フォーマット文字列(Cスタイル)
    /// - `...`: 可変引数 (フォーマットに対応する値)
    ///
    /// ## 戻り値
    /// 出力した文字数、またはエラーコード
    pub fn printf(format: *const c_char, ...) -> i32;
    /// メモリを確保する
    ///
    /// ## 引数
    /// - `size`: 確保するバイト数
    ///
    /// ## 戻り値
    /// 確保されたメモリのポインタ、またはNULL
    pub fn malloc(size: usize) -> *mut u8;
    /// メモリを開放する
    ///
    /// ## 引数
    /// - `ptr`: 開放するメモリのポインタ
    pub fn free(ptr: *mut u8);
    /// メモリを再確保する
    ///
    /// ## 引数
    /// - `ptr`: 再確保する元のメモリのポインタ
    /// - `size`: 新しいサイズ
    ///
    /// ## 戻り値
    /// 再確保されたメモリのポインタ、またはNULL
    pub fn realloc(ptr: *mut u8, size: usize) -> *mut u8;
    /// アライメントされたメモリを確保する
    ///
    /// ## 引数
    /// - `alignment`: アライメントのバイト数（2のべき乗でなければならない）
    /// - `size`: 確保するバイト数
    ///
    /// ## 戻り値
    /// 確保されたメモリのポインタ、またはNULL
    pub fn memalign(alignment: usize, size: usize) -> *mut u8;
}