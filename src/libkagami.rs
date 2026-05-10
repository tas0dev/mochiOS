// Host-side shim for Kagami minimal APIs.

#[cfg(all(unix, target_os = "linux", target_env = "gnu"))]
mod unix_impl {
    use memmap2::MmapMut;
    use std::fs::File;
    use std::os::unix::io::AsRawFd;
    use tempfile::tempfile;
    use wayland_client::protocol::wl_shm_pool::WlShmPool;
    use std::os::unix::io::BorrowedFd;
    use wayland_client::protocol::{
        wl_buffer, wl_compositor, wl_shm, wl_shm::Format, wl_registry, wl_shm_pool,
        wl_surface, wl_pointer, wl_keyboard, wl_seat, wl_callback, wl_shell, wl_shell_surface,
    };
    use wayland_protocols::xdg::shell::client::xdg_wm_base;
    use wayland_protocols::xdg::shell::client::xdg_surface;
    use wayland_protocols::xdg::shell::client::xdg_toplevel;
    use wayland_client::{Connection, EventQueue, QueueHandle, Dispatch};
    use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
    use wayland_client::protocol::wl_surface::WlSurface;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    // Registry state used only for initial global collection
    struct RegistryState;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &wl_registry::WlRegistry,
            _event: wl_registry::Event,
            _data: &GlobalListContents,
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {
            // no-op: global list helper maintains globals
        }
    }

    // Implement empty Dispatch handlers for objects we will create with the same QueueHandle
    // no-op handlers for various protocol objects using () userdata
    impl Dispatch<WlShmPool, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &WlShmPool,
            _event: wl_shm_pool::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {}
    }
    impl Dispatch<wl_buffer::WlBuffer, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &wl_buffer::WlBuffer,
            _event: wl_buffer::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {}
    }
    impl Dispatch<WlSurface, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &WlSurface,
            _event: wl_surface::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {}
    }
    impl Dispatch<wl_compositor::WlCompositor, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &wl_compositor::WlCompositor,
            _event: wl_compositor::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {}
    }
    impl Dispatch<wl_shm::WlShm, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &wl_shm::WlShm,
            _event: wl_shm::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {}
    }
    
    // Dedicated state for input event handling with proper Dispatch implementations
    struct InputState;
    impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for InputState {
        fn event(
            _state: &mut InputState,
            _proxy: &wl_registry::WlRegistry,
            _event: wl_registry::Event,
            _data: &GlobalListContents,
            _conn: &Connection,
            _qh: &QueueHandle<InputState>,
        ) {
            // no-op
        }
    }
    impl Dispatch<WlSurface, ()> for InputState {
        fn event(
            _state: &mut InputState,
            _proxy: &WlSurface,
            _event: wl_surface::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<InputState>,
        ) {}
    }
    impl Dispatch<wl_callback::WlCallback, Arc<AtomicBool>> for InputState {
        fn event(
            _state: &mut InputState,
            _proxy: &wl_callback::WlCallback,
            event: wl_callback::Event,
            data: &Arc<AtomicBool>,
            _conn: &Connection,
            _qh: &QueueHandle<InputState>,
        ) {
            if let wl_callback::Event::Done { .. } = event {
                data.store(true, Ordering::SeqCst);
            }
        }
    }
    
    // Pointer/Keyboard callbacks userdata
    struct PointerHandler(Arc<dyn Fn(f64, f64) + Send + Sync>);
    struct KeyboardHandler(Arc<dyn Fn(u32, wayland_client::WEnum<wl_keyboard::KeyState>) + Send + Sync>);

    impl Dispatch<wl_pointer::WlPointer, Arc<PointerHandler>> for InputState {
        fn event(
            _state: &mut InputState,
            _proxy: &wl_pointer::WlPointer,
            event: wl_pointer::Event,
            data: &Arc<PointerHandler>,
            _conn: &Connection,
            _qh: &QueueHandle<InputState>,
        ) {
            match event {
                wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                    println!("[libkagami] ✅ wl_pointer::Event::Motion fired: ({}, {})", surface_x, surface_y);
                    let callback = &data.0;
                    callback(surface_x, surface_y);
                }
                wl_pointer::Event::Enter { surface_x, surface_y, .. } => {
                    println!("[libkagami] ✅ wl_pointer::Event::Enter fired at ({}, {})", surface_x, surface_y);
                }
                wl_pointer::Event::Leave { .. } => {
                    println!("[libkagami] wl_pointer::Event::Leave fired");
                }
                wl_pointer::Event::Button { button, state, .. } => {
                    println!("[libkagami] ✅ wl_pointer::Event::Button fired: button={}, state={:?}", button, state);
                }
                wl_pointer::Event::Axis { .. } => {
                    println!("[libkagami] wl_pointer::Event::Axis fired (scroll)");
                }
                wl_pointer::Event::Frame => {
                    println!("[libkagami] wl_pointer::Event::Frame");
                }
                wl_pointer::Event::AxisSource { .. } => {
                    println!("[libkagami] wl_pointer::Event::AxisSource");
                }
                wl_pointer::Event::AxisStop { .. } => {
                    println!("[libkagami] wl_pointer::Event::AxisStop");
                }
                wl_pointer::Event::AxisDiscrete { .. } => {
                    println!("[libkagami] wl_pointer::Event::AxisDiscrete");
                }
                _ => {
                    println!("[libkagami] wl_pointer event (unhandled type)");
                }
            }
        }
    }

    impl Dispatch<wl_keyboard::WlKeyboard, Arc<KeyboardHandler>> for InputState {
        fn event(
            _state: &mut InputState,
            _proxy: &wl_keyboard::WlKeyboard,
            event: wl_keyboard::Event,
            data: &Arc<KeyboardHandler>,
            _conn: &Connection,
            _qh: &QueueHandle<InputState>,
        ) {
            match event {
                wl_keyboard::Event::Key { key, state, .. } => {
                    println!("[libkagami] ✅ wl_keyboard::Event::Key fired: key={}, state={:?}", key, state);
                    let callback = &data.0;
                    callback(key, state);
                }
                wl_keyboard::Event::Enter { .. } => {
                    println!("[libkagami] ✅ wl_keyboard::Event::Enter fired (keyboard focus acquired)");
                }
                wl_keyboard::Event::Leave { .. } => {
                    println!("[libkagami] wl_keyboard::Event::Leave fired (keyboard focus lost)");
                }
                wl_keyboard::Event::Keymap { .. } => {
                    println!("[libkagami] wl_keyboard::Event::Keymap");
                }
                wl_keyboard::Event::Modifiers { .. } => {
                    println!("[libkagami] wl_keyboard::Event::Modifiers");
                }
                wl_keyboard::Event::RepeatInfo { .. } => {
                    println!("[libkagami] wl_keyboard::Event::RepeatInfo");
                }
                _ => {
                    println!("[libkagami] wl_keyboard event (unhandled type)");
                }
            }
        }
    }

    impl Dispatch<wl_pointer::WlPointer, Arc<PointerHandler>> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &wl_pointer::WlPointer,
            event: wl_pointer::Event,
            data: &Arc<PointerHandler>,
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {
            match event {
                wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                    println!("[libkagami] ✅ wl_pointer::Event::Motion fired: ({}, {})", surface_x, surface_y);
                    let callback = &data.0;
                    callback(surface_x, surface_y);
                }
                wl_pointer::Event::Enter { surface_x, surface_y, .. } => {
                    println!("[libkagami] ✅ wl_pointer::Event::Enter fired at ({}, {})", surface_x, surface_y);
                }
                wl_pointer::Event::Leave { .. } => {
                    println!("[libkagami] wl_pointer::Event::Leave fired");
                }
                wl_pointer::Event::Button { button, state, .. } => {
                    println!("[libkagami] ✅ wl_pointer::Event::Button fired: button={}, state={:?}", button, state);
                }
                wl_pointer::Event::Axis { .. } => {
                    println!("[libkagami] wl_pointer::Event::Axis fired (scroll)");
                }
                wl_pointer::Event::Frame => {
                    println!("[libkagami] wl_pointer::Event::Frame");
                }
                wl_pointer::Event::AxisSource { .. } => {
                    println!("[libkagami] wl_pointer::Event::AxisSource");
                }
                wl_pointer::Event::AxisStop { .. } => {
                    println!("[libkagami] wl_pointer::Event::AxisStop");
                }
                wl_pointer::Event::AxisDiscrete { .. } => {
                    println!("[libkagami] wl_pointer::Event::AxisDiscrete");
                }
                _ => {
                    println!("[libkagami] wl_pointer event (unhandled type)");
                }
            }
        }
    }

    impl Dispatch<wl_keyboard::WlKeyboard, Arc<KeyboardHandler>> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &wl_keyboard::WlKeyboard,
            event: wl_keyboard::Event,
            data: &Arc<KeyboardHandler>,
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {
            match event {
                wl_keyboard::Event::Key { key, state, .. } => {
                    println!("[libkagami] ✅ wl_keyboard::Event::Key fired: key={}, state={:?}", key, state);
                    let callback = &data.0;
                    callback(key, state);
                }
                wl_keyboard::Event::Enter { .. } => {
                    println!("[libkagami] ✅ wl_keyboard::Event::Enter fired (keyboard focus acquired)");
                }
                wl_keyboard::Event::Leave { .. } => {
                    println!("[libkagami] wl_keyboard::Event::Leave fired (keyboard focus lost)");
                }
                wl_keyboard::Event::Keymap { .. } => {
                    println!("[libkagami] wl_keyboard::Event::Keymap");
                }
                wl_keyboard::Event::Modifiers { .. } => {
                    println!("[libkagami] wl_keyboard::Event::Modifiers");
                }
                wl_keyboard::Event::RepeatInfo { .. } => {
                    println!("[libkagami] wl_keyboard::Event::RepeatInfo");
                }
                _ => {
                    println!("[libkagami] wl_keyboard event (unhandled type)");
                }
            }
        }
    }

    impl Dispatch<wl_callback::WlCallback, Arc<AtomicBool>> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &wl_callback::WlCallback,
            event: wl_callback::Event,
            data: &Arc<AtomicBool>,
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {
            if let wl_callback::Event::Done { .. } = event {
                data.store(true, Ordering::SeqCst);
            }
        }
    }

    // minimal no-op handlers for shell related objects
    impl Dispatch<wl_seat::WlSeat, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &wl_seat::WlSeat,
            event: wl_seat::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {
            match event {
                wl_seat::Event::Capabilities { capabilities } => {
                    println!("[libkagami] wl_seat capabilities: {:?}", capabilities);
                }
                wl_seat::Event::Name { name } => {
                    println!("[libkagami] wl_seat name: {}", name);
                }
                _ => {}
            }
        }
    }
    impl Dispatch<wl_shell::WlShell, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &wl_shell::WlShell,
            _event: wl_shell::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {}
    }
    impl Dispatch<wl_shell_surface::WlShellSurface, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            proxy: &wl_shell_surface::WlShellSurface,
            event: wl_shell_surface::Event,
            _data: &(),
            conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {
            match event {
                wl_shell_surface::Event::Ping { serial } => {
                    println!("[libkagami] wl_shell_surface ping (serial={})", serial);
                    proxy.pong(serial);
                    let _ = conn.flush();
                }
                wl_shell_surface::Event::Configure { edges, width, height } => {
                    println!("[libkagami] wl_shell_surface configure: {}x{}, edges={:?}", width, height, edges);
                }
                wl_shell_surface::Event::PopupDone => {
                    println!("[libkagami] wl_shell_surface popup done");
                }
                _ => {}
            }
        }
    }

    // xdg toplevel handlers: respond to ping, no-op for surface/toplevel events
    impl Dispatch<xdg_wm_base::XdgWmBase, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            proxy: &xdg_wm_base::XdgWmBase,
            event: xdg_wm_base::Event,
            _data: &(),
            conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {
            if let xdg_wm_base::Event::Ping { serial } = event {
                // reply pong
                let _ = proxy.pong(serial);
                // flush to ensure delivery
                let _ = conn.flush();
            }
        }
    }
    impl Dispatch<xdg_surface::XdgSurface, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            proxy: &xdg_surface::XdgSurface,
            event: xdg_surface::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {
            if let xdg_surface::Event::Configure { serial } = event {
                // Acknowledge configure so the compositor can map the surface
                let _ = proxy.ack_configure(serial);
                let _ = _conn.flush();
                println!("libkagami: xdg_surface configure acked (serial={})", serial);
            }
        }
    }
    impl Dispatch<xdg_toplevel::XdgToplevel, ()> for RegistryState {
        fn event(
            _state: &mut RegistryState,
            _proxy: &xdg_toplevel::XdgToplevel,
            event: xdg_toplevel::Event,
            _data: &(),
            _conn: &Connection,
            _qh: &QueueHandle<RegistryState>,
        ) {
            match event {
                xdg_toplevel::Event::Configure { width, height, states } => {
                    if width == 0 || height == 0 {
                        println!("[libkagami] ⚠️  xdg_toplevel configure: {}x{} (zero size!), states={:?}", width, height, states);
                    } else {
                        println!("[libkagami] ✓ xdg_toplevel configure: {}x{}, states={:?}", width, height, states);
                    }
                }
                xdg_toplevel::Event::Close => {
                    println!("[libkagami] xdg_toplevel close requested");
                }
                _ => {}
            }
        }
    }

    fn connect_wayland() -> Result<(Connection, EventQueue<RegistryState>, GlobalList), String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("Wayland connect failed: {}", e))?;
        let (globals, event_queue) = registry_queue_init::<RegistryState>(&conn)
            .map_err(|e| format!("registry init failed: {:?}", e))?;
        Ok((conn, event_queue, globals))
    }

    /// wl_shm を使って匿名ファイル + mmap を作り、Pool と Buffer を返す。
    /// 返り値: (tempfile, mmap, pool, buffer)
    fn create_shm_buffer(
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<RegistryState>,
        width: i32,
        height: i32,
    ) -> Result<(File, MmapMut, WlShmPool, wl_buffer::WlBuffer), String> {
        let stride = (width * 4) as usize;
        let size = stride.checked_mul(height as usize).ok_or("size overflow")?;

        // 匿名テンポラリファイルを作る
        let tmp = tempfile().map_err(|e| format!("tempfile failed: {}", e))?;
        tmp.set_len(size as u64)
            .map_err(|e| format!("set_len failed: {}", e))?;

        // mmap
        let mmap = unsafe { MmapMut::map_mut(&tmp).map_err(|e| format!("mmap failed: {}", e))? };

        // pool と buffer
        let fd = tmp.as_raw_fd();
        // create BorrowedFd from raw fd
        let bfd = unsafe { BorrowedFd::borrow_raw(fd) };
        // Attempt to create pool/buffer using current API (requires QueueHandle)
        let pool = shm.create_pool(bfd, size as i32, qh, ());
        // Use XRGB8888 to avoid alpha blending issues on compositors that ignore alpha
        let buffer = pool.create_buffer(0, width, height, stride as i32, Format::Xrgb8888, qh, ());

        Ok((tmp, mmap, pool, buffer))
    }


    /// 高レベル表示管理: compositor/shm および EventQueue を保持する
    pub struct HostDisplay {
        conn: Connection,
        event_queue: EventQueue<RegistryState>,
        globals: GlobalList,
        compositor: wl_compositor::WlCompositor,
        shm: wl_shm::WlShm,
        pointer: Option<wl_pointer::WlPointer>,
        keyboard: Option<wl_keyboard::WlKeyboard>,
        shell_surface: Option<wl_shell_surface::WlShellSurface>,
        xdg_surface: Option<xdg_surface::XdgSurface>,
    }

    // Helper to register input handlers
    #[allow(dead_code)]
    pub fn register_pointer_and_keyboard(
        host: &mut HostDisplay,
        pointer_cb: Option<Arc<dyn Fn(f64,f64) + Send + Sync>>,
        keyboard_cb: Option<Arc<dyn Fn(u32, wayland_client::WEnum<wl_keyboard::KeyState>) + Send + Sync>>,
    ) -> Result<(), String> {
        let qh = host.event_queue.handle();
        // bind seat
        println!("[libkagami] Attempting to bind wl_seat...");
        let seat = host.globals.bind::<wl_seat::WlSeat, RegistryState, ()>(&qh, 1..=1, ())
            .map_err(|_| "seat not available".to_string())?;
        println!("[libkagami] ✓ wl_seat bound successfully");
        
        // Sync to receive capabilities event
        println!("[libkagami] Syncing to receive wl_seat capabilities...");
        host.conn.flush().map_err(|e| format!("flush failed: {}", e))?;
        let mut st = RegistryState;
        let _ = host.event_queue.roundtrip(&mut st).map_err(|e| format!("roundtrip failed: {}", e))?;
        println!("[libkagami] ✓ wl_seat sync complete");
        
        if let Some(pcb) = pointer_cb {
            println!("[libkagami] Getting wl_pointer...");
            let ud = Arc::new(PointerHandler(pcb));
            let pointer = seat.get_pointer(&qh, ud.clone());
            host.pointer = Some(pointer);
            println!("[libkagami] ✓ wl_pointer obtained and handler registered");
        }
        if let Some(kcb) = keyboard_cb {
            println!("[libkagami] Getting wl_keyboard...");
            let ud = Arc::new(KeyboardHandler(kcb));
            let keyboard = seat.get_keyboard(&qh, ud.clone());
            host.keyboard = Some(keyboard);
            println!("[libkagami] ✓ wl_keyboard obtained and handler registered");
        }
        
        // Flush to ensure pointer/keyboard requests are sent to the server
        println!("[libkagami] Flushing to send input device requests...");
        host.conn.flush().map_err(|e| format!("flush failed: {}", e))?;
        
        // Roundtrip to process initial input device events (e.g., Enter events)
        println!("[libkagami] Syncing input devices...");
        let mut st = RegistryState;
        let _ = host.event_queue.roundtrip(&mut st).map_err(|e| format!("roundtrip failed: {}", e))?;
        println!("[libkagami] ✓ Input devices synced and ready");
        
        Ok(())
    }

    impl HostDisplay {
        /// Wayland 接続して必要なグローバル（compositor, shm）まで取得する
        pub fn new() -> Result<Self, String> {
            let (conn, event_queue, globals) = connect_wayland()?;
            // obtain a queue handle for binding
            let qh = event_queue.handle();
            // 主要なグローバルを取得
            let compositor = globals
                .bind::<wl_compositor::WlCompositor, RegistryState, ()>(&qh, 1..=4, ())
                .map_err(|_| "Compositor not available".to_string())?;
            let shm = globals
                .bind::<wl_shm::WlShm, RegistryState, ()>(&qh, 1..=1, ())
                .map_err(|_| "wl_shm not available".to_string())?;
            println!("libkagami: connected to compositor and wl_shm");
            Ok(HostDisplay { conn, event_queue, globals, compositor, shm, pointer: None, keyboard: None, shell_surface: None, xdg_surface: None })
        }

        /// イベントのディスパッチを行う（呼び出し側でループする）
        pub fn dispatch(&mut self) -> Result<(), String> {
            let mut st = RegistryState;
            match self.event_queue.dispatch_pending(&mut st) {
                Ok(count) => {
                    if count > 0 {
                        println!("[libkagami] dispatch_pending processed {} events", count);
                    }
                    Ok(())
                }
                Err(e) => Err(format!("dispatch failed: {}", e))
            }
        }

        /// 新しい surface と double-buffer を作る
        pub fn create_surface(&mut self, width: i32, height: i32) -> Result<HostSurface, String> {
            let qh = self.event_queue.handle();
            let surface = self.compositor.create_surface(&qh, ());
            // create buffers
            let (tmp0, mmap0, _pool0, buffer0) = create_shm_buffer(&self.shm, &qh, width, height)?;
            let (tmp1, mmap1, _pool1, buffer1) = create_shm_buffer(&self.shm, &qh, width, height)?;
            let hs = HostSurface {
                surface,
                conn: self.conn.clone(),
                qh,
                width,
                height,
                stride: (width * 4) as usize,
                mmap0,
                mmap1,
                _tmp0: tmp0,
                _tmp1: tmp1,
                buffer0,
                buffer1,
                front: 0,
            };
            // Do not attach a buffer yet when creating the surface. When using xdg,
            // attaching a buffer before the xdg_surface configure is an error on some compositors.
            println!("libkagami: created surface ({}x{}), buffers allocated", width, height);
            Ok(hs)
        }

        /// Try to make a surface a toplevel using wl_shell or xdg-shell
        pub fn set_toplevel(&mut self, hs: &mut HostSurface) -> Result<(), String> {
            let qh = self.event_queue.handle();
            
            // Try wl_shell first (more direct and predictable for Weston)
            if let Ok(wl_shell) = self.globals.bind::<wl_shell::WlShell, RegistryState, ()>(&qh, 1..=1, ()) {
                println!("[libkagami] Using wl_shell protocol...");
                let shell_surface = wl_shell.get_shell_surface(&hs.surface, &qh, ());
                shell_surface.set_toplevel();
                shell_surface.set_class("ViewKit".to_string());
                shell_surface.set_title("ViewKit".to_string());
                println!("[libkagami] wl_shell_surface: set_toplevel, class=ViewKit, title=ViewKit");
                
                // CRITICAL: Store shell_surface to prevent it from being destroyed
                self.shell_surface = Some(shell_surface);
                
                self.conn.flush().map_err(|e| format!("conn flush failed: {}", e))?;
                
                // Attach buffer immediately (wl_shell doesn't require configure ack)
                hs.swap_and_commit().map_err(|e| format!("initial buffer attach failed: {}", e))?;
                println!("[libkagami] ✓ wl_shell buffer attached (960x540)");
                
                // Dispatch to let compositor process and map the window
                for i in 0..15 {
                    let mut st = RegistryState;
                    let _ = self.event_queue.dispatch_pending(&mut st);
                    if i % 5 == 0 {
                        println!("[libkagami] wl_shell dispatch #{}", i);
                    }
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                return Ok(());
            }
            
            // Fallback: Try xdg-shell
            if let Ok(xdg) = self.globals.bind::<xdg_wm_base::XdgWmBase, RegistryState, ()>(&qh, 1..=1, ()) {
                println!("[libkagami] wl_shell not available, trying xdg-shell...");
                let xsurf = xdg.get_xdg_surface(&hs.surface, &qh, ());
                let toplevel = xsurf.get_toplevel(&qh, ());
                let _ = toplevel.set_title("ViewKit".to_string());
                let _ = toplevel.set_app_id("ViewKit".to_string());
                println!("[libkagami] xdg-shell: title=ViewKit, app_id=ViewKit");
                
                // CRITICAL: Store xdg_surface to prevent it from being destroyed
                self.xdg_surface = Some(xsurf);
                
                hs.surface.commit();
                self.conn.flush().map_err(|e| format!("conn flush failed: {}", e))?;
                println!("[libkagami] Waiting for xdg_surface configure...");
                let mut st = RegistryState;
                let _ = self.event_queue.roundtrip(&mut st).map_err(|e| format!("roundtrip failed: {}", e))?;
                println!("[libkagami] ✓ xdg_surface configure acked");
                
                hs.swap_and_commit().map_err(|e| format!("initial buffer attach failed: {}", e))?;
                println!("[libkagami] ✓ xdg-shell buffer attached (960x540)");
                
                for i in 0..5 {
                    let mut st = RegistryState;
                    let _ = self.event_queue.dispatch_pending(&mut st);
                    if i == 0 || i == 4 {
                        println!("[libkagami] xdg-shell post-commit sync #{}", i);
                    }
                }
                return Ok(());
            }
            
            Err("Neither wl_shell nor xdg-shell available".to_string())
        }
    }

    /// Surface と double-buffer の小さなラッパ
    pub struct HostSurface {
        surface: WlSurface,
        conn: Connection,
        qh: QueueHandle<RegistryState>,
        width: i32,
        height: i32,
        stride: usize,
        mmap0: MmapMut,
        mmap1: MmapMut,
        _tmp0: File,
        _tmp1: File,
        buffer0: wl_buffer::WlBuffer,
        buffer1: wl_buffer::WlBuffer,
        front: usize,
    }

    impl HostSurface {
        /// Width accessor
        pub fn width(&self) -> i32 { self.width }
        /// Height accessor
        pub fn height(&self) -> i32 { self.height }
        /// Stride accessor
        pub fn stride(&self) -> usize { self.stride }

        /// 書き込み可能バッファスライスを取得
        pub fn back_buffer_mut(&mut self) -> &mut [u8] {
            if self.front == 0 { &mut self.mmap1[..] } else { &mut self.mmap0[..] }
        }

        /// 現在のフロントを attach + commit する
        pub fn commit_front(&mut self) -> Result<(), String> {
            if self.front == 0 {
                self.surface.attach(Some(&self.buffer0), 0, 0);
                self.front = 0;
            } else {
                self.surface.attach(Some(&self.buffer1), 0, 0);
                self.front = 1;
            }
            self.surface.damage_buffer(0, 0, self.width, self.height);
            self.surface.commit();
            let res = self.conn.flush().map_err(|e| format!("conn flush failed: {}", e));
            match self.front {
                0 => println!("libkagami: commit_front -> front=0 attached buffer0"),
                1 => println!("libkagami: commit_front -> front=1 attached buffer1"),
                _ => println!("libkagami: commit_front -> front={} (unknown)", self.front),
            }
            res
        }

        /// バッファをスワップして commit（back を front にする）
        pub fn swap_and_commit(&mut self) -> Result<(), String> {
            if self.front == 0 {
                // front 0 -> use buffer1 as new front
                self.mmap1.flush().map_err(|e| format!("mmap flush failed: {}", e))?;
                self.surface.attach(Some(&self.buffer1), 0, 0);
                self.front = 1;
            } else {
                self.mmap0.flush().map_err(|e| format!("mmap flush failed: {}", e))?;
                self.surface.attach(Some(&self.buffer0), 0, 0);
                self.front = 0;
            }
            self.surface.damage_buffer(0, 0, self.width, self.height);
            self.surface.commit();
            let res = self.conn.flush().map_err(|e| format!("conn flush failed: {}", e));
            match self.front {
                0 => println!("libkagami: swap_and_commit -> front=0 attached buffer0"),
                1 => println!("libkagami: swap_and_commit -> front=1 attached buffer1"),
                _ => println!("libkagami: swap_and_commit -> front={} (unknown)", self.front),
            }
            res
        }

        /// request a frame callback; provided AtomicBool is set true when done
        pub fn request_frame(&mut self, flag: Arc<AtomicBool>) -> Result<(), String> {
            // create frame callback with AtomicBool userdata so RegistryState::Dispatch handles Done
            let cb = self.surface.frame(&self.qh, flag.clone());
            let _ = cb;
            Ok(())
        }
    }

    // エクスポート
    pub use HostDisplay as host_HostDisplay;
    pub use HostSurface as host_HostSurface;
}

