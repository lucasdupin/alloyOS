#[cfg(any(test, feature = "host-tests"))]
pub mod tests {
    use alloc::vec;
    use alloc::vec::Vec;
    use core::fmt::Write;
    use metalos_proto::{
        DirtyRect, InputEventKind, SocketAddrV4, SyscallNo, AF_INET, SOCK_STREAM,
    };

    use crate::drivers::rng;
    use crate::drivers::virtio_gpu::VirtIoGpuDevice;
    use crate::drivers::virtio_input::{
        VirtIoInputDevice, VirtioInputRawEvent, ABS_X, ABS_Y, BTN_MOUSE_LEFT, EV_ABS, EV_KEY,
    };
    use crate::drivers::virtio_net::VirtIoNetDevice;
    use crate::fs::devfs::{DevFS, INO_DEV_URANDOM};
    use crate::fs::vfs::FileSystem;
    use crate::net::{
        rfc1071_checksum, ArpPacket, DhcpClient, DnsResolver, EthernetHeader, IcmpPacket,
        Ipv4Header, TcpSegment, UdpPacket, ETHERTYPE_ARP, ETHERTYPE_IPV4, IPPROTO_ICMP,
        IPPROTO_TCP, IPPROTO_UDP, TCP_FLAG_ACK, TCP_FLAG_SYN,
    };
    use crate::syscall::dispatch::dispatch_syscall;
    use crate::syscall::handlers::TimeSpec;

    pub struct MockWriter {
        pub buffer: Vec<u8>,
    }

