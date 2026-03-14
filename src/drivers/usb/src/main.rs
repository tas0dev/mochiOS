use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, Ordering as AtomicOrdering};
use std::alloc::{alloc_zeroed, Layout};

use swiftlib::{keyboard, mmio, mouse, port, time};

const PCI_CFG_ADDR_PORT: u16 = 0xCF8;
const PCI_CFG_DATA_PORT: u16 = 0xCFC;

const XHCI_CLASS_CODE: u8 = 0x0C;
const XHCI_SUBCLASS: u8 = 0x03;
const XHCI_PROG_IF: u8 = 0x30;

const XHCI_MMIO_MAP_SIZE: usize = 0x10000;
const PAGE_SIZE: usize = 4096;
const TRB_SIZE: usize = 16;

const ENOMEM: u64 = (-12i64) as u64;
const EINVAL: u64 = (-22i64) as u64;

const OP_USBCMD: usize = 0x00;
const OP_USBSTS: usize = 0x04;
const OP_CRCR: usize = 0x18;
const OP_CONFIG: usize = 0x38;

const USBCMD_RUN_STOP: u32 = 1 << 0;
const USBCMD_HCRST: u32 = 1 << 1;
const USBCMD_INTE: u32 = 1 << 2;
const USBSTS_HCHALTED: u32 = 1 << 0;
const USBSTS_EINT: u32 = 1 << 3;
const USBSTS_CNR: u32 = 1 << 11;

const RT_IR0_BASE: usize = 0x20;
const IR_IMAN: usize = 0x00;
const IR_IMOD: usize = 0x04;
const IR_ERSTSZ: usize = 0x08;
const IR_ERSTBA: usize = 0x10;
const IR_ERDP: usize = 0x18;

const IMAN_IP: u32 = 1 << 0;
const IMAN_IE: u32 = 1 << 1;
const ERDP_EHB: u64 = 1 << 3;

const TRB_TYPE_LINK: u32 = 6;
const TRB_TYPE_NOOP_CMD: u32 = 23;
const TRB_TYPE_TRANSFER_EVENT: u32 = 32;
const TRB_TYPE_COMMAND_COMPLETION: u32 = 33;
const TRB_TYPE_PORT_STATUS_CHANGE: u32 = 34;

#[derive(Clone, Copy)]
struct PciBdf {
    bus: u8,
    device: u8,
    function: u8,
}

#[derive(Clone, Copy)]
struct XhciController {
    bdf: PciBdf,
    vendor_id: u16,
    device_id: u16,
    bar0: u32,
    bar1: u32,
    mmio_base: u64,
    bar_is_64bit: bool,
}

#[derive(Clone, Copy)]
struct XhciRegs {
    base: *mut u8,
    cap_len: usize,
    op_base: usize,
    db_off: usize,
    rt_off: usize,
    max_ports: u8,
    max_slots: u8,
    hci_version: u16,
}

fn pci_config_address(bdf: PciBdf, offset: u8) -> u32 {
    0x8000_0000
        | ((bdf.bus as u32) << 16)
        | ((bdf.device as u32) << 11)
        | ((bdf.function as u32) << 8)
        | (u32::from(offset) & 0xFC)
}

fn pci_read_u32(bdf: PciBdf, offset: u8) -> u32 {
    let addr = pci_config_address(bdf, offset);
    port::outl(PCI_CFG_ADDR_PORT, addr);
    port::inl(PCI_CFG_DATA_PORT)
}

fn pci_read_u16(bdf: PciBdf, offset: u8) -> u16 {
    let aligned = offset & 0xFC;
    let shift = u32::from(offset & 0x02) * 8;
    ((pci_read_u32(bdf, aligned) >> shift) & 0xFFFF) as u16
}

fn pci_function_exists(bdf: PciBdf) -> bool {
    pci_read_u16(bdf, 0x00) != 0xFFFF
}

