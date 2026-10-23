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
  .text : ALIGN(4096) {
    KEEP(*(.text._start))
    *(.text .text.*)
  } :text
  . = ALIGN(4096);
  .rodata : {
    *(.rodata .rodata.*)
  } :rodata
  . = ALIGN(4096);
  .data : {
    *(.data .data.*)
    *(.got .got.*)
  } :data
  .init_array : ALIGN(8) {
    __init_array_start = .;
    KEEP(*(SORT_BY_INIT_PRIORITY(.init_array.*)))
    KEEP(*(.init_array))
    __init_array_end = .;
  } :data
  .fini_array : ALIGN(8) {
    __fini_array_start = .;
    KEEP(*(SORT_BY_INIT_PRIORITY(.fini_array.*)))
    KEEP(*(.fini_array))
    __fini_array_end = .;
  } :data
  .bss : ALIGN(16) {
    *(.bss .bss.*)
    *(COMMON)
    . += PROBE_TLS_SHIFT;
  } :data
  .tdata : ALIGN(64) {
    __alloyos_tdata_start = .;
    KEEP(*(.tdata._alloyos_anchor))
    *(.tdata .tdata.*)
    __alloyos_tdata_end = .;
  } :data :tls
  .tbss : ALIGN(16) {
    __alloyos_tbss_start = .;
    *(.tbss .tbss.* .tcommon)
    __alloyos_tbss_end = .;
  } :data :tls
  __alloyos_tls_align = ALIGNOF(.tdata);
  /DISCARD/ : {
    *(.eh_frame*)
    *(.comment*)
  }
}