#[cfg(all(unix, target_os = "linux", target_env = "musl"))]
mod mochi_impl {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    use swiftlib::ipc::{ipc_recv, ipc_send};
    use swiftlib::privileged;
    use swiftlib::task::{find_process_by_name, yield_now};
    use wayland_client::WEnum;
    use wayland_client::protocol::wl_keyboard;

    const IPC_BUF_SIZE: usize = 4128;
    const KAGAMI_PROCESS_CANDIDATES: [&str; 3] =
        ["/applications/Kagami.app/entry.elf", "Kagami.app", "entry.elf"];

    const OP_REQ_CREATE_WINDOW: u32 = 1;
    const OP_RES_WINDOW_CREATED: u32 = 2;
    const OP_REQ_ATTACH_SHARED: u32 = 5;
    const OP_REQ_PRESENT_SHARED: u32 = 6;
    const OP_RES_SHARED_ATTACHED: u32 = 7;
    const LAYER_APP: u8 = 1;

    struct SharedSurface {
        virt_addr: u64,
        page_count: u64,
        total_pixels: usize,
    }

    pub struct HostDisplay {
        kagami_tid: u64,
        ipc_buf: [u8; IPC_BUF_SIZE],
        pointer_callback: Option<Arc<dyn Fn(f64, f64) + Send + Sync>>,
        keyboard_callback: Option<Arc<dyn Fn(u32, WEnum<wl_keyboard::KeyState>) + Send + Sync>>,
    }