fn probe_xhci_controller(bdf: PciBdf) -> Option<XhciController> {
    let class_reg = pci_read_u32(bdf, 0x08);
    let class_code = ((class_reg >> 24) & 0xFF) as u8;
    let subclass = ((class_reg >> 16) & 0xFF) as u8;
    let prog_if = ((class_reg >> 8) & 0xFF) as u8;

    if class_code != XHCI_CLASS_CODE || subclass != XHCI_SUBCLASS || prog_if != XHCI_PROG_IF {
        return None;
    }

    let vendor_device = pci_read_u32(bdf, 0x00);
    let vendor_id = (vendor_device & 0xFFFF) as u16;
    let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;

    let bar0 = pci_read_u32(bdf, 0x10);
    let bar1 = pci_read_u32(bdf, 0x14);

    if (bar0 & 0x1) != 0 {
        println!(
            "[xHCI] controller {:02x}:{:02x}.{} uses I/O BAR (unsupported)",
            bdf.bus, bdf.device, bdf.function
        );
        return None;
    }

    let bar_is_64bit = (bar0 & 0x6) == 0x4;
    let mut mmio_base = u64::from(bar0 & 0xFFFF_FFF0);
    if bar_is_64bit {
        mmio_base |= u64::from(bar1) << 32;
    }

    if mmio_base == 0 {
        return None;
    }

    Some(XhciController {
        bdf,
        vendor_id,
        device_id,
        bar0,
        bar1,
        mmio_base,
        bar_is_64bit,
    })
}

fn find_xhci_controller() -> Option<XhciController> {
    for bus in 0u16..=255 {
        for device in 0u16..32 {
            let bdf0 = PciBdf {
                bus: bus as u8,
                device: device as u8,
                function: 0,
            };
            if !pci_function_exists(bdf0) {
                continue;
            }

            let header = pci_read_u32(bdf0, 0x0C);
            let header_type = ((header >> 16) & 0xFF) as u8;
            let function_count = if (header_type & 0x80) != 0 { 8 } else { 1 };

            for function in 0..function_count {
                let bdf = PciBdf {
                    bus: bus as u8,
                    device: device as u8,
                    function: function as u8,
                };
                if !pci_function_exists(bdf) {
                    continue;
                }
                if let Some(controller) = probe_xhci_controller(bdf) {
                    return Some(controller);
                }
            }
        }
    }
    None
}

#[inline]
fn mmio_read_u8(base: *mut u8, offset: usize) -> u8 {
    unsafe { read_volatile(base.add(offset) as *const u8) }
}

#[inline]
fn mmio_read_u16(base: *mut u8, offset: usize) -> u16 {
    unsafe { read_volatile(base.add(offset) as *const u16) }
}

#[inline]
fn mmio_read_u32(base: *mut u8, offset: usize) -> u32 {
    unsafe { read_volatile(base.add(offset) as *const u32) }
}

#[inline]
fn mmio_write_u32(base: *mut u8, offset: usize, value: u32) {
    unsafe {
        write_volatile(base.add(offset) as *mut u32, value);
    }
}

#[inline]
fn mmio_read_u64(base: *mut u8, offset: usize) -> u64 {
    let lo = u64::from(mmio_read_u32(base, offset));
    let hi = u64::from(mmio_read_u32(base, offset + 4));
    lo | (hi << 32)
}

#[inline]
fn mmio_write_u64(base: *mut u8, offset: usize, value: u64) {
    mmio_write_u32(base, offset, value as u32);
    mmio_write_u32(base, offset + 4, (value >> 32) as u32);
}

fn wait_until(timeout_ms: u64, mut condition: impl FnMut() -> bool) -> bool {
    for _ in 0..timeout_ms {
        if condition() {
            return true;
        }
        time::sleep_ms(1);
    }
    false
}

fn map_xhci_mmio(controller: &XhciController) -> Result<*mut u8, u64> {
    let page_base = controller.mmio_base & !0xFFF;
    let page_offset = (controller.mmio_base & 0xFFF) as usize;
    let map_size = XHCI_MMIO_MAP_SIZE.saturating_add(page_offset);
    let mapped = mmio::map_physical(page_base, map_size)?;
    Ok(unsafe { mapped.add(page_offset) })
}

struct DmaPage {
    virt: *mut u8,
    phys: u64,
    size: usize,
}

