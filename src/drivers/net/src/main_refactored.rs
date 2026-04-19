mod net_common;
mod pci;
mod util;
mod virtio;

use crate::net_common::{NetDevice, NetKind};
use crate::pci::{find_network_devices, enable_device_command_bits, try_map_mmio_bar0};
use crate::virtio::{virtio_legacy_init_pio, run_virtio_loop};
use swiftlib::time;

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
            if let Some(rt) = virtio_legacy_init_pio(dev) {
                try_map_mmio_bar0(dev);
                run_virtio_loop(rt);
            } else {
                println!("[NETDRV] virtio-net init failed");
                try_map_mmio_bar0(dev);
            }
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
