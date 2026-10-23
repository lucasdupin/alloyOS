//! Protected EL0 browser service IPC, form input roundtrip, crash containment, and restart recovery.
use super::{
    app_process::USER_BASE,
    ipc::{Endpoint, Port},
    pcb::Task,
    scheduler,
};
use alloc::{sync::Arc, vec::Vec};
use alloyos_proto::{
    web::{
        trusted_origin_from_packet, Error, FrameInfo, FrameTransfer, Opcode, Packet, View,
        MAX_PACKET_BYTES,
    },
    DirtyRect,
};

core::arch::global_asm!(
    ".section .text.web_service_probe,\"ax\"",
    ".global web_service_probe_start",
    ".global web_service_probe_end",
    "web_service_probe_start:",
    "    mov x19, #0x1000000000",
    "    add x20, x19, #0x1000",
    "    add x21, x19, #0x2000",
    // Browser service must be denied display-server syscalls (130..=134).
    "    mov x0, x21",
    "    mov x1, #0",
    "    mov x2, #0",
    "    mov x8, #130",
    "    svc #0",
    "    cmn x0, #1",
    "    b.ne 9f",
    "    mov x0, x21",
    "    mov x1, #64",
    "    mov x2, #64",
    "    mov x8, #134",
    "    svc #0",
    "    cmn x0, #1",
    "    b.ne 9f",
    // Browser service must be denied shared host descriptor syscalls (Write = 1).
    "    mov x0, #1",
    "    mov x1, x21",
    "    mov x2, #4",
    "    mov x8, #1",
    "    svc #0",
    "    cmn x0, #1",
    "    b.ne 9f",
    // Port 0 is receive-only; IPC_SEND (400) on port 0 must return EPERM (-1).
    "    mov x0, #0",
    "    mov x1, x21",
    "    mov x2, #4",
    "    mov x8, #400",
    "    svc #0",
    "    cmn x0, #1",
    "    b.ne 9f",
    // Port 1 is send-only; IPC_RECEIVE (401) on port 1 must return EPERM (-1).
    "    mov x0, #1",
    "    mov x1, x21",
    "    mov x2, #512",
    "    mov x8, #401",
    "    svc #0",
    "    cmn x0, #1",
    "    b.ne 9f",
    // Drain expected inbound command packets from port 0.
    "    ldr x22, [x20, #0]",
    "    ldr w24, [x20, #16]",
    "1:",
    "    cbz x22, 3f",
    "2:",
    "    mov x0, #0",
    "    mov x1, x21",
    "    mov x2, #512",
    "    mov x8, #401",
    "    svc #0",
    "    cmn x0, #11",
    "    b.ne 4f",
    "    mov x8, #24",
    "    svc #0",
    "    b 2b",
    "4:",
    // Minimum valid message is 8-byte sender PID + 28-byte web::Packet header.
    "    cmp x0, #36",
    "    b.lt 9f",
    // Sender PID at [x21 + 0] must be kernel supervisor (1).
    "    ldr x1, [x21, #0]",
    "    cmp x1, #1",
    "    b.ne 9f",
    // Packet version at [x21 + 8] must be web::VERSION (1).
    "    ldrh w1, [x21, #8]",
    "    cmp w1, #1",
    "    b.ne 9f",
    // Packet view.id at [x21 + 12] must be 1, view.generation at [x21 + 20] must match.
    "    ldr w1, [x21, #12]",
    "    cmp w1, #1",
    "    b.ne 9f",
    "    ldr w1, [x21, #20]",
    "    cmp w1, w24",
    "    b.ne 9f",
    "    sub x22, x22, #1",
    "    b 1b",
    "3:",
    // Queue must now report EAGAIN (-11).
    "    mov x0, #0",
    "    mov x1, x21",
    "    mov x2, #512",
    "    mov x8, #401",
    "    svc #0",
    "    cmn x0, #11",
    "    b.ne 9f",
    // Send pre-encoded outbound response packets over port 1.
    "    ldr x22, [x20, #8]",
    "    add x23, x20, #32",
    "5:",
    "    cbz x22, 6f",
    "    ldr x1, [x23, #0]",
    "    ldr x2, [x23, #8]",
    "    add x1, x20, x1",
    "    mov x0, #1",
    "    mov x8, #400",
    "    svc #0",
    "    cmp x0, x2",
    "    b.ne 9f",
    "    add x23, x23, #16",
    "    sub x22, x22, #1",
    "    b 5b",
    "6:",
    // If fault_after_send flag is non-zero, trigger an intentional EL0 data abort.
    "    ldr x0, [x20, #24]",
    "    cbz x0, 7f",
    "    mov x0, #0x40000000",
    "    ldr x0, [x0]",
    "7:",
    "    mov x0, #0",
    "    mov x8, #60",
    "    svc #0",
    "    b .",
    "9:",
    "    mov x0, #97",
    "    mov x8, #60",
    "    svc #0",
    "    b .",
    "web_service_probe_end:",
);