impl DmaPage {
    fn alloc(size: usize) -> Result<Self, u64> {
        if size == 0 {
            return Err(EINVAL);
        }
        let layout = Layout::from_size_align(size, PAGE_SIZE).map_err(|_| EINVAL)?;
        let virt = unsafe { alloc_zeroed(layout) };
        if virt.is_null() {
            return Err(ENOMEM);
        }
        let phys = mmio::virt_to_phys(virt as *const u8)?;
        if (phys & 0xFFF) != 0 {
            return Err(EINVAL);
        }
        Ok(Self { virt, phys, size })
    }
}

fn trb_read(page: &DmaPage, index: usize) -> [u32; 4] {
    let p = unsafe { page.virt.add(index * TRB_SIZE) as *const u32 };
    unsafe {
        [
            read_volatile(p.add(0)),
            read_volatile(p.add(1)),
            read_volatile(p.add(2)),
            read_volatile(p.add(3)),
        ]
    }
}

fn trb_write(page: &DmaPage, index: usize, trb: [u32; 4]) {
    let p = unsafe { page.virt.add(index * TRB_SIZE) as *mut u32 };
    unsafe {
        write_volatile(p.add(0), trb[0]);
        write_volatile(p.add(1), trb[1]);
        write_volatile(p.add(2), trb[2]);
        write_volatile(p.add(3), trb[3]);
    }
}

struct CommandRing {
    page: DmaPage,
    trb_count: usize,
    enqueue_idx: usize,
    cycle: bool,
}

impl CommandRing {
    fn new() -> Result<Self, u64> {
        let page = DmaPage::alloc(PAGE_SIZE)?;
        let trb_count = page.size / TRB_SIZE;
        if trb_count < 2 {
            return Err(EINVAL);
        }

        let link_index = trb_count - 1;
        let link = [
            (page.phys & 0xFFFF_FFF0) as u32,
            (page.phys >> 32) as u32,
            0,
            (TRB_TYPE_LINK << 10) | (1 << 1) | 1,
        ];
        trb_write(&page, link_index, link);

        Ok(Self {
            page,
            trb_count,
            enqueue_idx: 0,
            cycle: true,
        })
    }

    #[inline]
    fn ring_phys(&self) -> u64 {
        self.page.phys
    }

    #[inline]
    fn link_index(&self) -> usize {
        self.trb_count - 1
    }

    fn push_noop_command(&mut self) -> u64 {
        let idx = self.enqueue_idx;
        let trb_phys = self.page.phys + (idx * TRB_SIZE) as u64;
        let cycle_bit = if self.cycle { 1 } else { 0 };
        let trb = [0, 0, 0, (TRB_TYPE_NOOP_CMD << 10) | cycle_bit];
        trb_write(&self.page, idx, trb);
        compiler_fence(AtomicOrdering::SeqCst);

        self.enqueue_idx += 1;
        if self.enqueue_idx >= self.link_index() {
            self.enqueue_idx = 0;
            self.cycle = !self.cycle;
        }

        trb_phys
    }
}

struct EventRing {
    segment: DmaPage,
    erst: DmaPage,
    trb_count: usize,
    dequeue_idx: usize,
    ccs: bool,
}

impl EventRing {
    fn new() -> Result<Self, u64> {
        let segment = DmaPage::alloc(PAGE_SIZE)?;
        let erst = DmaPage::alloc(PAGE_SIZE)?;
        let trb_count = segment.size / TRB_SIZE;
        if trb_count == 0 {
            return Err(EINVAL);
        }

        let erst_entry = [
            segment.phys as u32,
            (segment.phys >> 32) as u32,
            trb_count as u32,
            0,
        ];
        let p = erst.virt as *mut u32;
        unsafe {
            write_volatile(p.add(0), erst_entry[0]);
            write_volatile(p.add(1), erst_entry[1]);
            write_volatile(p.add(2), erst_entry[2]);
            write_volatile(p.add(3), erst_entry[3]);
        }

        Ok(Self {
            segment,
            erst,
            trb_count,
            dequeue_idx: 0,
            ccs: true,
        })
    }

    fn dequeue_phys(&self) -> u64 {
        self.segment.phys + (self.dequeue_idx * TRB_SIZE) as u64
    }