    pub struct HostSurface {
        kagami_tid: u64,
        window_id: u32,
        width: i32,
        height: i32,
        stride: usize,
        front: usize,
        back0: Vec<u8>,
        back1: Vec<u8>,
        shared: SharedSurface,
    }

    pub fn register_pointer_and_keyboard(
        host: &mut HostDisplay,
        pointer_cb: Option<Arc<dyn Fn(f64, f64) + Send + Sync>>,
        keyboard_cb: Option<Arc<dyn Fn(u32, WEnum<wl_keyboard::KeyState>) + Send + Sync>>,
    ) -> Result<(), String> {
        if pointer_cb.is_some() {
            println!("[libkagami-mochi] Registering pointer callback");
            host.pointer_callback = pointer_cb;
        }
        if keyboard_cb.is_some() {
            println!("[libkagami-mochi] Registering keyboard callback");
            host.keyboard_callback = keyboard_cb;
        }
        println!("[libkagami-mochi] Input callbacks registered");
        Ok(())
    }

    impl HostDisplay {
        pub fn new() -> Result<Self, String> {
            let kagami_tid = parse_kagami_tid_from_args()
                .or_else(find_kagami_tid)
                .ok_or("Kagami not found; pass --kagami-tid=<tid> or launch from Kagami".to_string())?;
            Ok(Self {
                kagami_tid,
                ipc_buf: [0u8; IPC_BUF_SIZE],
                pointer_callback: None,
                keyboard_callback: None,
            })
        }

