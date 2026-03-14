pub const PCI_CFG_ADDR_PORT: u16 = 0xCF8;
pub const PCI_CFG_DATA_PORT: u16 = 0xCFC;

pub const XHCI_CLASS_CODE: u8 = 0x0C;
pub const XHCI_SUBCLASS: u8 = 0x03;
pub const XHCI_PROG_IF: u8 = 0x30;

pub const XHCI_MMIO_MAP_SIZE: usize = 0x10000;
pub const PAGE_SIZE: usize = 4096;
pub const TRB_SIZE: usize = 16;

pub const ENOMEM: u64 = (-12i64) as u64;
pub const EINVAL: u64 = (-22i64) as u64;

pub const OP_USBCMD: usize = 0x00;
pub const OP_USBSTS: usize = 0x04;
pub const OP_CRCR: usize = 0x18;
pub const OP_DCBAAP: usize = 0x30;
pub const OP_CONFIG: usize = 0x38;

pub const USBCMD_RUN_STOP: u32 = 1 << 0;
pub const USBCMD_HCRST: u32 = 1 << 1;
pub const USBCMD_INTE: u32 = 1 << 2;
pub const USBSTS_HCHALTED: u32 = 1 << 0;
pub const USBSTS_EINT: u32 = 1 << 3;
#[allow(unused)]
pub const USBSTS_CNR: u32 = 1 << 11;

pub const RT_IR0_BASE: usize = 0x20;
pub const IR_IMAN: usize = 0x00;
pub const IR_IMOD: usize = 0x04;
pub const IR_ERSTSZ: usize = 0x08;
pub const IR_ERSTBA: usize = 0x10;
pub const IR_ERDP: usize = 0x18;

pub const IMAN_IP: u32 = 1 << 0;
pub const IMAN_IE: u32 = 1 << 1;
pub const ERDP_EHB: u64 = 1 << 3;

pub const TRB_TYPE_LINK: u32 = 6;
pub const TRB_TYPE_NORMAL: u32 = 1;
pub const TRB_TYPE_SETUP_STAGE: u32 = 2;
pub const TRB_TYPE_DATA_STAGE: u32 = 3;
pub const TRB_TYPE_STATUS_STAGE: u32 = 4;
pub const TRB_TYPE_ENABLE_SLOT_CMD: u32 = 9;
pub const TRB_TYPE_ADDRESS_DEVICE_CMD: u32 = 11;
pub const TRB_TYPE_CONFIGURE_ENDPOINT_CMD: u32 = 12;
pub const TRB_TYPE_NOOP_CMD: u32 = 23;
pub const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
pub const TRB_TYPE_COMMAND_COMPLETION: u32 = 33;
pub const TRB_TYPE_PORT_STATUS_CHANGE: u32 = 34;

pub const USB_DESC_DEVICE: u16 = 0x01;
pub const USB_DESC_CONFIGURATION: u16 = 0x02;

#[derive(Clone, Copy)]
pub struct PciBdf {
    pub(crate) bus: u8,
    pub(crate) device: u8,
    pub(crate) function: u8,
}

#[derive(Clone, Copy)]
pub struct XhciController {
    pub(crate) bdf: PciBdf,
    pub(crate) vendor_id: u16,
    pub(crate) device_id: u16,
    pub(crate) bar0: u32,
    pub(crate) bar1: u32,
    pub(crate) mmio_base: u64,
    pub(crate) bar_is_64bit: bool,
}

#[allow(unused)]
#[derive(Clone, Copy)]
pub struct XhciRegs {
    pub(crate) base: *mut u8,
    pub(crate) cap_len: usize,
    pub(crate) op_base: usize,
    pub(crate) db_off: usize,
    pub(crate) rt_off: usize,
    pub(crate) max_ports: u8,
    pub(crate) max_slots: u8,
    pub(crate) hci_version: u16,
    pub(crate) hccparams1: u32,
    pub(crate) context_size: usize,
}


#[rustfmt::skip]
pub const MAP_NORMAL: [u8; 128] = [
    0,    0x1B, b'1', b'2', b'3', b'4', b'5', b'6',
    b'7', b'8', b'9', b'0', b'-', b'=', 0x08, b'\t',
    b'q', b'w', b'e', b'r', b't', b'y', b'u', b'i',
    b'o', b'p', b'[', b']', b'\n', 0,   b'a', b's',
    b'd', b'f', b'g', b'h', b'j', b'k', b'l', b';',
    b'\'',b'`', 0,   b'\\',b'z', b'x', b'c', b'v',
    b'b', b'n', b'm', b',', b'.', b'/', 0,   b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    b'7',
    b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1',
    b'2', b'3', b'0', b'.', 0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
];

#[rustfmt::skip]
pub const MAP_SHIFT: [u8; 128] = [
    0,    0x1B, b'!', b'@', b'#', b'$', b'%', b'^',
    b'&', b'*', b'(', b')', b'_', b'+', 0x08, b'\t',
    b'Q', b'W', b'E', b'R', b'T', b'Y', b'U', b'I',
    b'O', b'P', b'{', b'}', b'\n', 0,   b'A', b'S',
    b'D', b'F', b'G', b'H', b'J', b'K', b'L', b':',
    b'"', b'~', 0,   b'|', b'Z', b'X', b'C', b'V',
    b'B', b'N', b'M', b'<', b'>', b'?', 0,   b'*',
    0,    b' ', 0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    b'7',
    b'8', b'9', b'-', b'4', b'5', b'6', b'+', b'1',
    b'2', b'3', b'0', b'.', 0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
    0,    0,    0,    0,    0,    0,    0,    0,
];

pub const SC_LSHIFT: u8 = 0x2A;
pub const SC_RSHIFT: u8 = 0x36;
pub const SC_CAPSLOCK: u8 = 0x3A;
pub const SC_RELEASE: u8 = 0x80;

#[derive(Default)]
pub struct KeyboardDecoder {
    pub(crate) shift: bool,
    pub(crate) caps: bool,
}

pub struct TransferRing {
    pub(crate) page: DmaPage,
    pub(crate) trb_count: usize,
    pub(crate) enqueue_idx: usize,
    pub(crate) cycle: bool,
}

pub struct DmaPage {
    pub(crate) virt: *mut u8,
    pub(crate) phys: u64,
    pub(crate) size: usize,
}