    fn pop_event(&mut self) -> Option<[u32; 4]> {
        let trb = trb_read(&self.segment, self.dequeue_idx);
        let cycle = (trb[3] & 1) != 0;
        if cycle != self.ccs {
            return None;
        }

        self.dequeue_idx += 1;
        if self.dequeue_idx >= self.trb_count {
            self.dequeue_idx = 0;
            self.ccs = !self.ccs;
        }
        Some(trb)
    }
}

#[derive(Default)]
struct HidParserState {
    prev_keys: [u8; 6],
    prev_mouse_buttons: u8,
}

struct XhciRuntime {
    regs: XhciRegs,
    command_ring: CommandRing,
    event_ring: EventRing,
    pending_noop_phys: Option<u64>,
    hid: HidParserState,
}

fn read_xhci_regs(base: *mut u8) -> Option<XhciRegs> {
    let cap_len = mmio_read_u8(base, 0x00) as usize;
    if cap_len < 0x20 {
        return None;
    }

    let hci_version = mmio_read_u16(base, 0x02);
    let hcs_params1 = mmio_read_u32(base, 0x04);
    let max_slots = (hcs_params1 & 0xFF) as u8;
    let max_ports = ((hcs_params1 >> 24) & 0xFF) as u8;
    let db_off = (mmio_read_u32(base, 0x14) & !0x3) as usize;
    let rt_off = (mmio_read_u32(base, 0x18) & !0x1F) as usize;

    Some(XhciRegs {
        base,
        cap_len,
        op_base: cap_len,
        db_off,
        rt_off,
        max_ports,
        max_slots,
        hci_version,
    })
}

fn halt_xhci(regs: &XhciRegs) -> bool {
    let usbcmd_off = regs.op_base;
    let usbsts_off = regs.op_base + 0x04;

    let cmd = mmio_read_u32(regs.base, usbcmd_off);
    if (cmd & 0x1) != 0 {
        mmio_write_u32(regs.base, usbcmd_off, cmd & !0x1);
    }

    wait_until(300, || (mmio_read_u32(regs.base, usbsts_off) & 0x1) != 0)
}

fn reset_xhci(regs: &XhciRegs) -> bool {
    if !halt_xhci(regs) {
        println!("[xHCI] halt timeout");
        return false;
    }

    let usbcmd_off = regs.op_base;
    let usbsts_off = regs.op_base + 0x04;

    let cmd = mmio_read_u32(regs.base, usbcmd_off);
    mmio_write_u32(regs.base, usbcmd_off, cmd | (1 << 1));

    if !wait_until(1000, || (mmio_read_u32(regs.base, usbcmd_off) & (1 << 1)) == 0) {
        println!("[xHCI] controller reset timeout");
        return false;
    }
    if !wait_until(1000, || (mmio_read_u32(regs.base, usbsts_off) & (1 << 11)) == 0) {
        println!("[xHCI] CNR clear timeout");
        return false;
    }
    true
}

fn ring_doorbell(regs: &XhciRegs, doorbell: usize, value: u32) {
    mmio_write_u32(regs.base, regs.db_off + doorbell * 4, value);
}

fn setup_command_ring_register(regs: &XhciRegs, ring: &CommandRing) {
    let crcr = (ring.ring_phys() & !0x3F) | 1;
    mmio_write_u64(regs.base, regs.op_base + OP_CRCR, crcr);
}

fn setup_interrupter(regs: &XhciRegs, event_ring: &EventRing) {
    let ir_base = regs.rt_off + RT_IR0_BASE;

    mmio_write_u32(regs.base, ir_base + IR_IMOD, 0);
    mmio_write_u32(regs.base, ir_base + IR_ERSTSZ, 1);
    mmio_write_u64(regs.base, ir_base + IR_ERSTBA, event_ring.erst.phys);
    mmio_write_u64(regs.base, ir_base + IR_ERDP, event_ring.dequeue_phys() | ERDP_EHB);

    let iman = mmio_read_u32(regs.base, ir_base + IR_IMAN);
    mmio_write_u32(regs.base, ir_base + IR_IMAN, iman | IMAN_IE | IMAN_IP);
}

