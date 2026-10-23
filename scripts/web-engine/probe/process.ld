ENTRY(_start)
PROVIDE(PROBE_TLS_SHIFT = 0);
PHDRS {
 text PT_LOAD FLAGS(5);
 rodata PT_LOAD FLAGS(4);
 data PT_LOAD FLAGS(6);
 tls PT_TLS FLAGS(4);
}
SECTIONS {
 . = 0x1000000000;
 .text : ALIGN(4096) { KEEP(*(.text._start)) *(.text .text.*) } :text
 . = ALIGN(4096);
 .rodata : { *(.rodata .rodata.*) } :rodata
 . = ALIGN(4096);
 .data : { *(.data .data.*) *(.got .got.*) } :data
 .init_array : ALIGN(8) {
  __init_array_start = .;
  KEEP(*(SORT_BY_INIT_PRIORITY(.init_array.*))) KEEP(*(.init_array))
  __init_array_end = .;
 } :data
 .bss : { *(.bss .bss.*) *(COMMON) . += PROBE_TLS_SHIFT; } :data
 .tdata : ALIGN(64) { __tdata_start = .; *(.tdata .tdata.*) __tdata_end = .; } :data :tls
 .tbss : { __tbss_start = .; *(.tbss .tbss.*) __tbss_end = .; } :data :tls
 /DISCARD/ : { *(.eh_frame*) *(.comment*) }
}