        pub fn dispatch(&mut self) -> Result<(), String> {
            #[cfg(all(unix, target_os = "linux", target_env = "musl"))]
            {
                let (_, bytes_received) = ipc_recv(&mut self.ipc_buf);
                self.process_input_events(bytes_received as usize);
            }
            
            #[cfg(not(all(unix, target_os = "linux", target_env = "musl")))]
            {
                let _ = ipc_recv(&mut self.ipc_buf);
            }
            
            yield_now();
            Ok(())
        }
        
        fn process_input_events(&mut self, bytes_received: usize) {
            if bytes_received >= 4 {
                // Parse message type from first 4 bytes (little-endian)
                let msg_type = u32::from_le_bytes([
                    self.ipc_buf[0],
                    self.ipc_buf[1],
                    self.ipc_buf[2],
                    self.ipc_buf[3],
                ]);
                
                // Handle input events (if Kagami sends them)
                // Message format (hypothetical):
                // Type 100: Pointer Motion { type=100, x=f64, y=f64 }
                // Type 101: Keyboard Key { type=101, key=u32, state=u32 }
                if msg_type == 100 && bytes_received >= 20 {
                    // Pointer Motion: 4 (type) + 8 (x:f64) + 8 (y:f64) = 20 bytes
                    let x = f64::from_le_bytes([
                        self.ipc_buf[4], self.ipc_buf[5], self.ipc_buf[6], self.ipc_buf[7],
                        self.ipc_buf[8], self.ipc_buf[9], self.ipc_buf[10], self.ipc_buf[11],
                    ]);
                    let y = f64::from_le_bytes([
                        self.ipc_buf[12], self.ipc_buf[13], self.ipc_buf[14], self.ipc_buf[15],
                        self.ipc_buf[16], self.ipc_buf[17], self.ipc_buf[18], self.ipc_buf[19],
                    ]);
                    
                    if let Some(cb) = &self.pointer_callback {
                        println!("[libkagami-mochi] Pointer Motion: ({}, {})", x, y);
                        cb(x, y);
                    }
                } else if msg_type == 101 && bytes_received >= 12 {
                    // Keyboard Key: 4 (type) + 4 (key:u32) + 4 (state:u32) = 12 bytes
                    let key = u32::from_le_bytes([
                        self.ipc_buf[4], self.ipc_buf[5], self.ipc_buf[6], self.ipc_buf[7],
                    ]);
                    let state_raw = u32::from_le_bytes([
                        self.ipc_buf[8], self.ipc_buf[9], self.ipc_buf[10], self.ipc_buf[11],
                    ]);
                    
                    if let Some(cb) = &self.keyboard_callback {
                        // Convert state_raw to WEnum<wl_keyboard::KeyState>
                        let state = if state_raw == 0 {
                            WEnum::Value(wl_keyboard::KeyState::Released)
                        } else {
                            WEnum::Value(wl_keyboard::KeyState::Pressed)
                        };
                        println!("[libkagami-mochi] Keyboard Key {} ({:?})", key, state);
                        cb(key, state);
                    }
                }
            }
        }