fn start_xhci(regs: &XhciRegs) -> bool {
    let mut cfg = mmio_read_u32(regs.base, regs.op_base + OP_CONFIG);
    cfg = (cfg & !0xFF) | u32::from(core::cmp::max(regs.max_slots, 1));
    mmio_write_u32(regs.base, regs.op_base + OP_CONFIG, cfg);

    let st = mmio_read_u32(regs.base, regs.op_base + OP_USBSTS);
    mmio_write_u32(regs.base, regs.op_base + OP_USBSTS, st);

    let mut cmd = mmio_read_u32(regs.base, regs.op_base + OP_USBCMD);
    cmd |= USBCMD_INTE;
    cmd |= USBCMD_RUN_STOP;
    cmd &= !USBCMD_HCRST;
    mmio_write_u32(regs.base, regs.op_base + OP_USBCMD, cmd);

    wait_until(1000, || (mmio_read_u32(regs.base, regs.op_base + OP_USBSTS) & USBSTS_HCHALTED) == 0)
}

fn read_transfer_report_bytes(transfer_trb_phys: u64) -> Option<Vec<u8>> {
    let trb_map = mmio::map_physical(transfer_trb_phys & !0xFFF, PAGE_SIZE).ok()?;
    let trb_off = (transfer_trb_phys & 0xFFF) as usize;
    if trb_off + TRB_SIZE > PAGE_SIZE {
        return None;
    }

    let trb_ptr = unsafe { trb_map.add(trb_off) as *const u32 };
    let trb = unsafe {
        [
            read_volatile(trb_ptr.add(0)),
            read_volatile(trb_ptr.add(1)),
            read_volatile(trb_ptr.add(2)),
            read_volatile(trb_ptr.add(3)),
        ]
    };

    let req_len = (trb[2] & 0x1FFFF) as usize;
    if req_len == 0 || req_len > 64 {
        return None;
    }

    let data_phys = u64::from(trb[0]) | (u64::from(trb[1]) << 32);
    if data_phys == 0 {
        return None;
    }

    let data_off = (data_phys & 0xFFF) as usize;
    if data_off + req_len > PAGE_SIZE {
        return None;
    }
    let data_map = mmio::map_physical(data_phys & !0xFFF, PAGE_SIZE).ok()?;
    let data_ptr = unsafe { data_map.add(data_off) };
    let mut out = vec![0u8; req_len];
    for (i, b) in out.iter_mut().enumerate() {
        *b = unsafe { read_volatile(data_ptr.add(i)) };
    }
    Some(out)
}

fn hid_usage_to_char(usage: u8, shift: bool) -> Option<char> {
    match usage {
        0x04..=0x1D => {
            let c = (b'a' + (usage - 0x04)) as char;
            Some(if shift { c.to_ascii_uppercase() } else { c })
        }
        0x1E..=0x27 => {
            const NORMAL: [char; 10] = ['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'];
            const SHIFT: [char; 10] = ['!', '@', '#', '$', '%', '^', '&', '*', '(', ')'];
            let idx = (usage - 0x1E) as usize;
            Some(if shift { SHIFT[idx] } else { NORMAL[idx] })
        }
        0x2D => Some(if shift { '_' } else { '-' }),
        0x2E => Some(if shift { '+' } else { '=' }),
        0x2F => Some(if shift { '{' } else { '[' }),
        0x30 => Some(if shift { '}' } else { ']' }),
        0x31 => Some(if shift { '|' } else { '\\' }),
        0x33 => Some(if shift { ':' } else { ';' }),
        0x34 => Some(if shift { '"' } else { '\'' }),
        0x35 => Some(if shift { '~' } else { '`' }),
        0x36 => Some(if shift { '<' } else { ',' }),
        0x37 => Some(if shift { '>' } else { '.' }),
        0x38 => Some(if shift { '?' } else { '/' }),
        0x2C => Some(' '),
        _ => None,
    }
}

fn parse_hid_keyboard_report(slot: u8, ep: u8, report: &[u8], state: &mut HidParserState) -> bool {
    if report.len() < 8 {
        return false;
    }

    let modifiers = report[0];
    let shift = (modifiers & 0x22) != 0;
    let keys = &report[2..8];

    for &usage in keys {
        if usage == 0 {
            continue;
        }
        if state.prev_keys.contains(&usage) {
            continue;
        }
        if let Some(ch) = hid_usage_to_char(usage, shift) {
            println!("[xHCI][HID][slot:{} ep:{}] key '{}'", slot, ep, ch);
        } else {
            println!(
                "[xHCI][HID][slot:{} ep:{}] usage=0x{:02x} modifiers=0x{:02x}",
                slot, ep, usage, modifiers
            );
        }
    }

    state.prev_keys.copy_from_slice(keys);
    true
}