extern "C" {
    static web_service_probe_start: u8;
    static web_service_probe_end: u8;
}

const URL_BYTES: &[u8] = b"https://local.alloyos.test/local-form.html";
const TITLE_BYTES: &[u8] = b"alloyOS native web acceptance";
const FRAME_PIXELS: [u8; 16] = [
    0x10, 0x14, 0x18, 0xff, 0x89, 0xcf, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x10, 0x14, 0x18, 0xff,
];

fn encode_packet(opcode: Opcode, view: View, sequence: u32, payload: &[u8]) -> Vec<u8> {
    let mut buf = [0u8; MAX_PACKET_BYTES];
    let pkt = Packet {
        opcode,
        view,
        sequence,
        payload,
    };
    let len = pkt.encode(&mut buf).expect("encode valid web packet");
    buf[..len].to_vec()
}

fn enqueue_host_commands(to_browser: &Arc<Endpoint>, view: View) {
    let host_port = Port {
        endpoint: to_browser.clone(),
        send: true,
        receive: false,
    };
    let mut viewport = [0u8; 12];
    viewport[0..4].copy_from_slice(&64u32.to_le_bytes());
    viewport[4..8].copy_from_slice(&64u32.to_le_bytes());
    viewport[8..12].copy_from_slice(&1000u32.to_le_bytes());

    let mut scroll = [0u8; 8];
    scroll[0..4].copy_from_slice(&0i32.to_le_bytes());
    scroll[4..8].copy_from_slice(&(-120i32).to_le_bytes());

    let mut key = [0u8; 4];
    key[0..2].copy_from_slice(&13u16.to_le_bytes());
    key[2] = 1;
    key[3] = 0;

    let commands = [
        encode_packet(Opcode::Create, view, 1, &viewport),
        encode_packet(Opcode::Resize, view, 2, &viewport),
        encode_packet(Opcode::Navigate, view, 3, URL_BYTES),
        encode_packet(Opcode::Scroll, view, 4, &scroll),
        encode_packet(Opcode::Focus, view, 5, &[1]),
        encode_packet(Opcode::Composition, view, 6, b"Ada"),
        encode_packet(Opcode::CommitText, view, 7, b"Ada"),
        encode_packet(Opcode::Key, view, 8, &key),
    ];
    for cmd in &commands {
        assert_eq!(host_port.send(1, cmd), cmd.len() as i64);
    }
}

