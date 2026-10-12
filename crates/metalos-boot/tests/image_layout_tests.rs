use metalos_boot::create_boot_config;

#[test]
fn boot_config_layout_stub() {
    let config = create_boot_config();
    assert!(config.physical_memory.init);
    assert!(config.kernel_stack_size > 0);
    assert_eq!(config.kernel_stack_size, 128 * 1024);
}
