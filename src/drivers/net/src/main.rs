use swiftlib::{mmio, port, privileged, task, time};

const PCI_CFG_ADDR_PORT: u16 = 0xCF8;
const PCI_CFG_DATA_PORT: u16 = 0xCFC;

const PCI_COMMAND_IO: u16 = 1 << 0;
const PCI_COMMAND_MEM: u16 = 1 << 1;
const PCI_COMMAND_BUS_MASTER: u16 = 1 << 2;

const CLASS_NETWORK: u8 = 0x02;
const VIRTIO_NET_F_MAC: u32 = 5;

const VIRTIO_PIO_DEVICE_FEATURES: u16 = 0x00;
const VIRTIO_PIO_GUEST_FEATURES: u16 = 0x04;
const VIRTIO_PIO_QUEUE_ADDR_PFN: u16 = 0x08;
const VIRTIO_PIO_QUEUE_SIZE: u16 = 0x0C;
const VIRTIO_PIO_QUEUE_SELECT: u16 = 0x0E;
const VIRTIO_PIO_QUEUE_NOTIFY: u16 = 0x10;
const VIRTIO_PIO_DEVICE_STATUS: u16 = 0x12;
const VIRTIO_PIO_ISR_STATUS: u16 = 0x13;
const VIRTIO_PIO_DEVICE_CONFIG: u16 = 0x14;

const VIRTIO_STATUS_ACKNOWLEDGE: u8 = 1 << 0;
const VIRTIO_STATUS_DRIVER: u8 = 1 << 1;
const VIRTIO_STATUS_DRIVER_OK: u8 = 1 << 2;

const VIRTIO_QUEUE_RX: u16 = 0;
const VIRTIO_QUEUE_TX: u16 = 1;
const PAGE_SIZE: usize = 4096;

#[derive(Clone, Copy, Debug)]
struct PciBdf {
    bus: u8,
    device: u8,
    function: u8,
}

#[derive(Clone, Copy, Debug)]
enum NetKind {
    VirtioNet,
    E1000,
    Unknown,
}

#[derive(Clone, Copy, Debug)]
struct NetDevice {
    bdf: PciBdf,
    vendor_id: u16,
    device_id: u16,
    kind: NetKind,
    bar0: u32,
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

fn pci_write_u16(bdf: PciBdf, offset: u8, value: u16) {
    let aligned = offset & 0xFC;
    let mut reg = pci_read_u32(bdf, aligned);
    let shift = u32::from(offset & 0x02) * 8;
    reg &= !(0xFFFFu32 << shift);
    reg |= u32::from(value) << shift;
    let addr = pci_config_address(bdf, aligned);
    port::outl(PCI_CFG_ADDR_PORT, addr);
    port::outl(PCI_CFG_DATA_PORT, reg);
}

fn pci_function_exists(bdf: PciBdf) -> bool {
    pci_read_u16(bdf, 0x00) != 0xFFFF
}

fn classify_net_device(vendor_id: u16, device_id: u16) -> NetKind {
    match (vendor_id, device_id) {
        // virtio-net (legacy/transitional)
        (0x1AF4, 0x1000) => NetKind::VirtioNet,
        // virtio-net (modern)
        (0x1AF4, 0x1041) => NetKind::VirtioNet,
        // Intel e1000 family (QEMUでよく使う)
        (0x8086, 0x100E) | (0x8086, 0x100F) | (0x8086, 0x10D3) => NetKind::E1000,
        _ => NetKind::Unknown,
    }
}

fn enable_device_command_bits(bdf: PciBdf) {
    let mut command = pci_read_u16(bdf, 0x04);
    command |= PCI_COMMAND_IO | PCI_COMMAND_MEM | PCI_COMMAND_BUS_MASTER;
    pci_write_u16(bdf, 0x04, command);
}

fn find_network_devices() -> Vec<NetDevice> {
    let mut devices = Vec::new();

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

                let class_reg = pci_read_u32(bdf, 0x08);
                let class_code = ((class_reg >> 24) & 0xFF) as u8;
                if class_code != CLASS_NETWORK {
                    continue;
                }

                let vendor_device = pci_read_u32(bdf, 0x00);
                let vendor_id = (vendor_device & 0xFFFF) as u16;
                let device_id = ((vendor_device >> 16) & 0xFFFF) as u16;
                let bar0 = pci_read_u32(bdf, 0x10);
                let kind = classify_net_device(vendor_id, device_id);

                devices.push(NetDevice {
                    bdf,
                    vendor_id,
                    device_id,
                    kind,
                    bar0,
                });
            }
        }
    }

    devices
}