fn parse_hid_mouse_report(slot: u8, ep: u8, report: &[u8], state: &mut HidParserState) -> bool {
    if report.len() < 3 {
        return false;
    }

    let (buttons_idx, data_idx) = if (report[0] & 0xF8) == 0 {
        (0usize, 1usize)
    } else if report.len() >= 4 && (report[1] & 0xF8) == 0 {
        (1usize, 2usize)
    } else {
        return false;
    };

    if report.len() <= data_idx + 1 {
        return false;
    }

    let buttons = report[buttons_idx] & 0x07;
    let dx = report[data_idx] as i8;
    let dy = report[data_idx + 1] as i8;
    let wheel = if report.len() > data_idx + 2 {
        report[data_idx + 2] as i8
    } else {
        0
    };

    if dx != 0 || dy != 0 || wheel != 0 || buttons != state.prev_mouse_buttons {
        println!(
            "[xHCI][HID][slot:{} ep:{}] mouse dx={} dy={} wheel={} L={} R={} M={}",
            slot,
            ep,
            dx,
            dy,
            wheel,
            (buttons & 0x01) as u8,
            ((buttons >> 1) & 0x01) as u8,
            ((buttons >> 2) & 0x01) as u8
        );
    }
    state.prev_mouse_buttons = buttons;
    true
}

fn parse_hid_report(slot: u8, ep: u8, report: &[u8], state: &mut HidParserState) {
    if parse_hid_keyboard_report(slot, ep, report, state) {
        return;
    }
    let _ = parse_hid_mouse_report(slot, ep, report, state);
}

fn poll_xhci_events(runtime: &mut XhciRuntime) -> bool {
    let mut handled = false;
    let ir_base = runtime.regs.rt_off + RT_IR0_BASE;

    for _ in 0..64 {
        let Some(event) = runtime.event_ring.pop_event() else {
            break;
        };
        handled = true;

        let trb_type = ((event[3] >> 10) & 0x3F) as u32;
        let completion_code = ((event[2] >> 24) & 0xFF) as u8;
        let slot_id = ((event[3] >> 24) & 0xFF) as u8;
        let ep_id = ((event[3] >> 16) & 0x1F) as u8;

        match trb_type {
            TRB_TYPE_COMMAND_COMPLETION => {
                let cmd_ptr = u64::from(event[0]) | (u64::from(event[1]) << 32);
                if runtime.pending_noop_phys == Some(cmd_ptr) {
                    println!(
                        "[xHCI] No-Op command completion: code={} slot={}",
                        completion_code, slot_id
                    );
                    runtime.pending_noop_phys = None;
                } else {
                    println!(
                        "[xHCI] command completion: code={} slot={} ptr={:#x}",
                        completion_code, slot_id, cmd_ptr
                    );
                }
            }
            TRB_TYPE_PORT_STATUS_CHANGE => {
                let port_id = ((event[0] >> 24) & 0xFF) as u8;
                println!(
                    "[xHCI] port status change event: port={} code={}",
                    port_id, completion_code
                );
                dump_ports(&runtime.regs);
            }
            TRB_TYPE_TRANSFER_EVENT => {
                let transfer_ptr = u64::from(event[0]) | (u64::from(event[1]) << 32);
                println!(
                    "[xHCI] transfer event: slot={} ep={} code={} trb={:#x}",
                    slot_id, ep_id, completion_code, transfer_ptr
                );
                if completion_code == 1 {
                    if let Some(report) = read_transfer_report_bytes(transfer_ptr) {
                        parse_hid_report(slot_id, ep_id, &report, &mut runtime.hid);
                    }
                }
            }
            _ => {
                println!(
                    "[xHCI] event: type={} code={} slot={} ep={}",
                    trb_type, completion_code, slot_id, ep_id
                );
            }
        }

        mmio_write_u64(
            runtime.regs.base,
            ir_base + IR_ERDP,
            runtime.event_ring.dequeue_phys() | ERDP_EHB,
        );
    }

    if handled {
        mmio_write_u32(runtime.regs.base, runtime.regs.op_base + OP_USBSTS, USBSTS_EINT);
        let iman = mmio_read_u32(runtime.regs.base, ir_base + IR_IMAN);
        mmio_write_u32(runtime.regs.base, ir_base + IR_IMAN, iman | IMAN_IP | IMAN_IE);
    }

    handled
}

