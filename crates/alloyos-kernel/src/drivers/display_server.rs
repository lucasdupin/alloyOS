//! Kernel-owned presentation/input and supervised display IPC. No UI toolkit.
use crate::{
    sync::IrqMutex,
    task::{
        pcb::{Task, TaskState},
        scheduler,
    },
};
use alloc::{collections::VecDeque, string::String, sync::Arc, vec::Vec};
use alloyos_mcp::{
    display::{self, ChatItem, Event, State, TileEvent},
    AppTile,
};
use core::cell::RefCell;
/// Minimal kernel-owned startup bars; no font, assets or Slint dependency.
pub fn boot_splash(size: alloyos_proto::FramebufferInfo) {
    if !super::virtio_gpu::has_hardware_display() {
        return;
    }
    let mut pixels = alloc::vec![0xff101116;(size.width as usize)*(size.height as usize)];
    let bar_width = (size.width / 10).max(1);
    let bar_height = (size.height / 40).max(1);
    for bar in 0..3 {
        let x = size.width / 2 - bar_width * 2 + bar * bar_width * 3 / 2;
        let y = size.height / 2 - bar_height / 2;
        for row in y..y + bar_height {
            for col in x..x + bar_width {
                pixels[(row * size.width + col) as usize] = 0xff759aa3;
            }
        }
    }
    super::virtio_gpu::present_ui_frame(&pixels, size.width, size.height);
    crate::serial_println!("[splash] Kernel startup indicator presented");
}
static SERVICE: IrqMutex<Service> = IrqMutex::new(Service {
    pid: 0,
    state: Vec::new(),
    events: VecDeque::new(),
    frame: Vec::new(),
    width: 0,
    height: 0,
    presented: false,
    applied_pid: 0,
    applied_revision: 0,
    applied_digest: 0,
});
struct Service {
    pid: u64,
    state: Vec<u8>,
    events: VecDeque<Event>,
    frame: Vec<u32>,
    width: u32,
    height: u32,
    presented: bool,
    applied_pid: u64,
    applied_revision: u64,
    applied_digest: u64,
}
pub struct Dashboard {
    pinned: Vec<String>,
    revision: u64,
}
impl Dashboard {
    pub fn pinned_ids(&self) -> &[String] {
        &self.pinned
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
}
pub struct DisplayProxy {
    state: RefCell<State>,
    published: RefCell<Option<State>>,
    events: RefCell<VecDeque<Event>>,
    task: Option<Arc<Task>>,
    // Retain validated immutable ELF so restart never needs a large contiguous read buffer.
    executable: Option<Vec<u8>>,
    dashboard_revision: u64,
    retry_at: u64,
    restart_count: u32,
}
impl DisplayProxy {
    pub fn new() -> Self {
        let size = super::virtio_gpu::VIRTIO_GPU
            .lock()
            .as_ref()
            .expect("GPU")
            .framebuffer_info();
        super::virtio_input::VIRTIO_INPUT
            .lock()
            .set_display_size(size.width, size.height);
        {
            let mut s = SERVICE.lock();
            s.width = size.width;
            s.height = size.height;
        }
        let mut proxy = Self {
            state: RefCell::new(State {
                booting: true,
                ..State::default()
            }),
            published: RefCell::new(None),
            events: RefCell::new(VecDeque::new()),
            task: None,
            executable: None,
            dashboard_revision: 0,
            retry_at: 0,
            restart_count: 0,
        };
        proxy.launch();
        proxy.flush();
        proxy
    }
    fn launch(&mut self) {
        self.retry_at = super::rng::monotonic_time_ns()
            .saturating_add(1_000_000_000u64.saturating_mul(1u64 << self.restart_count.min(5)));
        let loaded = if self.executable.is_none() {
            match read_executable("/system/ferrofluid") {
                Ok(image) => Some(image),
                Err(_) => {
                    crate::serial_println!("[ferrofluidd] Cannot read /system/ferrofluid");
                    return;
                }
            }
        } else {
            None
        };
        let image = self.executable.as_ref().or(loaded.as_ref()).unwrap();
        match Task::user_executable(image, 8192, 16000) {
            Ok(task) => {
                if let Some(image) = loaded {
                    self.executable = Some(image);
                }
                *task.execution_deadline.lock() =
                    Some(super::rng::monotonic_time_ns().saturating_add(30_000_000_000));
                let task = Arc::new(task);
                {
                    let mut s = SERVICE.lock();
                    s.pid = task.pid;
                    s.presented = false;
                }
                crate::serial_println!(
                    "[ferrofluidd] protected EL0 executable loaded pid={}",
                    task.pid
                );
                scheduler::add_task(task.clone());
                self.task = Some(task);
                self.restart_count = self.restart_count.saturating_add(1);
            }
            Err(error) => {
                crate::serial_println!("[ferrofluidd] Cannot load /system/ferrofluid: {:?}", error);
                #[cfg(target_os = "none")]
                {
                    let _interrupts=crate::sync::InterruptGuard::new();
                    let heap = crate::mm::heap::ALLOCATOR.lock();
                    crate::serial_println!(
                        "[ferrofluidd] Kernel heap free={} used={}",
                        heap.free(),
                        heap.used()
                    );
                }
            }
        }
    }
    pub fn poll(&mut self) {
        if self
            .task
            .as_ref()
            .is_some_and(|task| task.state() == TaskState::Zombie)
        {
            SERVICE.lock().events.clear();
            self.events.borrow_mut().clear();
            self.task = None;
            self.flush();
            crate::serial_println!("[ferrofluidd] Display process stopped; recovering host state");
        }
        if self.task.is_none() && super::rng::monotonic_time_ns() >= self.retry_at {
            self.launch();
        }
        let mut service = SERVICE.lock();
        while let Some(event) = service.events.pop_front() {
            match event {
                Event::Dashboard(ids) => {
                    self.state.borrow_mut().pinned = ids;
                    self.dashboard_revision += 1;
                }
                Event::Applied { revision, digest } => {
                    service.applied_pid = service.pid;
                    service.applied_revision = revision;
                    service.applied_digest = digest;
                }
                Event::Ready => {
                    crate::serial_println!("[ferrofluidd] EL0 Slint ready");
                }
                event => self.events.borrow_mut().push_back(event),
            }
        }
    }
    pub fn flush(&self) {
        let mut state = self.state.borrow_mut();
        // Bound presentation state before serialization, preserving current replies.
        if state.items.len() > 100 {
            let n = state.items.len() - 100;
            state.items.drain(..n);
        }
        if state.transcript.len() > 32768 {
            let mut start = state.transcript.len() - 32768;
            while !state.transcript.is_char_boundary(start) {
                start += 1;
            }
            state.transcript.drain(..start);
        }
        if self.published.borrow().as_ref() == Some(&*state) {
            return;
        }
        state.revision = state.revision.wrapping_add(1);
        match serde_json::to_vec(&*state) {
            Ok(bytes) if bytes.len() <= display::MAX_STATE => {
                SERVICE.lock().state = bytes;
                *self.published.borrow_mut() = Some(state.clone());
            }
            _ => {
                crate::serial_println!("[ferrofluidd] Presentation state exceeded bounded IPC; trimming oldest items/tiles");
                state.items.clear();
                state.tiles.clear();
                trim_text(&mut state.partial, 32768);
                trim_text(&mut state.model_status, 4096);
                trim_text(&mut state.model, 256);
                loop {
                    if let Ok(bytes) = serde_json::to_vec(&*state) {
                        if bytes.len() <= display::MAX_STATE {
                            SERVICE.lock().state = bytes;
                            *self.published.borrow_mut() = Some(state.clone());
                            break;
                        }
                    }
                    if state.available.pop().is_none() {
                        state.pinned.clear();
                        break;
                    }
                }
            }
        }
    }
    #[cfg(feature = "process-runtime-selftest")]
    pub fn acceptance(&mut self) {
        let deadline = super::rng::monotonic_time_ns() + 15_000_000_000;
        while !SERVICE.lock().presented {
            assert!(
                super::rng::monotonic_time_ns() < deadline,
                "EL0 display first frame timeout"
            );
            scheduler::sleep_until(super::rng::monotonic_time_ns() + 16_666_667);
            self.poll();
        }
        let pid = self.task.as_ref().unwrap().pid;
        self.state
            .borrow_mut()
            .transcript
            .push_str("\nDisplay recovery verification");
        if self.state.borrow().pinned.is_empty() {
            self.state
                .borrow_mut()
                .pinned
                .push("process-recovery-probe".into());
        }
        let snapshot = self.state.borrow().clone();
        self.state.borrow_mut().fault_probe = true;
        self.flush();
        while self
            .task
            .as_ref()
            .is_some_and(|t| t.pid == pid && t.state() != TaskState::Zombie)
        {
            assert!(
                super::rng::monotonic_time_ns() < deadline,
                "display fault timeout"
            );
            scheduler::sleep_until(super::rng::monotonic_time_ns() + 16_666_667);
        }
        *self.state.borrow_mut() = snapshot.clone();
        self.flush();
        self.poll();
        while self.task.as_ref().is_none_or(|t| t.pid == pid) || !SERVICE.lock().presented {
            assert!(
                super::rng::monotonic_time_ns() < deadline,
                "display restart timeout"
            );
            scheduler::sleep_until(super::rng::monotonic_time_ns() + 16_666_667);
            self.poll();
        }
        let new_pid = self.task.as_ref().unwrap().pid;
        let expected = display::state_digest(&snapshot.transcript, &snapshot.pinned);
        loop {
            let applied = {
                let s = SERVICE.lock();
                s.applied_pid == new_pid && s.applied_digest == expected
            };
            if applied {
                break;
            }
            assert!(
                super::rng::monotonic_time_ns() < deadline,
                "EL0 restored-state ACK timeout"
            );
            scheduler::sleep_until(super::rng::monotonic_time_ns() + 16_666_667);
            self.poll();
        }
        assert_eq!(self.state.borrow().transcript, snapshot.transcript);
        assert_eq!(self.state.borrow().pinned, snapshot.pinned);
        crate::serial_println!(
            "DISPLAY-PROCESS: real EL0 fault, restart, conversation/dashboard restoration PASS"
        );
    }
    pub fn request_restart(&mut self) {
        if let Some(task) = self.task.as_ref() {
            task.set_exit_code(-125);
        }
        self.retry_at = super::rng::monotonic_time_ns();
    }
    pub fn set_booting(&self, v: bool) {
        self.state.borrow_mut().booting = v;
    }
    pub fn set_reply_pending(&self, v: bool) {
        self.state.borrow_mut().pending = v;
    }
    pub fn set_active_model(&self, v: &str) {
        self.state.borrow_mut().model = v.into();
    }
    pub fn set_model_selection_status(&self, v: &str) {
        self.state.borrow_mut().model_status = v.into();
    }
    pub fn set_transcript(&self, v: &str) {
        self.state.borrow_mut().transcript = v.into();
    }
    pub fn set_chat_items(&self, v: &[ChatItem]) {
        let mut s = self.state.borrow_mut();
        s.items = v.to_vec();
        s.partial.clear();
    }
    pub fn set_available_tiles(&self, v: &[AppTile]) {
        self.state.borrow_mut().available = v.to_vec();
    }
    pub fn set_tiles(&self, v: &[AppTile]) {
        self.state.borrow_mut().tiles = v.to_vec();
    }
    pub fn restore_dashboard(&self, v: &[String]) {
        self.state.borrow_mut().pinned = v.to_vec();
    }
    pub fn dashboard_state(&self) -> Dashboard {
        Dashboard {
            pinned: self.state.borrow().pinned.clone(),
            revision: self.dashboard_revision,
        }
    }
    pub fn update_streaming_reply(&self, _: &str, v: &str) {
        self.state.borrow_mut().partial = v.into();
    }
    pub fn update_streaming_chat_item(&self, v: &str) {
        self.state.borrow_mut().partial = v.into();
    }
    fn take(&self, p: impl Fn(&Event) -> bool) -> Option<Event> {
        let mut q = self.events.borrow_mut();
        let i = q.iter().position(p)?;
        q.remove(i)
    }
    pub fn take_submission(&self) -> Option<String> {
        match self.take(|e| matches!(e, Event::Submit(_))) {
            Some(Event::Submit(s)) => Some(s),
            _ => None,
        }
    }
    pub fn take_model_cycle(&self) -> bool {
        self.take(|e| matches!(e, Event::CycleModel)).is_some()
    }
    pub fn take_voice_toggle(&self) -> bool {
        self.take(|e| matches!(e, Event::VoiceToggle)).is_some()
    }
    pub fn take_voice_mute(&self) -> bool {
        self.take(|e| matches!(e, Event::VoiceMute)).is_some()
    }
    pub fn set_voice(&self, status: alloyos_mcp::display::VoiceState, caption: &str, error: &str) {
        let mut state = self.state.borrow_mut();
        state.voice_state = status;
        state.voice_caption = caption.into();
        state.voice_error = error.into();
        trim_text(&mut state.voice_caption, 8192);
        trim_text(&mut state.voice_error, 512);
    }
    pub fn set_voice_level(&self, level: u16) {
        self.state.borrow_mut().voice_level = level.min(1000);
    }
    pub fn set_voice_diagnostics(&self, text: &str) {
        let mut state = self.state.borrow_mut();
        state.voice_diagnostics = text.into();
        trim_text(&mut state.voice_diagnostics, 256);
    }
    pub fn take_tile_event(&self) -> Option<TileEvent> {
        match self.take(|e| matches!(e, Event::Tile(_))) {
            Some(Event::Tile(e)) => Some(e),
            _ => None,
        }
    }
    pub fn handle_input(&mut self, _: alloyos_proto::InputEvent) {} // Input is consumed only by EL0.
}
fn trim_text(text: &mut String, maximum: usize) {
    if text.len() > maximum {
        let mut end = maximum;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text.truncate(end);
    }
}
pub fn read_executable(path: &str) -> Result<Vec<u8>, ()> {
    let vfs = crate::fs::vfs::VFS.lock();
    let vfs = vfs.as_ref().ok_or(())?;
    let inode = vfs.lookup(path).map_err(|_| ())?;
    if inode.size == 0 || inode.size > 64 * 1024 * 1024 {
        return Err(());
    }
    let mut handle = crate::fs::vfs::FileHandle {
        ino: inode.ino,
        offset: 0,
        kind: inode.kind,
        path: path.into(),
    };
    let mut bytes = Vec::new();
    bytes.try_reserve_exact(inode.size).map_err(|_| ())?;
    bytes.resize(inode.size, 0);
    let n = vfs.read(&mut handle, &mut bytes).map_err(|_| ())?;
    if n != bytes.len() {
        return Err(());
    }
    Ok(bytes)
}
/// All caller pointers are copied through private backing before driver use.
pub fn syscall(
    number: u64,
    a1: u64,
    a2: u64,
    a3: u64,
    task: &Task,
    space: &mut crate::task::app_process::AddressSpace,
) -> i64 {
    let mut service = SERVICE.lock();
    if task.pid != service.pid {
        return -1;
    }
    match number {
        display::DISPLAY_INFO => {
            let mut b = [0u8; 8];
            b[..4].copy_from_slice(&service.width.to_le_bytes());
            b[4..].copy_from_slice(&service.height.to_le_bytes());
            space.copy_out(a1, &b).map(|_| 0).unwrap_or(-14)
        }
        display::DISPLAY_INPUT => {
            if space.validate_range(a1, 16, true).is_err() {
                return -14;
            }
            let Some(e) = super::virtio_input::VIRTIO_INPUT.lock().poll_ui_event() else {
                return -11;
            };
            let mut b = [0u8; 16];
            b[0] = e.kind as u8;
            b[2..4].copy_from_slice(&e.code.to_le_bytes());
            b[4..8].copy_from_slice(&e.x.to_le_bytes());
            b[8..12].copy_from_slice(&e.y.to_le_bytes());
            space.copy_out(a1, &b).map(|_| 0).unwrap_or(-14)
        }
        display::DISPLAY_STATE => {
            *task.execution_deadline.lock() =
                Some(super::rng::monotonic_time_ns().saturating_add(5_000_000_000));
            if a2 < service.state.len() as u64 {
                return -90;
            }
            space
                .copy_out(a1, &service.state)
                .map(|_| service.state.len() as i64)
                .unwrap_or(-14)
        }
        display::DISPLAY_EVENT => {
            if a2 > 65536 {
                return -90;
            }
            if service.events.len() >= 64 {
                return -11;
            }
            let mut b = alloc::vec![0;a2 as usize];
            if space.copy_in(a1, &mut b).is_err() {
                return -14;
            }
            if core::str::from_utf8(&b).is_err()
                || alloyos_js::process::validate_wire(core::str::from_utf8(&b).unwrap()).is_err()
            {
                return -22;
            }
            let Ok(e) = serde_json::from_slice::<Event>(&b) else {
                return -22;
            };
            match &e {
                Event::Submit(s) if s.len() > 16384 => return -90,
                Event::Dashboard(ids) if ids.len() > 64 || ids.iter().any(|id| id.len() > 128) => {
                    return -90
                }
                _ => {}
            }
            service.events.push_back(e);
            0
        }
        display::DISPLAY_PRESENT => {
            // Only the fixed display extent can be presented; no caller hardware addresses.
            let width = a2 as u32;
            let height = a3 as u32;
            if a2 != service.width as u64 || a3 != service.height as u64 {
                return -22;
            }
            let Some(count) = (width as usize).checked_mul(height as usize) else {
                return -22;
            };
            let Some(length) = count.checked_mul(4) else {
                return -22;
            };
            if length > 32 * 1024 * 1024 || space.validate_range(a1, length, false).is_err() {
                return -14;
            }
            service.frame.resize(count, 0);
            let bytes = unsafe {
                core::slice::from_raw_parts_mut(service.frame.as_mut_ptr() as *mut u8, length)
            };
            if space.copy_in(a1, bytes).is_err() {
                return -14;
            }
            if let Some(gpu) = super::virtio_gpu::VIRTIO_GPU.lock().as_mut() {
                gpu.present_scaled_argb(&service.frame, width, height, None);
            }
            if !service.presented {
                service.presented = true;
                crate::serial_println!("[ferrofluidd] EL0 frame presented {}x{}", width, height);
            }
            0
        }
        _ => -38,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_role_buffers_dimensions_nesting_and_backpressure_are_bounded() {
        let task = Task::user_process(&[0; 4], 20, 21).unwrap();
        let mut space = task.address_space.lock().take().unwrap();
        let prior = {
            let mut s = SERVICE.lock();
            let prior = s.pid;
            s.pid = task.pid;
            s.width = 1206;
            s.height = 2622;
            s.events.clear();
            prior
        };
        assert_eq!(
            syscall(display::DISPLAY_INFO, 0, 0, 0, &task, &mut space),
            -14
        );
        assert_eq!(
            syscall(
                display::DISPLAY_PRESENT,
                0,
                (1u64 << 32) + 1206,
                2622,
                &task,
                &mut space
            ),
            -22
        );
        let address = crate::task::app_process::USER_BASE + 4096;
        let nested = alloc::format!("{}0{}", "[".repeat(40), "]".repeat(40));
        space.copy_out(address, nested.as_bytes()).unwrap();
        assert_eq!(
            syscall(
                display::DISPLAY_EVENT,
                address,
                nested.len() as u64,
                0,
                &task,
                &mut space
            ),
            -22
        );
        let event = serde_json::to_vec(&Event::Ready).unwrap();
        space.copy_out(address, &event).unwrap();
        for _ in 0..64 {
            assert_eq!(
                syscall(
                    display::DISPLAY_EVENT,
                    address,
                    event.len() as u64,
                    0,
                    &task,
                    &mut space
                ),
                0
            );
        }
        assert_eq!(
            syscall(
                display::DISPLAY_EVENT,
                address,
                event.len() as u64,
                0,
                &task,
                &mut space
            ),
            -11
        );
        {
            let mut s = SERVICE.lock();
            s.pid = prior;
            s.events.clear();
        }
        assert_eq!(
            syscall(display::DISPLAY_INFO, address, 0, 0, &task, &mut space),
            -1
        );
    }
}