fn spawn_browser_service(
    view: View,
    fault_after_send: bool,
    to_browser: Arc<Endpoint>,
    from_browser: Arc<Endpoint>,
) -> Arc<Task> {
    let image = unsafe {
        let start = core::ptr::addr_of!(web_service_probe_start);
        let len = core::ptr::addr_of!(web_service_probe_end) as usize - start as usize;
        core::slice::from_raw_parts(start, len)
    };
    let task = Arc::new(Task::user_process(image, 4, 8).expect("spawn EL0 browser service"));

    let mut begin_frame = [0u8; 28];
    for (idx, val) in [2u32, 2, 8, 0, 0, 2, 2].into_iter().enumerate() {
        begin_frame[idx * 4..idx * 4 + 4].copy_from_slice(&val.to_le_bytes());
    }
    let mut chunk_payload = [0u8; 20];
    chunk_payload[0..4].copy_from_slice(&0u32.to_le_bytes());
    chunk_payload[4..20].copy_from_slice(&FRAME_PIXELS);

    let replies = [
        encode_packet(Opcode::Url, view, 1, URL_BYTES),
        encode_packet(Opcode::Title, view, 2, TITLE_BYTES),
        encode_packet(Opcode::Loading, view, 3, &[0]),
        encode_packet(Opcode::Editing, view, 4, &[1, 1]),
        encode_packet(Opcode::BeginFrame, view, 5, &begin_frame),
        encode_packet(Opcode::FrameChunk, view, 5, &chunk_payload),
        encode_packet(Opcode::EndFrame, view, 5, &[]),
    ];

    let mut page_data = Vec::with_capacity(1024);
    page_data.extend_from_slice(&8u64.to_le_bytes()); // expected_in_count
    page_data.extend_from_slice(&(replies.len() as u64).to_le_bytes()); // out_count
    page_data.extend_from_slice(&view.generation.to_le_bytes());
    page_data.extend_from_slice(&(u64::from(fault_after_send)).to_le_bytes());

    let mut payload_offset = 32 + replies.len() * 16;
    for reply in &replies {
        page_data.extend_from_slice(&(payload_offset as u64).to_le_bytes());
        page_data.extend_from_slice(&(reply.len() as u64).to_le_bytes());
        payload_offset += reply.len();
    }
    for reply in &replies {
        page_data.extend_from_slice(reply);
    }

    task.address_space
        .lock()
        .as_mut()
        .unwrap()
        .copy_out(USER_BASE + 0x1000, &page_data)
        .expect("populate browser service data page");

    let mut ports = task.ports.lock();
    ports.push(Port {
        endpoint: to_browser,
        send: false,
        receive: true,
    });
    ports.push(Port {
        endpoint: from_browser,
        send: true,
        receive: false,
    });
    drop(ports);

    task
}

fn verify_browser_replies(from_browser: &Arc<Endpoint>, expected_pid: u64, view: View) {
    let mut queue = from_browser.queue.lock();
    assert_eq!(queue.len(), 7, "expected 7 outbound browser packets");

    let url_msg = queue.pop_front().unwrap();
    assert_eq!(url_msg.sender, expected_pid);
    let url_pkt = Packet::decode(&url_msg.payload).unwrap();
    assert_eq!(url_pkt.opcode, Opcode::Url);
    let origin = trusted_origin_from_packet(&url_pkt, view).unwrap();
    assert_eq!(origin, "https://local.alloyos.test");
    assert_eq!(origin.scheme(), "https");
    assert_eq!(origin.host(), "local.alloyos.test");
    assert_eq!(origin.port(), None);
    assert!(!origin.is_about_blank());
    assert_eq!(
        trusted_origin_from_packet(
            &url_pkt,
            View {
                id: view.id,
                generation: view.generation + 1
            }
        ),
        Err(Error::StaleFrame)
    );

    let title_msg = queue.pop_front().unwrap();
    assert_eq!(title_msg.sender, expected_pid);
    let title_pkt = Packet::decode(&title_msg.payload).unwrap();
    assert_eq!(title_pkt.opcode, Opcode::Title);
    assert_eq!(title_pkt.payload, TITLE_BYTES);
    assert_eq!(
        trusted_origin_from_packet(&title_pkt, view),
        Err(Error::Opcode)
    );

    let loading_msg = queue.pop_front().unwrap();
    assert_eq!(loading_msg.sender, expected_pid);
    let loading_pkt = Packet::decode(&loading_msg.payload).unwrap();
    assert_eq!(loading_pkt.opcode, Opcode::Loading);
    assert_eq!(loading_pkt.payload, &[0]);

    let editing_msg = queue.pop_front().unwrap();
    assert_eq!(editing_msg.sender, expected_pid);
    let editing_pkt = Packet::decode(&editing_msg.payload).unwrap();
    assert_eq!(editing_pkt.opcode, Opcode::Editing);
    assert_eq!(editing_pkt.payload, &[1, 1]);

    let begin_msg = queue.pop_front().unwrap();
    let chunk_msg = queue.pop_front().unwrap();
    let end_msg = queue.pop_front().unwrap();
    for msg in [&begin_msg, &chunk_msg, &end_msg] {
        assert_eq!(msg.sender, expected_pid);
    }
    let begin_pkt = Packet::decode(&begin_msg.payload).unwrap();
    let chunk_pkt = Packet::decode(&chunk_msg.payload).unwrap();
    let end_pkt = Packet::decode(&end_msg.payload).unwrap();

    let info = FrameInfo::decode(begin_pkt.payload).unwrap();
    assert_eq!(info.width, 2);
    assert_eq!(info.height, 2);
    assert_eq!(info.stride, 8);
    assert_eq!(info.damage, DirtyRect::new(0, 0, 2, 2));

    let mut transfer = FrameTransfer::begin(&begin_pkt, view).unwrap();
    let mut staged = [0u8; 16];
    let (offset, bytes) = transfer.chunk(&chunk_pkt).unwrap();
    assert_eq!(offset, 0);
    staged[offset as usize..offset as usize + bytes.len()].copy_from_slice(bytes);
    transfer.finish(&end_pkt).unwrap();
    assert_eq!(staged, FRAME_PIXELS);
}