        pub fn create_surface(&mut self, width: i32, height: i32) -> Result<HostSurface, String> {
            if width <= 0 || height <= 0 {
                return Err("invalid surface size".into());
            }
            let window_id = create_window(self.kagami_tid, width as u16, height as u16)?;
            let shared = request_shared_surface(
                self.kagami_tid,
                &mut self.ipc_buf,
                window_id,
                width as u16,
                height as u16,
            )?;
            let size = (width as usize)
                .checked_mul(height as usize)
                .and_then(|v| v.checked_mul(4))
                .ok_or("surface size overflow")?;
            Ok(HostSurface {
                kagami_tid: self.kagami_tid,
                window_id,
                width,
                height,
                stride: width as usize * 4,
                front: 0,
                back0: vec![0; size],
                back1: vec![0; size],
                shared,
            })
        }

        pub fn set_toplevel(&mut self, hs: &mut HostSurface) -> Result<(), String> {
            hs.present().map_err(|e| format!("present failed: {}", e))
        }
    }

    impl HostSurface {
        pub fn width(&self) -> i32 { self.width }
        pub fn height(&self) -> i32 { self.height }
        pub fn stride(&self) -> usize { self.stride }

        pub fn back_buffer_mut(&mut self) -> &mut [u8] {
            if self.front == 0 {
                &mut self.back1
            } else {
                &mut self.back0
            }
        }