fn decode_port_speed(speed: u8) -> &'static str {
    match speed {
        1 => "full",
        2 => "low",
        3 => "high",
        4 => "super",
        5 => "super+",
        _ => "unknown",
    }
}

fn dump_ports(regs: &XhciRegs) {
    if regs.max_ports == 0 {
        println!("[xHCI] no root hub ports reported");
        return;
    }

    for port_index in 0..usize::from(regs.max_ports) {
        let portsc_off = regs.op_base + 0x400 + port_index * 0x10;
        let portsc = mmio_read_u32(regs.base, portsc_off);
        let connected = (portsc & (1 << 0)) != 0;
        let enabled = (portsc & (1 << 1)) != 0;
        let connect_change = (portsc & (1 << 17)) != 0;
        let speed = ((portsc >> 10) & 0x0F) as u8;

        if connected || enabled || connect_change {
            println!(
                "[xHCI] port {:02}: connected={} enabled={} speed={}({})",
                port_index + 1,
                connected as u8,
                enabled as u8,
                speed,
                decode_port_speed(speed)
            );
        }
    }
}

fn init_xhci_controller() -> Option<XhciRuntime> {
    let Some(controller) = find_xhci_controller() else {
        println!("[xHCI] no controller found on PCI bus");
        return None;
    };

    println!(
        "[xHCI] controller {:02x}:{:02x}.{} vendor={:04x} device={:04x}",
        controller.bdf.bus,
        controller.bdf.device,
        controller.bdf.function,
        controller.vendor_id,
        controller.device_id
    );
    println!(
        "[xHCI] BAR0={:#010x} BAR1={:#010x} {} MMIO={:#018x}",
        controller.bar0,
        controller.bar1,
        if controller.bar_is_64bit { "64-bit" } else { "32-bit" },
        controller.mmio_base
    );

    let mapped = match map_xhci_mmio(&controller) {
        Ok(ptr) => ptr,
        Err(err) => {
            println!("[xHCI] map mmio failed: {:#x}", err);
            return None;
        }
    };

    let Some(regs) = read_xhci_regs(mapped) else {
        println!("[xHCI] invalid capability header");
        return None;
    };

    println!(
        "[xHCI] version={:x}.{:02x} caplen=0x{:x} max_slots={} max_ports={}",
        regs.hci_version >> 8,
        regs.hci_version & 0xFF,
        regs.cap_len,
        regs.max_slots,
        regs.max_ports
    );
    println!(
        "[xHCI] runtime_off=0x{:x} doorbell_off=0x{:x}",
        regs.rt_off,
        regs.db_off
    );

    if reset_xhci(&regs) {
        println!("[xHCI] controller halted+reset complete");
    } else {
        println!("[xHCI] controller reset skipped due to timeout");
        return None;
    }

    dump_ports(&regs);

    let mut command_ring = match CommandRing::new() {
        Ok(r) => r,
        Err(err) => {
            println!("[xHCI] command ring alloc failed: {:#x}", err);
            return None;
        }
    };
    let event_ring = match EventRing::new() {
        Ok(r) => r,
        Err(err) => {
            println!("[xHCI] event ring alloc failed: {:#x}", err);
            return None;
        }
    };

    setup_command_ring_register(&regs, &command_ring);
    setup_interrupter(&regs, &event_ring);
    if !start_xhci(&regs) {
        println!("[xHCI] failed to run controller");
        return None;
    }

    let noop_phys = command_ring.push_noop_command();
    ring_doorbell(&regs, 0, 0);
    println!("[xHCI] command/event ring + interrupter configured");

    Some(XhciRuntime {
        regs,
        command_ring,
        event_ring,
        pending_noop_phys: Some(noop_phys),
        hid: HidParserState::default(),
    })
}

