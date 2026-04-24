//! 監査ログ
//!
//! panic の代わりに、拒否・隔離・破損検出・復旧不能な局所失敗を
//! append-only な簡易リングバッファへ記録する。

use core::fmt;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::interrupt::spinlock::SpinLock;

const AUDIT_CAPACITY: usize = 256;
const AUDIT_MSG_LEN: usize = 160;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuditEventKind {
    Deny,
    Fault,
    Revoke,
    Quarantine,
    Restart,
    Policy,
    Usercopy,
    Device,
    Exec,
    Ipc,
    Memory,
}

#[derive(Clone, Copy)]
pub struct AuditRecord {
    seq: u64,
    kind: AuditEventKind,
    len: usize,
    msg: [u8; AUDIT_MSG_LEN],
}

impl AuditRecord {
    const fn empty() -> Self {
        Self {
            seq: 0,
            kind: AuditEventKind::Fault,
            len: 0,
            msg: [0; AUDIT_MSG_LEN],
        }
    }

    fn write_message(&mut self, message: &str) {
        let bytes = message.as_bytes();
        let len = bytes.len().min(AUDIT_MSG_LEN);
        self.msg[..len].copy_from_slice(&bytes[..len]);
        if len < AUDIT_MSG_LEN {
            self.msg[len..].fill(0);
        }
        self.len = len;
    }

    pub fn message(&self) -> &str {
        core::str::from_utf8(&self.msg[..self.len]).unwrap_or("<invalid-audit-utf8>")
    }

    pub fn seq(&self) -> u64 {
        self.seq
    }

    pub fn kind(&self) -> AuditEventKind {
        self.kind
    }
}

impl fmt::Debug for AuditRecord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AuditRecord")
            .field("seq", &self.seq)
            .field("kind", &self.kind)
            .field("message", &self.message())
            .finish()
    }
}

static AUDIT_LOG: SpinLock<[AuditRecord; AUDIT_CAPACITY]> =
    SpinLock::new([AuditRecord::empty(); AUDIT_CAPACITY]);
static AUDIT_SEQ: AtomicUsize = AtomicUsize::new(1);

pub fn log(kind: AuditEventKind, message: &str) {
    let seq = AUDIT_SEQ.fetch_add(1, Ordering::Relaxed) as u64;
    let idx = (seq as usize) % AUDIT_CAPACITY;
    {
        let mut log = AUDIT_LOG.lock();
        let slot = &mut log[idx];
        slot.seq = seq;
        slot.kind = kind;
        slot.write_message(message);
    }
    crate::warn!("[AUDIT {:?} #{seq}] {}", kind, message);
}

pub fn snapshot_into(out: &mut [AuditRecord]) -> usize {
    if out.is_empty() {
        return 0;
    }
    let next_seq = AUDIT_SEQ.load(Ordering::Acquire) as u64;
    let start_seq = next_seq.saturating_sub(AUDIT_CAPACITY as u64).max(1);
    let log = AUDIT_LOG.lock();
    let mut written = 0;
    for seq in start_seq..next_seq {
        if written >= out.len() {
            break;
        }
        let idx = (seq as usize) % AUDIT_CAPACITY;
        if log[idx].seq == seq {
            out[written] = log[idx];
            written += 1;
        }
    }
    written
}