fn try_map_mmio_bar0(dev: NetDevice) {
    if (dev.bar0 & 0x1) != 0 {
        if let NetKind::VirtioNet = dev.kind {
            println!("[NETDRV] BAR0 is I/O space (legacy virtio-net PIO)");
        } else {
            println!("[NETDRV] BAR0 is I/O space (PIO), MMIO map skipped");
        }
        return;
    }

    let mmio_base = u64::from(dev.bar0 & 0xFFFF_FFF0);
    if mmio_base == 0 {
        println!("[NETDRV] BAR0 MMIO base is zero");
        return;
    }

    match mmio::map_physical(mmio_base, 0x1000) {
        Ok(mapped) => {
            println!(
                "[NETDRV] MMIO mapped phys={:#x} -> virt={:#x}",
                mmio_base, mapped as u64
            );
        }
        Err(errno) => {
            println!(
                "[NETDRV] MMIO map failed phys={:#x}, errno={}",
                mmio_base, errno
            );
        }
    }
}

fn virtio_pio_base(bar0: u32) -> Option<u16> {
    if (bar0 & 0x1) == 0 {
        return None;
    }
    let base = bar0 & 0xFFFF_FFFC;
    if base == 0 || base > 0xFFFF {
        return None;
    }
    Some(base as u16)
}