#[rustfmt::skip]
const MAP_NORMAL: [u8; 128] = [
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
const MAP_SHIFT: [u8; 128] = [
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

const SC_LSHIFT: u8 = 0x2A;
const SC_RSHIFT: u8 = 0x36;
const SC_CAPSLOCK: u8 = 0x3A;
const SC_RELEASE: u8 = 0x80;

#[derive(Default)]
struct KeyboardDecoder {
    shift: bool,
    caps: bool,
}

impl KeyboardDecoder {
    fn decode_scancode(&mut self, scancode: u8) -> Option<u8> {
        if scancode & SC_RELEASE != 0 {
            let make = scancode & !SC_RELEASE;
            if make == SC_LSHIFT || make == SC_RSHIFT {
                self.shift = false;
            }
            return None;
        }

        match scancode {
            SC_LSHIFT | SC_RSHIFT => {
                self.shift = true;
                return None;
            }
            SC_CAPSLOCK => {
                self.caps = !self.caps;
                return None;
            }
            _ => {}
        }

        let idx = scancode as usize;
        if idx >= 128 {
            return None;
        }

        let use_shift = self.shift ^ (self.caps && MAP_NORMAL[idx].is_ascii_alphabetic());
        let ch = if use_shift { MAP_SHIFT[idx] } else { MAP_NORMAL[idx] };
        if ch == 0 {
            None
        } else {
            Some(ch)
        }
    }
}

fn log_key_event(ch: u8) {
    match ch {
        b'\n' => println!("[xHCI][KBD] <ENTER>"),
        b'\t' => println!("[xHCI][KBD] <TAB>"),
        0x08 => println!("[xHCI][KBD] <BACKSPACE>"),
        b' '..=b'~' => println!("[xHCI][KBD] '{}'", ch as char),
        _ => println!("[xHCI][KBD] 0x{:02X}", ch),
    }
}

fn log_mouse_event(packet: mouse::MousePacket, last_buttons: &mut u8) {
    let moved = packet.dx != 0 || packet.dy != 0;
    let buttons_changed = packet.buttons != *last_buttons;
    if !moved && !buttons_changed {
        return;
    }

    let dy_screen = -(packet.dy as i16);
    println!(
        "[xHCI][MOUSE] dx={:>4}, dy={:>4}, L={} R={} M={}",
        packet.dx as i16,
        dy_screen,
        packet.left() as u8,
        packet.right() as u8,
        packet.middle() as u8
    );
    *last_buttons = packet.buttons;
}

fn run_input_monitor_loop(mut xhci: Option<XhciRuntime>) {
    let mut decoder = KeyboardDecoder::default();
    let mut last_buttons = 0u8;
    let mut warned_keyboard_err = false;
    let mut warned_mouse_err = false;

    loop {
        let mut handled_any = false;

        if let Some(runtime) = xhci.as_mut() {
            if poll_xhci_events(runtime) {
                handled_any = true;
            }
        }

        loop {
            match keyboard::read_scancode_tap() {
                Ok(Some(scancode)) => {
                    handled_any = true;
                    if let Some(ch) = decoder.decode_scancode(scancode) {
                        log_key_event(ch);
                    }
                }
                Ok(None) => break,
                Err(err) => {
                    if !warned_keyboard_err {
                        println!("[xHCI] keyboard tap error: {:#x}", err);
                        warned_keyboard_err = true;
                    }
                    break;
                }
            }
        }

        loop {
            match mouse::read_packet() {
                Ok(Some(packet)) => {
                    handled_any = true;
                    log_mouse_event(packet, &mut last_buttons);
                }
                Ok(None) => break,
                Err(err) => {
                    if !warned_mouse_err {
                        println!("[xHCI] mouse read error: {:#x}", err);
                        warned_mouse_err = true;
                    }
                    break;
                }
            }
        }

        if !handled_any {
            time::sleep_ms(2);
        }
    }
}

fn main() {
    println!("[xHCI] driver started");
    let xhci_runtime = init_xhci_controller();
    println!("[xHCI] input monitor mode enabled (keyboard tap + mouse packet)");
    run_input_monitor_loop(xhci_runtime);
}