        pub fn commit_front(&mut self) -> Result<(), String> {
            // mochi 実装では swap_and_commit() で既に present 済み。
            // ここで再送すると OP_REQ_PRESENT_SHARED が二重送信され、送信失敗を招く。
            Ok(())
        }

        pub fn swap_and_commit(&mut self) -> Result<(), String> {
            self.front = if self.front == 0 { 1 } else { 0 };
            self.present()
                .map_err(|e| format!("present(swap front={}) failed: {}", self.front, e))
        }

        pub fn request_frame(&mut self, flag: Arc<AtomicBool>) -> Result<(), String> {
            flag.store(true, Ordering::SeqCst);
            Ok(())
        }

        fn present(&self) -> Result<(), &'static str> {
            let src = if self.front == 0 {
                &self.back0
            } else {
                &self.back1
            };
            blit_shared_surface(&self.shared, src);
            present_shared(self.kagami_tid, self.window_id)?;
            Ok(())
        }
    }

    fn create_window(kagami_tid: u64, width: u16, height: u16) -> Result<u32, String> {
        let mut req = [0u8; 9];
        req[0..4].copy_from_slice(&OP_REQ_CREATE_WINDOW.to_le_bytes());
        req[4..6].copy_from_slice(&width.to_le_bytes());
        req[6..8].copy_from_slice(&height.to_le_bytes());
        req[8] = LAYER_APP;
        if (ipc_send(kagami_tid, &req) as i64) < 0 {
            return Err("send create window failed".into());
        }

        let mut recv = [0u8; IPC_BUF_SIZE];
        for _ in 0..512 {
            let (sender, len) = ipc_recv(&mut recv);
            if sender != kagami_tid || len < 8 {
                yield_now();
                continue;
            }
            let op = u32::from_le_bytes([recv[0], recv[1], recv[2], recv[3]]);
            if op != OP_RES_WINDOW_CREATED {
                continue;
            }
            let window_id = u32::from_le_bytes([recv[4], recv[5], recv[6], recv[7]]);
            return Ok(window_id);
        }
        Err("window create timeout".into())
    }

    fn request_shared_surface(
        kagami_tid: u64,
        ipc_buf: &mut [u8; IPC_BUF_SIZE],
        window_id: u32,
        width: u16,
        height: u16,
    ) -> Result<SharedSurface, String> {
        let total = (width as usize)
            .checked_mul(height as usize)
            .ok_or("size overflow")?;
        let total_bytes = total.checked_mul(4).ok_or("size overflow")?;
        let page_count = total_bytes.div_ceil(4096);
        if page_count == 0 {
            return Err("page_count was zero".into());
        }

        let mut phys_pages = vec![0u64; page_count];
        let virt_addr = unsafe {
            privileged::alloc_shared_pages(page_count as u64, Some(phys_pages.as_mut_slice()), 0)
        };
        if (virt_addr as i64) < 0 || virt_addr == 0 {
            return Err("alloc_shared_pages failed".into());
        }

        let mut attach = [0u8; 12];
        attach[0..4].copy_from_slice(&OP_REQ_ATTACH_SHARED.to_le_bytes());
        attach[4..8].copy_from_slice(&window_id.to_le_bytes());
        attach[8..10].copy_from_slice(&width.to_le_bytes());
        attach[10..12].copy_from_slice(&height.to_le_bytes());
        if (ipc_send(kagami_tid, &attach) as i64) < 0 {
            return Err("send attach request failed".into());
        }
        let send_pages_ret = unsafe { privileged::ipc_send_pages(kagami_tid, phys_pages.as_slice(), 0) };
        if (send_pages_ret as i64) < 0 {
            return Err("ipc_send_pages failed".into());
        }

        for _ in 0..512 {
            let (sender, len) = ipc_recv(ipc_buf);
            if sender != kagami_tid || len < 8 {
                yield_now();
                continue;
            }
            let op = u32::from_le_bytes([ipc_buf[0], ipc_buf[1], ipc_buf[2], ipc_buf[3]]);
            if op != OP_RES_SHARED_ATTACHED {
                continue;
            }
            let ack_window = u32::from_le_bytes([ipc_buf[4], ipc_buf[5], ipc_buf[6], ipc_buf[7]]);
            if ack_window == window_id {
                // alloc_shared_pages / ipc_send_pages と対になるページ配列は
                // mochiOS 側では解放時クラッシュを誘発するケースがあるため保持しない。
                core::mem::forget(phys_pages);
                return Ok(SharedSurface {
                    virt_addr,
                    page_count: page_count as u64,
                    total_pixels: total,
                });
            }
        }

        Err("shared attach ack timeout".into())
    }

    fn blit_shared_surface(surface: &SharedSurface, src_rgba_bytes: &[u8]) {
        let src_pixels = src_rgba_bytes.len() / 4;
        let mapped_pixels = (surface.page_count as usize).saturating_mul(4096) / 4;
        let count = surface.total_pixels.min(src_pixels).min(mapped_pixels);
        unsafe {
            let dst = core::slice::from_raw_parts_mut(surface.virt_addr as *mut u32, count);
            for (i, d) in dst.iter_mut().enumerate() {
                let base = i * 4;
                let b = src_rgba_bytes[base] as u32;
                let g = src_rgba_bytes[base + 1] as u32;
                let r = src_rgba_bytes[base + 2] as u32;
                *d = 0xFF00_0000 | (r << 16) | (g << 8) | b;
            }
        }
    }

    fn present_shared(kagami_tid: u64, window_id: u32) -> Result<(), &'static str> {
        let mut present = [0u8; 8];
        present[0..4].copy_from_slice(&OP_REQ_PRESENT_SHARED.to_le_bytes());
        present[4..8].copy_from_slice(&window_id.to_le_bytes());
        if (ipc_send(kagami_tid, &present) as i64) < 0 {
            return Err("send present failed");
        }
        Ok(())
    }

    fn find_kagami_tid() -> Option<u64> {
        for name in KAGAMI_PROCESS_CANDIDATES {
            if let Some(tid) = find_process_by_name(name) {
                return Some(tid);
            }
        }
        None
    }

    fn parse_kagami_tid_from_args() -> Option<u64> {
        for arg in std::env::args().skip(1) {
            if let Some(rest) = arg.strip_prefix("--kagami-tid=")
                && let Ok(tid) = rest.parse::<u64>()
                && tid != 0
            {
                return Some(tid);
            }
        }
        None
    }

    pub use HostDisplay as host_HostDisplay;
    pub use HostSurface as host_HostSurface;
}

#[cfg(not(any(
    all(unix, target_os = "linux", target_env = "gnu"),
    all(unix, target_os = "linux", target_env = "musl")
)))]
mod stub_impl {
    // mochiOS向けスタブ
    pub fn host_connect_wayland() -> Result<(), String> {
        Err("libkagami host shim is only available on unix hosts".into())
    }
    pub fn host_create_shm_buffer(_: &(), _: i32, _: i32) -> Result<(), String> {
        Err("libkagami host shim is only available on unix hosts".into())
    }
}

#[cfg(not(any(
    all(unix, target_os = "linux", target_env = "gnu"),
    all(unix, target_os = "linux", target_env = "musl")
)))]
pub use stub_impl::*;
// 公開インターフェース
#[cfg(all(unix, target_os = "linux", target_env = "gnu"))]
pub use unix_impl::{host_HostDisplay, host_HostSurface, register_pointer_and_keyboard};
#[cfg(all(unix, target_os = "linux", target_env = "musl"))]
pub use mochi_impl::{host_HostDisplay, host_HostSurface, register_pointer_and_keyboard};