fn virtio_legacy_init_pio(dev: NetDevice) {
    let Some(base) = virtio_pio_base(dev.bar0) else {
        println!("[NETDRV] virtio-net BAR0 is not legacy PIO");
        return;
    };

    let device_features = port::inl(base + VIRTIO_PIO_DEVICE_FEATURES);
    println!("[NETDRV] virtio legacy PIO base={:#x}", base);
    println!("[NETDRV] virtio device_features={:#010x}", device_features);

    port::outb(base + VIRTIO_PIO_DEVICE_STATUS, 0);
    port::outb(
        base + VIRTIO_PIO_DEVICE_STATUS,
        VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER,
    );
    port::outl(base + VIRTIO_PIO_GUEST_FEATURES, device_features);
    port::outb(
        base + VIRTIO_PIO_DEVICE_STATUS,
        VIRTIO_STATUS_ACKNOWLEDGE | VIRTIO_STATUS_DRIVER | VIRTIO_STATUS_DRIVER_OK,
    );

    let status = port::inb(base + VIRTIO_PIO_DEVICE_STATUS);
    println!("[NETDRV] virtio status={:#04x}", status);

    if (device_features & (1u32 << VIRTIO_NET_F_MAC)) != 0 {
        let mut mac = [0u8; 6];
        for (i, byte) in mac.iter_mut().enumerate() {
            *byte = port::inb(base + VIRTIO_PIO_DEVICE_CONFIG + i as u16);
        }
        println!(
            "[NETDRV] virtio MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        );
    } else {
        println!("[NETDRV] virtio MAC feature not advertised");
    }

    let isr = port::inb(base + VIRTIO_PIO_ISR_STATUS);
    println!("[NETDRV] virtio isr={:#04x}", isr);

    setup_virtio_legacy_queue(base, VIRTIO_QUEUE_RX);
    setup_virtio_legacy_queue(base, VIRTIO_QUEUE_TX);
}

fn align_up(value: usize, align: usize) -> usize {
    if align == 0 {
        return value;
    }
    (value + (align - 1)) & !(align - 1)
}

fn compute_virtqueue_bytes(queue_size: usize) -> usize {
    // descriptor table + avail ring + padding(used ring alignment) + used ring
    let desc_bytes = 16usize.saturating_mul(queue_size);
    let avail_bytes = 6usize.saturating_add(2usize.saturating_mul(queue_size));
    let used_bytes = 6usize.saturating_add(8usize.saturating_mul(queue_size));
    let used_off = align_up(desc_bytes.saturating_add(avail_bytes), PAGE_SIZE);
    used_off.saturating_add(used_bytes)
}

fn is_syscall_error(value: u64) -> bool {
    (-4095..=-1).contains(&(value as i64))
}

fn alloc_phys_contiguous(bytes: usize) -> Option<(u64, *mut u8)> {
    #[derive(Clone, Copy)]
    struct PageAlloc {
        virt: u64,
        phys: u64,
    }

    let page_count = align_up(bytes, PAGE_SIZE) / PAGE_SIZE;
    let required_run = page_count;
    let max_probe_pages = 64usize;
    let mut pool: Vec<PageAlloc> = Vec::new();

    for _ in 0..max_probe_pages {
        let mut phys_buf = [0u64; 1];
        let virt = unsafe { privileged::alloc_shared_pages(1, Some(&mut phys_buf), 0) };
        if is_syscall_error(virt) {
            println!("[NETDRV] alloc_shared_pages failed: errno={}", virt as i64);
            break;
        }
        pool.push(PageAlloc {
            virt,
            phys: phys_buf[0],
        });

        if pool.len() < required_run {
            continue;
        }

        let mut phys_sorted: Vec<u64> = pool.iter().map(|p| p.phys).collect();
        phys_sorted.sort_unstable();
        phys_sorted.dedup();

        for start_idx in 0..=phys_sorted.len().saturating_sub(required_run) {
            let start_phys = phys_sorted[start_idx];
            let mut contiguous = true;
            for step in 1..required_run {
                let expected = start_phys + (step as u64 * PAGE_SIZE as u64);
                if phys_sorted[start_idx + step] != expected {
                    contiguous = false;
                    break;
                }
            }
            if !contiguous {
                continue;
            }

            let selected_phys: Vec<u64> = (0..required_run)
                .map(|step| start_phys + (step as u64 * PAGE_SIZE as u64))
                .collect();

            let queue_virt = unsafe {
                privileged::map_physical_pages(task::gettid(), &selected_phys, 0)
            };
            if is_syscall_error(queue_virt) {
                println!(
                    "[NETDRV] map_physical_pages failed: errno={}",
                    queue_virt as i64
                );
                continue;
            }

            for page in &pool {
                let is_selected = selected_phys.iter().any(|&p| p == page.phys);
                let rc = privileged::unmap_pages(page.virt, 1, !is_selected);
                if rc != 0 {
                    println!(
                        "[NETDRV] unmap_pages failed virt={:#x} rc={}",
                        page.virt, rc as i64
                    );
                }
            }

            return Some((start_phys, queue_virt as *mut u8));
        }
    }

    for page in &pool {
        let rc = privileged::unmap_pages(page.virt, 1, true);
        if rc != 0 {
            println!(
                "[NETDRV] unmap_pages cleanup failed virt={:#x} rc={}",
                page.virt, rc as i64
            );
        }
    }

    None
}

fn setup_virtio_legacy_queue(base: u16, queue_index: u16) {
    port::outw(base + VIRTIO_PIO_QUEUE_SELECT, queue_index);
    let queue_size = port::inw(base + VIRTIO_PIO_QUEUE_SIZE);
    if queue_size == 0 {
        println!("[NETDRV] queue {} not available", queue_index);
        return;
    }

    let bytes = compute_virtqueue_bytes(queue_size as usize);
    let Some((phys, _virt)) = alloc_phys_contiguous(bytes) else {
        println!(
            "[NETDRV] queue {} allocation failed (size={} bytes)",
            queue_index, bytes
        );
        return;
    };

    let pfn = (phys >> 12) as u32;
    port::outl(base + VIRTIO_PIO_QUEUE_ADDR_PFN, pfn);
    let programmed = port::inl(base + VIRTIO_PIO_QUEUE_ADDR_PFN);
    if programmed != pfn {
        println!(
            "[NETDRV] queue {} PFN mismatch: wrote={:#x} read={:#x}",
            queue_index, pfn, programmed
        );
        return;
    }

    println!(
        "[NETDRV] queue {} ready size={} bytes={} pfn={:#x}",
        queue_index, queue_size, bytes, pfn
    );

    let _ = VIRTIO_PIO_QUEUE_NOTIFY;
}

fn init_device(dev: NetDevice) {
    println!(
        "[NETDRV] net device {:02x}:{:02x}.{} vendor={:04x} device={:04x} kind={:?}",
        dev.bdf.bus,
        dev.bdf.device,
        dev.bdf.function,
        dev.vendor_id,
        dev.device_id,
        dev.kind
    );

    enable_device_command_bits(dev.bdf);

    match dev.kind {
        NetKind::VirtioNet => {
            println!("[NETDRV] virtio-net detected");
            virtio_legacy_init_pio(dev);
            try_map_mmio_bar0(dev);
        }
        NetKind::E1000 => {
            println!("[NETDRV] e1000 detected (phase1: probe only)");
            try_map_mmio_bar0(dev);
        }
        NetKind::Unknown => {
            println!("[NETDRV] unknown NIC class device (phase1: probe only)");
            try_map_mmio_bar0(dev);
        }
    }
}

fn main() {
    println!("[NETDRV] network driver started");

    let devices = find_network_devices();
    if devices.is_empty() {
        println!("[NETDRV] no PCI network controller found");
    } else {
        println!("[NETDRV] found {} network controller(s)", devices.len());
        for dev in devices {
            init_device(dev);
        }
    }

    println!("[NETDRV] driver idle");
    loop {
        time::sleep_ms(1000);
    }
}