    impl Write for MockWriter {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            self.buffer.extend_from_slice(s.as_bytes());
            Ok(())
        }
    }

    #[test]
    fn serial_format_output() {
        let mut writer = MockWriter { buffer: Vec::new() };
        write!(writer, "metalOS v{}", "0.1.0").unwrap();
        let res = core::str::from_utf8(&writer.buffer).unwrap();
        assert_eq!(res, "metalOS v0.1.0");
    }

    #[test]
    fn serial_format_numbers() {
        let mut writer = MockWriter { buffer: Vec::new() };
        write!(writer, "port: {:#x}, dec: {}", 0x3F8, 42).unwrap();
        let res = core::str::from_utf8(&writer.buffer).unwrap();
        assert_eq!(res, "port: 0x3f8, dec: 42");
    }

    #[test]
    fn dev_urandom_and_getrandom_produce_entropy() {
        let devfs = DevFS::new();
        let ino = devfs.lookup("/dev/urandom").expect("/dev/urandom must exist");
        assert_eq!(ino.ino, INO_DEV_URANDOM);

        let mut buf1 = [0u8; 32];
        let mut buf2 = [0u8; 32];
        let n1 = devfs.read(INO_DEV_URANDOM, 0, &mut buf1).unwrap();
        let n2 = devfs.read(INO_DEV_URANDOM, 0, &mut buf2).unwrap();
        assert_eq!(n1, 32);
        assert_eq!(n2, 32);
        assert_ne!(buf1, [0u8; 32]);
        assert_ne!(buf1, buf2, "Consecutive CSPRNG reads must not be identical");

        let mut sys_buf = [0u8; 16];
        let ret = dispatch_syscall(
            SyscallNo::GetRandom as u64,
            sys_buf.as_mut_ptr() as u64,
            sys_buf.len() as u64,
            0,
            0,
            0,
            0,
        );
        assert_eq!(ret, 16);
        assert_ne!(sys_buf, [0u8; 16]);

        let mut ts = TimeSpec {
            tv_sec: 0,
            tv_nsec: 0,
        };
        let clk_ret = dispatch_syscall(
            SyscallNo::ClockGettime as u64,
            1,
            &mut ts as *mut TimeSpec as u64,
            0,
            0,
            0,
            0,
        );
        assert_eq!(clk_ret, 0);
        assert!(ts.tv_sec > 0 || ts.tv_nsec > 0);
        let _ = rng::next_u64();
    }

    #[test]
    fn virtio_gpu_dirty_rect_neon_blit() {
        let mut gpu = VirtIoGpuDevice::new(64, 64);
        let mut src = vec![0u8; 64 * 64 * 4];
        // Paint pixel (10, 10) with 0xFF336699 (LE: 0x99, 0x66, 0x33, 0xFF)
        let idx = (10 * 64 + 10) * 4;
        src[idx..idx + 4].copy_from_slice(&0xFF33_6699u32.to_le_bytes());

        let flushes = gpu
            .blit_dirty_rect(DirtyRect::new(8, 8, 16, 16), &src)
            .unwrap();
        assert_eq!(flushes, 1);
        assert_eq!(gpu.read_pixel(10, 10), Some(0xFF33_6699));
        assert_eq!(gpu.read_pixel(0, 0), Some(0));
    }

    #[test]
    fn virtio_input_translates_key_and_tablet_events() {
        let mut input = VirtIoInputDevice::new();
        let ev_abs_x = input
            .push_raw_event(VirtioInputRawEvent {
                type_: EV_ABS,
                code: ABS_X,
                value: 16384, // 50% of 32768 -> x = 640
            })
            .unwrap();
        assert_eq!(ev_abs_x.kind, InputEventKind::PointerMove);
        assert_eq!(ev_abs_x.x, 640);

        let ev_abs_y = input
            .push_raw_event(VirtioInputRawEvent {
                type_: EV_ABS,
                code: ABS_Y,
                value: 8192, // 25% of 32768 -> y = 200
            })
            .unwrap();
        assert_eq!(ev_abs_y.y, 200);

        let click = input
            .push_raw_event(VirtioInputRawEvent {
                type_: EV_KEY,
                code: BTN_MOUSE_LEFT,
                value: 1,
            })
            .unwrap();
        assert_eq!(click.kind, InputEventKind::PointerButtonDown);
        assert_eq!(click.x, 640);
        assert_eq!(click.y, 200);
    }

    #[test]
    fn portrait_input_uses_scanout_dimensions_and_discards_shell_bytes() {
        let mut input = VirtIoInputDevice::new();
        input.set_display_size(1206, 2622);
        input.push_raw_event(VirtioInputRawEvent { type_: EV_ABS, code: ABS_X, value: 16384 });
        input.push_raw_event(VirtioInputRawEvent { type_: EV_ABS, code: ABS_Y, value: 16384 });
        assert_eq!(input.poll_ui_event().unwrap().x, 603);
        assert_eq!(input.poll_ui_event().unwrap().y, 1311);
        input.push_raw_event(VirtioInputRawEvent { type_: EV_KEY, code: 32, value: 1 });
        assert_eq!(input.poll_ui_event().unwrap().code, 32);
        assert!(input.poll_ascii_byte().is_none());
    }

    #[test]
    fn tcp_ip_headers_checksums_and_dns_roundtrip() {
        let mut net_dev = VirtIoNetDevice::new_loopback();
        let eth = EthernetHeader {
            dst_mac: [0xff; 6],
            src_mac: net_dev.mac,
            ethertype: ETHERTYPE_ARP,
        };
        let arp = ArpPacket {
            oper: ArpPacket::OP_REQUEST,
            sha: net_dev.mac,
            spa: [10, 0, 2, 15],
            tha: [0; 6],
            tpa: [10, 0, 2, 2],
        };
        let mut frame = Vec::new();
        frame.extend_from_slice(&eth.to_bytes());
        frame.extend_from_slice(&arp.to_bytes());
        net_dev.transmit_frame(&frame).unwrap();

        let rx = net_dev.poll_receive().expect("Loopback frame must arrive");
        let (parsed_eth, payload) = EthernetHeader::parse(&rx).unwrap();
        assert_eq!(parsed_eth.ethertype, ETHERTYPE_ARP);
        let parsed_arp = ArpPacket::parse(payload).unwrap();
        assert_eq!(parsed_arp.tpa, [10, 0, 2, 2]);

        // IPv4 + ICMP Echo checksum verification
        let icmp = IcmpPacket {
            icmp_type: IcmpPacket::TYPE_ECHO_REQUEST,
            code: 0,
            identifier: 0x1234,
            sequence: 1,
            payload: b"metalOS-ping".to_vec(),
        };
        let icmp_raw = icmp.to_bytes();
        let ip_hdr = Ipv4Header::new([10, 0, 2, 15], [10, 0, 2, 2], IPPROTO_ICMP, icmp_raw.len());
        assert!(ip_hdr.verify_checksum());
        let mut ip_pkt = ip_hdr.to_bytes().to_vec();
        ip_pkt.extend_from_slice(&icmp_raw);
        let (parsed_ip, ip_body) = Ipv4Header::parse(&ip_pkt).unwrap();
        assert_eq!(parsed_ip.protocol, IPPROTO_ICMP);
        let parsed_icmp = IcmpPacket::parse(ip_body).unwrap();
        assert_eq!(parsed_icmp.payload, b"metalOS-ping");

        // UDP + DNS A-record query & response
        let query = DnsResolver::build_a_query(0xabcd, "api.anthropic.com");
        let udp = UdpPacket {
            src_port: 53000,
            dst_port: 53,
            payload: query.clone(),
        };
        let udp_raw = udp.to_bytes([10, 0, 2, 15], [10, 0, 2, 3]);
        assert_eq!(rfc1071_checksum(&ip_hdr.to_bytes()), 0);
        let parsed_udp = UdpPacket::parse(&udp_raw).unwrap();
        assert_eq!(parsed_udp.dst_port, 53);

        // Construct synthetic DNS response with pointer compression (0xC00C) and A record 160.79.104.10
        let mut resp = query;
        resp[2] = 0x81;
        resp[3] = 0x80;
        resp[6] = 0x00;
        resp[7] = 0x01; // ANCOUNT = 1
        resp.extend_from_slice(&[0xC0, 0x0C]); // Name pointer
        resp.extend_from_slice(&1u16.to_be_bytes()); // TYPE A
        resp.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN
        resp.extend_from_slice(&60u32.to_be_bytes()); // TTL 60
        resp.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH 4
        resp.extend_from_slice(&[160, 79, 104, 10]);
        assert_eq!(DnsResolver::parse_a_response(&resp), Some([160, 79, 104, 10]));

        // DHCP & TCP segment roundtrip
        let mut dhcp = DhcpClient::new();
        let disc = dhcp.build_discover(net_dev.mac);
        assert_eq!(&disc[236..240], &[99, 130, 83, 99]);

        let tcp = TcpSegment {
            src_port: 49152,
            dst_port: 443,
            seq_num: 100,
            ack_num: 200,
            flags: TCP_FLAG_SYN | TCP_FLAG_ACK,
            window: 65535,
            payload: b"TLS".to_vec(),
        };
        let tcp_raw = tcp.to_bytes([10, 0, 2, 15], [142, 250, 190, 74]);
        let parsed_tcp = TcpSegment::parse(&tcp_raw).unwrap();
        assert_eq!(parsed_tcp.dst_port, 443);
        assert_eq!(parsed_tcp.payload, b"TLS");
        let _ = (ETHERTYPE_IPV4, IPPROTO_TCP, IPPROTO_UDP);
    }

    #[test]
    fn socket_syscalls_connect_send_and_recv_loopback() {
        let fd = dispatch_syscall(SyscallNo::Socket as u64, AF_INET, SOCK_STREAM, 0, 0, 0, 0);
        assert!(fd >= 64, "Socket FD must be allocated in socket range");

        let addr = SocketAddrV4::new([127, 0, 0, 1], 8080);
        let rc = dispatch_syscall(
            SyscallNo::Connect as u64,
            fd as u64,
            &addr as *const SocketAddrV4 as u64,
            core::mem::size_of::<SocketAddrV4>() as u64,
            0,
            0,
            0,
        );
        assert_eq!(rc, 0);

        let msg = b"PING";
        let sent = dispatch_syscall(
            SyscallNo::Send as u64,
            fd as u64,
            msg.as_ptr() as u64,
            msg.len() as u64,
            0,
            0,
            0,
        );
        assert_eq!(sent, 4);

        let mut rx = [0u8; 4];
        let recvd = dispatch_syscall(
            SyscallNo::Recv as u64,
            fd as u64,
            rx.as_mut_ptr() as u64,
            rx.len() as u64,
            0,
            0,
            0,
        );
        assert_eq!(recvd, 4);
        assert_eq!(&rx, b"PONG");

        let closed = dispatch_syscall(SyscallNo::Close as u64, fd as u64, 0, 0, 0, 0, 0);
        assert_eq!(closed, 0);
    }
}
