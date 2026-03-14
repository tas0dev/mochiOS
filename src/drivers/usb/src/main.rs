use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{compiler_fence, Ordering as AtomicOrdering};
use std::alloc::{alloc_zeroed, Layout};

use swiftlib::{keyboard, mmio, mouse, port, time};

mod define;
use define::*;

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

    fn zero(&self) {
        unsafe {
            core::ptr::write_bytes(self.virt, 0, self.size);
        }
    }

    fn write_u32(&self, offset: usize, value: u32) {
        if offset + 4 > self.size {
            return;
        }
        unsafe {
            write_volatile(self.virt.add(offset) as *mut u32, value);
        }
    }

    fn read_u32(&self, offset: usize) -> u32 {
        if offset + 4 > self.size {
            return 0;
        }
        unsafe { read_volatile(self.virt.add(offset) as *const u32) }
    }

    fn write_bytes(&self, offset: usize, bytes: &[u8]) {
        if offset + bytes.len() > self.size {
            return;
        }
        unsafe {
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.virt.add(offset), bytes.len());
        }
    }

    fn read_bytes(&self, offset: usize, len: usize) -> Vec<u8> {
        if len == 0 || offset + len > self.size {
            return Vec::new();
        }
        let mut out = vec![0u8; len];
        for (i, b) in out.iter_mut().enumerate() {
            *b = unsafe { read_volatile(self.virt.add(offset + i)) };
        }
        out
    }
}

impl TransferRing {
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

    fn push_trb(&mut self, mut trb: [u32; 4]) -> u64 {
        let idx = self.enqueue_idx;
        let trb_phys = self.page.phys + (idx * TRB_SIZE) as u64;
        if self.cycle {
            trb[3] |= 1;
        } else {
            trb[3] &= !1;
        }
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
        self.push_command([0, 0, 0, TRB_TYPE_NOOP_CMD << 10])
    }

