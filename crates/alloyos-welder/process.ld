ENTRY(_start)
PHDRS { text PT_LOAD FLAGS(5); rodata PT_LOAD FLAGS(4); data PT_LOAD FLAGS(6); }
SECTIONS {
 . = 0x1000000000;
 .text : ALIGN(4096) { KEEP(*(.text._start)) *(.text .text.*) } :text
 . = ALIGN(4096);
 .rodata : { *(.rodata .rodata.*) } :rodata
 . = ALIGN(4096);
 .data : { *(.data .data.*) *(.got .got.*) } :data
 .bss : { *(.bss .bss.*) *(COMMON) } :data
 /DISCARD/ : { *(.eh_frame*) *(.comment*) }
}