fn probe(words: &[u32]) -> Arc<Task> {
    let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    Arc::new(Task::user_process(&bytes, 2, 3).expect("prepare EL0 probe"))
}

pub fn run() {
    let view_gen1 = View {
        id: 1,
        generation: 1,
    };
    let to_browser_gen1 = Endpoint::new();
    let from_browser_gen1 = Endpoint::new();
    enqueue_host_commands(&to_browser_gen1, view_gen1);

    let browser_gen1 =
        spawn_browser_service(view_gen1, true, to_browser_gen1, from_browser_gen1.clone());

    // Untrusted app without browser endpoint grants must fail IPC_SEND (400) with EPERM (-1).
    let app_denied = probe(&[
        0xd2800000, // mov x0, #0
        0xd2c00201, // mov x1, #0x1000000000
        0xd2800082, // mov x2, #4
        0xd2803208, // mov x8, #400 (IPC_SEND)
        0xd4000001, // svc #0
        0xd2800788, // mov x8, #60 (Exit)
        0xd4000001, // svc #0
    ]);
    // Concurrent peer process must survive the browser service fault cleanly.
    let peer_app = probe(&[
        0xd2800000, // mov x0, #0
        0xd2800788, // mov x8, #60 (Exit)
        0xd4000001, // svc #0
    ]);

    for task in [&browser_gen1, &app_denied, &peer_app] {
        scheduler::add_task(task.clone());
    }
    scheduler::start_preemption();

    let deadline = crate::drivers::rng::monotonic_time_ns() + 5_000_000_000;
    let mut gen1_reaped = false;
    while crate::drivers::rng::monotonic_time_ns() < deadline {
        if [&browser_gen1, &app_denied, &peer_app]
            .iter()
            .all(|t| t.exit_code().is_some())
            && [&browser_gen1, &app_denied, &peer_app]
                .iter()
                .all(|t| t.address_space.lock().is_none() && t.ports.lock().is_empty())
        {
            gen1_reaped = true;
            break;
        }
        crate::arch::wfi();
    }
    assert!(gen1_reaped, "EL0 browser service gen1 timed out");
    assert_eq!(browser_gen1.exit_code(), Some(-11));
    assert_eq!(app_denied.exit_code(), Some(-1));
    assert_eq!(peer_app.exit_code(), Some(0));
    verify_browser_replies(&from_browser_gen1, browser_gen1.pid, view_gen1);

    // Restart browser service with incremented view generation and clean exit.
    let view_gen2 = View {
        id: 1,
        generation: 2,
    };
    let to_browser_gen2 = Endpoint::new();
    let from_browser_gen2 = Endpoint::new();
    enqueue_host_commands(&to_browser_gen2, view_gen2);

    let browser_gen2 =
        spawn_browser_service(view_gen2, false, to_browser_gen2, from_browser_gen2.clone());
    assert_ne!(browser_gen2.pid, browser_gen1.pid);
    scheduler::add_task(browser_gen2.clone());

    let mut gen2_reaped = false;
    while crate::drivers::rng::monotonic_time_ns() < deadline {
        if browser_gen2.exit_code().is_some()
            && browser_gen2.address_space.lock().is_none()
            && browser_gen2.ports.lock().is_empty()
        {
            gen2_reaped = true;
            break;
        }
        crate::arch::wfi();
    }
    assert!(gen2_reaped, "EL0 browser service gen2 recovery timed out");
    assert_eq!(browser_gen2.exit_code(), Some(0));
    verify_browser_replies(&from_browser_gen2, browser_gen2.pid, view_gen2);

    crate::serial_println!(
        "[web-runtime] PASS: protected EL0 browser service IPC, local form input roundtrip, app denial, crash containment and restart recovery"
    );
}