    fn push_command(&mut self, mut trb: [u32; 4]) -> u64 {
        let idx = self.enqueue_idx;
        let trb_phys = self.page.phys + (idx * TRB_SIZE) as u64;
        if self.cycle {
            trb[3] |= 1;
        } else {
            trb[3] &= !1;
        }
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

#[derive(Clone, Copy)]
struct HidEndpointConfig {
    ep_addr: u8,
    dci: u8,
    ep_type: u8,
    max_packet: u16,
    interval: u8,
}

struct HidEndpointState {
    config: HidEndpointConfig,
    ring: TransferRing,
    report_buf: DmaPage,
    report_len: usize,
}

struct UsbDeviceState {
    port_id: u8,
    port_speed: u8,
    slot_id: u8,
    ep0_max_packet: u16,
    input_ctx: DmaPage,
    dev_ctx: DmaPage,
    ep0_ring: TransferRing,
    descriptor_buf: DmaPage,
    hid_ep: Option<HidEndpointState>,
}

enum PendingCommandKind {
    Noop,
    EnableSlot { port_id: u8 },
    AddressDevice { slot_id: u8 },
    ConfigureEndpoint { slot_id: u8, dci: u8 },
}

struct PendingCommand {
    trb_phys: u64,
    kind: PendingCommandKind,
}

enum PendingTransferKind {
    DeviceDescriptor8 { slot_id: u8 },
    ConfigHeader { slot_id: u8 },
    ConfigFull { slot_id: u8 },
    InterruptIn { slot_id: u8, dci: u8 },
}

struct PendingTransfer {
    trb_phys: u64,
    data_phys: u64,
    data_len: usize,
    kind: PendingTransferKind,
}

struct XhciRuntime {
    regs: XhciRegs,
    dcbaa: DmaPage,
    command_ring: CommandRing,
    event_ring: EventRing,
    devices: Vec<UsbDeviceState>,
    pending_commands: Vec<PendingCommand>,
    pending_transfers: Vec<PendingTransfer>,
    enumerating_port: Option<u8>,
    hid: HidParserState,
}

fn read_xhci_regs(base: *mut u8) -> Option<XhciRegs> {
    let cap_len = mmio_read_u8(base, 0x00) as usize;
    if cap_len < 0x20 {
        return None;
    }

    let hci_version = mmio_read_u16(base, 0x02);
    let hcs_params1 = mmio_read_u32(base, 0x04);
    let hccparams1 = mmio_read_u32(base, 0x10);
    let max_slots = (hcs_params1 & 0xFF) as u8;
    let max_ports = ((hcs_params1 >> 24) & 0xFF) as u8;
    let db_off = (mmio_read_u32(base, 0x14) & !0x3) as usize;
    let rt_off = (mmio_read_u32(base, 0x18) & !0x1F) as usize;
    let context_size = if (hccparams1 & (1 << 2)) != 0 { 64 } else { 32 };

    Some(XhciRegs {
        base,
        cap_len,
        op_base: cap_len,
        db_off,
        rt_off,
        max_ports,
        max_slots,
        hci_version,
        hccparams1,
        context_size,
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

#[inline]
fn input_ctx_offset(ctx_size: usize, index: usize) -> usize {
    ctx_size * index
}

#[inline]
fn device_ctx_offset(ctx_size: usize, dci: usize) -> usize {
    ctx_size * dci
}

#[inline]
fn endpoint_dci_from_address(ep_addr: u8) -> u8 {
    let ep_num = ep_addr & 0x0F;
    if ep_num == 0 {
        1
    } else if (ep_addr & 0x80) != 0 {
        ep_num.saturating_mul(2).saturating_add(1)
    } else {
        ep_num.saturating_mul(2)
    }
}

fn endpoint_type_from_descriptor(ep_addr: u8, attrs: u8) -> Option<u8> {
    match attrs & 0x3 {
        0 => Some(4), // control
        1 => Some(if (ep_addr & 0x80) != 0 { 5 } else { 1 }), // isoch
        2 => Some(if (ep_addr & 0x80) != 0 { 6 } else { 2 }), // bulk
        3 => Some(if (ep_addr & 0x80) != 0 { 7 } else { 3 }), // interrupt
        _ => None,
    }
}

fn default_ep0_max_packet(speed: u8) -> u16 {
    match speed {
        4 | 5 => 512,
        3 => 64,
        2 | 1 => 8,
        _ => 8,
    }
}

fn portsc_offset(regs: &XhciRegs, port_id: u8) -> usize {
    regs.op_base + 0x400 + (usize::from(port_id.saturating_sub(1)) * 0x10)
}

fn read_port_speed(regs: &XhciRegs, port_id: u8) -> u8 {
    let portsc = mmio_read_u32(regs.base, portsc_offset(regs, port_id));
    ((portsc >> 10) & 0x0F) as u8
}

fn find_first_connected_port(regs: &XhciRegs) -> Option<u8> {
    for p in 1..=regs.max_ports {
        let portsc = mmio_read_u32(regs.base, portsc_offset(regs, p));
        if (portsc & 1) != 0 {
            return Some(p);
        }
    }
    None
}

fn submit_command(runtime: &mut XhciRuntime, trb: [u32; 4], kind: PendingCommandKind) -> u64 {
    let trb_phys = runtime.command_ring.push_command(trb);
    runtime.pending_commands.push(PendingCommand { trb_phys, kind });
    ring_doorbell(&runtime.regs, 0, 0);
    trb_phys
}

fn write_dcbaa_slot(dcbaa: &DmaPage, slot_id: u8, value: u64) {
    if slot_id == 0 {
        return;
    }
    let off = usize::from(slot_id) * 8;
    if off + 8 > dcbaa.size {
        return;
    }
    dcbaa.write_u32(off, value as u32);
    dcbaa.write_u32(off + 4, (value >> 32) as u32);
}

fn find_device_index(runtime: &XhciRuntime, slot_id: u8) -> Option<usize> {
    runtime.devices.iter().position(|d| d.slot_id == slot_id)
}

fn build_address_input_context(regs: &XhciRegs, dev: &mut UsbDeviceState) {
    dev.input_ctx.zero();
    dev.input_ctx.write_u32(0x00, 0);
    dev.input_ctx.write_u32(0x04, 0x3); // add slot + ep0

    let slot_off = input_ctx_offset(regs.context_size, 1);
    let slot_dw0 = ((u32::from(dev.port_speed) & 0xF) << 20) | (1 << 27);
    let slot_dw1 = (u32::from(dev.port_id) & 0xFF) << 16;
    dev.input_ctx.write_u32(slot_off + 0x00, slot_dw0);
    dev.input_ctx.write_u32(slot_off + 0x04, slot_dw1);

    let ep0_off = input_ctx_offset(regs.context_size, 2);
    let tr_deq = (dev.ep0_ring.ring_phys() & !0xF) | 1;
    let ep0_dw1 = 3 | (4 << 3) | (u32::from(dev.ep0_max_packet) << 16); // CErr=3, Control EP
    dev.input_ctx.write_u32(ep0_off + 0x00, 0);
    dev.input_ctx.write_u32(ep0_off + 0x04, ep0_dw1);
    dev.input_ctx.write_u32(ep0_off + 0x08, tr_deq as u32);
    dev.input_ctx.write_u32(ep0_off + 0x0C, (tr_deq >> 32) as u32);
    dev.input_ctx
        .write_u32(ep0_off + 0x10, u32::from(dev.ep0_max_packet));
}

fn build_configure_endpoint_input_context(regs: &XhciRegs, dev: &mut UsbDeviceState) -> Option<u8> {
    let hid = dev.hid_ep.as_ref()?;
    let dci = hid.config.dci;

    dev.input_ctx.zero();
    dev.input_ctx.write_u32(0x00, 0); // drop flags
    dev.input_ctx
        .write_u32(0x04, (1u32 << 0) | (1u32 << u32::from(dci))); // add slot + endpoint

    // slot context は既存 device context をコピー
    let slot_in_off = input_ctx_offset(regs.context_size, 1);
    let slot_dev_off = device_ctx_offset(regs.context_size, 0);
    for off in (0..regs.context_size).step_by(4) {
        let v = dev.dev_ctx.read_u32(slot_dev_off + off);
        dev.input_ctx.write_u32(slot_in_off + off, v);
    }

    let mut slot_dw0 = dev.input_ctx.read_u32(slot_in_off);
    slot_dw0 &= !((0x1F << 27) | (0x0F << 20));
    slot_dw0 |= ((u32::from(dci) & 0x1F) << 27) | ((u32::from(dev.port_speed) & 0x0F) << 20);
    dev.input_ctx.write_u32(slot_in_off, slot_dw0);

    let ep_off = input_ctx_offset(regs.context_size, usize::from(dci) + 1);
    let tr_deq = (hid.ring.ring_phys() & !0xF) | 1;
    let interval = u32::from(core::cmp::max(hid.config.interval, 1));
    let ep_dw0 = interval << 16;
    let ep_dw1 = 3 | (u32::from(hid.config.ep_type) << 3) | (u32::from(hid.config.max_packet) << 16);
    dev.input_ctx.write_u32(ep_off + 0x00, ep_dw0);
    dev.input_ctx.write_u32(ep_off + 0x04, ep_dw1);
    dev.input_ctx.write_u32(ep_off + 0x08, tr_deq as u32);
    dev.input_ctx.write_u32(ep_off + 0x0C, (tr_deq >> 32) as u32);
    dev.input_ctx
        .write_u32(ep_off + 0x10, u32::from(hid.config.max_packet));

    Some(dci)
}

fn submit_enable_slot_for_port(runtime: &mut XhciRuntime, port_id: u8) {
    if runtime.enumerating_port.is_some() {
        return;
    }
    if runtime.devices.iter().any(|d| d.port_id == port_id) {
        return;
    }
    runtime.enumerating_port = Some(port_id);
    submit_command(
        runtime,
        [0, 0, 0, TRB_TYPE_ENABLE_SLOT_CMD << 10],
        PendingCommandKind::EnableSlot { port_id },
    );
    println!("[xHCI] submitted Enable Slot for port {}", port_id);
}

fn create_device_for_slot(runtime: &mut XhciRuntime, port_id: u8, slot_id: u8) -> Result<(), u64> {
    let speed = read_port_speed(&runtime.regs, port_id);
    let mut dev = UsbDeviceState {
        port_id,
        port_speed: speed,
        slot_id,
        ep0_max_packet: default_ep0_max_packet(speed),
        input_ctx: DmaPage::alloc(PAGE_SIZE)?,
        dev_ctx: DmaPage::alloc(PAGE_SIZE)?,
        ep0_ring: TransferRing::new()?,
        descriptor_buf: DmaPage::alloc(PAGE_SIZE)?,
        hid_ep: None,
    };
    dev.dev_ctx.zero();
    build_address_input_context(&runtime.regs, &mut dev);
    write_dcbaa_slot(&runtime.dcbaa, slot_id, dev.dev_ctx.phys);
    let input_ctx_phys = dev.input_ctx.phys;

    runtime.devices.push(dev);
    submit_command(
        runtime,
        [
            input_ctx_phys as u32,
            (input_ctx_phys >> 32) as u32,
            0,
            (TRB_TYPE_ADDRESS_DEVICE_CMD << 10) | (u32::from(slot_id) << 24),
        ],
        PendingCommandKind::AddressDevice { slot_id },
    );
    println!(
        "[xHCI] slot {} assigned to port {} (speed={} {})",
        slot_id,
        port_id,
        speed,
        decode_port_speed(speed)
    );
    Ok(())
}

fn submit_control_in_transfer(
    runtime: &mut XhciRuntime,
    slot_id: u8,
    request: u8,
    value: u16,
    index: u16,
    length: u16,
    kind: PendingTransferKind,
) -> bool {
    let Some(dev_idx) = find_device_index(runtime, slot_id) else {
        return false;
    };

    let (status_trb_phys, data_phys, data_len) = {
        let dev = &mut runtime.devices[dev_idx];
        dev.descriptor_buf.zero();

        let setup_packet = u64::from(0x80u8)
            | (u64::from(request) << 8)
            | (u64::from(value) << 16)
            | (u64::from(index) << 32)
            | (u64::from(length) << 48);

        let setup_trb = [
            setup_packet as u32,
            (setup_packet >> 32) as u32,
            8,
            (TRB_TYPE_SETUP_STAGE << 10) | (3 << 16) | (1 << 6), // TRT=IN, IDT
        ];
        let data_trb = [
            dev.descriptor_buf.phys as u32,
            (dev.descriptor_buf.phys >> 32) as u32,
            u32::from(length),
            (TRB_TYPE_DATA_STAGE << 10) | (1 << 16) | (1 << 4), // IN + CH
        ];
        let status_trb = [0, 0, 0, (TRB_TYPE_STATUS_STAGE << 10) | (1 << 5)]; // IOC

        let _ = dev.ep0_ring.push_trb(setup_trb);
        let _ = dev.ep0_ring.push_trb(data_trb);
        let status_trb_phys = dev.ep0_ring.push_trb(status_trb);
        (
            status_trb_phys,
            dev.descriptor_buf.phys,
            usize::from(length).min(dev.descriptor_buf.size),
        )
    };

    runtime.pending_transfers.push(PendingTransfer {
        trb_phys: status_trb_phys,
        data_phys,
        data_len,
        kind,
    });
    ring_doorbell(&runtime.regs, usize::from(slot_id), 1); // EP0
    true
}

fn submit_get_device_descriptor8(runtime: &mut XhciRuntime, slot_id: u8) -> bool {
    submit_control_in_transfer(
        runtime,
        slot_id,
        0x06,
        USB_DESC_DEVICE << 8,
        0,
        8,
        PendingTransferKind::DeviceDescriptor8 { slot_id },
    )
}

fn submit_get_config_header(runtime: &mut XhciRuntime, slot_id: u8) -> bool {
    submit_control_in_transfer(
        runtime,
        slot_id,
        0x06,
        USB_DESC_CONFIGURATION << 8,
        0,
        9,
        PendingTransferKind::ConfigHeader { slot_id },
    )
}

fn submit_get_config_full(runtime: &mut XhciRuntime, slot_id: u8, total_len: u16) -> bool {
    submit_control_in_transfer(
        runtime,
        slot_id,
        0x06,
        USB_DESC_CONFIGURATION << 8,
        0,
        total_len,
        PendingTransferKind::ConfigFull { slot_id },
    )
}

fn parse_hid_endpoint_from_config(config: &[u8]) -> Option<HidEndpointConfig> {
    let mut idx = 0usize;
    let mut in_hid_interface = false;

    while idx + 2 <= config.len() {
        let len = config[idx] as usize;
        if len < 2 || idx + len > config.len() {
            break;
        }

        let desc_type = config[idx + 1];
        match desc_type {
            0x04 => {
                if len >= 9 {
                    let class = config[idx + 5];
                    in_hid_interface = class == 0x03;
                } else {
                    in_hid_interface = false;
                }
            }
            0x05 if in_hid_interface && len >= 7 => {
                let ep_addr = config[idx + 2];
                let attrs = config[idx + 3];
                let transfer_type = attrs & 0x3;
                if (ep_addr & 0x80) == 0 || transfer_type != 0x03 {
                    idx += len;
                    continue;
                }

                let max_packet =
                    u16::from_le_bytes([config[idx + 4], config[idx + 5]]) & 0x07FF;
                let interval = core::cmp::max(config[idx + 6], 1);
                let dci = endpoint_dci_from_address(ep_addr);
                let ep_type = endpoint_type_from_descriptor(ep_addr, attrs)?;
                return Some(HidEndpointConfig {
                    ep_addr,
                    dci,
                    ep_type,
                    max_packet,
                    interval,
                });
            }
            _ => {}
        }

        idx += len;
    }

    None
}

fn submit_configure_endpoint_command(runtime: &mut XhciRuntime, slot_id: u8) -> bool {
    let Some(dev_idx) = find_device_index(runtime, slot_id) else {
        return false;
    };
    let Some(dci) = build_configure_endpoint_input_context(&runtime.regs, &mut runtime.devices[dev_idx]) else {
        return false;
    };
    let input_ctx_phys = runtime.devices[dev_idx].input_ctx.phys;
    submit_command(
        runtime,
        [
            input_ctx_phys as u32,
            (input_ctx_phys >> 32) as u32,
            0,
            (TRB_TYPE_CONFIGURE_ENDPOINT_CMD << 10) | (u32::from(slot_id) << 24),
        ],
        PendingCommandKind::ConfigureEndpoint { slot_id, dci },
    );
    true
}

fn submit_interrupt_in_transfer(runtime: &mut XhciRuntime, slot_id: u8, dci: u8) -> bool {
    let Some(dev_idx) = find_device_index(runtime, slot_id) else {
        return false;
    };

    let (trb_phys, data_phys, data_len, doorbell_target) = {
        let dev = &mut runtime.devices[dev_idx];
        let Some(hid) = dev.hid_ep.as_mut() else {
            return false;
        };
        let report_len = core::cmp::max(1usize, hid.report_len);
        hid.report_buf.zero();
        let trb = [
            hid.report_buf.phys as u32,
            (hid.report_buf.phys >> 32) as u32,
            report_len as u32,
            (TRB_TYPE_NORMAL << 10) | (1 << 5) | (1 << 2), // IOC + ISP
        ];
        let trb_phys = hid.ring.push_trb(trb);
        (trb_phys, hid.report_buf.phys, report_len, hid.config.dci)
    };

    let target = if dci == 0 { doorbell_target } else { dci };
    runtime.pending_transfers.push(PendingTransfer {
        trb_phys,
        data_phys,
        data_len,
        kind: PendingTransferKind::InterruptIn { slot_id, dci: target },
    });
    ring_doorbell(&runtime.regs, usize::from(slot_id), u32::from(target));
    true
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

fn handle_command_completion_event(
    runtime: &mut XhciRuntime,
    cmd_ptr: u64,
    completion_code: u8,
    slot_id_from_event: u8,
) {
    let Some(pos) = runtime
        .pending_commands
        .iter()
        .position(|c| c.trb_phys == cmd_ptr)
    else {
        println!(
            "[xHCI] command completion (untracked): code={} slot={} ptr={:#x}",
            completion_code, slot_id_from_event, cmd_ptr
        );
        return;
    };

    let pending = runtime.pending_commands.remove(pos);
    match pending.kind {
        PendingCommandKind::Noop => {
            println!(
                "[xHCI] No-Op command completion: code={} slot={}",
                completion_code, slot_id_from_event
            );
        }
        PendingCommandKind::EnableSlot { port_id } => {
            runtime.enumerating_port = None;
            if completion_code != 1 || slot_id_from_event == 0 {
                println!(
                    "[xHCI] Enable Slot failed: port={} code={}",
                    port_id, completion_code
                );
                return;
            }
            if let Err(err) = create_device_for_slot(runtime, port_id, slot_id_from_event) {
                println!(
                    "[xHCI] create slot/device failed: slot={} port={} err={:#x}",
                    slot_id_from_event, port_id, err
                );
            }
        }
        PendingCommandKind::AddressDevice { slot_id } => {
            if completion_code != 1 {
                println!(
                    "[xHCI] Address Device failed: slot={} code={}",
                    slot_id, completion_code
                );
                return;
            }
            println!("[xHCI] Address Device completed: slot={}", slot_id);
            let _ = submit_get_device_descriptor8(runtime, slot_id);
        }
        PendingCommandKind::ConfigureEndpoint { slot_id, dci } => {
            if completion_code != 1 {
                println!(
                    "[xHCI] Configure Endpoint failed: slot={} dci={} code={}",
                    slot_id, dci, completion_code
                );
                return;
            }
            println!(
                "[xHCI] Configure Endpoint completed: slot={} dci={}",
                slot_id, dci
            );
            let _ = submit_interrupt_in_transfer(runtime, slot_id, dci);
        }
    }
}

fn handle_transfer_event(
    runtime: &mut XhciRuntime,
    transfer_ptr: u64,
    completion_code: u8,
    slot_id: u8,
    ep_id: u8,
) {
    let Some(pos) = runtime
        .pending_transfers
        .iter()
        .position(|t| t.trb_phys == transfer_ptr)
    else {
        println!(
            "[xHCI] transfer event (untracked): slot={} ep={} code={} trb={:#x}",
            slot_id, ep_id, completion_code, transfer_ptr
        );
        return;
    };

    let pending = runtime.pending_transfers.remove(pos);
    let success = completion_code == 1 || completion_code == 13; // Success / Short Packet

    match pending.kind {
        PendingTransferKind::DeviceDescriptor8 { slot_id } => {
            if !success {
                println!(
                    "[xHCI] device descriptor transfer failed: slot={} code={}",
                    slot_id, completion_code
                );
                return;
            }
            if let Some(dev_idx) = find_device_index(runtime, slot_id) {
                let desc = runtime.devices[dev_idx].descriptor_buf.read_bytes(0, 8);
                if desc.len() >= 8 {
                    let mps = desc[7];
                    if matches!(mps, 8 | 16 | 32 | 64) {
                        runtime.devices[dev_idx].ep0_max_packet = u16::from(mps);
                    }
                    println!(
                        "[xHCI] device descriptor: slot={} max_packet0={}",
                        slot_id, runtime.devices[dev_idx].ep0_max_packet
                    );
                }
            }
            let _ = submit_get_config_header(runtime, slot_id);
        }
        PendingTransferKind::ConfigHeader { slot_id } => {
            if !success {
                println!(
                    "[xHCI] config header transfer failed: slot={} code={}",
                    slot_id, completion_code
                );
                return;
            }
            if let Some(dev_idx) = find_device_index(runtime, slot_id) {
                let head = runtime.devices[dev_idx].descriptor_buf.read_bytes(0, 9);
                if head.len() >= 4 {
                    let total = u16::from_le_bytes([head[2], head[3]]);
                    let clamped = usize::from(total).min(runtime.devices[dev_idx].descriptor_buf.size);
                    if clamped >= 9 {
                        let _ = submit_get_config_full(runtime, slot_id, clamped as u16);
                    }
                }
            }
        }
        PendingTransferKind::ConfigFull { slot_id } => {
            if !success {
                println!(
                    "[xHCI] full config transfer failed: slot={} code={}",
                    slot_id, completion_code
                );
                return;
            }
            let Some(dev_idx) = find_device_index(runtime, slot_id) else {
                return;
            };
            let config_bytes = runtime.devices[dev_idx]
                .descriptor_buf
                .read_bytes(0, pending.data_len);
            let Some(hid_cfg) = parse_hid_endpoint_from_config(&config_bytes) else {
                println!("[xHCI] HID interrupt endpoint not found in config descriptor");
                return;
            };

            let ring = match TransferRing::new() {
                Ok(r) => r,
                Err(err) => {
                    println!("[xHCI] HID ring alloc failed: {:#x}", err);
                    return;
                }
            };
            let report_buf = match DmaPage::alloc(PAGE_SIZE) {
                Ok(p) => p,
                Err(err) => {
                    println!("[xHCI] HID report buffer alloc failed: {:#x}", err);
                    return;
                }
            };
            let report_len = core::cmp::max(
                8usize,
                core::cmp::min(usize::from(hid_cfg.max_packet), 64usize),
            );
            runtime.devices[dev_idx].hid_ep = Some(HidEndpointState {
                config: hid_cfg,
                ring,
                report_buf,
                report_len,
            });
            println!(
                "[xHCI] HID endpoint selected: slot={} ep=0x{:02x} dci={} max_packet={} interval={}",
                slot_id, hid_cfg.ep_addr, hid_cfg.dci, hid_cfg.max_packet, hid_cfg.interval
            );
            let _ = submit_configure_endpoint_command(runtime, slot_id);
        }
        PendingTransferKind::InterruptIn { slot_id, dci } => {
            if !success {
                println!(
                    "[xHCI] interrupt IN transfer failed: slot={} dci={} code={}",
                    slot_id, dci, completion_code
                );
            }
            if let Some(dev_idx) = find_device_index(runtime, slot_id) {
                if let Some(hid) = runtime.devices[dev_idx].hid_ep.as_ref() {
                    let report = hid.report_buf.read_bytes(0, hid.report_len);
                    if !report.is_empty() {
                        parse_hid_report(slot_id, dci, &report, &mut runtime.hid);
                    }
                }
            }
            let _ = submit_interrupt_in_transfer(runtime, slot_id, dci);
        }
    }
}

fn handle_port_status_change_event(runtime: &mut XhciRuntime, port_id: u8, completion_code: u8) {
    println!(
        "[xHCI] port status change event: port={} code={}",
        port_id, completion_code
    );
    dump_ports(&runtime.regs);
    let portsc = mmio_read_u32(runtime.regs.base, portsc_offset(&runtime.regs, port_id));
    if (portsc & 1) != 0 {
        submit_enable_slot_for_port(runtime, port_id);
    }
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
                handle_command_completion_event(runtime, cmd_ptr, completion_code, slot_id);
            }
            TRB_TYPE_PORT_STATUS_CHANGE => {
                let port_id = ((event[0] >> 24) & 0xFF) as u8;
                handle_port_status_change_event(runtime, port_id, completion_code);
            }
            TRB_TYPE_TRANSFER_EVENT => {
                let transfer_ptr = u64::from(event[0]) | (u64::from(event[1]) << 32);
                handle_transfer_event(runtime, transfer_ptr, completion_code, slot_id, ep_id);
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

    let command_ring = match CommandRing::new() {
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
    let dcbaa = match DmaPage::alloc(PAGE_SIZE) {
        Ok(p) => p,
        Err(err) => {
            println!("[xHCI] DCBAA alloc failed: {:#x}", err);
            return None;
        }
    };
    dcbaa.zero();

    setup_command_ring_register(&regs, &command_ring);
    setup_interrupter(&regs, &event_ring);
    mmio_write_u64(regs.base, regs.op_base + OP_DCBAAP, dcbaa.phys);
    if !start_xhci(&regs) {
        println!("[xHCI] failed to run controller");
        return None;
    }

    let mut runtime = XhciRuntime {
        regs,
        dcbaa,
        command_ring,
        event_ring,
        devices: Vec::new(),
        pending_commands: Vec::new(),
        pending_transfers: Vec::new(),
        enumerating_port: None,
        hid: HidParserState::default(),
    };
    let _ = submit_command(
        &mut runtime,
        [0, 0, 0, TRB_TYPE_NOOP_CMD << 10],
        PendingCommandKind::Noop,
    );
    if let Some(port) = find_first_connected_port(&runtime.regs) {
        submit_enable_slot_for_port(&mut runtime, port);
    }
    println!("[xHCI] command/event ring + interrupter configured");

    Some(runtime)
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
